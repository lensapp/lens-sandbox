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
    run_id: u32,
    args: RunImageArgs,
    frame_tx: Sender<WireFrame>,
    input_rx: tokio::sync::mpsc::Receiver<crate::vm::session_client::SessionInput>,
) {
    let result = orchestrate(run_id, args, frame_tx.clone(), input_rx)
        .instrument(tracing::Span::current())
        .await;
    emit_completion(&frame_tx, result).await;
}

#[allow(clippy::cognitive_complexity)]
// top-level boot sequence: tools → caches → ingest → supervisor → runtime → vm spec → session
#[tracing::instrument(
    name = "lns.run",
    skip_all,
    fields(
        run_id = run_id,
        image = args.image.as_deref().unwrap_or("<imageless>"),
        cpus = args.cpus,
        mem_mib = args.mem,
        supervised = args.policy_path.is_some(),
    ),
    err,
)]
async fn orchestrate(
    run_id: u32,
    args: RunImageArgs,
    frame_tx: Sender<WireFrame>,
    input_rx: tokio::sync::mpsc::Receiver<crate::vm::session_client::SessionInput>,
) -> Result<i32> {
    log::attach_to_run_span(frame_tx.clone());

    let forwards = crate::forward::establish(
        std::sync::Arc::new(crate::forward::real::VsockForwarder::new(run_id)),
        &crate::forward::plan(&args.published_ports),
    )?;

    let started = std::time::Instant::now();
    let prepare_started = std::time::Instant::now();

    let cache_dir = cache::root()?;
    let layer_cache = oci_layer_cache::LayerCache::new(cache_dir.join("layers"));
    let content_store = content_store::ContentStore::new(cache_dir.join("content"));
    let run_scratch_dir = cache_dir.join("runs").join(run_id.to_string());
    let descriptor_builder = composefs::descriptor::DescriptorBuilder::new(cache_dir);
    let mut run_scratch =
        super::scratch::RunScratchGuard::new(run_scratch_dir, super::scratch::RealRemoveDir);
    let policy: Option<PathBuf> = args.policy_path.as_deref().map(PathBuf::from);

    let tools_then_session = async {
        let guest_tools = guest_tools::ensure().await?;
        log::debug!("guest tools ready at +{:.2?}", prepare_started.elapsed());
        let (session, initrd) = tokio::try_join!(
            supervisor::SupervisorSession::start_if_policy(
                run_id,
                policy.as_deref().map(Path::new),
                guest_tools.root.clone(),
                args.env.clone(),
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
            args.image.as_deref(),
            &args.cmd,
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
        let upper_disk_path = upperfs::provision(run_id).await?;
        log::debug!("upper disk ready at +{:.2?}", prepare_started.elapsed());
        Ok::<_, anyhow::Error>(upper_disk_path)
    };
    let volumes_fut = async {
        let resolved = crate::volume_store::resolve(&args.volumes, run_id).await?;
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
    for vol in &args.volumes {
        crate::audit::record_volume_attached(run_id, &vol.name, &vol.target)?;
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
            run_id,
            &bind.host_source,
            &bind.target,
            &bind.kept_paths,
            &bind.dropped_paths,
        )?;
    }

    let imageless = args.image.is_none();
    let runtime_layer = runtime_layer::for_run(
        imageless,
        &content_store,
        &guest_tools,
        session.as_ref().map(|s| &s.assets),
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
        imageless,
    );
    let exec = vm::ExecSpec::for_run(&run_as, &args.cmd, image.config.as_ref(), session.as_ref());

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

    let spec = vm::VmSpec {
        run_id,
        cpus: args.cpus,
        memory_mib: args.mem,
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
    let argv = build_workload_argv(image.config.as_ref(), &args.cmd, session.is_some());
    let workdir = crate::workload_cwd::resolve(
        args.workdir.as_deref(),
        crate::workload_cwd::image_workdir(image.config.as_ref()).as_deref(),
    );
    let composed = exec_env_strings(
        image.config.as_ref(),
        &args.cmd,
        &args.env,
        session.is_some(),
        session
            .as_ref()
            .map(|s| s.managed_env_vars.as_slice())
            .unwrap_or(&[]),
        workdir.as_deref(),
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
    crate::run_registry::set_connector(run_id, connector.clone());
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
    crate::run_registry::set_exit_code(run_id, session_code);

    super::shutdown::shutdown_after_session(forwards, std::time::Duration::from_secs(2), vm_task)
        .await?;

    log::info!("Finished", "in {:.2?}", started.elapsed());
    Ok(session_code)
}
