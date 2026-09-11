use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use etherparse::{IpNumber, Ipv4Header, PacketBuilder, TcpHeader, UdpHeader};
use futures_util::{SinkExt, StreamExt};
use netstack_smoltcp::StackBuilder;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc::{Receiver, Sender, channel};
use tokio::task::JoinSet;

use super::dhcp::{self, Lease, ReplyTo};
use super::dns::{self, Resolver};
use super::link::{BROADCAST_MAC, GATEWAY_MAC, Link, Received};
use super::policy::{Boundary, Refusal};
use crate::log;

/// A guest's ethernet frames, up to an untagged 1500-byte MTU.
pub const MAX_FRAME: usize = 1514;

/// Vz refuses an MTU below this and the guest's link is configured for it.
pub const MTU: usize = 1500;

/// One TCP window per socket. 512 KiB carries a 1 Gbit/s path at 4 ms without stalling on the window.
const TCP_WINDOW_BYTES: u32 = 512 * 1024;

const CHANNEL_DEPTH: usize = 512;

const UDP_IDLE: Duration = Duration::from_secs(60);

const UDP_DATAGRAM_BYTES: usize = 65_535;

const PACKET_TTL: u8 = 64;

/// What the guest's link is and where it may reach. One run, one stack, no state shared with any other run.
#[derive(Debug, Clone, Copy)]
pub struct Config {
    pub lease: Lease,
    pub boundary: Boundary,
}

impl Config {
    pub fn for_subnet(network: Ipv4Addr, boundary: Boundary) -> Self {
        let octets = network.octets();
        Self {
            lease: Lease {
                gateway: Ipv4Addr::new(octets[0], octets[1], octets[2], 1),
                guest: Ipv4Addr::new(octets[0], octets[1], octets[2], 2),
                netmask: Ipv4Addr::new(255, 255, 255, 0),
                duration: Duration::from_secs(24 * 60 * 60),
            },
            boundary,
        }
    }
}

/// Frames in, frames out. The macOS device is a datagram socketpair; a vhost-user front end would hand over the same two channels.
pub struct Frames {
    pub from_guest: Receiver<Vec<u8>>,
    pub to_guest: Sender<Vec<u8>>,
}

/// Every task serving one guest's network. Dropping it aborts them all, so a stopped run leaves nothing behind.
pub struct Running {
    tasks: JoinSet<()>,
}

impl Running {
    /// A front end adds the tasks that carry its own transport, so one guard owns every task of a run's network.
    pub fn adopt<F: Future<Output = ()> + Send + 'static>(&mut self, task: F) {
        self.tasks.spawn(task);
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.tasks.abort_all();
    }
}

pub fn start(config: Config, resolver: Arc<dyn Resolver>, frames: Frames) -> Result<Running> {
    let (stack, runner, udp, tcp) = StackBuilder::default()
        .enable_tcp(true)
        .enable_udp(true)
        .mtu(MTU)
        .stack_buffer_size(CHANNEL_DEPTH)
        .tcp_buffer_size(CHANNEL_DEPTH)
        .udp_buffer_size(CHANNEL_DEPTH)
        .tcp_recv_buffer_size(TCP_WINDOW_BYTES)
        .tcp_send_buffer_size(TCP_WINDOW_BYTES)
        .build()
        .context("building the guest's userspace network stack")?;
    let runner = runner.context("the stack was built without its TCP runner")?;
    let udp = udp.context("the stack was built without its UDP socket")?;
    let tcp = tcp.context("the stack was built without its TCP listener")?;

    let link = Arc::new(Mutex::new(Link::new(GATEWAY_MAC, config.lease.gateway)));
    let refused = Arc::new(Mutex::new(HashSet::new()));
    let (stack_sink, stack_stream) = stack.split();
    let (udp_read, udp_write) = udp.split();
    let (replies_tx, replies_rx) = channel(CHANNEL_DEPTH);

    let mut tasks = JoinSet::new();
    tasks.spawn(async move {
        let _ = runner.await;
    });
    tasks.spawn(from_guest(
        config,
        refused,
        Arc::clone(&link),
        frames.from_guest,
        frames.to_guest.clone(),
        stack_sink,
    ));
    tasks.spawn(to_guest(link, stack_stream, frames.to_guest));
    tasks.spawn(accept_tcp(tcp));
    tasks.spawn(accept_udp(config, resolver, udp_read, replies_tx));
    tasks.spawn(write_udp(udp_write, replies_rx));
    Ok(Running { tasks })
}

