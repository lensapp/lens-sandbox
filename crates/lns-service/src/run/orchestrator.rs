use std::path::{Path, PathBuf};

use anyhow::Result;
use lns_ipc::{RunImageArgs, WireFrame};
use tokio::sync::mpsc::Sender;
use tracing::Instrument;

use crate::{
    cache, composefs, content_store, guest_tools, image, ingest, initramfs, kernel, log,
    oci_layer_cache, runtime_layer, supervisor, upperfs, vm,
};

use super::emit_completion;
#[cfg(target_os = "macos")]
use super::{
    build_workload_argv, connector_never_arrived, exec_env_strings, vm_ended_before_connector,
};

#[cfg(target_os = "macos")]
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

#[cfg(not(target_os = "macos"))]
pub async fn handle(run_id: u32, args: RunImageArgs, frame_tx: Sender<WireFrame>) {
    let result = orchestrate(run_id, args, frame_tx.clone())
        .instrument(tracing::Span::current())
        .await;
    emit_completion(&frame_tx, result).await;
}
#[allow(clippy::cognitive_complexity)] // top-level boot sequence: tools → caches → ingest → supervisor → runtime → vm spec
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
    #[cfg(target_os = "macos")] input_rx: tokio::sync::mpsc::Receiver<
        crate::vm::session_client::SessionInput,
    >,
) -> Result<i32> {
    log::attach_to_run_span(frame_tx.clone());

    #[cfg(target_os = "macos")]
    let forwards = crate::forward::establish(
        std::sync::Arc::new(crate::forward::real::VsockForwarder::new(run_id)),
        &crate::forward::plan(&args.published_ports),
    )?;

    #[cfg(not(target_os = "macos"))]
    if !args.published_ports.is_empty() {
        anyhow::bail!("port publishing (-p) is only supported on macOS hosts");
    }

    let started = std::time::Instant::now();
    let mut phase = std::time::Instant::now();
    let mut lap = move || {
        let elapsed = phase.elapsed();
        phase = std::time::Instant::now();
        elapsed
    };
    let guest_tools = guest_tools::ensure().await?;
    log::debug!("prepared guest tools in {:.2?}", lap());

    let cache_dir = cache::root()?;
    let layer_cache = oci_layer_cache::LayerCache::new(cache_dir.join("layers"));
    let content_store = content_store::ContentStore::new(cache_dir.join("content"));
    let descriptor_builder = composefs::descriptor::DescriptorBuilder::new(cache_dir);

    let mut image =
        ingest::run(args.image.as_deref(), &args.cmd, &layer_cache, image::pull).await?;
    log::debug!("prepared image layers in {:.2?}", lap());

    let policy: Option<PathBuf> = args.policy_path.as_deref().map(PathBuf::from);
    let session = supervisor::SupervisorSession::start_if_policy(
        run_id,
        policy.as_deref().map(Path::new),
        guest_tools.root.clone(),
        args.env.clone(),
    )
    .await?;
    log::debug!("prepared supervisor session in {:.2?}", lap());

    let imageless = args.image.is_none();
    let runtime_layer = runtime_layer::for_run(
        imageless,
        &content_store,
        &guest_tools,
        session.as_ref().map(|s| &s.assets),
    )?;
    log::debug!("prepared runtime layer in {:.2?}", lap());

    let layers = std::mem::take(&mut image.bytes);
    let layer_digests = std::mem::take(&mut image.digests);
    let descriptor_cs = content_store.clone();
    let descriptor = tokio::task::spawn_blocking(move || {
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
    .map_err(|e| anyhow::anyhow!("composefs descriptor build task panicked: {e}"))??;
    log::debug!("prepared composefs descriptor in {:.2?}", lap());
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

    let kernel_path = kernel::ensure().await?;
    log::debug!("prepared kernel in {:.2?}", lap());
    let initrd = initramfs::build(&guest_tools).await?;
    log::debug!("prepared initramfs in {:.2?}", lap());

    let upper_disk_path = upperfs::provision(run_id).await?;
    log::debug!("prepared upper disk in {:.2?}", lap());
    log::debug!(path = %upper_disk_path.display(), "upper disk provisioned");

    let (volume_attachments, volume_leases) =
        crate::volume_store::resolve(&args.volumes, run_id).await?;
    log::debug!("prepared volumes in {:.2?}", lap());
    for vol in &args.volumes {
        crate::audit::record_volume_attached(run_id, &vol.name, &vol.target)?;
    }

    #[cfg(target_os = "macos")]
    let console_fd = {
        let run_dir = upper_disk_path
            .parent()
            .map(std::path::Path::to_path_buf)
            .unwrap_or_else(|| std::path::PathBuf::from("."));
        vm::diag_console::spawn(run_dir.join("console.log"), args.debug)?
    };

    #[cfg(target_os = "macos")]
    let (connector_tx, connector_rx) = tokio::sync::oneshot::channel::<vm::VsockConnector>();

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
        #[cfg(target_os = "macos")]
        vsock: session.as_ref().map(|s| vm::VsockChannel {
            port: crate::relay::VSOCK_PORT,
            fd_tx: s.relay.fd_tx.clone(),
        }),
        #[cfg(target_os = "macos")]
        connector_tx: Some(connector_tx),
        #[cfg(target_os = "macos")]
        console_fd,
        debug: args.debug,
        exec,
    };

    #[cfg(not(target_os = "macos"))]
    let _ = &frame_tx;

    #[cfg(target_os = "macos")]
    {
        let initial_winsize = args
            .initial_winsize
            .map(|(rows, cols)| lns_session::Winsize { rows, cols });
        let argv = build_workload_argv(image.config.as_ref(), &args.cmd, session.is_some());
        let composed = exec_env_strings(
            image.config.as_ref(),
            &args.cmd,
            &args.env,
            session.is_some(),
            session
                .as_ref()
                .map(|s| s.managed_env_vars.as_slice())
                .unwrap_or(&[]),
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
            tty: args.tty,
            stdin: args.stdin,
            initial_winsize,
        };

        let frame_tx_for_session = frame_tx.clone();
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
                        vm_task.abort();
                        return Err(connector_never_arrived());
                    }
                }
            }
        };
        log::info!(
            "Booted",
            "microVM   ({:.2}s)",
            boot_start.elapsed().as_secs_f64()
        );
        let connector = std::sync::Arc::new(connector);
        crate::run_registry::set_connector(run_id, connector.clone());
        let _vm_stop_guard = vm::VmStopGuard::new(connector.clone());

        let fd = connector
            .connect(lns_session::BROKER_PORT, std::time::Duration::from_secs(30))
            .await?;
        log::debug!("connected broker in {:.2?}", lap());
        let session_code =
            vm::session_client::run_session_on_fd(fd, params, frame_tx_for_session, input_rx)
                .await?;
        log::debug!("workload ran for {:.2?}", lap());
        log::debug!(code = session_code, "broker session ended");
        crate::run_registry::set_exit_code(run_id, session_code);

        super::shutdown::shutdown_after_session(
            forwards,
            std::time::Duration::from_secs(2),
            vm_task,
        )
        .await?;

        log::info!("Finished", "in {:.2?}", started.elapsed());
        return Ok(session_code);
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _volume_leases = volume_leases;
        vm::boot(spec, None).await?;
        log::info!("Finished", "in {:.2?}", started.elapsed());
        Ok(0)
    }
}
