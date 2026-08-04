//! Booting one sidecar: its own guest and rootfs, no network device, and two bridges — its egress into the run's proxy, and each service it exposes into the workload.

use std::os::fd::RawFd;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::sync::mpsc::UnboundedReceiver;

use super::{SIDECAR_PROXY_PORT, Sidecar, bridge, ready};
use crate::{
    composefs, content_store, guest_tools, image, ingest, log, oci_layer_cache, runtime_layer,
    upperfs, vm, volume_store,
};

const BOOT_BUDGET: Duration = Duration::from_secs(120);
const READY_BUDGET: Duration = Duration::from_secs(60);
const STOP_BUDGET: Duration = Duration::from_secs(20);
const SESSION_DIAL_TIMEOUT: Duration = Duration::from_secs(30);

/// What every sidecar of a run shares: the caches the run already warmed and the kernel and initramfs it already has. Owned, because the bring-up outlives the borrow the boot sequence could lend it.
pub(crate) struct Host {
    pub run_id: String,
    pub cache_dir: PathBuf,
    pub layer_cache: oci_layer_cache::LayerCache,
    pub content_store: content_store::ContentStore,
    pub guest_tools: guest_tools::GuestTools,
    pub kernel: PathBuf,
    pub initrd: PathBuf,
    pub debug: bool,
}

/// A booted sidecar. `stop` waits for the power-off, which a volume-backed guest needs or its filesystem stays dirty and the next run fails its first write.
pub(crate) struct Running {
    id: String,
    connector: Arc<dyn vm::GuestTransport>,
    task: Option<tokio::task::JoinHandle<Result<()>>>,
    session_input: Option<tokio::sync::mpsc::Sender<vm::session_client::SessionInput>>,
    _leases: Vec<volume_store::VolumeLease>,
}