type StackSink = futures_util::stream::SplitSink<netstack_smoltcp::Stack, Vec<u8>>;
type StackStream = futures_util::stream::SplitStream<netstack_smoltcp::Stack>;

async fn from_guest(
    config: Config,
    refused: Refused,
    link: Arc<Mutex<Link>>,
    mut frames: Receiver<Vec<u8>>,
    answers: Sender<Vec<u8>>,
    mut stack: StackSink,
) {
    while let Some(frame) = frames.recv().await {
        let outcome = {
            let mut link = link.lock().expect("link poisoned");
            match link.receive(&frame) {
                Received::Answer(reply) => Some(Outcome::Frame(reply)),
                Received::Ipv4(packet) => ingress(&config, &refused, &link, packet),
                Received::Dropped => None,
            }
        };
        match outcome {
            Some(Outcome::Frame(reply)) => drop(answers.send(reply).await),
            Some(Outcome::Packet(packet)) => drop(stack.send(packet).await),
            None => {}
        }
    }
}

enum Outcome {
    Frame(Vec<u8>),
    Packet(Vec<u8>),
}

fn ingress(config: &Config, refused: &Refused, link: &Link, packet: &[u8]) -> Option<Outcome> {
    match classify(config, packet)? {
        Ingress::Dhcp(reply) => Some(Outcome::Frame(dhcp_frame(config, &reply))),
        Ingress::Forward => Some(Outcome::Packet(packet.to_vec())),
        Ingress::Refused {
            destination,
            refusal,
            reset,
        } => {
            note_refusal(refused, destination, refusal);
            reset.map(|packet| Outcome::Frame(link.send_ipv4(&packet)))
        }
    }
}

/// What the host does with one IPv4 packet the guest sent, decided before any socket exists for it.
#[derive(Debug, PartialEq, Eq)]
enum Ingress {
    Dhcp(dhcp::Reply),
    Refused {
        destination: SocketAddr,
        refusal: Refusal,
        reset: Option<Vec<u8>>,
    },
    Forward,
}

fn classify(config: &Config, packet: &[u8]) -> Option<Ingress> {
    let Ok((header, rest)) = Ipv4Header::from_slice(packet) else {
        return None;
    };
    let source = Ipv4Addr::from(header.source);
    let destination = Ipv4Addr::from(header.destination);
    match header.protocol {
        IpNumber::UDP => {
            let Ok((udp, payload)) = UdpHeader::from_slice(rest) else {
                return None;
            };
            if udp.destination_port == dhcp::SERVER_PORT {
                return dhcp::answer(&config.lease, payload).map(Ingress::Dhcp);
            }
            let to = SocketAddr::new(IpAddr::V4(destination), udp.destination_port);
            if to == gateway_resolver(config) {
                return Some(Ingress::Forward);
            }
            Some(refused_or_forward(config, to, None))
        }
        IpNumber::TCP => {
            let Ok((tcp, _)) = TcpHeader::from_slice(rest) else {
                return None;
            };
            let to = SocketAddr::new(IpAddr::V4(destination), tcp.destination_port);
            let reset = (tcp.syn && !tcp.ack).then(|| reset_packet(source, destination, &tcp));
            Some(refused_or_forward(config, to, reset))
        }
        _ => Some(Ingress::Forward),
    }
}

fn gateway_resolver(config: &Config) -> SocketAddr {
    SocketAddr::new(IpAddr::V4(config.lease.gateway), dns::PORT)
}

fn refused_or_forward(config: &Config, destination: SocketAddr, reset: Option<Vec<u8>>) -> Ingress {
    match config.boundary.refusal(destination) {
        Some(refusal) => Ingress::Refused {
            destination,
            refusal,
            reset,
        },
        None => Ingress::Forward,
    }
}

/// A refused connection is answered the way a closed port is, so the guest fails at once instead of waiting out a timeout.
fn reset_packet(source: Ipv4Addr, destination: Ipv4Addr, syn: &TcpHeader) -> Vec<u8> {
    let builder = PacketBuilder::ipv4(destination.octets(), source.octets(), PACKET_TTL)
        .tcp(syn.destination_port, syn.source_port, 0, 0)
        .rst()
        .ack(syn.sequence_number.wrapping_add(1));
    let mut packet = Vec::with_capacity(builder.size(0));
    let _ = builder.write(&mut packet, &[]);
    packet
}

