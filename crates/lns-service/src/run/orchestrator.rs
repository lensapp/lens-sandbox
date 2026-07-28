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

pub async fn handle(
    run_id: String,
    microvm: String,
    args: RunImageArgs,
    frame_tx: Sender<WireFrame>,
    input_rx: tokio::sync::mpsc::Receiver<crate::vm::session_client::SessionInput>,
) {
    let auto_remove = args.auto_remove;
    let finished_run_id = run_id.clone();
    let result = orchestrate(run_id, microvm, args, frame_tx.clone(), input_rx)
        .instrument(tracing::Span::current())
        .await;
    let code = emit_completion(&frame_tx, result).await;
    if auto_remove {
        crate::run_registry::set_exit_code(&finished_run_id, code);
        let _ = crate::run_registry::remove_if_exited(&finished_run_id);
    }
}

#[allow(clippy::cognitive_complexity)]
// top-level boot sequence: tools → caches → ingest → supervisor → runtime → vm spec → session
#[tracing::instrument(
    name = "lns.run",
    skip_all,
    fields(
        run_id = %run_id,
        image = args.image.as_deref().unwrap_or("<imageless>"),
        cpus = args.cpus,
        mem_mib = args.mem,
        supervised = args.policy_path.is_some(),
    ),
    err,
)]
async fn orchestrate(
    run_id: String,
    microvm: String,
    args: RunImageArgs,
    frame_tx: Sender<WireFrame>,
    input_rx: tokio::sync::mpsc::Receiver<crate::vm::session_client::SessionInput>,
) -> Result<i32> {
    log::attach_to_run_span(frame_tx.clone());

    let forwards = crate::forward::establish(
        std::sync::Arc::new(crate::forward::real::VsockForwarder::new(run_id.clone())),
        &crate::forward::plan(&args.published_ports),
    )?;

    let started = std::time::Instant::now();
    let prepare_started = std::time::Instant::now();

    let cache_dir = cache::root()?;
    let layer_cache = oci_layer_cache::LayerCache::new(cache_dir.join("layers"));
    let content_store = content_store::ContentStore::new(cache_dir.join("content"));
    let run_scratch_dir = cache_dir.join("runs").join(&run_id);
    let descriptor_builder = composefs::descriptor::DescriptorBuilder::new(cache_dir);
    let mut run_scratch =
        super::scratch::RunScratchGuard::new(run_scratch_dir, super::scratch::RealRemoveDir);
    let policy: Option<PathBuf> = args.policy_path.as_deref().map(PathBuf::from);

    // A local definition plans directly; a published sandbox reference boots its base image; a plain image passes through unchanged.
    let resolved_image = args.resolved_image.as_deref().or(args.image.as_deref());
    let sandbox_plan = match (args.definition.as_deref(), resolved_image) {
        (Some(definition), _) => Some(crate::artifact::real::plan_local(definition).await?),
        (None, Some(image_ref)) => {
            crate::artifact::real::peek_and_plan(image_ref, args.verify_sandbox, &run_id, &microvm)
                .await?
        }
        (None, None) => None,
    };
    if let Some(plan) = &sandbox_plan {
        crate::artifact::real::refuse_unknown_connectors(
            plan.workload.policy.as_ref(),
            &plan.workload.credentials,
        )?;
        crate::artifact::real::refuse_unbound_required_credentials(&plan.workload.credentials)?;
        gate_declared_sign_ins(&plan.workload.credentials, &frame_tx).await?;
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
            supervisor::SupervisorSession::start_if_policy(
                run_id.clone(),
                microvm.clone(),
                policy.as_deref().map(Path::new),
                sandbox_plan
                    .as_ref()
                    .and_then(|p| p.workload.policy.as_ref()),
                sandbox_plan
                    .as_ref()
                    .map(|p| p.workload.credentials.as_slice())
                    .unwrap_or_default(),
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
    let upper_fut = async {
        let upper_disk_path = upperfs::provision(&run_id).await?;
        log::debug!("upper disk ready at +{:.2?}", prepare_started.elapsed());
        Ok::<_, anyhow::Error>(upper_disk_path)
    };
    let volumes_fut = async {
        let resolved = crate::volume_store::resolve(&args.volumes, &run_id).await?;
        log::debug!("volumes ready at +{:.2?}", prepare_started.elapsed());
        Ok::<_, anyhow::Error>(resolved)
    };

    // join! rather than try_join! so the detached spawn_blocking inside upperfs::provision always settles before the scratch guard can fire — a cancelled provision would keep writing and re-orphan the run dir we just cleaned.
    let (tools_res, image_res, kernel_res, upper_res, volumes_res) = tokio::join!(
        tools_then_session,
        image_fut,
        kernel_fut,
        upper_fut,
        volumes_fut
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
            libc: crate::tools::libc::detect_libc_for(&image.digests, &image.bytes)?,
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
    let runtime_layer = runtime_layer::for_run(
        imageless,
        &content_store,
        &guest_tools,
        session.as_ref().map(|s| &s.assets),
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
            log::progress("Assembling", "rootfs", 0, 0);
            let descriptor_cs = content_store.clone();
            tokio::task::spawn_blocking(move || {
                descriptor_builder.build(
                    &descriptor_cs,
                    &composefs::descriptor::DescriptorRequest {
                        layer_digests: &layer_digests,
                        layers: &layers,
                        runtime_layer: runtime_layer.as_ref(),
                    },
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
        session.as_ref(),
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

    let (cpus, memory_mib) = super::sandbox_vm_size(
        sandbox_plan
            .as_ref()
            .and_then(|p| p.workload.resources.as_ref()),
        args.cpus,
        args.cpus_explicit,
        args.mem,
        args.mem_explicit,
    );
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
        vsock: session.as_ref().map(|s| vm::VsockChannel {
            port: crate::relay::VSOCK_PORT,
            fd_tx: s.relay.fd_tx.clone(),
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
        session.is_some(),
    );
    let workdir = crate::workload_cwd::resolve(
        args.workdir.as_deref(),
        crate::workload_cwd::image_workdir(image.config.as_ref()).as_deref(),
    );
    let composed = exec_env_strings(
        image.config.as_ref(),
        args.entrypoint.as_deref(),
        &cmd,
        session.is_some(),
        super::EnvInputs {
            user_env: &env,
            extra_managed: session
                .as_ref()
                .map(|s| s.managed_env_vars.as_slice())
                .unwrap_or(&[]),
            workdir: workdir.as_deref(),
            tool_bin_paths: ensured_tools
                .as_ref()
                .map(|ensured| ensured.bin_paths.as_slice())
                .unwrap_or_default(),
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
    let env: Vec<String> = composed.env;

    let params = vm::session_client::SessionParams {
        argv,
        env,
        cwd: workdir,
        hostname: args.hostname,
        tty: args.tty,
        stdin: args.stdin,
        initial_winsize,
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
    crate::run_registry::set_connector(&run_id, connector.clone());
    crate::run_registry::set_tool_bin_paths(
        &run_id,
        ensured_tools
            .as_ref()
            .map(|ensured| ensured.bin_paths.clone())
            .unwrap_or_default(),
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

/// Block the boot on any required oauth-kind credential slot with no armed machine grant: drive its sign-in host-side (streaming the verification frames to the client), and abort the launch if it does not complete. A bare `spec.connectors` id never gates here — it is offered reactively on first use.
async fn gate_declared_sign_ins(
    credentials: &[lns_artifact::spec::CredentialSlot],
    frame_tx: &Sender<WireFrame>,
) -> Result<()> {
    use crate::artifact::credential_boot::{
        BootGate, ConnectChoice, SlotPlan, boot_gate, plan_declared_connectors, resolve_connect,
        sign_in_gate_ids,
    };
    use crate::credential_flow::store::{
        CredentialStore, JsonFileCredentialStore, default_credentials_path,
    };
    use lns_ipc::Response;

    let declared = sign_in_gate_ids(credentials);
    if declared.is_empty() {
        return Ok(());
    }
    let user = lns_policy::connectors::Catalog::load_or_default(
        &lns_policy::connectors::default_connectors_path(),
    )
    .unwrap_or_default();
    let catalog = lns_policy::connectors::effective_connectors(&user);
    let state = JsonFileCredentialStore::new(default_credentials_path())
        .load()
        .unwrap_or_default();
    let plans = plan_declared_connectors(&declared, &catalog, &state);
    if boot_gate(&plans) == BootGate::StartWorkload {
        return Ok(());
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
                let _ = frame_tx
                    .send(WireFrame::Json(Response::RunLog {
                        level: lns_ipc::LogLevel::Info,
                        verb: None,
                        message: format!("signed in to {id}"),
                    }))
                    .await;
            }
            Response::OauthSignInFailed { reason } => {
                if resolve_connect(&prompt, ConnectChoice::Decline).starts_workload() {
                    continue;
                }
                anyhow::bail!(
                    "sign-in for connector {id} did not complete ({reason}); launch aborted"
                );
            }
            other => {
                anyhow::bail!("unexpected sign-in response for connector {id}: {other:?}");
            }
        }
    }
    Ok(())
}
