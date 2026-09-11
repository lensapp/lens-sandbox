// Each platform compiles the other's backend helpers too; only its own are reachable from a run.
#![cfg_attr(any(target_os = "macos", target_os = "linux"), allow(dead_code))]

use std::ffi::OsString;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};

use super::cloud_hypervisor::process::{Child, Spawner};
use super::cloud_hypervisor::vmm_bin::{is_executable_file, which};
use crate::download::{PinnedArtifact, RealFetcher, RealFs, ensure_pinned};

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub mod real;

pub const GVPROXY_VERSION: &str = "0.8.9";

/// The `gvproxy-darwin` asset of the pinned release; `sha256sums` from the same release is the only thing that says these bytes are it.
const GVPROXY_DARWIN_SHA256: &str =
    "c6f7b4bc7f21bf810b5cf54e04d979b014c5d96472a03a9e97fe62a00940067c";

const RELEASE_BASE: &str = "https://github.com/containers/gvisor-tap-vsock/releases/download";

const MAX_GVPROXY_BYTES: u64 = 64 * 1024 * 1024;

/// gvproxy and passt both serve this subnet, with the gateway and DNS resolver on the first address; the guest learns both over DHCP.
pub const GUEST_SUBNET: &str = "192.168.127.0/24";

/// The address the backend answers as gateway and as resolver.
pub const GUEST_GATEWAY: &str = "192.168.127.1";

/// passt hands the guest the host's own address unless told otherwise, which leaves a guest unable to tell host traffic from its own.
pub const GUEST_ADDRESS: &str = "192.168.127.2";

pub const GUEST_PREFIX_LEN: &str = "24";

/// VZFileHandleNetworkDeviceAttachment refuses an MTU below 1500 and gvproxy defaults to the same value.
pub const GUEST_MTU: u32 = 1500;

/// Where the distro packages put passt; the service must find one without a `PATH` that carries `/usr/sbin`.
const PACKAGED_PASST_DIRS: &[&str] =
    &["/usr/bin", "/usr/local/bin", "/usr/sbin", "/usr/local/sbin"];

/// `sun_path` holds 104 bytes on macOS, so a deep lns home makes a run's socket unbindable rather than slow.
const MAX_UNIX_PATH_BYTES: usize = 104;

const SEND_BUFFER_BYTES: libc::c_int = 1024 * 1024;

/// Vz asks for a receive buffer of at least twice the send buffer, and four times it for throughput.
const RECEIVE_BUFFER_BYTES: libc::c_int = 4 * 1024 * 1024;

/// gvproxy learns the address to answer on from the first datagram it is sent, and consumes this one rather than treating it as a frame.
const VFKIT_HANDSHAKE: &[u8] = b"VFKT";

const SOCKET_POLL_INTERVAL: Duration = Duration::from_millis(10);

pub const START_TIMEOUT: Duration = Duration::from_secs(10);

/// The platform whose backends a run may choose from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Host {
    MacOs,
    Linux,
}

impl Host {
    pub const THIS: Host = if cfg!(target_os = "macos") {
        Host::MacOs
    } else {
        Host::Linux
    };

    fn backends(self) -> &'static [Backend] {
        match self {
            Host::MacOs => &[Backend::Gvproxy, Backend::Vmnet],
            Host::Linux => &[Backend::Passt, Backend::Off],
        }
    }

    fn default_backend(self) -> Backend {
        self.backends()[0]
    }

    fn choices_phrase(self) -> String {
        self.backends()
            .iter()
            .map(|b| b.choice_phrase())
            .collect::<Vec<_>>()
            .join(" or ")
    }
}

/// What the guest's link hangs off. `LNS_NETDEV` names one for a run: `vmnet` restores the Apple NAT bridge, `none` the Linux guest with no device at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Gvproxy,
    Passt,
    Vmnet,
    Off,
}

