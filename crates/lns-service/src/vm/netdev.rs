#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use std::ffi::OsString;
use std::net::{IpAddr, Ipv4Addr};
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

use engine::{Counters, Frames, Gateway, MAX_FRAME, Running};
use policy::Boundary;

/// The link the service serves each guest on. Its first address is the gateway, its second is the guest.
pub const GUEST_NETWORK: Ipv4Addr = Ipv4Addr::new(192, 168, 127, 0);

/// The one prefix this link is served on: a /24 with the gateway at `.1` and the guest at `.2`.
pub const GUEST_PREFIX: u8 = 24;

/// Frames queued between the guest's device and the stack. A guest that outruns the stack loses frames, as it would on a busy wire.
const FRAME_QUEUE: usize = 512;

const DROPPED_FRAME: &str = "a frame the stack could not take in time";

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

/// The subnet a run's guest is served on. `LNS_GUEST_SUBNET` moves it when 192.168.127.0/24 is taken on the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Subnet {
    pub network: Ipv4Addr,
    pub prefix: u8,
}

impl Subnet {
    pub fn gateway(self) -> Ipv4Addr {
        self.address(1)
    }

    pub fn guest(self) -> Ipv4Addr {
        self.address(2)
    }

    fn address(self, host: u8) -> Ipv4Addr {
        let octets = self.network.octets();
        Ipv4Addr::new(octets[0], octets[1], octets[2], host)
    }

    pub fn cidr(self) -> String {
        format!("{}/{}", self.network, self.prefix)
    }

    pub fn contains(self, address: Ipv4Addr) -> bool {
        address.octets()[..3] == self.network.octets()[..3]
    }

    pub fn from_env(env_get: impl Fn(&str) -> Option<OsString>) -> Result<Self> {
        let Some(value) = env_get("LNS_GUEST_SUBNET") else {
            return Ok(Subnet {
                network: GUEST_NETWORK,
                prefix: GUEST_PREFIX,
            });
        };
        let asked = PathBuf::from(&value).display().to_string();
        let subnet = value.to_str().and_then(parse_subnet);
        subnet.with_context(|| {
            format!(
                "LNS_GUEST_SUBNET={asked} is not a /24 network address. Write it like \
                 192.168.127.0/24: a /24 whose last octet is 0."
            )
        })
    }
}

fn parse_subnet(value: &str) -> Option<Subnet> {
    let (network, prefix) = value.split_once('/')?;
    let network: Ipv4Addr = network.parse().ok()?;
    if prefix != "24" || network.octets()[3] != 0 {
        return None;
    }
    Some(Subnet {
        network,
        prefix: GUEST_PREFIX,
    })
}

/// A host address inside the guest's subnet is unreachable from the guest, because the boundary refuses that subnet whole.
pub fn overlap_warning(subnet: Subnet, host_addresses: &[Ipv4Addr]) -> Option<String> {
    let clashing: Vec<String> = host_addresses
        .iter()
        .filter(|address| subnet.contains(**address))
        .map(ToString::to_string)
        .collect();
    if clashing.is_empty() {
        return None;
    }
    Some(format!(
        "This host holds {} in the guest subnet {}, which no guest can reach. \
         Move the guest link with LNS_GUEST_SUBNET=<a /24>.",
        clashing.join(", "),
        subnet.cidr()
    ))
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
    subnet: Subnet,
    resolvers: Arc<dns::Resolvers>,
    upstream: Arc<dyn dns::Upstream>,
) -> Result<Running> {
    let (to_stack, from_guest) = channel(FRAME_QUEUE);
    let (to_guest, from_stack) = channel(FRAME_QUEUE);
    let counters = Arc::new(Counters::default());
    let mut running = engine::start(
        engine::Config::for_subnet(
            subnet.network,
            Boundary::around(subnet.network, subnet.prefix),
        ),
        Gateway {
            address: IpAddr::V4(subnet.gateway()),
            resolvers,
            upstream,
        },
        Frames {
            from_guest,
            to_guest,
        },
    )?;
    let socket = Arc::new(host);
    running.adopt(read_frames(Arc::clone(&socket), to_stack, counters));
    running.adopt(write_frames(socket, from_stack));
    Ok(running)
}