impl super::supervise::Guest for Running {
    async fn stop(mut self) {
        self.connector.request_stop();
        let Some(mut task) = self.task.take() else {
            return;
        };
        if tokio::time::timeout(STOP_BUDGET, &mut task).await.is_err() {
            log::warn!(
                "sidecar {} did not power off within {STOP_BUDGET:?}; a volume it wrote may need a repair on the next run",
                self.id
            );
            task.abort();
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.connector.request_stop();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

/// Every sidecar of a run starts against the same warmed caches and the same workload guest.
pub(crate) struct VmGuests {
    pub host: Host,
    pub main_guest: Arc<dyn vm::GuestTransport>,
}

impl super::supervise::Guests for VmGuests {
    type Guest = Running;

    async fn start(
        &self,
        sidecar: &Sidecar,
        service_rx: Vec<UnboundedReceiver<RawFd>>,
        proxy_ca: Option<&str>,
    ) -> Result<Running> {
        launch(
            &self.host,
            sidecar,
            self.main_guest.clone(),
            service_rx,
            proxy_ca,
        )
        .await
    }
}

/// Boot `sidecar` and bring up its bridges: egress into `main_guest`'s proxy port, and one `service_rx` per exposed service into the sidecar itself.
async fn launch(
    host: &Host,
    sidecar: &Sidecar,
    main_guest: Arc<dyn vm::GuestDialer>,
    service_rx: Vec<UnboundedReceiver<RawFd>>,
    proxy_ca: Option<&str>,
) -> Result<Running> {
    let guest_id = format!("{}-{}", host.run_id, sidecar.id);
    let mut rootfs = ingest::run(
        Some(&sidecar.image),
        &sidecar.argv,
        &image::want_arch(),
        &host.layer_cache,
        image::pull,
    )
    .await
    .with_context(|| format!("pulling the rootfs for sidecar {}", sidecar.id))?;

    let descriptor = descriptor(host, &mut rootfs).await?;
    let upper_disk = upperfs::provision(&guest_id).await?;
    #[cfg(target_os = "macos")]
    let console_fd = vm::diag_console::spawn(console_log(&upper_disk, &guest_id), host.debug)?;
    let (volumes, leases) = volume_store::resolve(&sidecar.volumes, &guest_id).await?;

    let run_as = vm::resolve_run_as(Some("0"), Some(0), None, None);
    let mut exec =
        vm::ExecSpec::for_run(&run_as, None, &sidecar.argv, rootfs.config.as_ref(), None);
    exec.kernel_env.extend(sidecar.revforward_cmdline());

    let (egress_fd_tx, egress_fd_rx) = tokio::sync::mpsc::unbounded_channel();
    let (connector_tx, connector_rx) = tokio::sync::oneshot::channel();
    let spec = vm::VmSpec {
        run_id: guest_id.clone(),
        cpus: sidecar.cpus,
        memory_mib: sidecar.memory_mib,
        kernel: host.kernel.clone(),
        initrd: host.initrd.clone(),
        composefs_descriptor: descriptor.path,
        content_share: host.content_store.root().to_path_buf(),
        content_tag: "lns-content".to_string(),
        descriptor_sha256: Some(descriptor.descriptor_sha256),
        upper_disk,
        volumes,
        binds: Vec::new(),
        workload_uid: run_as.uid,
        workload_gid: vm::host_known_workload_gid(&run_as),
        vsock: Vec::from_iter(sidecar.egress_via_proxy.then(|| vm::VsockChannel {
            port: lns_session::SIDECAR_EGRESS_PORT,
            fd_tx: egress_fd_tx,
        })),
        no_nic: true,
        connector_tx: Some(connector_tx),
        #[cfg(target_os = "macos")]
        console_fd,
        debug: host.debug,
        exec,
    };

    if sidecar.egress_via_proxy {
        bridge::spawn(
            format!("{}/egress", sidecar.id),
            egress_fd_rx,
            main_guest,
            SIDECAR_PROXY_PORT,
        );
    }

    let mut running = boot(spec, connector_rx, sidecar, leases).await?;
    running.session_input = Some(
        open_primary_session(&running, sidecar, rootfs.config.as_ref(), proxy_ca)
            .await
            .with_context(|| format!("starting sidecar {}", sidecar.id))?,
    );

    for (expose, source_rx) in sidecar.expose.iter().zip(service_rx) {
        bridge::spawn(
            format!("{}/{}", sidecar.id, expose.guest_port),
            source_rx,
            running.connector.clone(),
            expose.guest_port,
        );
        ready::wait(running.connector.as_ref(), expose.guest_port, READY_BUDGET)
            .await
            .with_context(|| format!("sidecar {} service {}", sidecar.id, expose.guest_port))?;
    }
    log::info!("Started", "sidecar {}", sidecar.id);
    Ok(running)
}

#[cfg(target_os = "macos")]
fn console_log(upper_disk: &std::path::Path, guest_id: &str) -> PathBuf {
    upper_disk
        .parent()
        .unwrap_or(std::path::Path::new("."))
        .join(format!("{guest_id}-console.log"))
}

async fn descriptor(
    host: &Host,
    rootfs: &mut ingest::IngestedImage,
) -> Result<composefs::descriptor::BuiltDescriptor> {
    let layer = runtime_layer::for_run(false, &host.content_store, &host.guest_tools, None, &[])?;
    let layers = std::mem::take(&mut rootfs.bytes);
    let digests = std::mem::take(&mut rootfs.digests);
    let builder = composefs::descriptor::DescriptorBuilder::new(host.cache_dir.clone());
    if let Some(hit) = builder.cached(&composefs::descriptor::DescriptorRequest {
        layer_digests: &digests,
        layers: &layers,
        runtime_layer: layer.as_ref(),
    })? {
        return Ok(hit);
    }
    let content_store = host.content_store.clone();
    tokio::task::spawn_blocking(move || {
        builder.build(
            &content_store,
            &composefs::descriptor::DescriptorRequest {
                layer_digests: &digests,
                layers: &layers,
                runtime_layer: layer.as_ref(),
            },
        )
    })
    .await
    .map_err(|e| anyhow::anyhow!("sidecar composefs descriptor build task panicked: {e}"))?
}

async fn boot(
    spec: vm::VmSpec,
    connector_rx: tokio::sync::oneshot::Receiver<Arc<dyn vm::GuestTransport>>,
    sidecar: &Sidecar,
    leases: Vec<volume_store::VolumeLease>,
) -> Result<Running> {
    let mut task = tokio::spawn(vm::boot(spec, None));
    let mut connector_rx = connector_rx;
    let connector = tokio::select! {
        biased;
        r = &mut task => anyhow::bail!("sidecar {} ended before it was reachable: {r:?}", sidecar.id),
        c = &mut connector_rx => c.with_context(|| format!("sidecar {} never became reachable", sidecar.id))?,
        _ = tokio::time::sleep(BOOT_BUDGET) => {
            task.abort();
            anyhow::bail!("sidecar {} was not reachable within {BOOT_BUDGET:?}", sidecar.id);
        }
    };
    Ok(Running {
        id: sidecar.id.clone(),
        connector,
        task: Some(task),
        session_input: None,
        _leases: leases,
    })
}

/// The sidecar's own service is its primary session, so it keeps running after this returns. The returned sender holds the session's input channel open for as long as the sidecar lives.
async fn open_primary_session(
    running: &Running,
    sidecar: &Sidecar,
    config: Option<&oci_client::config::ConfigFile>,
    proxy_ca: Option<&str>,
) -> Result<tokio::sync::mpsc::Sender<vm::session_client::SessionInput>> {
    let (frame_tx, mut frame_rx) = tokio::sync::mpsc::channel::<lns_ipc::WireFrame>(64);
    let id = sidecar.id.clone();
    tokio::spawn(async move {
        while let Some(frame) = frame_rx.recv().await {
            if let Some(line) = super::session_output(&frame) {
                log::debug!("sidecar {id}: {line}");
            }
        }
    });
    let (input_tx, input_rx) = tokio::sync::mpsc::channel(1);
    let fd = running
        .connector
        .connect(lns_session::BROKER_PORT, SESSION_DIAL_TIMEOUT)
        .await?;
    let params = vm::session_client::SessionParams {
        argv: crate::run::build_workload_argv(config, None, &sidecar.argv, false),
        env: sidecar.session_env(proxy_ca),
        cwd: None,
        hostname: None,
        tty: false,
        stdin: false,
        initial_winsize: None,
        confine: false,
    };
    tokio::spawn(vm::session_client::run_session_on_fd(
        fd, params, frame_tx, input_rx,
    ));
    Ok(input_tx)
}