impl Backend {
    pub fn label(self) -> &'static str {
        match self {
            Backend::Gvproxy => "gvproxy",
            Backend::Passt => "passt",
            Backend::Vmnet => "vmnet",
            Backend::Off => "none",
        }
    }

    pub fn detail(self) -> &'static str {
        match self {
            Backend::Gvproxy | Backend::Passt => GUEST_SUBNET,
            Backend::Vmnet => "Apple NAT bridge",
            Backend::Off => "no guest network",
        }
    }

    fn choice_phrase(self) -> String {
        let what = match self {
            Backend::Gvproxy | Backend::Passt => {
                "the default, a per-run userspace network process".to_string()
            }
            Backend::Vmnet => "the Apple NAT bridge, kept for one release".to_string(),
            Backend::Off => "no network device at all, kept for one release".to_string(),
        };
        format!("{} ({what})", self.label())
    }

    pub fn from_env(host: Host, env_get: impl Fn(&str) -> Option<OsString>) -> Result<Self> {
        let Some(value) = env_get("LNS_NETDEV") else {
            return Ok(host.default_backend());
        };
        if let Some(found) = host
            .backends()
            .iter()
            .find(|b| Some(b.label()) == value.to_str())
        {
            return Ok(*found);
        }
        bail!(
            "LNS_NETDEV={} is not a network backend on this host. Use {}.",
            PathBuf::from(value).display(),
            host.choices_phrase()
        )
    }
}

/// How the guest's virtio-net device reaches the host on macOS: the Apple NAT bridge, or a datagram socket the backend process holds the other end of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetAttachment {
    Nat,
    DatagramFd(RawFd),
}

/// How the guest's virtio-net device reaches the host on Linux: cloud-hypervisor connects to the vhost-user socket the run's passt bound.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VhostUserNet {
    pub socket: PathBuf,
    pub mac: String,
}

impl VhostUserNet {
    /// cloud-hypervisor's default `vhost_mode=client` connects to the socket passt binds, so lns never has to bind it itself.
    pub fn cloud_hypervisor_arg(&self) -> String {
        format!(
            "vhost_user=true,socket={},mac={}",
            self.socket.display(),
            self.mac
        )
    }
}

/// Where a run's network backend lives, beside the run's vsock socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetLayout {
    /// The socket the backend binds: unixgram for gvproxy, vhost-user for passt.
    pub backend: PathBuf,
    /// The unixgram socket the VM's datagram endpoint binds, so gvproxy has an address to answer.
    pub vm: PathBuf,
    pub log: PathBuf,
    pub pid: PathBuf,
}