fn dhcp_frame(config: &Config, reply: &dhcp::Reply) -> Vec<u8> {
    let (mac, destination) = match reply.to {
        ReplyTo::Broadcast => (BROADCAST_MAC, Ipv4Addr::BROADCAST),
        ReplyTo::Unicast { mac, ip } => (mac, ip),
    };
    let builder = PacketBuilder::ethernet2(GATEWAY_MAC, mac)
        .ipv4(
            config.lease.gateway.octets(),
            destination.octets(),
            PACKET_TTL,
        )
        .udp(dhcp::SERVER_PORT, dhcp::CLIENT_PORT);
    let mut frame = Vec::with_capacity(builder.size(reply.payload.len()));
    let _ = builder.write(&mut frame, &reply.payload);
    frame
}

async fn to_guest(link: Arc<Mutex<Link>>, mut stack: StackStream, frames: Sender<Vec<u8>>) {
    while let Some(Ok(packet)) = stack.next().await {
        let frame = link.lock().expect("link poisoned").send_ipv4(&packet);
        let _ = frames.send(frame).await;
    }
}

type Refused = Arc<Mutex<HashSet<SocketAddr>>>;

fn note_refusal(refused: &Refused, destination: SocketAddr, refusal: Refusal) {
    if refused
        .lock()
        .expect("refusals poisoned")
        .insert(destination)
    {
        log::debug!(
            "the guest network refused {destination}: {}",
            refusal.reason()
        );
    }
}

async fn accept_tcp(mut listener: netstack_smoltcp::TcpListener) {
    while let Some((guest, _, destination)) = listener.next().await {
        tokio::spawn(carry_tcp(guest, destination));
    }
}

async fn carry_tcp(mut guest: netstack_smoltcp::TcpStream, destination: SocketAddr) {
    if let Ok(mut host) = tokio::net::TcpStream::connect(destination).await {
        let _ = tokio::io::copy_bidirectional(&mut guest, &mut host).await;
        let _ = host.shutdown().await;
        let _ = guest.shutdown().await;
    }
}

type UdpReply = (Vec<u8>, SocketAddr, SocketAddr);

async fn write_udp(mut write: netstack_smoltcp::udp::WriteHalf, mut replies: Receiver<UdpReply>) {
    while let Some(reply) = replies.recv().await {
        let _ = write.send(reply).await;
    }
}

async fn accept_udp(
    config: Config,
    resolver: Arc<dyn Resolver>,
    mut datagrams: netstack_smoltcp::udp::ReadHalf,
    replies: Sender<UdpReply>,
) {
    let gateway_dns = gateway_resolver(&config);
    let mut flows: HashMap<(SocketAddr, SocketAddr), Sender<Vec<u8>>> = HashMap::new();
    while let Some((payload, source, destination)) = datagrams.next().await {
        if destination == gateway_dns {
            tokio::spawn(answer_dns(
                Arc::clone(&resolver),
                payload,
                source,
                destination,
                replies.clone(),
            ));
            continue;
        }
        let key = (source, destination);
        let sender = match flows.get(&key) {
            Some(open) if !open.is_closed() => open.clone(),
            _ => {
                let (tx, rx) = channel(CHANNEL_DEPTH);
                tokio::spawn(carry_udp(source, destination, rx, replies.clone()));
                flows.insert(key, tx.clone());
                tx
            }
        };
        let _ = sender.send(payload).await;
    }
}

/// One host socket per (guest source, destination) flow, kept for as long as the flow is used and no longer.
async fn carry_udp(
    source: SocketAddr,
    destination: SocketAddr,
    outbound: Receiver<Vec<u8>>,
    replies: Sender<UdpReply>,
) {
    if let Ok(host) = host_socket(destination).await {
        relay_udp(host, source, destination, outbound, replies).await;
    }
}

async fn relay_udp(
    host: tokio::net::UdpSocket,
    source: SocketAddr,
    destination: SocketAddr,
    mut outbound: Receiver<Vec<u8>>,
    replies: Sender<UdpReply>,
) {
    let mut buffer = vec![0u8; UDP_DATAGRAM_BYTES];
    loop {
        let step = tokio::time::timeout(UDP_IDLE, async {
            tokio::select! {
                outgoing = outbound.recv() => outgoing.map(Step::Out),
                incoming = host.recv(&mut buffer) => incoming.ok().map(Step::In),
            }
        })
        .await;
        match step {
            Ok(Some(Step::Out(datagram))) => drop(host.send(&datagram).await),
            Ok(Some(Step::In(read))) => {
                let reply = (buffer[..read].to_vec(), destination, source);
                let _ = replies.send(reply).await;
            }
            Ok(None) | Err(_) => return,
        }
    }
}

async fn host_socket(destination: SocketAddr) -> std::io::Result<tokio::net::UdpSocket> {
    let bind = if destination.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let socket = tokio::net::UdpSocket::bind(bind).await?;
    socket.connect(destination).await?;
    Ok(socket)
}

