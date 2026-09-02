#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::os::fd::RawFd;
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::sync::mpsc::UnboundedSender;

use super::launch::{BindIdMap, ShareAccess, SocketLayout, cloud_hypervisor_args, virtiofsd_args};
use super::process::{Child, Spawner};
use super::vmm_bin::VmmBinaries;
use super::vsock;
use crate::relay::VSOCK_PORT;
use crate::vm::{BindAttachment, VmSpec};

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
    pub virtiofsd: Vec<C>,
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
    layout.remove_stale()?;
    let content = spawn_virtiofsd(
        spawner,
        &bins.virtiofsd,
        &layout.virtiofsd,
        &spec.content_share,
        None,
        ShareAccess::ReadOnly,
        timeouts.virtiofsd,
    )
    .await?;
    let mut virtiofsd = vec![content];

    let (host_uid, host_gid) = host_ids();
    let bind_id_map = bind_id_map(spec, host_uid, host_gid);
    for (i, bind) in spec.binds.iter().enumerate() {
        match prepare_bind(
            spawner,
            bins,
            layout,
            bind,
            i,
            bind_id_map,
            timeouts.virtiofsd,
        )
        .await
        {
            Ok(child) => virtiofsd.push(child),
            Err(e) => {
                kill_and_reap_all(&mut virtiofsd).await;
                return Err(e);
            }
        }
    }

    if let Some(tx) = relay_fd_tx {
        let listener = vsock::bind_guest_listener(&layout.vsock, VSOCK_PORT)?;
        super::real::spawn_accept_loop(listener, tx);
    }

    let ch_args = cloud_hypervisor_args(spec, layout);
    let mut cloud_hypervisor = match spawner
        .spawn(&bins.cloud_hypervisor, &ch_args)
        .context("spawning cloud-hypervisor")
    {
        Ok(child) => child,
        Err(e) => {
            // virtiofsd is already running; reap it before bailing so a
            // cloud-hypervisor spawn failure can't leave a zombie behind.
            kill_and_reap_all(&mut virtiofsd).await;
            return Err(e);
        }
    };
    if let Err(e) = wait_for_socket(&layout.vsock, timeouts.cloud_hypervisor).await {
        let _ = cloud_hypervisor.start_kill();
        let _ = cloud_hypervisor.wait().await;
        kill_and_reap_all(&mut virtiofsd).await;
        return Err(e.context("cloud-hypervisor did not expose its vsock socket"));
    }

    let connector = vsock::HybridVsockConnector::new(&layout.vsock, &layout.api);
    Ok(RunningVm {
        virtiofsd,
        cloud_hypervisor,
        connector,
    })
}

async fn prepare_bind<S: Spawner>(
    spawner: &S,
    bins: &VmmBinaries,
    layout: &SocketLayout,
    bind: &BindAttachment,
    index: usize,
    id_map: Option<BindIdMap>,
    timeout: Duration,
) -> Result<S::Child> {
    tokio::fs::metadata(&bind.host_source)
        .await
        .with_context(|| {
            format!(
                "host bind source is not accessible: {}",
                bind.host_source.display()
            )
        })?;
    spawn_virtiofsd(
        spawner,
        &bins.virtiofsd,
        &layout.bind_virtiofsd(index),
        &bind.host_source,
        id_map,
        match bind.read_only {
            true => ShareAccess::ReadOnly,
            false => ShareAccess::ReadWrite,
        },
        timeout,
    )
    .await
}

/// virtiofsd runs as this unprivileged user, so a guest write is EPERM unless the guest's own ids are mapped onto it. Every guest that writes to a bind needs the map, including the provisioner guest, which runs as root and whose gid the host cannot look up.
fn bind_id_map(spec: &VmSpec, host_uid: u32, host_gid: u32) -> Option<BindIdMap> {
    let guest_uid = spec.workload_uid?;
    Some(BindIdMap {
        guest_uid,
        guest_gid: spec.workload_gid.unwrap_or(guest_uid),
        host_uid,
        host_gid,
    })
}

fn host_ids() -> (u32, u32) {
    // SAFETY: geteuid/getegid are reentrant and always succeed per POSIX.
    unsafe { (libc::geteuid(), libc::getegid()) }
}