impl NetLayout {
    pub fn for_run_dir(run_dir: &Path) -> Self {
        Self {
            backend: run_dir.join("net.sock"),
            vm: run_dir.join("net-vm.sock"),
            log: run_dir.join("net.log"),
            pid: run_dir.join("net.pid"),
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

/// passt serves the guest a private address rather than the host's own, so nothing in the guest can mistake host traffic for its own, and answers DNS on the gateway address it advertises.
pub fn passt_args(layout: &NetLayout) -> Vec<String> {
    vec![
        "--vhost-user".to_string(),
        "--socket".to_string(),
        layout.backend.display().to_string(),
        "--foreground".to_string(),
        "--pid".to_string(),
        layout.pid.display().to_string(),
        "--log-file".to_string(),
        layout.log.display().to_string(),
        "--ipv4-only".to_string(),
        "--mtu".to_string(),
        GUEST_MTU.to_string(),
        "--address".to_string(),
        GUEST_ADDRESS.to_string(),
        "--netmask".to_string(),
        GUEST_PREFIX_LEN.to_string(),
        "--gateway".to_string(),
        GUEST_GATEWAY.to_string(),
        "--dns".to_string(),
        GUEST_GATEWAY.to_string(),
        "--dns-forward".to_string(),
        GUEST_GATEWAY.to_string(),
        // The gateway address would otherwise reach the host's own loopback services, past every egress rule.
        "--no-map-gw".to_string(),
        "--tcp-ports".to_string(),
        "none".to_string(),
        "--udp-ports".to_string(),
        "none".to_string(),
    ]
}

/// Clearing the multicast bit and setting the local one makes an address no vendor owns, so a run can never collide with real hardware.
pub fn locally_administered_mac(bytes: [u8; 6]) -> String {
    let first = (bytes[0] & 0xfe) | 0x02;
    format!(
        "{first:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        bytes[1], bytes[2], bytes[3], bytes[4], bytes[5]
    )
}

pub fn random_guest_mac() -> String {
    locally_administered_mac(rand::random())
}

pub fn packaged_passt_dirs() -> Vec<PathBuf> {
    PACKAGED_PASST_DIRS.iter().map(PathBuf::from).collect()
}

/// The passt that serves a Linux guest: named by `LNS_PASST_BIN`, on `PATH`, or where a distro package puts it. There is no verified build to fetch, so a host without one is refused.
pub fn located_passt(env_get: &impl Fn(&str) -> Option<OsString>) -> Result<PathBuf> {
    located_passt_in(&packaged_passt_dirs(), env_get)
}

pub fn located_passt_in(
    dirs: &[PathBuf],
    env_get: &impl Fn(&str) -> Option<OsString>,
) -> Result<PathBuf> {
    if let Some(value) = env_get("LNS_PASST_BIN") {
        let path = PathBuf::from(value);
        if !path.is_file() {
            bail!(
                "LNS_PASST_BIN={} is not a regular file. Point it at a passt binary, \
                 or unset it to let lns use the one this host has installed.",
                path.display()
            );
        }
        return Ok(path);
    }
    if let Some(found) = which("passt", env_get) {
        return Ok(found);
    }
    if let Some(found) = dirs
        .iter()
        .map(|dir| dir.join("passt"))
        .find(|path| is_executable_file(path))
    {
        return Ok(found);
    }
    bail!(
        "passt is not installed. lns serves each Linux guest's network with a per-run passt, \
         and there is no verified static build to fetch. Install it with `apt install passt` \
         or `dnf install passt`, or point LNS_PASST_BIN at a passt binary. \
         LNS_NETDEV=none boots the guest with no network device at all."
    )
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

/// One backend process to start: what to run, and the words a failure to start it is reported in.
struct BackendProcess<'a> {
    program: &'a Path,
    args: Vec<String>,
    label: &'a str,
    socket_kind: &'a str,
}

pub(crate) async fn start_with<S: Spawner>(
    spawner: &S,
    program: &Path,
    layout: &NetLayout,
    timeout: Duration,
    connect: impl Fn(&NetLayout) -> Result<OwnedFd>,
) -> Result<RunningNetDev<S::Child>> {
    let mut child = spawn_and_wait(
        spawner,
        &BackendProcess {
            program,
            args: gvproxy_args(layout),
            label: "gvproxy",
            socket_kind: "vfkit",
        },
        layout,
        timeout,
    )
    .await?;
    match connect(layout) {
        Ok(fd) => Ok(RunningNetDev { child, fd }),
        Err(e) => {
            reap(&mut child).await;
            Err(e)
        }
    }
}

/// passt binds the vhost-user socket itself, so it has to be listening before cloud-hypervisor connects to it as a client.
pub(crate) async fn start_passt_with<S: Spawner>(
    spawner: &S,
    program: &Path,
    layout: &NetLayout,
    timeout: Duration,
) -> Result<S::Child> {
    spawn_and_wait(
        spawner,
        &BackendProcess {
            program,
            args: passt_args(layout),
            label: "passt",
            socket_kind: "vhost-user",
        },
        layout,
        timeout,
    )
    .await
}

async fn spawn_and_wait<S: Spawner>(
    spawner: &S,
    process: &BackendProcess<'_>,
    layout: &NetLayout,
    timeout: Duration,
) -> Result<S::Child> {
    layout.refuse_paths_no_socket_can_hold()?;
    layout.remove_stale()?;
    let mut child = spawner
        .spawn(process.program, &process.args)
        .with_context(|| {
            format!(
                "spawning {} at {}",
                process.label,
                process.program.display()
            )
        })?;
    if let Err(e) = wait_for_socket(&layout.backend, timeout).await {
        reap(&mut child).await;
        return Err(e.context(format!(
            "{} did not expose its {} socket",
            process.label, process.socket_kind
        )));
    }
    Ok(child)
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
    fn the_default_backend_on_macos_serves_the_guest_from_a_per_run_gvproxy() {
        let backend =
            Backend::from_env(Host::MacOs, |_| None).expect("an unset LNS_NETDEV decides nothing");
        assert_eq!(backend, Backend::Gvproxy);
        assert_eq!(backend.label(), "gvproxy");
        assert_eq!(backend.detail(), GUEST_SUBNET);
    }

    #[test]
    fn the_default_backend_on_linux_serves_the_guest_from_a_per_run_passt() {
        let backend =
            Backend::from_env(Host::Linux, |_| None).expect("an unset LNS_NETDEV decides nothing");
        assert_eq!(backend, Backend::Passt);
        assert_eq!(backend.label(), "passt");
        assert_eq!(backend.detail(), GUEST_SUBNET);
    }

    #[test]
    fn lns_netdev_vmnet_restores_the_apple_nat_bridge() {
        let backend = Backend::from_env(Host::MacOs, |k| {
            (k == "LNS_NETDEV").then(|| OsString::from("vmnet"))
        })
        .expect("vmnet is the documented escape hatch");
        assert_eq!(backend, Backend::Vmnet);
        assert_eq!(backend.label(), "vmnet");
        assert_eq!(backend.detail(), "Apple NAT bridge");
    }

    #[test]
    fn lns_netdev_none_restores_a_linux_guest_with_no_network_device() {
        let backend = Backend::from_env(Host::Linux, |k| {
            (k == "LNS_NETDEV").then(|| OsString::from("none"))
        })
        .expect("none is the documented escape hatch");
        assert_eq!(backend, Backend::Off);
        assert_eq!(backend.label(), "none");
        assert_eq!(backend.detail(), "no guest network");
    }

    #[test]
    fn lns_netdev_gvproxy_names_the_default_explicitly() {
        let backend = Backend::from_env(Host::MacOs, |k| {
            (k == "LNS_NETDEV").then(|| OsString::from("gvproxy"))
        })
        .expect("naming the default is allowed");
        assert_eq!(backend, Backend::Gvproxy);
    }

    #[test]
    fn lns_netdev_passt_names_the_linux_default_explicitly() {
        let backend = Backend::from_env(Host::Linux, |k| {
            (k == "LNS_NETDEV").then(|| OsString::from("passt"))
        })
        .expect("naming the default is allowed");
        assert_eq!(backend, Backend::Passt);
    }

    #[test]
    fn an_unknown_backend_is_refused_by_name_with_both_choices() {
        let err = Backend::from_env(Host::MacOs, |k| {
            (k == "LNS_NETDEV").then(|| OsString::from("wireguard"))
        })
        .expect_err("a typo must not silently pick a backend");
        let msg = format!("{err:#}");
        assert!(msg.contains("wireguard"), "names what was asked for: {msg}");
        assert!(msg.contains("gvproxy"), "names the default: {msg}");
        assert!(msg.contains("vmnet"), "names the escape hatch: {msg}");
    }

    #[test]
    fn a_backend_the_other_platform_serves_is_refused_with_this_platforms_choices() {
        let err = Backend::from_env(Host::Linux, |k| {
            (k == "LNS_NETDEV").then(|| OsString::from("gvproxy"))
        })
        .expect_err("gvproxy serves no Linux guest");
        let msg = format!("{err:#}");
        assert!(msg.contains("gvproxy"), "names what was asked for: {msg}");
        assert!(msg.contains("passt"), "names the default: {msg}");
        assert!(msg.contains("none"), "names the escape hatch: {msg}");
    }

    #[test]
    fn a_backend_name_that_is_not_text_is_refused_like_any_other_typo() {
        use std::os::unix::ffi::OsStringExt;
        let err = Backend::from_env(Host::MacOs, |k| {
            (k == "LNS_NETDEV").then(|| OsString::from_vec(vec![0x66, 0x80, 0x6f]))
        })
        .expect_err("bytes that are not a backend name decide nothing");
        assert!(format!("{err:#}").contains("LNS_NETDEV"));
    }

    #[test]
    fn this_host_asks_for_a_backend_its_own_platform_serves() {
        let default =
            Backend::from_env(Host::THIS, |_| None).expect("an unset LNS_NETDEV decides nothing");
        assert!(
            Host::THIS.backends().contains(&default),
            "a host must never default to a backend it cannot start: {default:?}"
        );
    }

    #[test]
    fn the_backend_sockets_live_in_the_run_dir_beside_the_vsock_socket() {
        let layout = layout_in(Path::new("/cache/runs/7"));
        assert_eq!(layout.backend, PathBuf::from("/cache/runs/7/net.sock"));
        assert_eq!(layout.vm, PathBuf::from("/cache/runs/7/net-vm.sock"));
        assert_eq!(layout.log, PathBuf::from("/cache/runs/7/net.log"));
        assert_eq!(layout.pid, PathBuf::from("/cache/runs/7/net.pid"));
    }

    #[test]
    fn passt_serves_the_vhost_user_socket_on_a_private_subnet_and_publishes_no_host_port() {
        let args = passt_args(&layout_in(Path::new("/cache/runs/7")));
        assert_eq!(
            args,
            vec![
                "--vhost-user".to_string(),
                "--socket".to_string(),
                "/cache/runs/7/net.sock".to_string(),
                "--foreground".to_string(),
                "--pid".to_string(),
                "/cache/runs/7/net.pid".to_string(),
                "--log-file".to_string(),
                "/cache/runs/7/net.log".to_string(),
                "--ipv4-only".to_string(),
                "--mtu".to_string(),
                "1500".to_string(),
                "--address".to_string(),
                "192.168.127.2".to_string(),
                "--netmask".to_string(),
                "24".to_string(),
                "--gateway".to_string(),
                "192.168.127.1".to_string(),
                "--dns".to_string(),
                "192.168.127.1".to_string(),
                "--dns-forward".to_string(),
                "192.168.127.1".to_string(),
                "--no-map-gw".to_string(),
                "--tcp-ports".to_string(),
                "none".to_string(),
                "--udp-ports".to_string(),
                "none".to_string(),
            ]
        );
    }

    #[test]
    fn the_address_passt_advertises_as_the_resolver_is_one_it_answers_itself() {
        let args = passt_args(&layout_in(Path::new("/cache/runs/7")));
        let value_after = |flag: &str| {
            let at = args.iter().position(|a| a == flag).expect(flag);
            args[at + 1].clone()
        };
        assert_eq!(value_after("--dns"), value_after("--dns-forward"));
        assert_eq!(value_after("--dns"), GUEST_GATEWAY);
    }

    #[test]
    fn a_guest_mac_is_locally_administered_and_unicast_whatever_the_random_bytes_are() {
        assert_eq!(
            locally_administered_mac([0xff, 0x01, 0x02, 0x03, 0x04, 0x05]),
            "fe:01:02:03:04:05"
        );
        assert_eq!(
            locally_administered_mac([0x00, 0xab, 0xcd, 0xef, 0x10, 0x20]),
            "02:ab:cd:ef:10:20"
        );
    }

    #[test]
    fn every_run_is_handed_a_well_formed_mac_no_vendor_owns() {
        let mac = random_guest_mac();
        let octets: Vec<&str> = mac.split(':').collect();
        assert_eq!(octets.len(), 6, "{mac}");
        assert!(
            octets
                .iter()
                .all(|o| o.len() == 2 && o.chars().all(|c| c.is_ascii_hexdigit())),
            "{mac}"
        );
        let first = u8::from_str_radix(octets[0], 16).expect("an octet is hex");
        assert_eq!(
            first & 0x03,
            0x02,
            "locally administered and unicast: {mac}"
        );
    }

    #[test]
    fn the_guest_link_becomes_the_vhost_user_net_device_cloud_hypervisor_connects_to() {
        let net = VhostUserNet {
            socket: PathBuf::from("/cache/runs/7/net.sock"),
            mac: "3a:1b:2c:3d:4e:5f".to_string(),
        };
        assert_eq!(
            net.cloud_hypervisor_arg(),
            "vhost_user=true,socket=/cache/runs/7/net.sock,mac=3a:1b:2c:3d:4e:5f"
        );
    }

    #[test]
    fn lns_passt_bin_is_used_without_consulting_path() {
        let d = tempfile::TempDir::new().unwrap();
        let pinned = executable(d.path(), "my-passt");
        let for_env = pinned.clone();
        let env = move |k: &str| (k == "LNS_PASST_BIN").then(|| for_env.clone().into_os_string());
        assert_eq!(located_passt(&env).unwrap(), pinned);
    }

    #[test]
    fn lns_passt_bin_naming_nothing_refuses_the_run_rather_than_falling_back() {
        let env = |k: &str| (k == "LNS_PASST_BIN").then(|| OsString::from("/does/not/exist/passt"));
        let err = located_passt(&env).expect_err("a wrong override must not be papered over");
        let msg = format!("{err:#}");
        assert!(msg.contains("LNS_PASST_BIN"), "{msg}");
        assert!(msg.contains("/does/not/exist/passt"), "{msg}");
    }

    #[test]
    fn a_passt_on_path_serves_the_guest() {
        let d = tempfile::TempDir::new().unwrap();
        let on_path = executable(d.path(), "passt");
        let dir = d.path().to_path_buf();
        let env = move |k: &str| (k == "PATH").then(|| dir.clone().into_os_string());
        assert_eq!(located_passt_in(&[], &env).unwrap(), on_path);
    }

    #[test]
    fn a_passt_the_distro_package_installed_is_found_without_it_being_on_path() {
        let d = tempfile::TempDir::new().unwrap();
        let packaged = executable(d.path(), "passt");
        let dirs = [PathBuf::from("/nowhere"), d.path().to_path_buf()];
        assert_eq!(located_passt_in(&dirs, &|_| None).unwrap(), packaged);
    }

    #[test]
    fn a_host_without_passt_is_refused_by_the_package_that_provides_it() {
        let err = located_passt_in(&[PathBuf::from("/nowhere")], &|_| None)
            .expect_err("a guest with no network must not boot");
        let msg = format!("{err:#}");
        assert!(msg.contains("passt is not installed"), "{msg}");
        assert!(
            msg.contains("apt install passt"),
            "names the package: {msg}"
        );
        assert!(
            msg.contains("dnf install passt"),
            "names the package: {msg}"
        );
        assert!(msg.contains("LNS_PASST_BIN"), "names the override: {msg}");
        assert!(msg.contains("LNS_NETDEV=none"), "names the hatch: {msg}");
    }

    #[test]
    fn the_well_known_paths_are_the_ones_the_distro_packages_use() {
        assert!(packaged_passt_dirs().contains(&PathBuf::from("/usr/bin")));
        assert!(packaged_passt_dirs().contains(&PathBuf::from("/usr/local/bin")));
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

        let err = start_with(
            &FakeSpawner::failing(),
            Path::new("/nowhere/gvproxy"),
            &layout,
            START_TIMEOUT,
            connect_datagram,
        )
        .await
        .err()
        .expect("a missing binary refuses the run");

        assert!(format!("{err:#}").contains("/nowhere/gvproxy"), "{err:#}");
    }

    #[tokio::test]
    async fn a_backend_that_never_listens_is_reaped_rather_than_left_behind() {
        let d = tempfile::TempDir::new().unwrap();
        let layout = layout_in(d.path());
        let spawner = FakeSpawner::silent();

        let err = start_with(
            &spawner,
            Path::new("/usr/local/bin/gvproxy"),
            &layout,
            Duration::from_millis(30),
            connect_datagram,
        )
        .await
        .err()
        .expect("no socket means no guest network");

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

        let err = start_with(
            &spawner,
            Path::new("/usr/local/bin/gvproxy"),
            &layout,
            START_TIMEOUT,
            |_| anyhow::bail!("simulated attach failure"),
        )
        .await
        .err()
        .expect("a VM end that cannot attach is not a running backend");

        assert!(format!("{err:#}").contains("simulated attach failure"));
        assert!(spawner.killed.load(Ordering::SeqCst), "the child is killed");
        assert!(spawner.waited.load(Ordering::SeqCst), "and reaped");
    }

    #[tokio::test]
    async fn a_run_dir_too_deep_for_a_socket_refuses_before_gvproxy_is_spawned() {
        let layout = layout_in(&PathBuf::from("/").join("d".repeat(120)));
        let spawner = FakeSpawner::silent();

        let err = start_with(
            &spawner,
            Path::new("/usr/local/bin/gvproxy"),
            &layout,
            START_TIMEOUT,
            connect_datagram,
        )
        .await
        .err()
        .expect("a path no socket can hold refuses the run");

        assert!(format!("{err:#}").contains("unix socket path"), "{err:#}");
        assert!(
            spawner.spawned.lock().unwrap().is_empty(),
            "nothing is spawned for a run that cannot have a socket"
        );
    }

    #[tokio::test]
    async fn a_started_passt_binds_the_socket_cloud_hypervisor_will_connect_to() {
        let d = tempfile::TempDir::new().unwrap();
        let layout = layout_in(d.path());
        let spawner = FakeSpawner::listening(&layout.backend);

        let running = start_passt_with(
            &spawner,
            Path::new("/usr/bin/passt"),
            &layout,
            START_TIMEOUT,
        )
        .await
        .expect("the backend comes up");

        drop(running);
        let spawned = spawner.spawned.lock().unwrap().clone();
        assert_eq!(spawned.len(), 1);
        assert_eq!(spawned[0].0, PathBuf::from("/usr/bin/passt"));
        assert_eq!(spawned[0].1, passt_args(&layout));
        assert!(
            !spawner.killed.load(Ordering::SeqCst),
            "a healthy backend stays up"
        );
    }

    #[tokio::test]
    async fn a_passt_that_cannot_be_spawned_names_the_binary_it_tried() {
        let d = tempfile::TempDir::new().unwrap();
        let layout = layout_in(d.path());

        let err = start_passt_with(
            &FakeSpawner::failing(),
            Path::new("/nowhere/passt"),
            &layout,
            START_TIMEOUT,
        )
        .await
        .err()
        .expect("a missing binary refuses the run");

        assert!(format!("{err:#}").contains("/nowhere/passt"), "{err:#}");
    }

    #[tokio::test]
    async fn a_passt_that_never_binds_its_socket_is_reaped_rather_than_left_behind() {
        let d = tempfile::TempDir::new().unwrap();
        let layout = layout_in(d.path());
        let spawner = FakeSpawner::silent();

        let err = start_passt_with(
            &spawner,
            Path::new("/usr/bin/passt"),
            &layout,
            Duration::from_millis(30),
        )
        .await
        .err()
        .expect("no socket means cloud-hypervisor has nothing to connect to");

        assert!(
            format!("{err:#}").contains("did not expose its vhost-user socket"),
            "{err:#}"
        );
        assert!(spawner.killed.load(Ordering::SeqCst), "the child is killed");
        assert!(spawner.waited.load(Ordering::SeqCst), "and reaped");
    }

    #[tokio::test]
    async fn a_previous_boots_passt_socket_does_not_stop_the_next_one() {
        let d = tempfile::TempDir::new().unwrap();
        let layout = layout_in(d.path());
        std::fs::write(&layout.backend, b"stale").unwrap();
        let spawner = FakeSpawner::listening(&layout.backend);

        start_passt_with(
            &spawner,
            Path::new("/usr/bin/passt"),
            &layout,
            START_TIMEOUT,
        )
        .await
        .expect("a restart reuses its run dir");
    }
}
