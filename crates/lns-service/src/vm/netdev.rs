#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use std::ffi::OsString;
use std::net::Ipv4Addr;
use std::os::fd::{AsRawFd, OwnedFd, RawFd};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use tokio::net::UnixDatagram;
use tokio::sync::mpsc::{Receiver, Sender, channel};

pub mod dhcp;
pub mod dns;
pub mod engine;
pub mod link;
pub mod policy;
pub mod resolver;

#[cfg(target_os = "macos")]
pub mod real;

use engine::{Frames, Running};
use policy::Boundary;

/// The link the service serves each guest on. Its first address is the gateway, its second is the guest.
pub const GUEST_NETWORK: Ipv4Addr = Ipv4Addr::new(192, 168, 127, 0);
pub const GUEST_PREFIX: u8 = 24;
pub const GUEST_SUBNET: &str = "192.168.127.0/24";

/// 1500 for the link, 14 for the ethernet header, 4 for a VLAN tag we never send but must not truncate either.
const MAX_FRAME: usize = 1518;

const FRAME_QUEUE: usize = 512;

const SEND_BUFFER_BYTES: libc::c_int = 1024 * 1024;

/// Vz asks for a receive buffer of at least twice the send buffer, and four times it for throughput.
const RECEIVE_BUFFER_BYTES: libc::c_int = 4 * 1024 * 1024;

/// What the guest's link hangs off. `LNS_NETDEV=vmnet` restores the Apple NAT bridge for one release.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Netstack,
    Vmnet,
}

impl Backend {
    pub fn label(self) -> &'static str {
        match self {
            Backend::Netstack => "netstack",
            Backend::Vmnet => "vmnet",
        }
    }

    pub fn detail(self) -> &'static str {
        match self {
            Backend::Netstack => GUEST_SUBNET,
            Backend::Vmnet => "Apple NAT bridge",
        }
    }

    pub fn from_env(env_get: impl Fn(&str) -> Option<OsString>) -> Result<Self> {
        let Some(value) = env_get("LNS_NETDEV") else {
            return Ok(Backend::Netstack);
        };
        match value.to_str() {
            Some("netstack") => Ok(Backend::Netstack),
            Some("vmnet") => Ok(Backend::Vmnet),
            _ => bail!(
                "LNS_NETDEV={} is not a network backend. Use netstack (the default, the \
                 in-process userspace network) or vmnet (the Apple NAT bridge, kept for one \
                 release).",
                PathBuf::from(value).display()
            ),
        }
    }
}

/// How the guest's virtio-net device reaches the host: the Apple NAT bridge, or a datagram socket this process holds the other end of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetAttachment {
    Nat,
    DatagramFd(RawFd),
}

/// The two ends of one guest's link. The VM end goes to Vz as a file handle; the host end carries frames to and from the stack.
pub struct DevicePair {
    pub vm: OwnedFd,
    pub host: UnixDatagram,
}

pub fn device_pair() -> Result<DevicePair> {
    let (vm, host) =
        std::os::unix::net::UnixDatagram::pair().context("creating the guest's network device")?;
    for end in [&vm, &host] {
        set_buffer_sizes(end)?;
    }
    host.set_nonblocking(true)
        .context("making the host end of the guest's network device non-blocking")?;
    Ok(DevicePair {
        vm: OwnedFd::from(vm),
        host: UnixDatagram::from_std(host)
            .context("handing the host end of the guest's network device to tokio")?,
    })
}