async fn spawn_virtiofsd<S: Spawner>(
    spawner: &S,
    bin: &Path,
    socket: &Path,
    shared_dir: &Path,
    id_map: Option<BindIdMap>,
    access: ShareAccess,
    timeout: Duration,
) -> Result<S::Child> {
    let args = virtiofsd_args(socket, shared_dir, id_map, access);
    let mut child = spawner.spawn(bin, &args).context("spawning virtiofsd")?;
    if let Err(e) = wait_for_socket(socket, timeout).await {
        let _ = child.start_kill();
        let _ = child.wait().await;
        return Err(e.context("virtiofsd did not expose its socket"));
    }
    Ok(child)
}

async fn kill_and_reap_all<C: Child>(children: &mut [C]) {
    for child in children {
        let _ = child.start_kill();
        let _ = child.wait().await;
    }
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
        waited: Arc<AtomicBool>,
    }

    impl Child for FakeChild {
        async fn wait(&mut self) -> std::io::Result<ExitStatus> {
            self.waited.store(true, Ordering::SeqCst);
            Ok(ExitStatus::from_raw(0))
        }
        fn start_kill(&mut self) -> std::io::Result<()> {
            self.killed.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    #[derive(Clone)]
    struct Spawned {
        program: String,
        args: Vec<String>,
        killed: Arc<AtomicBool>,
        waited: Arc<AtomicBool>,
    }

    struct FakeSpawner {
        suppress: Vec<PathBuf>,
        /// Program file names whose `spawn` should fail (e.g. "cloud-hypervisor").
        fail_program: Vec<String>,
        spawned: Arc<Mutex<Vec<Spawned>>>,
    }

    impl FakeSpawner {
        fn ready() -> Self {
            Self {
                suppress: Vec::new(),
                fail_program: Vec::new(),
                spawned: Arc::new(Mutex::new(Vec::new())),
            }
        }
        fn suppressing(socket: PathBuf) -> Self {
            Self {
                suppress: vec![socket],
                fail_program: Vec::new(),
                spawned: Arc::new(Mutex::new(Vec::new())),
            }
        }
        fn failing_to_spawn(program: &str) -> Self {
            Self {
                suppress: Vec::new(),
                fail_program: vec![program.to_string()],
                spawned: Arc::new(Mutex::new(Vec::new())),
            }
        }
        fn records(&self) -> Vec<Spawned> {
            self.spawned.lock().unwrap().clone()
        }
        fn programs(&self) -> Vec<String> {
            self.records().into_iter().map(|s| s.program).collect()
        }
    }

    fn spawned_socket(args: &[String]) -> PathBuf {
        args.iter()
            .find_map(|a| {
                if a.contains("cid=") {
                    a.find("socket=")
                        .map(|i| PathBuf::from(&a[i + "socket=".len()..]))
                } else {
                    a.strip_prefix("--socket-path=").map(PathBuf::from)
                }
            })
            .expect("every spawned VMM carries a socket path")
    }

    impl Spawner for FakeSpawner {
        type Child = FakeChild;
        fn spawn(&self, program: &Path, args: &[String]) -> std::io::Result<FakeChild> {
            let name = program.file_name().unwrap().to_string_lossy().into_owned();
            if self.fail_program.contains(&name) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    "fake spawn failure",
                ));
            }
            let socket = spawned_socket(args);
            if !self.suppress.contains(&socket) {
                std::fs::write(&socket, b"").unwrap();
            }
            let killed = Arc::new(AtomicBool::new(false));
            let waited = Arc::new(AtomicBool::new(false));
            self.spawned.lock().unwrap().push(Spawned {
                program: name,
                args: args.to_vec(),
                killed: killed.clone(),
                waited: waited.clone(),
            });
            Ok(FakeChild { killed, waited })
        }
    }

    fn arg_with_prefix(args: &[String], prefix: &str) -> String {
        args.iter()
            .find_map(|a| a.strip_prefix(prefix))
            .unwrap_or_else(|| panic!("no arg with prefix {prefix} in {args:?}"))
            .to_string()
    }

    fn bind(host_source: &Path, target: &str, read_only: bool) -> BindAttachment {
        BindAttachment {
            host_source: host_source.to_path_buf(),
            target: target.to_string(),
            read_only,
            dropped_paths: vec![],
            seeded_paths: Vec::new(),
        }
    }

    fn spec(run_dir: &Path) -> VmSpec {
        VmSpec {
            run_id: "aa01".to_string(),
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
            binds: vec![],
            workload_uid: Some(65534),
            workload_gid: Some(65534),
            debug: false,
            exec: ExecSpec::from_image_config(None, None, &["true".into()]),
            vsock: None,
            connector_tx: None,
            #[cfg(target_os = "macos")]
            console_fd: -1,
        }
    }

    #[test]
    fn every_guest_that_writes_to_a_bind_gets_a_map() {
        // The provisioner guest runs as root and the host cannot look up its gid; without a map virtiofsd serves the writable share as its own unprivileged user and the guest's tar fails with EPERM.
        let dir = tempfile::TempDir::new().unwrap();
        let mut root_guest = spec(dir.path());
        root_guest.workload_uid = Some(0);
        root_guest.workload_gid = None;
        assert_eq!(
            bind_id_map(&root_guest, 501, 20),
            Some(BindIdMap {
                guest_uid: 0,
                guest_gid: 0,
                host_uid: 501,
                host_gid: 20,
            })
        );

        let sandbox_user = spec(dir.path());
        assert_eq!(
            bind_id_map(&sandbox_user, 501, 20),
            Some(BindIdMap {
                guest_uid: 65534,
                guest_gid: 65534,
                host_uid: 501,
                host_gid: 20,
            })
        );

        let mut unsupervised = spec(dir.path());
        unsupervised.workload_uid = None;
        assert_eq!(bind_id_map(&unsupervised, 501, 20), None);
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
        let spawner = FakeSpawner::ready();
        let mut running = launch(&spawner, &spec(d.path()), &bins(), &layout, None, &fast())
            .await
            .expect("launch should succeed once both sockets appear");

        assert_eq!(
            spawner.programs(),
            vec!["virtiofsd".to_string(), "cloud-hypervisor".to_string()],
            "virtiofsd must come up before cloud-hypervisor connects to its socket"
        );
        assert_eq!(
            running.virtiofsd.len(),
            1,
            "with no binds only the content share is served"
        );
        assert!(running.cloud_hypervisor.wait().await.unwrap().success());
        let _ = running.connector;
    }

    #[test]
    fn default_timeouts_leave_headroom_for_virtiofsd_and_cloud_hypervisor() {
        let t = LaunchTimeouts::default();
        assert!(t.virtiofsd >= Duration::from_secs(5));
        assert!(t.cloud_hypervisor >= t.virtiofsd);
    }

    #[tokio::test]
    async fn launch_kills_virtiofsd_when_its_socket_never_appears() {
        let d = tempfile::TempDir::new().unwrap();
        let layout = SocketLayout::for_run_dir(d.path());
        let spawner = FakeSpawner::suppressing(layout.virtiofsd.clone());
        let err = launch(&spawner, &spec(d.path()), &bins(), &layout, None, &fast())
            .await
            .map(|_| ())
            .expect_err("a missing virtiofsd socket must abort the launch");

        assert!(
            format!("{err:#}").contains("virtiofsd did not expose its socket"),
            "got {err:#}"
        );
        let recs = spawner.records();
        assert_eq!(
            recs.len(),
            1,
            "cloud-hypervisor must not start if virtiofsd never readies"
        );
        assert!(
            recs[0].killed.load(Ordering::SeqCst),
            "virtiofsd must be killed on socket timeout"
        );
        assert!(
            recs[0].waited.load(Ordering::SeqCst),
            "virtiofsd must be reaped after kill"
        );
    }

    #[tokio::test]
    async fn launch_kills_both_when_the_vsock_socket_never_appears() {
        let d = tempfile::TempDir::new().unwrap();
        let layout = SocketLayout::for_run_dir(d.path());
        let spawner = FakeSpawner::suppressing(layout.vsock.clone());
        let err = launch(&spawner, &spec(d.path()), &bins(), &layout, None, &fast())
            .await
            .map(|_| ())
            .expect_err("a missing vsock socket must abort the launch");

        assert!(
            format!("{err:#}").contains("cloud-hypervisor did not expose its vsock socket"),
            "got {err:#}"
        );
        assert_eq!(
            spawner.programs(),
            vec!["virtiofsd".to_string(), "cloud-hypervisor".to_string()]
        );
        assert!(
            spawner
                .records()
                .iter()
                .all(|r| r.killed.load(Ordering::SeqCst)),
            "both cloud-hypervisor and virtiofsd must be cleaned up"
        );
        assert!(
            spawner
                .records()
                .iter()
                .all(|r| r.waited.load(Ordering::SeqCst)),
            "both cloud-hypervisor and virtiofsd must be reaped after kill"
        );
    }

    #[tokio::test]
    async fn launch_reaps_virtiofsd_when_cloud_hypervisor_fails_to_spawn() {
        let d = tempfile::TempDir::new().unwrap();
        let layout = SocketLayout::for_run_dir(d.path());
        let spawner = FakeSpawner::failing_to_spawn("cloud-hypervisor");
        let err = launch(&spawner, &spec(d.path()), &bins(), &layout, None, &fast())
            .await
            .map(|_| ())
            .expect_err("a cloud-hypervisor spawn failure must abort the launch");

        assert!(
            format!("{err:#}").contains("spawning cloud-hypervisor"),
            "got {err:#}"
        );
        let recs = spawner.records();
        assert_eq!(
            recs.len(),
            1,
            "only the content virtiofsd is recorded; cloud-hypervisor never spawns"
        );
        // Regression: a cloud-hypervisor spawn failure used to drop the
        // already-running virtiofsd with only best-effort kill_on_drop, which
        // can leave a zombie. It must now be killed and reaped before we bail.
        assert!(
            recs[0].killed.load(Ordering::SeqCst),
            "the content virtiofsd must be killed when cloud-hypervisor fails to spawn"
        );
        assert!(
            recs[0].waited.load(Ordering::SeqCst),
            "the content virtiofsd must be reaped (waited on), not just signalled"
        );
    }

    #[tokio::test]
    async fn launch_binds_the_relay_listener_when_a_fd_channel_is_supplied() {
        let d = tempfile::TempDir::new().unwrap();
        let layout = SocketLayout::for_run_dir(d.path());
        let spawner = FakeSpawner::ready();
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

    #[tokio::test]
    async fn launch_serves_each_host_bind_as_its_own_virtiofsd_before_cloud_hypervisor() {
        let d = tempfile::TempDir::new().unwrap();
        let layout = SocketLayout::for_run_dir(d.path());
        let proj = tempfile::TempDir::new().unwrap();
        let data = tempfile::TempDir::new().unwrap();
        let mut s = spec(d.path());
        s.binds = vec![
            bind(proj.path(), "/work", false),
            bind(data.path(), "/data", true),
        ];
        let spawner = FakeSpawner::ready();
        let running = launch(&spawner, &s, &bins(), &layout, None, &fast())
            .await
            .expect("launch with host binds should succeed");

        assert_eq!(
            spawner.programs(),
            vec![
                "virtiofsd".to_string(),
                "virtiofsd".to_string(),
                "virtiofsd".to_string(),
                "cloud-hypervisor".to_string(),
            ],
            "the content share plus one virtiofsd per bind, all before cloud-hypervisor"
        );
        assert_eq!(running.virtiofsd.len(), 3);

        let recs = spawner.records();
        assert_eq!(
            arg_with_prefix(&recs[1].args, "--shared-dir="),
            proj.path().display().to_string(),
            "bind 0 serves its host dir"
        );
        assert_eq!(
            arg_with_prefix(&recs[1].args, "--socket-path="),
            layout.bind_virtiofsd(0).display().to_string(),
            "bind 0 is exposed on the bind-0 socket"
        );
        assert_eq!(
            arg_with_prefix(&recs[2].args, "--shared-dir="),
            data.path().display().to_string(),
            "bind 1 serves its host dir"
        );
        assert_eq!(
            arg_with_prefix(&recs[2].args, "--socket-path="),
            layout.bind_virtiofsd(1).display().to_string(),
            "bind 1 is exposed on the bind-1 socket"
        );

        let maps_workload_uid =
            |r: &Spawned| r.args.iter().any(|a| a.starts_with("--uid-map=:65534:"));
        assert!(
            !maps_workload_uid(&recs[0]),
            "the world-readable content share keeps 1:1 uids"
        );
        assert!(
            maps_workload_uid(&recs[1]) && maps_workload_uid(&recs[2]),
            "every bind maps the workload uid to the host user so an unprivileged workload can reach it"
        );
        let rejects_guest_writes = |r: &Spawned| r.args.iter().any(|a| a == "--readonly");
        assert!(
            rejects_guest_writes(&recs[0]),
            "the content store must reject guest writes"
        );
        assert!(
            !rejects_guest_writes(&recs[1]),
            "a writable bind must accept guest writes"
        );
        assert!(
            rejects_guest_writes(&recs[2]),
            "a read-only bind must reject guest writes"
        );
    }

    #[tokio::test]
    async fn a_bind_is_mapped_even_when_the_host_cannot_look_up_the_workload_gid() {
        let d = tempfile::TempDir::new().unwrap();
        let layout = SocketLayout::for_run_dir(d.path());
        let proj = tempfile::TempDir::new().unwrap();
        let mut s = spec(d.path());
        s.workload_uid = Some(0);
        s.workload_gid = None;
        s.binds = vec![bind(proj.path(), "/work", false)];
        let spawner = FakeSpawner::ready();
        let running = launch(&spawner, &s, &bins(), &layout, None, &fast())
            .await
            .expect("a bind boots without a known gid");

        assert_eq!(running.virtiofsd.len(), 2);
        let bind_args = &spawner.records()[1].args;
        assert!(
            bind_args.iter().any(|a| a == "--sandbox=namespace")
                && bind_args.iter().any(|a| a.starts_with("--uid-map=:0:"))
                && bind_args.iter().any(|a| a.starts_with("--gid-map=:0:")),
            "1:1 passthrough would make the guest's own write EPERM against virtiofsd's unprivileged user: {bind_args:?}"
        );
    }

    #[tokio::test]
    async fn launch_kills_every_virtiofsd_when_a_bind_socket_never_appears() {
        let d = tempfile::TempDir::new().unwrap();
        let layout = SocketLayout::for_run_dir(d.path());
        let proj = tempfile::TempDir::new().unwrap();
        let mut s = spec(d.path());
        s.binds = vec![bind(proj.path(), "/work", false)];
        let spawner = FakeSpawner::suppressing(layout.bind_virtiofsd(0));
        let err = launch(&spawner, &s, &bins(), &layout, None, &fast())
            .await
            .map(|_| ())
            .expect_err("a hung bind virtiofsd must abort the launch");

        assert!(
            format!("{err:#}").contains("virtiofsd did not expose its socket"),
            "got {err:#}"
        );
        let recs = spawner.records();
        assert_eq!(
            recs.len(),
            2,
            "the content and bind virtiofsd are spawned; cloud-hypervisor never starts"
        );
        assert!(
            recs.iter().all(|r| r.killed.load(Ordering::SeqCst)),
            "both the content and the bind virtiofsd are torn down"
        );
        assert!(
            recs.iter().all(|r| r.waited.load(Ordering::SeqCst)),
            "both the content and the bind virtiofsd are reaped after kill"
        );
    }

    #[tokio::test]
    async fn launch_bails_before_cloud_hypervisor_when_a_bind_source_is_missing() {
        let d = tempfile::TempDir::new().unwrap();
        let layout = SocketLayout::for_run_dir(d.path());
        let mut s = spec(d.path());
        s.binds = vec![bind(Path::new("/no/such/host/path"), "/work", false)];
        let spawner = FakeSpawner::ready();
        let err = launch(&spawner, &s, &bins(), &layout, None, &fast())
            .await
            .map(|_| ())
            .expect_err("a missing bind source must abort the launch");

        assert!(
            format!("{err:#}").contains("host bind source is not accessible"),
            "got {err:#}"
        );
        let recs = spawner.records();
        assert_eq!(
            recs.len(),
            1,
            "only the content virtiofsd is spawned before the bad bind is rejected"
        );
        assert!(
            recs[0].killed.load(Ordering::SeqCst),
            "the content virtiofsd is torn down when a later bind fails"
        );
        assert!(
            recs[0].waited.load(Ordering::SeqCst),
            "the content virtiofsd is reaped after kill when a later bind fails"
        );
    }
}
