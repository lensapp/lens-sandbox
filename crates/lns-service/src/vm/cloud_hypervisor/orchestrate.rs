#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::os::fd::RawFd;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::sync::mpsc::UnboundedSender;

use super::launch::{SocketLayout, cloud_hypervisor_args, virtiofsd_args};
use super::process::{Child, Spawner};
use super::vmm_bin::VmmBinaries;
use super::vsock;
use crate::relay::VSOCK_PORT;
use crate::vm::VmSpec;

const SOCKET_POLL_INTERVAL: Duration = Duration::from_millis(10);

pub(crate) struct LaunchTimeouts {
    pub virtiofsd: Duration,
    pub cloud_hypervisor: Duration,
}

impl Default for LaunchTimeouts {
    fn default() -> Self {
        Self {
            virtiofsd: Duration::from_secs(10),
            cloud_hypervisor: Duration::from_secs(15),
        }
    }
}

pub(crate) struct RunningVm<C: Child> {
    pub virtiofsd: C,
    pub cloud_hypervisor: C,
    pub connector: vsock::HybridVsockConnector,
}

pub(crate) async fn launch<S: Spawner>(
    spawner: &S,
    spec: &VmSpec,
    bins: &VmmBinaries,
    layout: &SocketLayout,
    relay_fd_tx: Option<UnboundedSender<RawFd>>,
    timeouts: &LaunchTimeouts,
) -> Result<RunningVm<S::Child>> {
    let vfsd_args = virtiofsd_args(&layout.virtiofsd, &spec.content_share);
    let mut virtiofsd = spawner
        .spawn(&bins.virtiofsd, &vfsd_args)
        .context("spawning virtiofsd")?;
    if let Err(e) = wait_for_socket(&layout.virtiofsd, timeouts.virtiofsd).await {
        let _ = virtiofsd.start_kill();
        return Err(e.context("virtiofsd did not expose its socket"));
    }

    if let Some(tx) = relay_fd_tx {
        let listener = vsock::bind_guest_listener(&layout.vsock, VSOCK_PORT)?;
        vsock::spawn_accept_loop(listener, tx);
    }

    let ch_args = cloud_hypervisor_args(spec, layout);
    let mut cloud_hypervisor = spawner
        .spawn(&bins.cloud_hypervisor, &ch_args)
        .context("spawning cloud-hypervisor")?;
    if let Err(e) = wait_for_socket(&layout.vsock, timeouts.cloud_hypervisor).await {
        let _ = cloud_hypervisor.start_kill();
        let _ = virtiofsd.start_kill();
        return Err(e.context("cloud-hypervisor did not expose its vsock socket"));
    }

    let connector = vsock::HybridVsockConnector::new(&layout.vsock, &layout.api);
    Ok(RunningVm {
        virtiofsd,
        cloud_hypervisor,
        connector,
    })
}