fn set_buffer_sizes(socket: &std::os::unix::net::UnixDatagram) -> Result<()> {
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

/// Serve one guest's network over `host`, one datagram per ethernet frame. The guard owns every task, including the two that carry frames.
pub fn serve(
    host: UnixDatagram,
    resolver: Arc<dyn dns::Resolver>,
    boundary: Boundary,
) -> Result<Running> {
    let (to_stack, from_guest) = channel(FRAME_QUEUE);
    let (to_guest, from_stack) = channel(FRAME_QUEUE);
    let mut running = engine::start(
        engine::Config::for_subnet(GUEST_NETWORK, boundary),
        resolver,
        Frames {
            from_guest,
            to_guest,
        },
    )?;
    let socket = Arc::new(host);
    running.adopt(read_frames(Arc::clone(&socket), to_stack));
    running.adopt(write_frames(socket, from_stack));
    Ok(running)
}

async fn read_frames(socket: Arc<UnixDatagram>, frames: Sender<Vec<u8>>) {
    let mut buffer = vec![0u8; MAX_FRAME];
    while let Ok(read) = socket.recv(&mut buffer).await {
        if frames.send(buffer[..read].to_vec()).await.is_err() {
            return;
        }
    }
}

async fn write_frames(socket: Arc<UnixDatagram>, mut frames: Receiver<Vec<u8>>) {
    while let Some(frame) = frames.recv().await {
        let _ = socket.send(&frame).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixDatagram as StdUnixDatagram;

    #[test]
    fn the_default_backend_serves_the_guest_from_the_in_process_netstack() {
        let backend = Backend::from_env(|_| None).expect("an unset LNS_NETDEV decides nothing");
        assert_eq!(backend, Backend::Netstack);
        assert_eq!(backend.label(), "netstack");
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
    fn lns_netdev_netstack_names_the_default_explicitly() {
        let backend =
            Backend::from_env(|k| (k == "LNS_NETDEV").then(|| OsString::from("netstack")))
                .expect("naming the default is allowed");
        assert_eq!(backend, Backend::Netstack);
    }

    #[test]
    fn an_unknown_backend_is_refused_by_name_with_both_choices() {
        let err = Backend::from_env(|k| (k == "LNS_NETDEV").then(|| OsString::from("gvproxy")))
            .expect_err("a typo must not silently pick a backend");
        let msg = format!("{err:#}");
        assert!(msg.contains("gvproxy"), "names what was asked for: {msg}");
        assert!(msg.contains("netstack"), "names the default: {msg}");
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
    fn the_guest_link_is_the_subnet_the_documentation_names() {
        assert_eq!(GUEST_NETWORK, Ipv4Addr::new(192, 168, 127, 0));
        assert_eq!(GUEST_PREFIX, 24);
        assert_eq!(GUEST_SUBNET, "192.168.127.0/24");
    }

    fn buffer_of(fd: RawFd, option: libc::c_int) -> libc::c_int {
        let mut value: libc::c_int = 0;
        let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
        // SAFETY: `fd` is open for the whole call and `value`/`len` are initialised out-params of the size we declare.
        let rc = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                option,
                std::ptr::from_mut(&mut value).cast(),
                &mut len,
            )
        };
        assert_eq!(rc, 0, "the buffer size must be readable back");
        value
    }

    #[tokio::test]
    async fn the_device_pair_is_sized_for_the_frames_vz_pushes_through_it() {
        let pair = device_pair().expect("a socketpair needs nothing of the filesystem");

        assert!(
            buffer_of(pair.vm.as_raw_fd(), libc::SO_SNDBUF) > 0,
            "Vz writes frames into this buffer; a default-sized one drops them"
        );
        assert!(buffer_of(pair.vm.as_raw_fd(), libc::SO_RCVBUF) > 0);
        assert!(buffer_of(pair.host.as_raw_fd(), libc::SO_SNDBUF) > 0);
    }

    #[tokio::test]
    async fn the_two_ends_of_the_pair_carry_one_frame_per_datagram() {
        let pair = device_pair().unwrap();
        let vm = pair.vm;
        // SAFETY: `vm` owns the descriptor and is forgotten below, so it is closed exactly once.
        let vm_end =
            unsafe { <StdUnixDatagram as std::os::fd::FromRawFd>::from_raw_fd(vm.as_raw_fd()) };

        vm_end.send(b"a frame").unwrap();
        let mut heard = [0u8; MAX_FRAME];
        let read = pair.host.recv(&mut heard).await.unwrap();

        assert_eq!(&heard[..read], b"a frame");
        std::mem::forget(vm);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_served_device_answers_the_guests_first_question_over_the_socket_pair() {
        let pair = device_pair().unwrap();
        let vm = std::os::unix::net::UnixDatagram::from(pair.vm);
        vm.set_nonblocking(true).unwrap();
        let vm = UnixDatagram::from_std(vm).unwrap();
        let _running = serve(
            pair.host,
            Arc::new(resolver::SystemResolver::from_resolv_conf("")),
            Boundary::around(GUEST_NETWORK, GUEST_PREFIX),
        )
        .expect("the stack comes up over a socket pair");

        vm.send(&link::wrap(
            [0x02, 0x11, 0x22, 0x33, 0x44, 0x55],
            link::GATEWAY_MAC,
            etherparse::EtherType::IPV6,
            &[0x60],
        ))
        .await
        .unwrap();
        vm.send(&arp_request_for_the_gateway()).await.unwrap();

        let mut heard = [0u8; MAX_FRAME];
        let read = tokio::time::timeout(std::time::Duration::from_secs(5), vm.recv(&mut heard))
            .await
            .expect("the gateway answers its own address")
            .unwrap();

        let (header, payload) = etherparse::Ethernet2Header::from_slice(&heard[..read]).unwrap();
        assert_eq!(
            header.ether_type,
            etherparse::EtherType::ARP,
            "the IPv6 frame before it was dropped, not answered"
        );
        assert_eq!(header.source, link::GATEWAY_MAC);
        assert_eq!(header.ether_type, etherparse::EtherType::ARP);
        assert_eq!(payload[14..18], [192, 168, 127, 1]);
    }

    fn arp_request_for_the_gateway() -> Vec<u8> {
        let guest = [0x02, 0x11, 0x22, 0x33, 0x44, 0x55];
        let mut arp = Vec::new();
        arp.extend_from_slice(&1u16.to_be_bytes());
        arp.extend_from_slice(&etherparse::EtherType::IPV4.0.to_be_bytes());
        arp.push(6);
        arp.push(4);
        arp.extend_from_slice(&1u16.to_be_bytes());
        arp.extend_from_slice(&guest);
        arp.extend_from_slice(&[192, 168, 127, 2]);
        arp.extend_from_slice(&[0u8; 6]);
        arp.extend_from_slice(&[192, 168, 127, 1]);
        link::wrap(guest, link::BROADCAST_MAC, etherparse::EtherType::ARP, &arp)
    }

    #[tokio::test]
    async fn the_reader_stops_when_the_stack_it_feeds_is_gone() {
        let (host, vm) = StdUnixDatagram::pair().unwrap();
        host.set_nonblocking(true).unwrap();
        let host = Arc::new(UnixDatagram::from_std(host).unwrap());
        let (frames, gone) = channel(1);
        drop(gone);
        vm.send(b"a frame").unwrap();

        read_frames(host, frames).await;
    }

    #[tokio::test]
    async fn the_writer_stops_when_the_channel_that_feeds_it_is_gone() {
        let (host, _vm) = StdUnixDatagram::pair().unwrap();
        host.set_nonblocking(true).unwrap();
        let host = Arc::new(UnixDatagram::from_std(host).unwrap());
        let (frames, queue) = channel(1);
        frames.send(b"a frame".to_vec()).await.unwrap();
        drop(frames);

        write_frames(host, queue).await;
    }

    #[test]
    fn set_buffer_sizes_surfaces_a_kernel_refusal_rather_than_a_silent_drop() {
        let (socket, _peer) = StdUnixDatagram::pair().unwrap();
        let raw = socket.as_raw_fd();
        drop(socket);
        // SAFETY: the fd is closed, so every setsockopt on it is EBADF; nothing else can have reopened it in this statement.
        let closed = unsafe { <StdUnixDatagram as std::os::fd::FromRawFd>::from_raw_fd(raw) };
        let err = set_buffer_sizes(&closed).expect_err("a closed fd cannot be sized");
        std::mem::forget(closed);
        assert!(
            format!("{err:#}").contains("guest network socket buffers"),
            "{err:#}"
        );
    }
}
