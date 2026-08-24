use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use lns_ipc::{RunImageArgs, WireFrame};
use tokio::sync::mpsc::Sender;
use tracing::Instrument;

use crate::{
    cache, composefs, content_store, guest_tools, image, ingest, initramfs, kernel, log,
    oci_layer_cache, runtime_layer, supervisor, upperfs, vm,
};

use super::{
    build_workload_argv, connector_never_arrived, emit_completion, exec_env_strings,
    vm_ended_before_connector,
};

/// What a run is turned away for before it exists: its host ports, what its reference resolves to, and whether it can be identified at all. Everything here is decided without a registry entry, so a refusal costs no run id and no run name.
pub struct PreparedRun {
    forwards: crate::forward::ForwardGuard,
    document: PreparedDocument,
    workload: super::WorkloadIdentity,
}

/// What a run will boot from: the definition its request carried, what its published reference resolved to, or neither.
enum PreparedDocument {
    Local(String),
    Published(Box<crate::artifact::real::ResolvedForRun>),
    Imageless,
}

/// Decide everything a run can be refused for before it starts. What is left in [`orchestrate`] either needs the planned document — which discloses and materializes — or streams to a client that is already attached.
pub async fn prepare(run_id: &str, args: &RunImageArgs) -> Result<PreparedRun> {
    let forwards = crate::forward::establish(
        std::sync::Arc::new(crate::forward::real::VsockForwarder::new(
            run_id.to_string(),
        )),
        &crate::forward::plan(&args.published_ports),
    )?;
    let resolved_image = args.resolved_image.as_deref().or(args.image.as_deref());
    let document = match (args.definition.as_deref(), resolved_image) {
        (Some(definition), _) => {
            crate::artifact::mixin::refuse_mixins_without_a_document(&args.mixins)?;
            PreparedDocument::Local(definition.to_string())
        }
        (None, Some(image_ref)) => {
            match crate::artifact::real::resolve_for_run(
                image_ref,
                args.verify_sandbox,
                &args.mixins,
                args.policy_path.as_deref().map(Path::new),
            )
            .await?
            {
                Some(resolved) => PreparedDocument::Published(Box::new(resolved)),
                None => PreparedDocument::Imageless,
            }
        }
        (None, None) => PreparedDocument::Imageless,
    };
    let digest = match &document {
        PreparedDocument::Published(resolved) => Some(resolved.digest.as_str()),
        _ => None,
    };
    // Refuse an unidentifiable run before its sign-in gate can drag the user through a device flow.
    let workload = super::workload_identity(args, resolved_image, digest)?;
    Ok(PreparedRun {
        forwards,
        document,
        workload,
    })
}

pub async fn handle(
    run_id: String,
    microvm: String,
    args: RunImageArgs,
    prepared: PreparedRun,
    frame_tx: Sender<WireFrame>,
    input_rx: tokio::sync::mpsc::Receiver<crate::vm::session_client::SessionInput>,
) {
    let auto_remove = args.auto_remove;
    let finished_run_id = run_id.clone();
    let result = orchestrate(run_id, microvm, args, prepared, frame_tx.clone(), input_rx)
        .instrument(tracing::Span::current())
        .await;
    let code = emit_completion(&frame_tx, result).await;
    if auto_remove {
        crate::run_registry::set_exit_code(&finished_run_id, code);
        let _ = crate::run_registry::remove_if_exited(&finished_run_id);
    }
}

/// Every run boots with a supervisor session: `SupervisorSession::start` refuses rather than returning without one.
const SUPERVISED: bool = true;