enum Step {
    Out(Vec<u8>),
    In(usize),
}

async fn answer_dns(
    resolver: Arc<dyn Resolver>,
    query: Vec<u8>,
    source: SocketAddr,
    gateway: SocketAddr,
    replies: Sender<UdpReply>,
) {
    if let Some(answer) = dns::answer(&query, resolver.as_ref()).await {
        let _ = replies.send((answer, gateway, source)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::future::BoxFuture;
    use smoltcp::iface::{Interface, SocketHandle, SocketSet};
    use smoltcp::phy::{Device, DeviceCapabilities, Medium};
    use smoltcp::socket::{dhcpv4, tcp, udp};
    use smoltcp::time::Instant;
    use smoltcp::wire::{EthernetAddress, HardwareAddress, IpCidr, IpEndpoint, Ipv4Address};
    use tokio::io::AsyncReadExt;

    const GUEST_MAC: [u8; 6] = [0x02, 0x11, 0x22, 0x33, 0x44, 0x55];
    const SUBNET: Ipv4Addr = Ipv4Addr::new(192, 168, 127, 0);
    const PATIENCE: Duration = Duration::from_secs(10);

    struct TestResolver;

    impl Resolver for TestResolver {
        fn lookup(&self, name: String) -> BoxFuture<'static, std::io::Result<Vec<IpAddr>>> {
            Box::pin(async move {
                match name.as_str() {
                    "example.test" => Ok(vec![IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))]),
                    _ => Err(std::io::Error::other("no such name")),
                }
            })
        }

        fn forward(&self, _query: Vec<u8>) -> BoxFuture<'static, std::io::Result<Vec<u8>>> {
            Box::pin(async { Err(std::io::Error::other("no upstream in a test")) })
        }
    }

    struct GuestDevice {
        inbound: Receiver<Vec<u8>>,
        outbound: Sender<Vec<u8>>,
    }

    struct GuestRx(Vec<u8>);

    impl smoltcp::phy::RxToken for GuestRx {
        fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
            f(&self.0)
        }
    }

    struct GuestTx<'a>(&'a Sender<Vec<u8>>);

    impl smoltcp::phy::TxToken for GuestTx<'_> {
        fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
            let mut frame = vec![0u8; len];
            let out = f(&mut frame);
            let _ = self.0.try_send(frame);
            out
        }
    }

    impl Device for GuestDevice {
        type RxToken<'a>
            = GuestRx
        where
            Self: 'a;
        type TxToken<'a>
            = GuestTx<'a>
        where
            Self: 'a;

        fn receive(&mut self, _now: Instant) -> Option<(GuestRx, GuestTx<'_>)> {
            let frame = self.inbound.try_recv().ok()?;
            Some((GuestRx(frame), GuestTx(&self.outbound)))
        }

        fn transmit(&mut self, _now: Instant) -> Option<GuestTx<'_>> {
            Some(GuestTx(&self.outbound))
        }

        fn capabilities(&self) -> DeviceCapabilities {
            let mut capabilities = DeviceCapabilities::default();
            capabilities.medium = Medium::Ethernet;
            capabilities.max_transmission_unit = MTU;
            capabilities
        }
    }

    /// A real guest for the stack under test: smoltcp over the other end of the frame channels, with nothing of this crate in it.
    struct FakeGuest {
        device: GuestDevice,
        iface: Interface,
        sockets: SocketSet<'static>,
        dhcp: SocketHandle,
        _running: Running,
    }

    #[derive(Debug, PartialEq, Eq)]
    struct Lease {
        address: IpCidr,
        router: Option<Ipv4Address>,
        dns: Vec<Ipv4Address>,
    }

    impl FakeGuest {
        fn on(boundary: Boundary) -> Self {
            let (to_stack, from_guest) = channel(CHANNEL_DEPTH);
            let (to_guest, from_stack) = channel(CHANNEL_DEPTH);
            let running = start(
                Config::for_subnet(SUBNET, boundary),
                Arc::new(TestResolver),
                Frames {
                    from_guest,
                    to_guest,
                },
            )
            .expect("the stack comes up");

            let mut device = GuestDevice {
                inbound: from_stack,
                outbound: to_stack,
            };
            let mut config =
                smoltcp::iface::Config::new(HardwareAddress::Ethernet(EthernetAddress(GUEST_MAC)));
            config.random_seed = 0x5eed;
            let iface = Interface::new(config, &mut device, Instant::now());
            let mut sockets = SocketSet::new(Vec::new());
            let dhcp = sockets.add(dhcpv4::Socket::new());
            Self {
                device,
                iface,
                sockets,
                dhcp,
                _running: running,
            }
        }

        fn poll(&mut self) {
            self.iface
                .poll(Instant::now(), &mut self.device, &mut self.sockets);
        }

        async fn settle(&mut self) {
            self.poll();
            tokio::time::sleep(Duration::from_millis(2)).await;
        }

        async fn lease(&mut self) -> Lease {
            let deadline = tokio::time::Instant::now() + PATIENCE;
            loop {
                self.settle().await;
                if let Some(dhcpv4::Event::Configured(config)) =
                    self.sockets.get_mut::<dhcpv4::Socket>(self.dhcp).poll()
                {
                    let lease = Lease {
                        address: IpCidr::Ipv4(config.address),
                        router: config.router,
                        dns: config.dns_servers.to_vec(),
                    };
                    let address = config.address;
                    let router = config.router;
                    self.iface.update_ip_addrs(|addrs| {
                        addrs.clear();
                        addrs.push(IpCidr::Ipv4(address)).expect("one address");
                    });
                    if let Some(router) = router {
                        self.iface
                            .routes_mut()
                            .add_default_ipv4_route(router)
                            .expect("a default route");
                    }
                    return lease;
                }
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "the guest never got a lease"
                );
            }
        }

        fn bind_udp(&mut self, from: u16) -> SocketHandle {
            let handle = self.sockets.add(udp::Socket::new(
                udp::PacketBuffer::new(vec![udp::PacketMetadata::EMPTY; 4], vec![0; 4096]),
                udp::PacketBuffer::new(vec![udp::PacketMetadata::EMPTY; 4], vec![0; 4096]),
            ));
            self.sockets
                .get_mut::<udp::Socket>(handle)
                .bind(from)
                .expect("a free client port");
            handle
        }

        async fn exchange(
            &mut self,
            handle: SocketHandle,
            to: SocketAddr,
            payload: &[u8],
        ) -> Vec<u8> {
            self.sockets
                .get_mut::<udp::Socket>(handle)
                .send_slice(payload, IpEndpoint::from((to.ip(), to.port())))
                .expect("the datagram is sent");

            let deadline = tokio::time::Instant::now() + PATIENCE;
            loop {
                self.settle().await;
                if let Ok((back, _)) = self.sockets.get_mut::<udp::Socket>(handle).recv() {
                    return back.to_vec();
                }
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "nothing came back from {to}"
                );
            }
        }

        async fn resolve(&mut self, name: &str, gateway: Ipv4Addr) -> Vec<u8> {
            let mut question = hickory_proto::op::Message::new(
                0x1234,
                hickory_proto::op::MessageType::Query,
                hickory_proto::op::OpCode::Query,
            );
            question.add_query(hickory_proto::op::Query::query(
                hickory_proto::rr::Name::from_ascii(format!("{name}.")).unwrap(),
                hickory_proto::rr::RecordType::A,
            ));
            let server = SocketAddr::new(IpAddr::V4(gateway), dns::PORT);
            let handle = self.bind_udp(5353);
            self.exchange(handle, server, &question.to_vec().unwrap())
                .await
        }

        fn open(&mut self, destination: SocketAddr, local_port: u16) -> SocketHandle {
            let handle = self.sockets.add(tcp::Socket::new(
                tcp::SocketBuffer::new(vec![0; 8192]),
                tcp::SocketBuffer::new(vec![0; 8192]),
            ));
            let endpoint = IpEndpoint::from((destination.ip(), destination.port()));
            self.sockets
                .get_mut::<tcp::Socket>(handle)
                .connect(self.iface.context(), endpoint, local_port)
                .expect("the connect is attempted");
            handle
        }

        async fn connected(&mut self, handle: SocketHandle, within: Duration) -> bool {
            let deadline = tokio::time::Instant::now() + within;
            loop {
                self.settle().await;
                if self.sockets.get::<tcp::Socket>(handle).may_send() {
                    return true;
                }
                if tokio::time::Instant::now() >= deadline {
                    return false;
                }
            }
        }

        async fn half_closed(&mut self, handle: SocketHandle, within: Duration) -> bool {
            let deadline = tokio::time::Instant::now() + within;
            loop {
                self.settle().await;
                if matches!(
                    self.sockets.get::<tcp::Socket>(handle).state(),
                    tcp::State::Closed | tcp::State::CloseWait | tcp::State::TimeWait
                ) {
                    return true;
                }
                if tokio::time::Instant::now() >= deadline {
                    return false;
                }
            }
        }

        async fn closed(&mut self, handle: SocketHandle, within: Duration) -> bool {
            let deadline = tokio::time::Instant::now() + within;
            loop {
                self.settle().await;
                if self.sockets.get::<tcp::Socket>(handle).state() == tcp::State::Closed {
                    return true;
                }
                if tokio::time::Instant::now() >= deadline {
                    return false;
                }
            }
        }
    }

    fn config(boundary: Boundary) -> Config {
        Config::for_subnet(SUBNET, boundary)
    }

    fn udp_packet(destination: &str, payload: &[u8]) -> Vec<u8> {
        let to: SocketAddr = destination.parse().unwrap();
        let IpAddr::V4(ip) = to.ip() else {
            unreachable!("the fixtures are IPv4")
        };
        let builder =
            PacketBuilder::ipv4([192, 168, 127, 2], ip.octets(), PACKET_TTL).udp(45_000, to.port());
        let mut packet = Vec::with_capacity(builder.size(payload.len()));
        builder.write(&mut packet, payload).unwrap();
        packet
    }

    fn tcp_packet(destination: &str, syn: bool) -> Vec<u8> {
        let to: SocketAddr = destination.parse().unwrap();
        let IpAddr::V4(ip) = to.ip() else {
            unreachable!("the fixtures are IPv4")
        };
        let builder = PacketBuilder::ipv4([192, 168, 127, 2], ip.octets(), PACKET_TTL).tcp(
            45_000,
            to.port(),
            0x1000,
            65_535,
        );
        let builder = if syn { builder.syn() } else { builder.ack(1) };
        let mut packet = Vec::with_capacity(builder.size(0));
        builder.write(&mut packet, &[]).unwrap();
        packet
    }

    #[test]
    fn a_dhcp_datagram_is_answered_by_the_gateway_rather_than_routed() {
        let discover = {
            let mut message = vec![0u8; 236];
            message[0] = 1;
            message[1] = 1;
            message[2] = 6;
            message.extend_from_slice(&[99, 130, 83, 99]);
            message.extend_from_slice(&[53, 1, 1, 255]);
            message
        };
        let packet = udp_packet("255.255.255.255:67", &discover);

        let Some(Ingress::Dhcp(reply)) = classify(&config(Boundary::around(SUBNET, 24)), &packet)
        else {
            panic!("the guest's own gateway leases its address");
        };
        assert_eq!(
            reply.to,
            ReplyTo::Unicast {
                mac: [0; 6],
                ip: Ipv4Addr::new(192, 168, 127, 2)
            }
        );
    }

    #[test]
    fn a_question_for_the_gateway_resolver_is_carried_even_though_the_subnet_is_refused() {
        assert_eq!(
            classify(
                &config(Boundary::around(SUBNET, 24)),
                &udp_packet("192.168.127.1:53", b"q")
            ),
            Some(Ingress::Forward),
            "DNS is the one service the gateway offers"
        );
    }

    #[test]
    fn a_datagram_for_any_other_gateway_port_is_dropped_without_a_reply() {
        let Some(Ingress::Refused {
            destination,
            refusal,
            reset,
        }) = classify(
            &config(Boundary::around(SUBNET, 24)),
            &udp_packet("192.168.127.1:8080", b"x"),
        )
        else {
            panic!("there is no control API on this gateway");
        };
        assert_eq!(destination, "192.168.127.1:8080".parse().unwrap());
        assert_eq!(refusal, Refusal::GuestSubnet);
        assert_eq!(reset, None, "a refused datagram is dropped, not answered");
    }

    #[test]
    fn a_syn_to_a_refused_destination_is_answered_with_a_reset() {
        let Some(Ingress::Refused {
            refusal,
            reset: Some(reset),
            ..
        }) = classify(
            &config(Boundary::around(SUBNET, 24)),
            &tcp_packet("127.0.0.1:8080", true),
        )
        else {
            panic!("a refused connection fails at once");
        };
        assert_eq!(refusal, Refusal::Loopback);

        let (header, rest) = Ipv4Header::from_slice(&reset).unwrap();
        assert_eq!(Ipv4Addr::from(header.source), Ipv4Addr::new(127, 0, 0, 1));
        assert_eq!(
            Ipv4Addr::from(header.destination),
            Ipv4Addr::new(192, 168, 127, 2)
        );
        let (tcp, _) = TcpHeader::from_slice(rest).unwrap();
        assert!(tcp.rst, "the guest is told the destination is closed");
        assert!(tcp.ack);
        assert_eq!(tcp.acknowledgment_number, 0x1001);
        assert_eq!(tcp.source_port, 8080);
        assert_eq!(tcp.destination_port, 45_000);
    }

    #[test]
    fn a_later_packet_of_a_refused_connection_is_dropped_without_another_reset() {
        let Some(Ingress::Refused { reset, .. }) = classify(
            &config(Boundary::around(SUBNET, 24)),
            &tcp_packet("127.0.0.1:8080", false),
        ) else {
            panic!("every packet of a refused flow stays refused");
        };
        assert_eq!(reset, None, "only the opening SYN is worth an answer");
    }

    #[test]
    fn an_allowed_destination_is_forwarded_to_the_stack() {
        let boundary = Boundary::around(SUBNET, 24);
        assert_eq!(
            classify(&config(boundary), &tcp_packet("93.184.216.34:443", true)),
            Some(Ingress::Forward)
        );
        assert_eq!(
            classify(&config(boundary), &udp_packet("1.1.1.1:443", b"x")),
            Some(Ingress::Forward)
        );
    }

    #[test]
    fn a_protocol_this_gateway_does_not_decide_on_is_left_to_the_stack() {
        let builder = PacketBuilder::ipv4([192, 168, 127, 2], [93, 184, 216, 34], PACKET_TTL)
            .icmpv4_echo_request(1, 1);
        let mut packet = Vec::with_capacity(builder.size(0));
        builder.write(&mut packet, &[]).unwrap();
        assert_eq!(
            classify(&config(Boundary::around(SUBNET, 24)), &packet),
            Some(Ingress::Forward)
        );
    }

    #[test]
    fn bytes_that_are_not_a_packet_this_gateway_can_read_decide_nothing() {
        let boundary = Boundary::around(SUBNET, 24);
        assert_eq!(classify(&config(boundary), &[0x45, 0x00]), None);

        let mut truncated_udp = udp_packet("1.1.1.1:53", b"x");
        truncated_udp.truncate(22);
        assert_eq!(classify(&config(boundary), &truncated_udp), None);

        let mut truncated_tcp = tcp_packet("1.1.1.1:443", true);
        truncated_tcp.truncate(24);
        assert_eq!(classify(&config(boundary), &truncated_tcp), None);

        let mut not_dhcp = udp_packet("255.255.255.255:67", b"not a lease request");
        not_dhcp.truncate(not_dhcp.len());
        assert_eq!(classify(&config(boundary), &not_dhcp), None);
    }

    #[test]
    fn the_link_leases_the_second_address_of_the_subnet_from_its_first() {
        let config = Config::for_subnet(Ipv4Addr::new(10, 9, 8, 0), Boundary::Permissive);
        assert_eq!(config.lease.gateway, Ipv4Addr::new(10, 9, 8, 1));
        assert_eq!(config.lease.guest, Ipv4Addr::new(10, 9, 8, 2));
        assert_eq!(config.lease.netmask, Ipv4Addr::new(255, 255, 255, 0));
        assert_eq!(config.lease.duration, Duration::from_secs(86_400));
    }

    #[test]
    fn a_lease_a_client_can_only_hear_broadcast_is_sent_to_the_broadcast_address() {
        let reply = dhcp::Reply {
            payload: vec![1, 2, 3],
            to: ReplyTo::Broadcast,
        };
        let frame = dhcp_frame(&config(Boundary::Permissive), &reply);

        let (header, packet) = etherparse::Ethernet2Header::from_slice(&frame).unwrap();
        assert_eq!(header.destination, BROADCAST_MAC);
        assert_eq!(header.source, GATEWAY_MAC);
        let (ip, rest) = Ipv4Header::from_slice(packet).unwrap();
        assert_eq!(ip.destination, [255, 255, 255, 255]);
        assert_eq!(ip.source, [192, 168, 127, 1]);
        let (udp, payload) = UdpHeader::from_slice(rest).unwrap();
        assert_eq!((udp.source_port, udp.destination_port), (67, 68));
        assert_eq!(payload, &[1, 2, 3]);
    }

    #[test]
    fn a_lease_a_client_can_hear_on_its_own_address_is_sent_there() {
        let reply = dhcp::Reply {
            payload: vec![9],
            to: ReplyTo::Unicast {
                mac: GUEST_MAC,
                ip: Ipv4Addr::new(192, 168, 127, 2),
            },
        };
        let frame = dhcp_frame(&config(Boundary::Permissive), &reply);

        let (header, packet) = etherparse::Ethernet2Header::from_slice(&frame).unwrap();
        assert_eq!(header.destination, GUEST_MAC);
        let (ip, _) = Ipv4Header::from_slice(packet).unwrap();
        assert_eq!(ip.destination, [192, 168, 127, 2]);
    }

    #[tokio::test]
    async fn a_flow_whose_guest_has_stopped_sending_closes_its_host_socket() {
        let (outbound, rx) = channel(1);
        let (replies, mut heard) = channel(1);
        drop(outbound);

        carry_udp(
            "192.168.127.2:5000".parse().unwrap(),
            "93.184.216.34:53".parse().unwrap(),
            rx,
            replies,
        )
        .await;

        assert!(heard.try_recv().is_err(), "a closed flow answers nothing");
    }

    #[tokio::test]
    async fn bytes_that_are_not_a_question_get_no_answer_from_the_gateway() {
        let (replies, mut heard) = channel(1);

        answer_dns(
            Arc::new(TestResolver),
            b"not a question".to_vec(),
            "192.168.127.2:5353".parse().unwrap(),
            "192.168.127.1:53".parse().unwrap(),
            replies,
        )
        .await;

        assert!(heard.try_recv().is_err());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_guest_leases_its_address_resolves_a_name_and_reaches_a_host_through_the_stack() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let served = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut heard = [0u8; 5];
            stream.read_exact(&mut heard).await.unwrap();
            stream.write_all(b"pong").await.unwrap();
            stream.shutdown().await.unwrap();
            heard
        });

        let mut guest = FakeGuest::on(Boundary::Permissive);

        let lease = guest.lease().await;
        assert_eq!(
            lease.address,
            IpCidr::new(Ipv4Address::new(192, 168, 127, 2).into(), 24),
            "the guest takes the one address this link leases"
        );
        assert_eq!(lease.router, Some(Ipv4Address::new(192, 168, 127, 1)));
        assert_eq!(lease.dns, vec![Ipv4Address::new(192, 168, 127, 1)]);

        let answer = guest
            .resolve("example.test", Ipv4Addr::new(192, 168, 127, 1))
            .await;
        let answer = hickory_proto::op::Message::from_vec(&answer).expect("a DNS answer");
        assert_eq!(answer.metadata.id, 0x1234);
        assert_eq!(
            answer.answers[0].data,
            hickory_proto::rr::RData::A(hickory_proto::rr::rdata::A(
                "93.184.216.34".parse().unwrap()
            ))
        );

        let echo = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let echo_address = echo.local_addr().unwrap();
        tokio::spawn(async move {
            let mut heard = [0u8; 16];
            for _ in 0..2 {
                let (read, from) = echo.recv_from(&mut heard).await.unwrap();
                heard[..read].reverse();
                echo.send_to(&heard[..read], from).await.unwrap();
            }
        });
        let flow = guest.bind_udp(5354);
        assert_eq!(
            guest.exchange(flow, echo_address, b"ping").await,
            b"gnip",
            "a UDP flow is carried both ways on one host socket"
        );
        assert_eq!(
            guest.exchange(flow, echo_address, b"more").await,
            b"erom",
            "the second datagram of a flow reuses the host socket the first opened"
        );

        let handle = guest.open(address, 40_000);
        assert!(
            guest.connected(handle, PATIENCE).await,
            "a permitted destination is connected to on the host"
        );
        guest
            .sockets
            .get_mut::<tcp::Socket>(handle)
            .send_slice(b"ping!")
            .unwrap();

        let deadline = tokio::time::Instant::now() + PATIENCE;
        let mut back = Vec::new();
        while back.len() < 4 {
            guest.settle().await;
            let socket = guest.sockets.get_mut::<tcp::Socket>(handle);
            if socket.can_recv() {
                socket
                    .recv(|data| {
                        back.extend_from_slice(data);
                        (data.len(), ())
                    })
                    .unwrap();
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the host's reply never came back, got {back:?}"
            );
        }
        assert_eq!(back, b"pong");
        assert!(
            guest.half_closed(handle, PATIENCE).await,
            "the host closing its end closes the guest's"
        );
        assert_eq!(&served.await.unwrap(), b"ping!", "the host heard the guest");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_default_boundary_refuses_the_gateway_and_the_hosts_loopback() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let loopback = listener.local_addr().unwrap();

        let mut guest = FakeGuest::on(Boundary::around(SUBNET, 24));
        guest.lease().await;

        for destination in [loopback, "192.168.127.1:80".parse().unwrap()] {
            let handle = guest.open(destination, 40_001 + destination.port() % 100);
            assert!(
                guest.closed(handle, PATIENCE).await,
                "{destination} must be refused at once, not opened and not left to time out"
            );
        }
    }
}