async fn read_frames(socket: Arc<UnixDatagram>, frames: Sender<Vec<u8>>, counters: Arc<Counters>) {
    let mut buffer = vec![0u8; MAX_FRAME];
    while let Ok(read) = socket.recv(&mut buffer).await {
        match frames.try_send(buffer[..read].to_vec()) {
            Ok(()) => {}
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => counters.note(DROPPED_FRAME),
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => return,
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
    }

    #[test]
    fn lns_netdev_vmnet_restores_the_apple_nat_bridge() {
        let backend = Backend::from_env(|k| (k == "LNS_NETDEV").then(|| OsString::from("vmnet")))
            .expect("vmnet is the documented escape hatch");
        assert_eq!(backend, Backend::Vmnet);
        assert_eq!(backend.label(), "vmnet");
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

    fn subnet_of(value: &str) -> Result<Subnet> {
        Subnet::from_env(|k| (k == "LNS_GUEST_SUBNET").then(|| OsString::from(value)))
    }

    #[test]
    fn the_guest_link_is_the_subnet_the_documentation_names() {
        let subnet = Subnet::from_env(|_| None).expect("an unset override decides nothing");
        assert_eq!(subnet.network, Ipv4Addr::new(192, 168, 127, 0));
        assert_eq!(subnet.prefix, 24);
        assert_eq!(subnet.cidr(), "192.168.127.0/24");
        assert_eq!(subnet.gateway(), Ipv4Addr::new(192, 168, 127, 1));
        assert_eq!(subnet.guest(), Ipv4Addr::new(192, 168, 127, 2));
    }

    #[test]
    fn lns_guest_subnet_moves_the_link_gateway_and_guest_together() {
        let subnet = subnet_of("10.99.7.0/24").expect("a /24 is a link this service can serve");

        assert_eq!(subnet.network, Ipv4Addr::new(10, 99, 7, 0));
        assert_eq!(subnet.gateway(), Ipv4Addr::new(10, 99, 7, 1));
        assert_eq!(subnet.guest(), Ipv4Addr::new(10, 99, 7, 2));
        assert_eq!(subnet.cidr(), "10.99.7.0/24");
        assert!(subnet.contains(Ipv4Addr::new(10, 99, 7, 254)));
        assert!(!subnet.contains(Ipv4Addr::new(10, 99, 8, 1)));
    }

    #[test]
    fn a_guest_subnet_that_is_not_a_slash_24_network_is_refused_by_name() {
        for asked in [
            "10.99.7.0/16",
            "10.99.7.1/24",
            "10.99.7.0",
            "not-a-subnet",
            "10.99.7.0/24/24",
        ] {
            let err = subnet_of(asked).expect_err("a link nobody can serve must be named");
            let msg = format!("{err:#}");
            assert!(msg.contains(asked), "names what was asked for: {msg}");
            assert!(msg.contains("192.168.127.0/24"), "shows the shape: {msg}");
        }
    }

    #[test]
    fn a_guest_subnet_that_is_not_text_is_refused_like_any_other_typo() {
        use std::os::unix::ffi::OsStringExt;
        let err = Subnet::from_env(|k| {
            (k == "LNS_GUEST_SUBNET").then(|| OsString::from_vec(vec![0x66, 0x80, 0x6f]))
        })
        .expect_err("bytes that are not a subnet decide nothing");
        assert!(format!("{err:#}").contains("LNS_GUEST_SUBNET"));
    }

    #[test]
    fn a_host_address_inside_the_guest_subnet_is_named_before_the_guest_boots() {
        let subnet = Subnet::from_env(|_| None).unwrap();

        let warning = overlap_warning(
            subnet,
            &[Ipv4Addr::new(10, 0, 0, 5), Ipv4Addr::new(192, 168, 127, 40)],
        )
        .expect("an address the guest cannot reach is worth saying out loud");

        assert!(warning.contains("192.168.127.40"), "{warning}");
        assert!(warning.contains("192.168.127.0/24"), "{warning}");
        assert!(
            warning.contains("LNS_GUEST_SUBNET"),
            "names the remedy: {warning}"
        );
    }

    #[test]
    fn a_host_that_shares_nothing_with_the_guest_subnet_is_not_warned_about() {
        let subnet = Subnet::from_env(|_| None).unwrap();
        assert_eq!(overlap_warning(subnet, &[Ipv4Addr::new(10, 0, 0, 5)]), None);
        assert_eq!(overlap_warning(subnet, &[]), None);
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

        let full = vec![0xa5u8; MAX_FRAME];
        vm_end.send(&full).unwrap();
        let mut heard = [0u8; MAX_FRAME];
        let read = pair.host.recv(&mut heard).await.unwrap();

        assert_eq!(
            &heard[..read],
            &full[..],
            "a frame of the whole link's size arrives whole, tag and all"
        );
        std::mem::forget(vm);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_served_device_answers_the_guests_first_question_over_the_socket_pair() {
        let pair = device_pair().unwrap();
        let vm = std::os::unix::net::UnixDatagram::from(pair.vm);
        vm.set_nonblocking(true).unwrap();
        let vm = UnixDatagram::from_std(vm).unwrap();
        let subnet = Subnet::from_env(|_| None).unwrap();
        let _running = serve(
            pair.host,
            subnet,
            Arc::new(dns::Resolvers::new(
                Arc::new(NoResolvers),
                dns::REFRESH_AFTER,
            )),
            Arc::new(resolver::HostUpstream),
        )
        .expect("the stack comes up over a socket pair");

        vm.send(&link::wrap(
            TEST_GUEST_MAC,
            link::GATEWAY_MAC,
            etherparse::EtherType::IPV6,
            &[0x60],
        ))
        .await
        .unwrap();
        vm.send(&fragment()).await.unwrap();
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
            "the IPv6 frame and the fragment before it were dropped, not answered"
        );
        assert_eq!(header.source, link::GATEWAY_MAC);
        assert_eq!(payload[14..18], [192, 168, 127, 1]);

        vm.send(&ping_for_the_gateway()).await.unwrap();
        let read = tokio::time::timeout(std::time::Duration::from_secs(5), vm.recv(&mut heard))
            .await
            .expect("the gateway answers a ping at its own address")
            .unwrap();

        let (_, packet) = etherparse::Ethernet2Header::from_slice(&heard[..read]).unwrap();
        let (ip, rest) = etherparse::Ipv4Header::from_slice(packet).unwrap();
        assert_eq!(ip.source, [192, 168, 127, 1]);
        let (icmp, _) = etherparse::Icmpv4Header::from_slice(rest).unwrap();
        assert_eq!(
            icmp.icmp_type,
            etherparse::Icmpv4Type::EchoReply(etherparse::IcmpEchoHeader { id: 3, seq: 4 })
        );
    }

    const TEST_GUEST_MAC: link::Mac = [0x02, 0x11, 0x22, 0x33, 0x44, 0x55];

    fn fragment() -> Vec<u8> {
        let builder =
            etherparse::PacketBuilder::ipv4([192, 168, 127, 2], [1, 1, 1, 1], 64).udp(45_000, 443);
        let mut packet = Vec::with_capacity(builder.size(4));
        builder.write(&mut packet, b"half").unwrap();
        packet[6] |= 0x20;
        link::wrap(
            TEST_GUEST_MAC,
            link::GATEWAY_MAC,
            etherparse::EtherType::IPV4,
            &packet,
        )
    }

    fn ping_for_the_gateway() -> Vec<u8> {
        let builder = etherparse::PacketBuilder::ipv4([192, 168, 127, 2], [192, 168, 127, 1], 64)
            .icmpv4_echo_request(3, 4);
        let mut packet = Vec::with_capacity(builder.size(0));
        builder.write(&mut packet, &[]).unwrap();
        link::wrap(
            TEST_GUEST_MAC,
            link::GATEWAY_MAC,
            etherparse::EtherType::IPV4,
            &packet,
        )
    }

    fn arp_request_for_the_gateway() -> Vec<u8> {
        let guest = TEST_GUEST_MAC;
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

    struct NoResolvers;

    impl dns::Sources for NoResolvers {
        fn scopes(&self) -> Vec<dns::Scope> {
            Vec::new()
        }
    }

    #[tokio::test]
    async fn the_reader_stops_when_the_stack_it_feeds_is_gone() {
        let (host, vm) = StdUnixDatagram::pair().unwrap();
        host.set_nonblocking(true).unwrap();
        let host = Arc::new(UnixDatagram::from_std(host).unwrap());
        let (frames, gone) = channel(1);
        drop(gone);
        vm.send(b"a frame").unwrap();

        read_frames(host, frames, Arc::new(Counters::default())).await;
    }

    #[tokio::test]
    async fn a_guest_that_outruns_the_stack_loses_frames_rather_than_the_run() {
        let (host, vm) = StdUnixDatagram::pair().unwrap();
        host.set_nonblocking(true).unwrap();
        let host = Arc::new(UnixDatagram::from_std(host).unwrap());
        let (frames, mut queue) = channel(1);
        let counters = Arc::new(Counters::default());
        vm.send(b"the frame that fits").unwrap();
        vm.send(b"the frame that does not").unwrap();
        drop(vm);

        tokio::time::timeout(
            std::time::Duration::from_millis(200),
            read_frames(host, frames, Arc::clone(&counters)),
        )
        .await
        .ok();

        assert_eq!(queue.try_recv().unwrap(), b"the frame that fits");
        assert_eq!(
            counters.seen(DROPPED_FRAME),
            1,
            "the drop is counted, not silent"
        );
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