// top-level boot sequence: tools → caches → ingest → supervisor → runtime → vm spec → session
#[tracing::instrument(
    name = "lns.run",
    skip_all,
    fields(
        run_id = %run_id,
        image = args.image.as_deref().unwrap_or("<imageless>"),
        cpus = args.cpus,
        mem_mib = args.mem,
    ),
    err,
)]
async fn orchestrate(
    run_id: String,
    microvm: String,
    args: RunImageArgs,
    prepared: PreparedRun,
    frame_tx: Sender<WireFrame>,
    input_rx: tokio::sync::mpsc::Receiver<crate::vm::session_client::SessionInput>,
) -> Result<i32> {
    log::attach_to_run_span(frame_tx.clone());

    let PreparedRun {
        forwards,
        document,
        workload,
    } = prepared;

    let started = std::time::Instant::now();
    let prepare_started = std::time::Instant::now();

    let cache_dir = cache::root()?;
    let layer_cache = oci_layer_cache::LayerCache::new(cache_dir.join("layers"));
    let content_store = content_store::ContentStore::new(cache_dir.join("content"));
    let run_scratch_dir = cache_dir.join("runs").join(&run_id);
    let descriptor_builder = composefs::descriptor::DescriptorBuilder::new(cache_dir.clone());
    let mut run_scratch =
        super::scratch::RunScratchGuard::new(run_scratch_dir, super::scratch::RealRemoveDir);
    let policy: Option<PathBuf> = args.policy_path.as_deref().map(PathBuf::from);

    // A local definition plans directly; a published sandbox reference boots what it resolved to; a plain image passes through unchanged.
    let resolved_image = args.resolved_image.as_deref().or(args.image.as_deref());
    let sandbox_plan = match document {
        PreparedDocument::Local(definition) => Some(
            crate::artifact::real::plan_local(
                &definition,
                args.authored_egress.as_deref(),
                &crate::artifact::packed_from_the_wire(&args.packed_filesets),
                &args.denied_host_paths,
            )
            .await?,
        ),
        PreparedDocument::Published(resolved) => Some(
            crate::artifact::real::plan_resolved(
                *resolved,
                &run_id,
                &microvm,
                &args.denied_host_paths,
            )
            .await?,
        ),
        PreparedDocument::Imageless => None,
    };
    let mut signed_in = Vec::new();
    let mut revocations_at_gate = std::collections::HashMap::new();
    if let Some(plan) = &sandbox_plan {
        crate::artifact::real::refuse_unknown_connectors(plan.workload.policy.as_ref())?;
        // Read before the gate opens: a device flow can hold it for minutes, and a disconnect landing inside that window must win over the grant the sign-in earns.
        revocations_at_gate = policy
            .as_deref()
            .map(supervisor::revocations_before_gate)
            .unwrap_or_default();
        signed_in = gate_declared_sign_ins(&plan.workload.credentials, &frame_tx).await?;
    }
    let tool_requests = match &sandbox_plan {
        Some(plan) if !plan.workload.tools.is_empty() => {
            let requests = lns_artifact::tools::parse_all(&plan.workload.tools)?;
            crate::tools::registry::refuse_unknown_tools(&requests)?;
            requests
        }
        _ => Vec::new(),
    };
    let launch = sandbox_plan
        .as_ref()
        .map(|plan| super::sandbox_launch(&plan.workload, &args.cmd, &args.env));
    let image_ref: Option<String> = match &launch {
        Some(l) => Some(l.image.clone()),
        None => resolved_image.map(str::to_string),
    };
    let cmd: Vec<String> = launch
        .as_ref()
        .map(|l| l.cmd.clone())
        .unwrap_or_else(|| args.cmd.clone());
    let env: Vec<String> = launch
        .as_ref()
        .map(|l| l.env.clone())
        .unwrap_or_else(|| args.env.clone());
    crate::run_registry::set_resolved_command_and_env(&run_id, &cmd, &env);

    let tools_then_session = async {
        let guest_tools = guest_tools::ensure().await?;
        log::debug!("guest tools ready at +{:.2?}", prepare_started.elapsed());
        let (session, initrd) = tokio::try_join!(
            supervisor::SupervisorSession::start(
                run_id.clone(),
                microvm.clone(),
                policy.as_deref().map(Path::new),
                sandbox_plan
                    .as_ref()
                    .and_then(|p| p.workload.policy.as_ref()),
                supervisor::RunConsent {
                    credentials: sandbox_plan
                        .as_ref()
                        .map(|p| p.workload.credentials.as_slice())
                        .unwrap_or_default(),
                    workload: workload.clone(),
                    signed_in: signed_in.clone(),
                    revocations_at_gate: revocations_at_gate.clone(),
                },
                guest_tools.root.clone(),
                env.clone(),
            ),
            initramfs::build(&guest_tools),
        )?;
        log::debug!(
            "supervisor session and initramfs ready at +{:.2?}",
            prepare_started.elapsed()
        );
        Ok::<_, anyhow::Error>((guest_tools, session, initrd))
    };
    let image_fut = async {
        let image = ingest::run(
            image_ref.as_deref(),
            &cmd,
            &image::want_arch(),
            &layer_cache,
            image::pull,
        )
        .await?;
        log::debug!("image layers ready at +{:.2?}", prepare_started.elapsed());
        Ok::<_, anyhow::Error>(image)
    };
    let kernel_fut = async {
        let kernel_path = kernel::ensure().await?;
        log::debug!("kernel ready at +{:.2?}", prepare_started.elapsed());
        Ok::<_, anyhow::Error>(kernel_path)
    };
    let vm_size = super::sandbox_vm_size(
        sandbox_plan
            .as_ref()
            .and_then(|p| p.workload.resources.as_ref()),
        args.cpus,
        args.cpus_explicit,
        args.mem,
        args.mem_explicit,
        lns_artifact::resources::host::probe(),
    );

    let upper_fut = async {
        let upper_disk_path = upperfs::provision(&run_id, vm_size.disk_bytes).await?;
        log::debug!("upper disk ready at +{:.2?}", prepare_started.elapsed());
        Ok::<_, anyhow::Error>(upper_disk_path)
    };
    let volumes_fut = async {
        let resolved = crate::volume_store::resolve(&args.volumes, &run_id).await?;
        log::debug!("volumes ready at +{:.2?}", prepare_started.elapsed());
        Ok::<_, anyhow::Error>(resolved)
    };
    let workload_ca_fut = async {
        let spec = crate::tools::provisioner::real::workload_ca_spec(
            &cache_dir,
            crate::tools::host_arch(),
        )
        .await;
        log::debug!(
            "workload CA store ready at +{:.2?}",
            prepare_started.elapsed()
        );
        spec
    };

    // join! rather than try_join! so the detached spawn_blocking inside upperfs::provision always settles before the scratch guard can fire — a cancelled provision would keep writing and re-orphan the run dir we just cleaned.
    let (tools_res, image_res, kernel_res, upper_res, volumes_res, workload_ca_spec) = tokio::join!(
        tools_then_session,
        image_fut,
        kernel_fut,
        upper_fut,
        volumes_fut,
        workload_ca_fut
    );
    let (guest_tools, session, initrd) = tools_res?;
    let mut image = image_res?;
    let kernel_path = kernel_res?;
    let upper_disk_path = upper_res?;
    let (volume_attachments, volume_leases) = volumes_res?;
    log::debug!(path = %upper_disk_path.display(), "upper disk provisioned");
    crate::audit::record_run_launched(
        &run_id,
        &microvm,
        args.image.as_deref().unwrap_or("<imageless>"),
        &crate::oauth::RealClock,
    )?;
    for vol in &args.volumes {
        crate::audit::record_volume_attached(
            &run_id,
            &microvm,
            &vol.name,
            &vol.target,
            &crate::oauth::RealClock,
        )?;
    }

    let bind_attachments: Vec<vm::BindAttachment> = args
        .binds
        .iter()
        .map(|b| vm::BindAttachment {
            host_source: std::path::PathBuf::from(&b.host_source),
            target: b.target.clone(),
            read_only: b.read_only,
            dropped_paths: b.dropped_paths.clone(),
        })
        .collect();
    for bind in &args.binds {
        crate::audit::record_bind_attached(
            &run_id,
            &microvm,
            &bind.host_source,
            &bind.target,
            &bind.kept_paths,
            &bind.dropped_paths,
            &crate::oauth::RealClock,
        )?;
    }

    let ensured_tools = if tool_requests.is_empty() {
        None
    } else {
        let target = crate::tools::ProvisionTarget {
            arch: crate::tools::host_arch(),
            libc: crate::tools::libc::detect_libc_off_runtime(&image.digests, &image.bytes)?,
        };
        crate::tools::registry::refuse_libc_unsupported(
            &tool_requests,
            &target,
            image_ref.as_deref().unwrap_or_default(),
        )?;
        let ensured = crate::tools::real::ensure_for_run(
            &format!("{run_id}-tools"),
            &content_store,
            &tool_requests,
            &target,
            &|outcome| {
                // The tool is cached the moment this runs; a chain we cannot append to is a warning, not a reason to fail a launch whose tool is already on disk.
                if let Err(e) = crate::audit::record_tool_provisioned(
                    &run_id,
                    &microvm,
                    outcome,
                    &crate::oauth::RealClock,
                ) {
                    log::warn!("could not record the provisioned tool in the run's chain: {e:#}");
                }
            },
        )
        .await?;
        Some(ensured)
    };

    let imageless = args.image.is_none();
    let mut fileset_specs: Vec<runtime_layer::RuntimeFileSpec> = sandbox_plan
        .as_ref()
        .map(|p| p.fileset_specs.clone())
        .unwrap_or_default();
    if let Some(ensured) = &ensured_tools {
        fileset_specs.extend(ensured.specs.iter().cloned());
    }
    fileset_specs.extend(workload_ca_spec);
    let runtime_layer = runtime_layer::for_run(
        imageless,
        &content_store,
        &guest_tools,
        Some(&session.assets),
        &fileset_specs,
    )?;

    let layers = std::mem::take(&mut image.bytes);
    let layer_digests = std::mem::take(&mut image.digests);
    let probe = composefs::descriptor::DescriptorRequest {
        layer_digests: &layer_digests,
        layers: &layers,
        runtime_layer: runtime_layer.as_ref(),
    };
    let cached_descriptor = descriptor_builder.cached(&probe)?;
    let descriptor = match cached_descriptor {
        Some(hit) => hit,
        None => {
            let descriptor_cs = content_store.clone();
            let run_span = tracing::Span::current();
            tokio::task::spawn_blocking(move || {
                descriptor_builder.build(
                    &descriptor_cs,
                    &composefs::descriptor::DescriptorRequest {
                        layer_digests: &layer_digests,
                        layers: &layers,
                        runtime_layer: runtime_layer.as_ref(),
                    },
                    &super::assembling_progress(run_span),
                )
            })
            .await
            .map_err(|e| anyhow::anyhow!("composefs descriptor build task panicked: {e}"))??
        }
    };
    log::debug!(
        "composefs descriptor ready at +{:.2?}",
        prepare_started.elapsed()
    );
    log::debug!(
        path = %descriptor.path.display(),
        size = descriptor.size,
        sha256 = %descriptor.descriptor_sha256,
        "composefs descriptor materialised",
    );

    let run_as = vm::resolve_run_as(
        args.sandbox_user.as_deref(),
        args.sandbox_uid,
        sandbox_plan
            .as_ref()
            .and_then(|plan| plan.workload.user.as_deref()),
        image
            .config
            .as_ref()
            .and_then(|c| c.config.as_ref())
            .and_then(|c| c.user.as_deref()),
    );
    let exec = vm::ExecSpec::for_run(
        &run_as,
        args.entrypoint.as_deref(),
        &cmd,
        image.config.as_ref(),
        Some(&session),
    );

    #[cfg(target_os = "macos")]
    let console_fd = {
        let run_dir = upper_disk_path
            .parent()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        vm::diag_console::spawn(run_dir.join("console.log"), args.debug)?
    };

    let (connector_tx, connector_rx) =
        tokio::sync::oneshot::channel::<Arc<dyn vm::GuestTransport>>();

    let (cpus, memory_mib) = (vm_size.cpus, vm_size.mem_mib);
    crate::run_registry::set_resolved_size(&run_id, cpus, memory_mib);

    let spec = vm::VmSpec {
        run_id: run_id.clone(),
        cpus,
        memory_mib,
        kernel: kernel_path,
        initrd,
        composefs_descriptor: descriptor.path.clone(),
        content_share: content_store.root().to_path_buf(),
        content_tag: "lns-content".to_string(),
        descriptor_sha256: Some(descriptor.descriptor_sha256.clone()),
        upper_disk: upper_disk_path,
        volumes: volume_attachments,
        binds: bind_attachments,
        workload_uid: run_as.uid,
        workload_gid: vm::host_known_workload_gid(&run_as),
        vsock: Some(vm::VsockChannel {
            port: crate::relay::VSOCK_PORT,
            fd_tx: session.relay.fd_tx.clone(),
        }),
        connector_tx: Some(connector_tx),
        #[cfg(target_os = "macos")]
        console_fd,
        debug: args.debug,
        exec,
    };

    let initial_winsize = args
        .initial_winsize
        .map(|(rows, cols)| lns_session::Winsize { rows, cols });
    let argv = build_workload_argv(
        image.config.as_ref(),
        args.entrypoint.as_deref(),
        &cmd,
        SUPERVISED,
    );
    let workdir = crate::workload_cwd::resolve(
        args.workdir.as_deref(),
        crate::workload_cwd::image_workdir(image.config.as_ref()).as_deref(),
    );
    let tool_runtime = ensured_tools
        .as_ref()
        .map(|ensured| crate::workload_env::ToolRuntime {
            bin_paths: ensured.bin_paths.clone(),
            env: ensured.env.clone(),
        })
        .unwrap_or_default();
    let composed = exec_env_strings(
        image.config.as_ref(),
        args.entrypoint.as_deref(),
        &cmd,
        SUPERVISED,
        super::EnvInputs {
            user_env: &env,
            extra_managed: session.managed_env_vars.as_slice(),
            workdir: workdir.as_deref(),
            tools: &tool_runtime,
        },
    );
    for refused in &composed.refused {
        let _ = frame_tx
            .send(WireFrame::Json(lns_ipc::Response::RunLog {
                level: lns_ipc::LogLevel::Warn,
                verb: None,
                message: crate::workload_env::refusal_warning(refused),
            }))
            .await;
    }
    let exec_environment = crate::run_registry::ExecEnvironment {
        session_env: composed.env.clone(),
        tools: tool_runtime,
        placeholders: session.placeholder_env.clone(),
        workdir: workdir.clone(),
        declared_identity_keys: crate::workload_env::declared_identity_keys(&env),
    };
    let env: Vec<String> = composed.env;

    let params = vm::session_client::SessionParams {
        argv,
        env,
        cwd: workdir,
        hostname: args.hostname,
        tty: args.tty,
        stdin: args.stdin,
        initial_winsize,
        confine: !SUPERVISED,
        dies_with_client: false,
    };

    let frame_tx_for_session = frame_tx.clone();
    log::progress("Booting", "microVM", 0, 0);
    let boot_start = std::time::Instant::now();
    let mut vm_task = tokio::spawn(async move {
        let _volume_leases = volume_leases;
        vm::boot(spec, None).await
    });

    let connector = tokio::select! {
        biased;
        r = &mut vm_task => return Err(vm_ended_before_connector(r)),
        c = connector_rx => {
            match c {
                Ok(c) => c,
                Err(_) => {
                    // The boot task only drops the connector while returning an error; await it (bounded) to surface that cause, not a generic message.
                    return Err(
                        match tokio::time::timeout(std::time::Duration::from_secs(5), &mut vm_task).await {
                            Ok(r) => vm_ended_before_connector(r),
                            Err(_) => {
                                vm_task.abort();
                                connector_never_arrived()
                            }
                        },
                    );
                }
            }
        }
    };
    log::info!(
        "Booted",
        "microVM   ({:.2}s)",
        boot_start.elapsed().as_secs_f64()
    );
    crate::run_registry::set_connector_with_environment(
        &run_id,
        connector.clone(),
        exec_environment,
    );
    let _vm_stop_guard = vm::VmStopGuard::new(connector.clone());

    log::progress("Connecting", "session", 0, 0);
    let connect_started = std::time::Instant::now();
    let fd = connector
        .connect(lns_session::BROKER_PORT, std::time::Duration::from_secs(30))
        .await?;
    run_scratch.keep();
    log::debug!("connected broker in {:.2?}", connect_started.elapsed());
    let session_started = std::time::Instant::now();
    let session_code =
        vm::session_client::run_session_on_fd(fd, params, frame_tx_for_session, input_rx).await?;
    log::debug!("workload ran for {:.2?}", session_started.elapsed());
    log::debug!(code = session_code, "broker session ended");
    crate::run_registry::set_exit_code(&run_id, session_code);

    super::shutdown::shutdown_after_session(forwards, std::time::Duration::from_secs(2), vm_task)
        .await?;

    log::info!("Finished", "in {:.2?}", started.elapsed());
    Ok(session_code)
}

