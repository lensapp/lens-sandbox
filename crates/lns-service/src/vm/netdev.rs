#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use std::ffi::OsString;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};

use super::cloud_hypervisor::process::{Child, Spawner};
use super::cloud_hypervisor::vmm_bin::which;
use crate::download::{PinnedArtifact, RealFetcher, RealFs, ensure_pinned};

#[cfg(target_os = "macos")]
pub mod real;

pub const GVPROXY_VERSION: &str = "0.8.9";

/// The `gvproxy-darwin` asset of the pinned release; `sha256sums` from the same release is the only thing that says these bytes are it.
const GVPROXY_DARWIN_SHA256: &str =
    "c6f7b4bc7f21bf810b5cf54e04d979b014c5d96472a03a9e97fe62a00940067c";

const RELEASE_BASE: &str = "https://github.com/containers/gvisor-tap-vsock/releases/download";

const MAX_GVPROXY_BYTES: u64 = 64 * 1024 * 1024;

/// gvproxy serves this subnet, with its gateway and DNS resolver on the first address; the guest learns both over DHCP.
pub const GUEST_SUBNET: &str = "192.168.127.0/24";

/// VZFileHandleNetworkDeviceAttachment refuses an MTU below 1500 and gvproxy defaults to the same value.
pub const GUEST_MTU: u32 = 1500;

/// `sun_path` holds 104 bytes on macOS, so a deep lns home makes a run's socket unbindable rather than slow.
const MAX_UNIX_PATH_BYTES: usize = 104;

const SEND_BUFFER_BYTES: libc::c_int = 1024 * 1024;

/// Vz asks for a receive buffer of at least twice the send buffer, and four times it for throughput.
const RECEIVE_BUFFER_BYTES: libc::c_int = 4 * 1024 * 1024;

/// gvproxy learns the address to answer on from the first datagram it is sent, and consumes this one rather than treating it as a frame.
const VFKIT_HANDSHAKE: &[u8] = b"VFKT";

const SOCKET_POLL_INTERVAL: Duration = Duration::from_millis(10);

pub const START_TIMEOUT: Duration = Duration::from_secs(10);

/// What the guest's link hangs off. `LNS_NETDEV=vmnet` restores the Apple NAT bridge for one release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Gvproxy,
    Vmnet,
}

impl Backend {
    pub fn label(self) -> &'static str {
        match self {
            Backend::Gvproxy => "gvproxy",
            Backend::Vmnet => "vmnet",
        }
    }

    pub fn detail(self) -> &'static str {
        match self {
            Backend::Gvproxy => GUEST_SUBNET,
            Backend::Vmnet => "Apple NAT bridge",
        }
    }

    pub fn from_env(env_get: impl Fn(&str) -> Option<OsString>) -> Result<Self> {
        let Some(value) = env_get("LNS_NETDEV") else {
            return Ok(Backend::Gvproxy);
        };
        match value.to_str() {
            Some("gvproxy") => Ok(Backend::Gvproxy),
            Some("vmnet") => Ok(Backend::Vmnet),
            _ => bail!(
                "LNS_NETDEV={} is not a network backend. Use gvproxy (the default, a per-run \
                 userspace network process) or vmnet (the Apple NAT bridge, kept for one release).",
                PathBuf::from(value).display()
            ),
        }
    }
}

/// How the guest's virtio-net device reaches the host: the Apple NAT bridge, or a datagram socket the backend process holds the other end of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetAttachment {
    Nat,
    DatagramFd(RawFd),
}

/// Where a run's network backend lives, beside the run's vsock socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetLayout {
    /// The unixgram socket gvproxy binds and listens on.
    pub backend: PathBuf,
    /// The unixgram socket the VM's datagram endpoint binds, so gvproxy has an address to answer.
    pub vm: PathBuf,
    pub log: PathBuf,
}

impl NetLayout {
    pub fn for_run_dir(run_dir: &Path) -> Self {
        Self {
            backend: run_dir.join("net.sock"),
            vm: run_dir.join("net-vm.sock"),
            log: run_dir.join("net.log"),
        }
    }