async fn wait_for_socket(path: &Path, timeout: Duration) -> Result<()> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if tokio::fs::metadata(path).await.is_ok() {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            bail!(
                "timed out after {timeout:?} waiting for {} to appear",
                path.display()
            );
        }
        tokio::time::sleep(SOCKET_POLL_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vm::ExecSpec;
    use std::os::unix::process::ExitStatusExt;
    use std::path::PathBuf;
    use std::process::ExitStatus;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    struct FakeChild {
        killed: Arc<AtomicBool>,
    }

    impl Child for FakeChild {
        async fn wait(&mut self) -> std::io::Result<ExitStatus> {
            Ok(ExitStatus::from_raw(0))
        }
        fn start_kill(&mut self) -> std::io::Result<()> {
            self.killed.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    struct FakeSpawner {
        virtiofsd_socket: Option<PathBuf>,
        vsock_socket: Option<PathBuf>,
        spawned: Arc<Mutex<Vec<String>>>,
        virtiofsd_killed: Arc<AtomicBool>,
        ch_killed: Arc<AtomicBool>,
    }

    impl FakeSpawner {
        fn new(virtiofsd_socket: Option<PathBuf>, vsock_socket: Option<PathBuf>) -> Self {
            Self {
                virtiofsd_socket,
                vsock_socket,
                spawned: Arc::new(Mutex::new(Vec::new())),
                virtiofsd_killed: Arc::new(AtomicBool::new(false)),
                ch_killed: Arc::new(AtomicBool::new(false)),
            }
        }
        fn programs(&self) -> Vec<String> {
            self.spawned.lock().unwrap().clone()
        }
    }

    impl Spawner for FakeSpawner {
        type Child = FakeChild;
        fn spawn(&self, program: &Path, _args: &[String]) -> std::io::Result<FakeChild> {
            let name = program.file_name().unwrap().to_string_lossy();
            self.spawned.lock().unwrap().push(name.clone().into_owned());
            let (create, killed) = if name.contains("virtiofsd") {
                (self.virtiofsd_socket.clone(), self.virtiofsd_killed.clone())
            } else {
                (self.vsock_socket.clone(), self.ch_killed.clone())
            };
            if let Some(path) = create {
                std::fs::write(&path, b"").unwrap();
            }
            Ok(FakeChild { killed })
        }
    }

    fn spec(run_dir: &Path) -> VmSpec {
        VmSpec {
            run_id: 1,
            cpus: 2,
            memory_mib: 1024,
            kernel: PathBuf::from("/k"),
            initrd: PathBuf::from("/i"),
            composefs_descriptor: PathBuf::from("/d"),
            content_share: PathBuf::from("/content"),
            content_tag: "lns-content".into(),
            descriptor_sha256: None,
            upper_disk: run_dir.join("disk/upper"),
            volumes: vec![],
            debug: false,
            exec: ExecSpec::from_image_config(None, &["true".into()]),
            #[cfg(target_os = "macos")]
            vsock: None,
            #[cfg(target_os = "macos")]
            connector_tx: None,
            #[cfg(target_os = "macos")]
            console_fd: -1,
        }
    }

    fn bins() -> VmmBinaries {
        VmmBinaries {
            cloud_hypervisor: PathBuf::from("/usr/bin/cloud-hypervisor"),
            virtiofsd: PathBuf::from("/usr/bin/virtiofsd"),
        }
    }

    fn fast() -> LaunchTimeouts {
        LaunchTimeouts {
            virtiofsd: Duration::from_millis(200),
            cloud_hypervisor: Duration::from_millis(200),
        }
    }

    #[tokio::test]
    async fn launch_spawns_virtiofsd_then_cloud_hypervisor_and_returns_a_connector() {
        let d = tempfile::TempDir::new().unwrap();
        let layout = SocketLayout::for_run_dir(d.path());
        let spawner = FakeSpawner::new(Some(layout.virtiofsd.clone()), Some(layout.vsock.clone()));
        let running = launch(&spawner, &spec(d.path()), &bins(), &layout, None, &fast())
            .await
            .expect("launch should succeed once both sockets appear");

        assert_eq!(
            spawner.programs(),
            vec!["virtiofsd".to_string(), "cloud-hypervisor".to_string()],
            "virtiofsd must come up before cloud-hypervisor connects to its socket"
        );
        let _ = running;
    }

    #[tokio::test]
    async fn launch_kills_virtiofsd_when_its_socket_never_appears() {
        let d = tempfile::TempDir::new().unwrap();
        let layout = SocketLayout::for_run_dir(d.path());
        let spawner = FakeSpawner::new(None, Some(layout.vsock.clone()));
        let err = launch(&spawner, &spec(d.path()), &bins(), &layout, None, &fast())
            .await
            .map(|_| ())
            .expect_err("a missing virtiofsd socket must abort the launch");

        assert!(
            format!("{err:#}").contains("virtiofsd did not expose its socket"),
            "got {err:#}"
        );
        assert!(
            spawner.virtiofsd_killed.load(Ordering::SeqCst),
            "virtiofsd must be killed on socket timeout"
        );
        assert_eq!(
            spawner.programs(),
            vec!["virtiofsd".to_string()],
            "cloud-hypervisor must not start if virtiofsd never readies"
        );
    }

    #[tokio::test]
    async fn launch_kills_both_when_the_vsock_socket_never_appears() {
        let d = tempfile::TempDir::new().unwrap();
        let layout = SocketLayout::for_run_dir(d.path());
        let spawner = FakeSpawner::new(Some(layout.virtiofsd.clone()), None);
        let err = launch(&spawner, &spec(d.path()), &bins(), &layout, None, &fast())
            .await
            .map(|_| ())
            .expect_err("a missing vsock socket must abort the launch");

        assert!(
            format!("{err:#}").contains("cloud-hypervisor did not expose its vsock socket"),
            "got {err:#}"
        );
        assert!(
            spawner.ch_killed.load(Ordering::SeqCst),
            "CH must be killed"
        );
        assert!(
            spawner.virtiofsd_killed.load(Ordering::SeqCst),
            "virtiofsd must be cleaned up too"
        );
    }

    #[tokio::test]
    async fn launch_binds_the_relay_listener_when_a_fd_channel_is_supplied() {
        let d = tempfile::TempDir::new().unwrap();
        let layout = SocketLayout::for_run_dir(d.path());
        let spawner = FakeSpawner::new(Some(layout.virtiofsd.clone()), Some(layout.vsock.clone()));
        let (fd_tx, _fd_rx) = tokio::sync::mpsc::unbounded_channel();
        launch(
            &spawner,
            &spec(d.path()),
            &bins(),
            &layout,
            Some(fd_tx),
            &fast(),
        )
        .await
        .expect("launch should succeed");

        let relay_path = vsock::guest_listener_path(&layout.vsock, VSOCK_PORT);
        assert!(
            relay_path.exists(),
            "the guest→host relay listener socket must be bound before the guest boots"
        );
    }
}