/// Block the boot on any declared credential whose supplying connector signs in with oauth and holds no armed machine grant: drive that sign-in host-side (streaming the verification frames to the client), and abort the launch if it does not complete. Returns the ids whose sign-in the user completed this launch — that consent becomes the workload's grant, so the credential arms now and the next run skips the sign-in. A bare `spec.connectors` id never gates here — it is offered reactively on first use.
async fn gate_declared_sign_ins(
    credentials: &[lns_spec::Credential],
    frame_tx: &Sender<WireFrame>,
) -> Result<Vec<String>> {
    use crate::artifact::credential_boot::{
        BootGate, SlotPlan, boot_gate, plan_declared_connectors, sign_in_gate_ids,
    };
    use crate::credential_flow::store::{CredentialStore, JsonFileCredentialStore};
    use lns_ipc::Response;

    let mut signed_in = Vec::new();
    if credentials.is_empty() {
        return Ok(signed_in);
    }
    let user = lns_policy::connectors::Catalog::load_or_default(&lns_ipc::connectors_path())
        .unwrap_or_default();
    let catalog = lns_policy::connectors::effective_connectors(&user);
    let declared = sign_in_gate_ids(credentials, &catalog);
    if declared.is_empty() {
        return Ok(signed_in);
    }
    let state = JsonFileCredentialStore::new(lns_ipc::credentials_path())
        .load()
        .unwrap_or_default();
    let plans = plan_declared_connectors(&declared, &catalog, &state);
    if boot_gate(&plans) == BootGate::StartWorkload {
        return Ok(signed_in);
    }
    for plan in plans {
        let SlotPlan::Connect(prompt) = plan else {
            continue;
        };
        let id = prompt.connector.clone();
        let _ = frame_tx
            .send(WireFrame::Json(Response::RunLog {
                level: lns_ipc::LogLevel::Info,
                verb: None,
                message: format!("connector {id} needs a sign-in before the workload starts"),
            }))
            .await;
        let (progress_tx, mut progress_rx) = tokio::sync::mpsc::unbounded_channel::<Response>();
        let sign_in = crate::ipc::adapter::run_connector_sign_in(&id, progress_tx);
        tokio::pin!(sign_in);
        let terminal = loop {
            tokio::select! {
                biased;
                Some(resp) = progress_rx.recv() => {
                    let _ = frame_tx.send(WireFrame::Json(resp)).await;
                }
                res = &mut sign_in => break res,
            }
        };
        while let Ok(resp) = progress_rx.try_recv() {
            let _ = frame_tx.send(WireFrame::Json(resp)).await;
        }
        match terminal {
            Response::OauthSignInComplete => {
                signed_in.push(id.clone());
                let _ = frame_tx
                    .send(WireFrame::Json(Response::RunLog {
                        level: lns_ipc::LogLevel::Info,
                        verb: None,
                        message: format!("signed in to {id}"),
                    }))
                    .await;
            }
            Response::OauthSignInFailed { reason } => {
                // Every declared credential is a requirement, so a sign-in the user abandons refuses the run rather than starting a workload that cannot reach its service.
                anyhow::bail!(
                    "sign-in for connector {id} did not complete ({reason}); launch aborted"
                );
            }
            other => {
                anyhow::bail!("unexpected sign-in response for connector {id}: {other:?}");
            }
        }
    }
    Ok(signed_in)
}