    /// A restarted run reuses its run dir, and binding refuses a socket path that still exists.
    pub fn remove_stale(&self) -> std::io::Result<()> {
        for sock in [&self.backend, &self.vm] {
            match std::fs::remove_file(sock) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    pub fn refuse_paths_no_socket_can_hold(&self) -> Result<()> {
        for sock in [&self.backend, &self.vm] {
            let len = sock.as_os_str().len();
            if len >= MAX_UNIX_PATH_BYTES {
                bail!(
                    "{} is {len} bytes long, and a unix socket path holds at most \
                     {} — move the lns home somewhere shorter with LNS_HOME.",
                    sock.display(),
                    MAX_UNIX_PATH_BYTES - 1
                );
            }
        }
        Ok(())
    }
}

pub fn gvproxy_args(layout: &NetLayout) -> Vec<String> {
    vec![
        "--mtu".to_string(),
        GUEST_MTU.to_string(),
        // Every run would otherwise bind the same host port to forward ssh into a guest that runs no sshd.
        "--ssh-port".to_string(),
        "-1".to_string(),
        "--listen-vfkit".to_string(),
        format!("unixgram://{}", layout.backend.display()),
        "--log-file".to_string(),
        layout.log.display().to_string(),
    ]
}

pub fn gvproxy_url(base: &str) -> String {
    format!("{base}/v{GVPROXY_VERSION}/gvproxy-darwin")
}

pub fn gvproxy_cache_dir(cache_root: &Path) -> PathBuf {
    cache_root.join("gvproxy").join(GVPROXY_VERSION)
}

/// The gvproxy the host already has: named by `LNS_GVPROXY_BIN`, or on `PATH`. `None` leaves the lns-managed copy to answer.
pub fn located_gvproxy(env_get: &impl Fn(&str) -> Option<OsString>) -> Result<Option<PathBuf>> {
    if let Some(value) = env_get("LNS_GVPROXY_BIN") {
        let path = PathBuf::from(value);
        if !path.is_file() {
            bail!(
                "LNS_GVPROXY_BIN={} is not a regular file. Point it at a gvproxy binary, \
                 or unset it to let lns use the one it manages under ~/.lns/gvproxy.",
                path.display()
            );
        }
        return Ok(Some(path));
    }
    Ok(which("gvproxy", env_get))
}

pub async fn ensure_gvproxy(
    cache_root: &Path,
    env_get: impl Fn(&str) -> Option<OsString>,
) -> Result<PathBuf> {
    if let Some(found) = located_gvproxy(&env_get)? {
        return Ok(found);
    }
    let base = env_get("LNS_GVPROXY_CDN")
        .and_then(|v| v.into_string().ok())
        .unwrap_or_else(|| RELEASE_BASE.to_string());
    let url = gvproxy_url(&base);
    let label = format!("gvproxy v{GVPROXY_VERSION}");
    ensure_pinned(
        &RealFetcher {
            max_bytes: MAX_GVPROXY_BYTES,
            method: lns_ipc::Method::AssetDownload,
        },
        &RealFs,
        &gvproxy_cache_dir(cache_root),
        &PinnedArtifact {
            filename: "gvproxy",
            url: &url,
            sha256: GVPROXY_DARWIN_SHA256,
            mode: Some(0o755),
            label: &label,
        },
    )
    .await
}

/// The VM's end of the link: a datagram socket bound in the run dir and connected to gvproxy, handed to Vz as a file handle.
pub fn connect_datagram(layout: &NetLayout) -> Result<OwnedFd> {
    let socket = UnixDatagram::bind(&layout.vm)
        .with_context(|| format!("binding the guest network socket {}", layout.vm.display()))?;
    set_buffer_sizes(&socket)?;
    socket
        .connect(&layout.backend)
        .with_context(|| format!("connecting to gvproxy at {}", layout.backend.display()))?;
    socket
        .send(VFKIT_HANDSHAKE)
        .context("announcing this VM's address to gvproxy")?;
    Ok(OwnedFd::from(socket))
}

fn set_buffer_sizes(socket: &UnixDatagram) -> Result<()> {
    for (option, bytes) in [
        (libc::SO_SNDBUF, SEND_BUFFER_BYTES),
        (libc::SO_RCVBUF, RECEIVE_BUFFER_BYTES),
    ] {
        // SAFETY: the fd is owned by `socket` and outlives the call; the value is one initialised c_int of the length we pass.
        let rc = unsafe {
            libc::setsockopt(
                socket.as_raw_fd(),
                libc::SOL_SOCKET,
                option,
                std::ptr::from_ref(&bytes).cast(),
                std::mem::size_of_val(&bytes) as libc::socklen_t,
            )
        };
        if rc != 0 {
            return Err(std::io::Error::last_os_error())
                .context("sizing the guest network socket buffers Vz requires");
        }
    }
    Ok(())
}

pub(crate) struct RunningNetDev<C: Child> {
    pub(crate) child: C,
    pub(crate) fd: OwnedFd,
}

pub(crate) async fn start_with<S: Spawner>(
    spawner: &S,
    program: &Path,
    layout: &NetLayout,
    timeout: Duration,
    connect: impl Fn(&NetLayout) -> Result<OwnedFd>,
) -> Result<RunningNetDev<S::Child>> {
    layout.refuse_paths_no_socket_can_hold()?;
    layout.remove_stale()?;
    let mut child = spawner
        .spawn(program, &gvproxy_args(layout))
        .with_context(|| format!("spawning gvproxy at {}", program.display()))?;
    if let Err(e) = wait_for_socket(&layout.backend, timeout).await {
        reap(&mut child).await;
        return Err(e.context("gvproxy did not expose its vfkit socket"));
    }
    match connect(layout) {
        Ok(fd) => Ok(RunningNetDev { child, fd }),
        Err(e) => {
            reap(&mut child).await;
            Err(e)
        }
    }
}

async fn reap<C: Child>(child: &mut C) {
    let _ = child.start_kill();
    let _ = child.wait().await;
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
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    fn layout_in(dir: &Path) -> NetLayout {
        NetLayout::for_run_dir(dir)
    }

    #[test]
    fn the_default_backend_serves_the_guest_from_a_per_run_gvproxy() {
        let backend = Backend::from_env(|_| None).expect("an unset LNS_NETDEV decides nothing");
        assert_eq!(backend, Backend::Gvproxy);
        assert_eq!(backend.label(), "gvproxy");
        assert_eq!(backend.detail(), GUEST_SUBNET);
    }

    #[test]
    fn lns_netdev_vmnet_restores_the_apple_nat_bridge() {
        let backend = Backend::from_env(|k| (k == "LNS_NETDEV").then(|| OsString::from("vmnet")))
            .expect("vmnet is the documented escape hatch");
        assert_eq!(backend, Backend::Vmnet);
        assert_eq!(backend.label(), "vmnet");
        assert_eq!(backend.detail(), "Apple NAT bridge");
    }

    #[test]
    fn lns_netdev_gvproxy_names_the_default_explicitly() {
        let backend = Backend::from_env(|k| (k == "LNS_NETDEV").then(|| OsString::from("gvproxy")))
            .expect("naming the default is allowed");
        assert_eq!(backend, Backend::Gvproxy);
    }

    #[test]
    fn an_unknown_backend_is_refused_by_name_with_both_choices() {
        let err = Backend::from_env(|k| (k == "LNS_NETDEV").then(|| OsString::from("passt")))
            .expect_err("a typo must not silently pick a backend");
        let msg = format!("{err:#}");
        assert!(msg.contains("passt"), "names what was asked for: {msg}");
        assert!(msg.contains("gvproxy"), "names the default: {msg}");
        assert!(msg.contains("vmnet"), "names the escape hatch: {msg}");
    }

    #[test]
    fn a_backend_name_that_is_not_text_is_refused_like_any_other_typo() {
        use std::os::unix::ffi::OsStringExt;
        let err = Backend::from_env(|k| {
            (k == "LNS_NETDEV").then(|| OsString::from_vec(vec![0x66, 0x80, 0x6f]))
        })
        .expect_err("bytes that are not a backend name decide nothing");
        assert!(format!("{err:#}").contains("LNS_NETDEV"));
    }

    #[test]
    fn the_backend_sockets_live_in_the_run_dir_beside_the_vsock_socket() {
        let layout = layout_in(Path::new("/cache/runs/7"));
        assert_eq!(layout.backend, PathBuf::from("/cache/runs/7/net.sock"));
        assert_eq!(layout.vm, PathBuf::from("/cache/runs/7/net-vm.sock"));
        assert_eq!(layout.log, PathBuf::from("/cache/runs/7/net.log"));
    }

    #[test]
    fn remove_stale_clears_both_sockets_a_previous_boot_left_behind() {
        let d = tempfile::TempDir::new().unwrap();
        let layout = layout_in(d.path());
        std::fs::write(&layout.backend, b"stale").unwrap();
        std::fs::write(&layout.vm, b"stale").unwrap();
        std::fs::write(d.path().join("upper.img"), b"keep").unwrap();

        layout.remove_stale().unwrap();

        assert!(!layout.backend.exists());
        assert!(!layout.vm.exists());
        assert!(d.path().join("upper.img").exists(), "only sockets go");
    }

    #[test]
    fn remove_stale_on_a_clean_run_dir_is_a_no_op() {
        let d = tempfile::TempDir::new().unwrap();
        layout_in(d.path()).remove_stale().unwrap();
    }

    #[test]
    fn remove_stale_surfaces_a_socket_it_cannot_unlink() {
        let d = tempfile::TempDir::new().unwrap();
        let layout = layout_in(d.path());
        std::fs::create_dir(&layout.backend).unwrap();
        std::fs::write(layout.backend.join("occupant"), b"x").unwrap();
        assert!(layout.remove_stale().is_err());
    }

    #[test]
    fn a_run_dir_too_deep_for_a_socket_path_is_refused_before_anything_is_spawned() {
        let deep = PathBuf::from("/").join("d".repeat(120));
        let err = layout_in(&deep)
            .refuse_paths_no_socket_can_hold()
            .expect_err("a path no socket can hold must be named, not attempted");
        let msg = format!("{err:#}");
        assert!(msg.contains("unix socket path"), "{msg}");
        assert!(msg.contains("LNS_HOME"), "names the remedy: {msg}");
    }

    #[test]
    fn an_ordinary_run_dir_holds_both_socket_paths() {
        layout_in(Path::new("/Users/someone/.lns/runs/9e8d7c6b0000"))
            .refuse_paths_no_socket_can_hold()
            .expect("the shipped layout fits");
    }

    #[test]
    fn gvproxy_serves_the_vfkit_socket_at_the_mtu_vz_accepts_and_publishes_no_host_port() {
        let args = gvproxy_args(&layout_in(Path::new("/cache/runs/7")));
        assert_eq!(
            args,
            vec![
                "--mtu".to_string(),
                "1500".to_string(),
                "--ssh-port".to_string(),
                "-1".to_string(),
                "--listen-vfkit".to_string(),
                "unixgram:///cache/runs/7/net.sock".to_string(),
                "--log-file".to_string(),
                "/cache/runs/7/net.log".to_string(),
            ]
        );
    }

    #[test]
    fn the_pinned_asset_is_the_darwin_build_of_the_pinned_release() {
        assert_eq!(
            gvproxy_url(RELEASE_BASE),
            "https://github.com/containers/gvisor-tap-vsock/releases/download/v0.8.9/gvproxy-darwin"
        );
        assert_eq!(GVPROXY_DARWIN_SHA256.len(), 64);
        assert!(GVPROXY_DARWIN_SHA256.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn the_managed_copy_is_versioned_so_a_pin_bump_never_reuses_old_bytes() {
        assert_eq!(
            gvproxy_cache_dir(Path::new("/home/someone/.lns")),
            PathBuf::from("/home/someone/.lns/gvproxy/0.8.9")
        );
    }

    fn executable(dir: &Path, name: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, b"#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn lns_gvproxy_bin_is_used_without_consulting_path() {
        let d = tempfile::TempDir::new().unwrap();
        let pinned = executable(d.path(), "my-gvproxy");
        let for_env = pinned.clone();
        let env = move |k: &str| (k == "LNS_GVPROXY_BIN").then(|| for_env.clone().into_os_string());
        assert_eq!(located_gvproxy(&env).unwrap(), Some(pinned));
    }

    #[test]
    fn lns_gvproxy_bin_naming_nothing_refuses_the_run_rather_than_falling_back() {
        let env =
            |k: &str| (k == "LNS_GVPROXY_BIN").then(|| OsString::from("/does/not/exist/gvproxy"));
        let err = located_gvproxy(&env).expect_err("a wrong override must not be papered over");
        let msg = format!("{err:#}");
        assert!(msg.contains("LNS_GVPROXY_BIN"), "{msg}");
        assert!(msg.contains("/does/not/exist/gvproxy"), "{msg}");
        assert!(msg.contains("~/.lns/gvproxy"), "names the fallback: {msg}");
    }

    #[test]
    fn a_gvproxy_on_path_is_preferred_over_downloading_one() {
        let d = tempfile::TempDir::new().unwrap();
        let on_path = executable(d.path(), "gvproxy");
        let dir = d.path().to_path_buf();
        let env = move |k: &str| (k == "PATH").then(|| dir.clone().into_os_string());
        assert_eq!(located_gvproxy(&env).unwrap(), Some(on_path));
    }

    #[test]
    fn nothing_on_the_host_leaves_the_managed_copy_to_answer() {
        let d = tempfile::TempDir::new().unwrap();
        let dir = d.path().to_path_buf();
        let env = move |k: &str| (k == "PATH").then(|| dir.clone().into_os_string());
        assert_eq!(located_gvproxy(&env).unwrap(), None);
    }

    #[tokio::test]
    async fn ensure_returns_the_located_binary_without_fetching_anything() {
        let d = tempfile::TempDir::new().unwrap();
        let pinned = executable(d.path(), "my-gvproxy");
        let for_env = pinned.clone();
        let env = move |k: &str| (k == "LNS_GVPROXY_BIN").then(|| for_env.clone().into_os_string());
        let cache = tempfile::TempDir::new().unwrap();
        assert_eq!(ensure_gvproxy(cache.path(), env).await.unwrap(), pinned);
    }

    #[tokio::test]
    async fn a_download_whose_bytes_are_not_the_pin_refuses_to_install_them() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .and(wiremock::matchers::path(format!(
                "/v{GVPROXY_VERSION}/gvproxy-darwin"
            )))
            .respond_with(
                wiremock::ResponseTemplate::new(200).set_body_bytes(b"not gvproxy".as_slice()),
            )
            .expect(1)
            .mount(&server)
            .await;
        let cache = tempfile::TempDir::new().unwrap();
        let base = server.uri();
        let env = move |k: &str| match k {
            "LNS_GVPROXY_CDN" => Some(OsString::from(base.clone())),
            _ => None,
        };

        let err = ensure_gvproxy(cache.path(), env)
            .await
            .expect_err("bytes that are not the pinned gvproxy must not run a guest's network");

        let msg = format!("{err:#}");
        assert!(msg.contains("sha256 mismatch"), "{msg}");
        assert!(msg.contains("Refusing to install"), "{msg}");
        assert!(
            !gvproxy_cache_dir(cache.path()).join("gvproxy").exists(),
            "unverified bytes must never land at the managed path"
        );
    }

    #[test]
    fn the_vm_socket_announces_its_own_address_so_gvproxy_can_answer_it() {
        let d = tempfile::TempDir::new().unwrap();
        let layout = layout_in(d.path());
        let gvproxy = UnixDatagram::bind(&layout.backend).expect("stand in for gvproxy");

        let fd = connect_datagram(&layout).expect("the VM end connects");

        let mut buf = [0u8; 8];
        let (read, from) = gvproxy.recv_from(&mut buf).expect("the handshake arrives");
        assert_eq!(&buf[..read], VFKIT_HANDSHAKE);
        assert_eq!(
            from.as_pathname(),
            Some(layout.vm.as_path()),
            "gvproxy learns the address to answer on from this datagram alone"
        );
        assert!(fd.as_raw_fd() >= 0);
    }

    #[test]
    fn a_socket_path_already_taken_surfaces_which_path_could_not_be_bound() {
        let d = tempfile::TempDir::new().unwrap();
        let layout = layout_in(d.path());
        std::fs::write(&layout.vm, b"occupied").unwrap();

        let err = connect_datagram(&layout).expect_err("bind must not silently reuse a path");

        let msg = format!("{err:#}");
        assert!(msg.contains("binding the guest network socket"), "{msg}");
        assert!(msg.contains("net-vm.sock"), "{msg}");
    }

    #[test]
    fn a_backend_that_is_not_listening_is_reported_against_its_own_path() {
        let d = tempfile::TempDir::new().unwrap();
        let layout = layout_in(d.path());

        let err = connect_datagram(&layout).expect_err("there is nothing to connect to");

        let msg = format!("{err:#}");
        assert!(msg.contains("connecting to gvproxy"), "{msg}");
        assert!(msg.contains("net.sock"), "{msg}");
    }

    #[test]
    fn the_socket_buffers_vz_requires_are_sized_before_the_handshake() {
        let d = tempfile::TempDir::new().unwrap();
        let layout = layout_in(d.path());
        let _gvproxy = UnixDatagram::bind(&layout.backend).expect("stand in for gvproxy");
        let fd = connect_datagram(&layout).expect("the VM end connects");

        let mut value: libc::c_int = 0;
        let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
        // SAFETY: `fd` is open for the whole call and `value`/`len` are initialised out-params of the size we declare.
        let rc = unsafe {
            libc::getsockopt(
                fd.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_SNDBUF,
                std::ptr::from_mut(&mut value).cast(),
                &mut len,
            )
        };
        assert_eq!(rc, 0, "the send buffer must be readable back");
        assert!(
            value > 0,
            "Vz reads frames out of this buffer; a default-sized one drops them"
        );
    }

    #[test]
    fn set_buffer_sizes_surfaces_a_kernel_refusal_rather_than_a_silent_drop() {
        let (socket, _peer) = UnixDatagram::pair().unwrap();
        let raw = socket.as_raw_fd();
        drop(socket);
        // SAFETY: the fd is closed, so every setsockopt on it is EBADF; nothing else can have reopened it in this statement.
        let closed = unsafe { <UnixDatagram as std::os::fd::FromRawFd>::from_raw_fd(raw) };
        let err = set_buffer_sizes(&closed).expect_err("a closed fd cannot be sized");
        std::mem::forget(closed);
        assert!(
            format!("{err:#}").contains("guest network socket buffers"),
            "{err:#}"
        );
    }

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

    type SpawnLog = Arc<Mutex<Vec<(PathBuf, Vec<String>)>>>;

    struct FakeSpawner {
        /// The socket a spawn creates, standing in for gvproxy binding its listener.
        creates: Option<PathBuf>,
        fails: bool,
        spawned: SpawnLog,
        killed: Arc<AtomicBool>,
        waited: Arc<AtomicBool>,
    }

    impl FakeSpawner {
        fn listening(socket: &Path) -> Self {
            Self {
                creates: Some(socket.to_path_buf()),
                fails: false,
                spawned: Arc::new(Mutex::new(Vec::new())),
                killed: Arc::new(AtomicBool::new(false)),
                waited: Arc::new(AtomicBool::new(false)),
            }
        }
        fn silent() -> Self {
            Self {
                creates: None,
                fails: false,
                spawned: Arc::new(Mutex::new(Vec::new())),
                killed: Arc::new(AtomicBool::new(false)),
                waited: Arc::new(AtomicBool::new(false)),
            }
        }
        fn failing() -> Self {
            Self {
                creates: None,
                fails: true,
                spawned: Arc::new(Mutex::new(Vec::new())),
                killed: Arc::new(AtomicBool::new(false)),
                waited: Arc::new(AtomicBool::new(false)),
            }
        }
    }

    impl Spawner for FakeSpawner {
        type Child = FakeChild;

        fn spawn(&self, program: &Path, args: &[String]) -> std::io::Result<FakeChild> {
            self.spawned
                .lock()
                .unwrap()
                .push((program.to_path_buf(), args.to_vec()));
            if self.fails {
                return Err(std::io::Error::from(std::io::ErrorKind::NotFound));
            }
            if let Some(socket) = &self.creates {
                UnixDatagram::bind(socket).map(std::mem::forget)?;
            }
            Ok(FakeChild {
                killed: Arc::clone(&self.killed),
                waited: Arc::clone(&self.waited),
            })
        }
    }

    #[tokio::test]
    async fn a_started_backend_hands_back_the_socket_the_vm_will_be_attached_to() {
        let d = tempfile::TempDir::new().unwrap();
        let layout = layout_in(d.path());
        let spawner = FakeSpawner::listening(&layout.backend);

        let running = start_with(
            &spawner,
            Path::new("/usr/local/bin/gvproxy"),
            &layout,
            START_TIMEOUT,
            connect_datagram,
        )
        .await
        .expect("the backend comes up");

        assert!(running.fd.as_raw_fd() >= 0);
        let spawned = spawner.spawned.lock().unwrap().clone();
        assert_eq!(spawned.len(), 1);
        assert_eq!(spawned[0].0, PathBuf::from("/usr/local/bin/gvproxy"));
        assert_eq!(spawned[0].1, gvproxy_args(&layout));
        assert!(
            !spawner.killed.load(Ordering::SeqCst),
            "a healthy backend stays up"
        );
    }

    #[tokio::test]
    async fn a_previous_boots_sockets_do_not_stop_the_next_one() {
        let d = tempfile::TempDir::new().unwrap();
        let layout = layout_in(d.path());
        std::fs::write(&layout.backend, b"stale").unwrap();
        std::fs::write(&layout.vm, b"stale").unwrap();
        let spawner = FakeSpawner::listening(&layout.backend);

        start_with(
            &spawner,
            Path::new("/usr/local/bin/gvproxy"),
            &layout,
            START_TIMEOUT,
            connect_datagram,
        )
        .await
        .expect("a restart reuses its run dir");
    }

    #[tokio::test]
    async fn a_backend_that_cannot_be_spawned_names_the_binary_it_tried() {
        let d = tempfile::TempDir::new().unwrap();
        let layout = layout_in(d.path());

        let Err(err) = start_with(
            &FakeSpawner::failing(),
            Path::new("/nowhere/gvproxy"),
            &layout,
            START_TIMEOUT,
            connect_datagram,
        )
        .await
        else {
            panic!("a missing binary refuses the run");
        };

        assert!(format!("{err:#}").contains("/nowhere/gvproxy"), "{err:#}");
    }

    #[tokio::test]
    async fn a_backend_that_never_listens_is_reaped_rather_than_left_behind() {
        let d = tempfile::TempDir::new().unwrap();
        let layout = layout_in(d.path());
        let spawner = FakeSpawner::silent();

        let Err(err) = start_with(
            &spawner,
            Path::new("/usr/local/bin/gvproxy"),
            &layout,
            Duration::from_millis(30),
            connect_datagram,
        )
        .await
        else {
            panic!("no socket means no guest network");
        };

        assert!(
            format!("{err:#}").contains("did not expose its vfkit socket"),
            "{err:#}"
        );
        assert!(spawner.killed.load(Ordering::SeqCst), "the child is killed");
        assert!(spawner.waited.load(Ordering::SeqCst), "and reaped");
    }

    #[tokio::test]
    async fn a_vm_end_that_cannot_be_connected_reaps_the_backend_it_started() {
        let d = tempfile::TempDir::new().unwrap();
        let layout = layout_in(d.path());
        let spawner = FakeSpawner::listening(&layout.backend);

        let Err(err) = start_with(
            &spawner,
            Path::new("/usr/local/bin/gvproxy"),
            &layout,
            START_TIMEOUT,
            |_| anyhow::bail!("simulated attach failure"),
        )
        .await
        else {
            panic!("a VM end that cannot attach is not a running backend");
        };

        assert!(format!("{err:#}").contains("simulated attach failure"));
        assert!(spawner.killed.load(Ordering::SeqCst), "the child is killed");
        assert!(spawner.waited.load(Ordering::SeqCst), "and reaped");
    }

    #[tokio::test]
    async fn a_run_dir_too_deep_for_a_socket_refuses_before_gvproxy_is_spawned() {
        let layout = layout_in(&PathBuf::from("/").join("d".repeat(120)));
        let spawner = FakeSpawner::silent();

        let Err(err) = start_with(
            &spawner,
            Path::new("/usr/local/bin/gvproxy"),
            &layout,
            START_TIMEOUT,
            connect_datagram,
        )
        .await
        else {
            panic!("a path no socket can hold refuses the run");
        };

        assert!(format!("{err:#}").contains("unix socket path"), "{err:#}");
        assert!(
            spawner.spawned.lock().unwrap().is_empty(),
            "nothing is spawned for a run that cannot have a socket"
        );
    }
}
