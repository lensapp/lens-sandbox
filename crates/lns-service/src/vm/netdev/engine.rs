use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use etherparse::{
    IpNumber, Ipv4Header, PacketBuilder, TcpHeader, UdpHeader, icmpv4::DestUnreachableHeader,
};
use futures_util::future::BoxFuture;
use futures_util::{SinkExt, Stream, StreamExt};
use hickory_proto::op::Message;
use netstack_smoltcp::StackBuilder;
use smoltcp::wire::{Ipv4Packet, UdpPacket};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc::{Receiver, Sender, channel};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

use super::dhcp::{self, Lease, ReplyTo};
use super::dns::{self, Resolvers, Upstream};
use super::link::{BROADCAST_MAC, GATEWAY_MAC, Link, Received};
use super::policy::{Boundary, Refusal};
use crate::log;

/// Vz refuses an MTU below this and the guest's link is configured for it.
pub const MTU: usize = 1500;

/// The MTU, 14 for the ethernet header, 4 for a VLAN tag we never send but must not truncate either.
pub const MAX_FRAME: usize = MTU + 14 + 4;

/// One TCP window per direction per flow. 256 KiB carries a gigabit path at 2 ms without stalling on the window.
const TCP_WINDOW_BYTES: u32 = 256 * 1024;

/// At most this many TCP flows are carried for one guest at a time.
pub const MAX_TCP_FLOWS: usize = 1024;

/// At most this many UDP flows are carried for one guest at a time.
pub const MAX_UDP_FLOWS: usize = 512;

/// At most this many of a guest's DNS queries are in flight at once.
pub const MAX_DNS_IN_FLIGHT: usize = 256;

const TCP_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a flow the guest opened but never finished opening holds its place.
const TCP_HALF_OPEN_TIMEOUT: Duration = Duration::from_secs(30);

/// How often the half-open flows are looked over.
const TCP_HALF_OPEN_SWEEP: Duration = Duration::from_secs(5);

const CHANNEL_DEPTH: usize = 512;

const UDP_IDLE: Duration = Duration::from_secs(60);

const UDP_DATAGRAM_BYTES: usize = 65_535;

/// The MTU less an IPv4 and a UDP header: the largest answer this link carries to the guest in one datagram.
const DNS_UDP_CEILING: usize = MTU - 28;

/// What a guest that advertises no EDNS payload size can take, per RFC 1035.
const DNS_UDP_MINIMUM: usize = 512;

/// RFC 1035 §4.2.2: a DNS connection carries one or more queries, and an idle one is closed.
const DNS_TCP_IDLE: Duration = Duration::from_secs(10);

/// How many items the library refuses to hand over in a row before the reader waits between polls.
const UNUSABLE_BEFORE_WAITING: usize = 4;

/// How long the reader waits between polls once the library hands nothing over.
const UNUSABLE_WAIT: Duration = Duration::from_millis(100);

const PACKET_TTL: u8 = 64;

/// How many distinct refused destinations one run logs before it only counts them.
const MAX_LOGGED_REFUSALS: usize = 1024;

/// How much of a refused datagram an ICMP port-unreachable carries back, per RFC 792.
const UNREACHABLE_QUOTE_BYTES: usize = 8;

/// What the guest's link is and where it may reach. One run, one stack, no state shared with any other run.
#[derive(Debug, Clone, Copy)]
pub struct Config {
    pub lease: Lease,
    pub boundary: Boundary,
    pub limits: Limits,
}

/// How much of the host one guest may hold at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub tcp_flows: usize,
    pub udp_flows: usize,
    pub dns_in_flight: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            tcp_flows: MAX_TCP_FLOWS,
            udp_flows: MAX_UDP_FLOWS,
            dns_in_flight: MAX_DNS_IN_FLIGHT,
        }
    }
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
            limits: Limits::default(),
        }
    }
}

/// What the gateway answers DNS with: the host's resolver list, and the way a query reaches one.
pub struct Gateway {
    pub address: IpAddr,
    pub resolvers: Arc<Resolvers>,
    pub upstream: Arc<dyn Upstream>,
}

/// Everything one query needs answered, whatever transport the guest asked over.
#[derive(Clone)]
struct Resolving {
    gateway: Arc<Gateway>,
    allowance: Arc<Allowance>,
    counters: Arc<Counters>,
    relays: Relays,
}

/// Frames in, frames out. The macOS device is a datagram socketpair; a vhost-user front end would hand over the same two channels.
pub struct Frames {
    pub from_guest: Receiver<Vec<u8>>,
    pub to_guest: Sender<Vec<u8>>,
}

/// Every task serving one guest's network. Dropping it aborts them all, so a stopped run leaves nothing behind.
pub struct Running {
    tasks: JoinSet<()>,
    relays: Relays,
}

impl Running {
    /// A front end adds the tasks that carry its own transport, so one guard owns every task of a run's network.
    pub fn adopt<F: Future<Output = ()> + Send + 'static>(&mut self, task: F) {
        self.tasks.spawn(task);
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.relays.token.cancel();
        self.relays.tracker.close();
        self.tasks.abort_all();
    }
}

/// The owner of every relay a listener starts, so a flow opened mid-run ends with the run and not with its own sockets.
#[derive(Clone)]
struct Relays {
    tracker: TaskTracker,
    token: CancellationToken,
}

impl Relays {
    fn new() -> Self {
        Self {
            tracker: TaskTracker::new(),
            token: CancellationToken::new(),
        }
    }

    fn carry<F: Future<Output = ()> + Send + 'static>(&self, relay: F) {
        let token = self.token.clone();
        self.tracker.spawn(async move {
            tokio::select! {
                () = token.cancelled() => {}
                () = relay => {}
            }
        });
    }
}

/// What the guest network threw away, and how often. Every drop is silent on the wire, so it is never silent in the log.
#[derive(Default)]
pub struct Counters {
    counts: Mutex<HashMap<&'static str, u64>>,
}

impl Counters {
    pub fn note(&self, what: &'static str) {
        let mut counts = self.counts.lock().expect("counters poisoned");
        let seen = counts.entry(what).or_default();
        *seen += 1;
        let seen = *seen;
        log::debug!("the guest network dropped {what} ({seen} so far)");
    }

    pub fn seen(&self, what: &str) -> u64 {
        self.counts
            .lock()
            .expect("counters poisoned")
            .get(what)
            .copied()
            .unwrap_or_default()
    }
}

/// The flows one guest holds. A permit lives as long as its flow and is given back when the flow ends.
pub struct Allowance {
    tcp: Arc<Semaphore>,
    udp: Arc<Semaphore>,
    dns: Arc<Semaphore>,
}

impl Allowance {
    pub fn of(limits: Limits) -> Self {
        Self {
            tcp: Arc::new(Semaphore::new(limits.tcp_flows)),
            udp: Arc::new(Semaphore::new(limits.udp_flows)),
            dns: Arc::new(Semaphore::new(limits.dns_in_flight)),
        }
    }

    pub fn tcp_flow(&self) -> Option<OwnedSemaphorePermit> {
        Arc::clone(&self.tcp).try_acquire_owned().ok()
    }

    pub fn udp_flow(&self) -> Option<OwnedSemaphorePermit> {
        Arc::clone(&self.udp).try_acquire_owned().ok()
    }

    pub fn dns_query(&self) -> Option<OwnedSemaphorePermit> {
        Arc::clone(&self.dns).try_acquire_owned().ok()
    }
}

/// One guest TCP flow, by the two ends the guest named. The library allocates one socket per SYN, so the engine decides per flow.
type Flow = (SocketAddr, SocketAddr);

/// What the engine does with a SYN before the stack may allocate for it.
#[derive(Debug, PartialEq, Eq)]
enum Admission {
    Fresh,
    Repeated,
    Full,
}

struct Admitted {
    _permit: OwnedSemaphorePermit,
    serial: u64,
    since: tokio::time::Instant,
    established: bool,
    claimed: bool,
    cancel: CancellationToken,
}

#[derive(Default)]
struct Open {
    flows: HashMap<Flow, Admitted>,
    next_serial: u64,
}

/// The flows one guest's stack holds. A flow takes its place here — and its permit — before the library sees the SYN that would allocate for it.
pub struct Admissions {
    open: Mutex<Open>,
    allowance: Arc<Allowance>,
}

impl Admissions {
    fn new(allowance: Arc<Allowance>) -> Self {
        Self {
            open: Mutex::new(Open::default()),
            allowance,
        }
    }

    fn admit(&self, flow: Flow) -> Admission {
        let mut open = self.open.lock().expect("admissions poisoned");
        if open.flows.contains_key(&flow) {
            return Admission::Repeated;
        }
        let Some(permit) = self.allowance.tcp_flow() else {
            return Admission::Full;
        };
        let serial = open.next_serial;
        open.next_serial += 1;
        open.flows.insert(
            flow,
            Admitted {
                _permit: permit,
                serial,
                since: tokio::time::Instant::now(),
                established: false,
                claimed: false,
                cancel: CancellationToken::new(),
            },
        );
        Admission::Fresh
    }

    /// A segment the guest sends after its SYN is the only proof its handshake finished; until one arrives the flow is half-open.
    fn established(&self, flow: Flow) {
        let mut open = self.open.lock().expect("admissions poisoned");
        if let Some(admitted) = open.flows.get_mut(&flow) {
            admitted.established = true;
        }
    }

    /// One admitted flow is one stream: a second claim of the same flow is nobody's, and may not release what the first holds.
    fn claim(self: &Arc<Self>, flow: Flow) -> Option<Claim> {
        let mut open = self.open.lock().expect("admissions poisoned");
        let admitted = open.flows.get_mut(&flow).filter(|held| !held.claimed)?;
        admitted.claimed = true;
        Some(Claim {
            admissions: Arc::clone(self),
            flow,
            serial: admitted.serial,
            cancel: admitted.cancel.clone(),
        })
    }

    /// A guest that opens flows it never finishes would hold them all; each one leaves after this long, and its relay ends with it.
    fn expire(&self, after: Duration) -> usize {
        let mut open = self.open.lock().expect("admissions poisoned");
        let before = open.flows.len();
        open.flows.retain(|_, admitted| {
            let keep = admitted.established || admitted.since.elapsed() < after;
            if !keep {
                admitted.cancel.cancel();
            }
            keep
        });
        before - open.flows.len()
    }

    #[cfg(test)]
    fn held(&self) -> usize {
        self.open.lock().expect("admissions poisoned").flows.len()
    }

    fn release(&self, flow: Flow, serial: u64) {
        let mut open = self.open.lock().expect("admissions poisoned");
        if open
            .flows
            .get(&flow)
            .is_some_and(|held| held.serial == serial)
        {
            open.flows.remove(&flow);
        }
    }
}

/// The place one relay holds in the admissions table. Dropping it gives the place and its permit back.
struct Claim {
    admissions: Arc<Admissions>,
    flow: Flow,
    serial: u64,
    cancel: CancellationToken,
}

impl Drop for Claim {
    fn drop(&mut self) {
        self.admissions.release(self.flow, self.serial);
    }
}

async fn expire_half_open(admissions: Arc<Admissions>, counters: Arc<Counters>) {
    loop {
        tokio::time::sleep(TCP_HALF_OPEN_SWEEP).await;
        for _ in 0..admissions.expire(TCP_HALF_OPEN_TIMEOUT) {
            counters.note(DROPPED_TCP_HALF_OPEN);
        }
    }
}

pub const DROPPED_TCP_FLOWS: &str = "a TCP flow over the guest's limit";
pub const DROPPED_TCP_REPEATED: &str = "a repeated SYN for a flow the stack already has";
pub const DROPPED_TCP_HALF_OPEN: &str = "a TCP flow the guest never finished opening";
pub const DROPPED_TCP_UNADMITTED: &str = "a TCP stream whose flow had already ended";
pub const DROPPED_UDP_FLOWS: &str = "a UDP flow over the guest's limit";
pub const DROPPED_DNS_IN_FLIGHT: &str = "a DNS query over the guest's limit";
pub const DROPPED_FRAGMENT: &str = "an IPv4 fragment, which this gateway does not reassemble";
pub const DROPPED_PROTOCOL: &str = "a packet of a protocol this gateway does not carry";
pub const DROPPED_MALFORMED: &str = "a packet too short or malformed to read";
pub const DROPPED_UNUSABLE: &str = "a packet the stack would not hand over";
pub const DROPPED_DNS_ANSWER: &str = "a DNS answer the gateway could not fit to the guest";

/// The library ends its stream on one item it cannot parse, which would end UDP and DNS for the rest of the run; a run reads on until it is cancelled.
async fn next_usable<S: Stream + Unpin>(
    stream: &mut S,
    counters: &Counters,
    token: &CancellationToken,
) -> Option<S::Item> {
    let mut unusable = 0usize;
    loop {
        let item = tokio::select! {
            () = token.cancelled() => return None,
            item = stream.next() => item,
        };
        if item.is_some() {
            return item;
        }
        unusable += 1;
        if unusable > UNUSABLE_BEFORE_WAITING {
            tokio::select! {
                () = token.cancelled() => return None,
                () = tokio::time::sleep(UNUSABLE_WAIT) => {}
            }
        } else {
            counters.note(DROPPED_UNUSABLE);
        }
    }
}

pub fn start(config: Config, gateway: Gateway, frames: Frames) -> Result<Running> {
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
    let refused = Arc::new(Mutex::new(Refusals::default()));
    let counters = Arc::new(Counters::default());
    let allowance = Arc::new(Allowance::of(config.limits));
    let admissions = Arc::new(Admissions::new(Arc::clone(&allowance)));
    let (stack_sink, stack_stream) = stack.split();
    let (udp_read, udp_write) = udp.split();
    let (replies_tx, replies_rx) = channel(CHANNEL_DEPTH);

    let relays = Relays::new();
    let mut tasks = JoinSet::new();
    tasks.spawn(async move {
        let _ = runner.await;
    });
    tasks.spawn(from_guest(
        config,
        refused,
        Arc::clone(&counters),
        Arc::clone(&admissions),
        Arc::clone(&link),
        frames.from_guest,
        frames.to_guest.clone(),
        stack_sink,
    ));
    tasks.spawn(to_guest(link, stack_stream, frames.to_guest));
    tasks.spawn(expire_half_open(
        Arc::clone(&admissions),
        Arc::clone(&counters),
    ));
    let resolving = Resolving {
        gateway: Arc::new(gateway),
        allowance,
        counters,
        relays: relays.clone(),
    };
    tasks.spawn(accept_tcp(
        admissions,
        resolving.clone(),
        gateway_resolver(&config),
        tcp,
    ));
    tasks.spawn(accept_udp(config, resolving, udp_read, replies_tx));
    tasks.spawn(write_udp(udp_write, replies_rx));
    Ok(Running { tasks, relays })
}

type StackSink = futures_util::stream::SplitSink<netstack_smoltcp::Stack, Vec<u8>>;
type StackStream = futures_util::stream::SplitStream<netstack_smoltcp::Stack>;

#[allow(clippy::too_many_arguments)]
async fn from_guest(
    config: Config,
    refused: Refused,
    counters: Arc<Counters>,
    admissions: Arc<Admissions>,
    link: Arc<Mutex<Link>>,
    mut frames: Receiver<Vec<u8>>,
    answers: Sender<Vec<u8>>,
    mut stack: StackSink,
) {
    while let Some(frame) = frames.recv().await {
        let outcome = {
            let mut link = link.lock().expect("link poisoned");
            match link.receive(&frame) {
                Received::Answer(reply) => Outcome::Frame(reply),
                Received::Ipv4(packet) => {
                    ingress(&config, &refused, &counters, &admissions, &link, packet)
                }
                Received::Dropped(what) => Outcome::Dropped(what),
            }
        };
        match outcome {
            Outcome::Frame(reply) => drop(answers.send(reply).await),
            Outcome::Packet(packet) => drop(stack.send(packet).await),
            Outcome::Dropped(what) => counters.note(what),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Frame(Vec<u8>),
    Packet(Vec<u8>),
    Dropped(&'static str),
}

fn ingress(
    config: &Config,
    refused: &Refused,
    counters: &Counters,
    admissions: &Admissions,
    link: &Link,
    packet: &[u8],
) -> Outcome {
    let Some(packet) = whole_packet(packet) else {
        return Outcome::Dropped(DROPPED_MALFORMED);
    };
    match classify(config, admissions, packet) {
        Ingress::Dhcp(reply) => Outcome::Frame(dhcp_frame(config, &reply)),
        Ingress::Answer(packet) => Outcome::Frame(link.send_ipv4(&packet)),
        Ingress::Forward => Outcome::Packet(packet.to_vec()),
        Ingress::Dropped(what) => Outcome::Dropped(what),
        Ingress::OverLimit { answer, counted } => {
            counters.note(counted);
            Outcome::Frame(link.send_ipv4(&answer))
        }
        Ingress::Refused {
            destination,
            refusal,
            answer,
        } => {
            note_refusal(refused, counters, destination, refusal);
            Outcome::Frame(link.send_ipv4(&answer))
        }
    }
}

/// The packet the guest sent, trimmed to the length its own header states: a frame padded to the 60-byte ethernet minimum carries trailing bytes the stack must not read as payload.
fn whole_packet(packet: &[u8]) -> Option<&[u8]> {
    let checked = Ipv4Packet::new_checked(packet).ok()?;
    packet.get(..usize::from(checked.total_len()))
}

/// What the host does with one IPv4 packet the guest sent, decided before any socket exists for it.
#[derive(Debug, PartialEq, Eq)]
enum Ingress {
    Dhcp(dhcp::Reply),
    Answer(Vec<u8>),
    Refused {
        destination: SocketAddr,
        refusal: Refusal,
        answer: Vec<u8>,
    },
    /// Over a bound the guest may not pass, answered the way a refusal is and counted as its own drop.
    OverLimit {
        answer: Vec<u8>,
        counted: &'static str,
    },
    Forward,
    Dropped(&'static str),
}

fn classify(config: &Config, admissions: &Admissions, packet: &[u8]) -> Ingress {
    let Ok((header, rest)) = Ipv4Header::from_slice(packet) else {
        return Ingress::Dropped(DROPPED_MALFORMED);
    };
    if header.more_fragments || header.fragment_offset.value() != 0 {
        return Ingress::Dropped(DROPPED_FRAGMENT);
    }
    let source = Ipv4Addr::from(header.source);
    let destination = Ipv4Addr::from(header.destination);
    match header.protocol {
        IpNumber::UDP => udp_ingress(config, source, destination, rest),
        IpNumber::TCP => tcp_ingress(config, admissions, source, destination, rest),
        IpNumber::ICMP => icmp_ingress(config, source, destination, rest),
        _ => Ingress::Dropped(DROPPED_PROTOCOL),
    }
}

fn udp_ingress(config: &Config, source: Ipv4Addr, destination: Ipv4Addr, rest: &[u8]) -> Ingress {
    let Ok((udp, payload)) = UdpHeader::from_slice(rest) else {
        return Ingress::Dropped(DROPPED_MALFORMED);
    };
    if UdpPacket::new_checked(rest).is_err() {
        return Ingress::Dropped(DROPPED_MALFORMED);
    }
    if udp.destination_port == dhcp::SERVER_PORT {
        return match dhcp::answer(&config.lease, payload) {
            Some(reply) => Ingress::Dhcp(reply),
            None => Ingress::Dropped(DROPPED_MALFORMED),
        };
    }
    let to = SocketAddr::new(IpAddr::V4(destination), udp.destination_port);
    if to == gateway_resolver(config) {
        return Ingress::Forward;
    }
    let unreachable = port_unreachable(source, destination, &udp, payload);
    refused_or_forward(config, to, unreachable)
}

fn tcp_ingress(
    config: &Config,
    admissions: &Admissions,
    source: Ipv4Addr,
    destination: Ipv4Addr,
    rest: &[u8],
) -> Ingress {
    let Ok((tcp, _)) = TcpHeader::from_slice(rest) else {
        return Ingress::Dropped(DROPPED_MALFORMED);
    };
    let to = SocketAddr::new(IpAddr::V4(destination), tcp.destination_port);
    let reset = reset_packet(source, destination, &tcp);
    if to != gateway_resolver(config)
        && let Some(refusal) = config.boundary.refusal(to)
    {
        return Ingress::Refused {
            destination: to,
            refusal,
            answer: reset,
        };
    }
    let flow = (SocketAddr::new(IpAddr::V4(source), tcp.source_port), to);
    if !tcp.syn || tcp.ack {
        admissions.established(flow);
        return Ingress::Forward;
    }
    match admissions.admit(flow) {
        Admission::Fresh => Ingress::Forward,
        Admission::Repeated => Ingress::Dropped(DROPPED_TCP_REPEATED),
        Admission::Full => Ingress::OverLimit {
            answer: reset,
            counted: DROPPED_TCP_FLOWS,
        },
    }
}

/// The gateway answers a ping at its own address, so a guest can tell a dead link from a refused destination.
fn icmp_ingress(config: &Config, source: Ipv4Addr, destination: Ipv4Addr, rest: &[u8]) -> Ingress {
    let Ok((icmp, payload)) = etherparse::Icmpv4Header::from_slice(rest) else {
        return Ingress::Dropped(DROPPED_MALFORMED);
    };
    let etherparse::Icmpv4Type::EchoRequest(echo) = icmp.icmp_type else {
        return Ingress::Dropped(DROPPED_PROTOCOL);
    };
    if destination != config.lease.gateway {
        return Ingress::Dropped(DROPPED_PROTOCOL);
    }
    let builder = PacketBuilder::ipv4(destination.octets(), source.octets(), PACKET_TTL)
        .icmpv4_echo_reply(echo.id, echo.seq);
    let mut reply = Vec::with_capacity(builder.size(payload.len()));
    let _ = builder.write(&mut reply, payload);
    Ingress::Answer(reply)
}

fn gateway_resolver(config: &Config) -> SocketAddr {
    SocketAddr::new(IpAddr::V4(config.lease.gateway), dns::PORT)
}

fn refused_or_forward(config: &Config, destination: SocketAddr, answer: Vec<u8>) -> Ingress {
    match config.boundary.refusal(destination) {
        Some(refusal) => Ingress::Refused {
            destination,
            refusal,
            answer,
        },
        None => Ingress::Forward,
    }
}

/// A refused connection is answered the way a closed port is, so the guest fails at once instead of waiting out a timeout.
fn reset_packet(source: Ipv4Addr, destination: Ipv4Addr, segment: &TcpHeader) -> Vec<u8> {
    let ip = PacketBuilder::ipv4(destination.octets(), source.octets(), PACKET_TTL);
    let ports = (segment.destination_port, segment.source_port);
    // RFC 793: a segment that carries an acknowledgement is reset from its own ACK; one that does not is reset from zero.
    let builder = if segment.ack {
        ip.tcp(ports.0, ports.1, segment.acknowledgment_number, 0)
            .rst()
    } else {
        ip.tcp(ports.0, ports.1, 0, 0)
            .rst()
            .ack(segment.sequence_number.wrapping_add(1))
    };
    let mut packet = Vec::with_capacity(builder.size(0));
    let _ = builder.write(&mut packet, &[]);
    packet
}

/// A refused datagram is answered the way a closed port is: the quoted header lets the guest match it to what it sent.
fn port_unreachable(
    source: Ipv4Addr,
    destination: Ipv4Addr,
    udp: &UdpHeader,
    payload: &[u8],
) -> Vec<u8> {
    let quoted = quote(source, destination, udp, payload);
    let builder = PacketBuilder::ipv4(destination.octets(), source.octets(), PACKET_TTL).icmpv4(
        etherparse::Icmpv4Type::DestinationUnreachable(DestUnreachableHeader::Port),
    );
    let mut packet = Vec::with_capacity(builder.size(quoted.len()));
    let _ = builder.write(&mut packet, &quoted);
    packet
}

fn quote(source: Ipv4Addr, destination: Ipv4Addr, udp: &UdpHeader, payload: &[u8]) -> Vec<u8> {
    let mut quoted = Vec::new();
    let length = UdpHeader::LEN + payload.len();
    if let Ok(header) = Ipv4Header::new(
        u16::try_from(length).unwrap_or(u16::MAX),
        PACKET_TTL,
        IpNumber::UDP,
        source.octets(),
        destination.octets(),
    ) {
        let _ = header.write(&mut quoted);
    }
    quoted.extend_from_slice(&udp.to_bytes());
    quoted.extend_from_slice(&payload[..payload.len().min(UNREACHABLE_QUOTE_BYTES)]);
    quoted
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

type Refused = Arc<Mutex<Refusals>>;

/// A guest that scans ports refuses a destination it never repeats, so what is remembered to log once is bounded and the count is not.
#[derive(Default)]
struct Refusals {
    logged: HashSet<SocketAddr>,
    capped: bool,
}

fn note_refusal(refused: &Refused, counters: &Counters, destination: SocketAddr, refusal: Refusal) {
    counters.note(refusal.reason());
    let mut refusals = refused.lock().expect("refusals poisoned");
    if refusals.logged.contains(&destination) {
        return;
    }
    if refusals.logged.len() >= MAX_LOGGED_REFUSALS {
        if !refusals.capped {
            refusals.capped = true;
            log::debug!(
                "the guest network has refused {MAX_LOGGED_REFUSALS} distinct destinations; further refusals are counted, not logged"
            );
        }
        return;
    }
    refusals.logged.insert(destination);
    let reason = refusal.reason();
    log::debug!("the guest network refused {destination}: {reason}");
}

/// One stream per admitted flow. The flow took its place at ingress, so here it is only claimed — and given back when the relay ends.
async fn accept_tcp<G, S>(
    admissions: Arc<Admissions>,
    resolving: Resolving,
    resolver: SocketAddr,
    mut listener: S,
) where
    S: Stream<Item = (G, SocketAddr, SocketAddr)> + Unpin,
    G: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let relays = resolving.relays.clone();
    while let Some((guest, source, destination)) =
        next_usable(&mut listener, &resolving.counters, &relays.token).await
    {
        let Some(claim) = admissions.claim((source, destination)) else {
            resolving.counters.note(DROPPED_TCP_UNADMITTED);
            continue;
        };
        let cancel = claim.cancel.clone();
        let carried: BoxFuture<'static, ()> = if destination == resolver {
            Box::pin(serve_dns_over_tcp(guest, resolving.clone()))
        } else {
            Box::pin(carry_tcp(guest, destination))
        };
        relays.carry(async move {
            let _claim = claim;
            tokio::select! {
                () = cancel.cancelled() => {}
                () = carried => {}
            }
        });
    }
}

/// The gateway answers a query over TCP as it does over UDP, so a guest that sees TC set has somewhere to ask again.
async fn serve_dns_over_tcp<G: AsyncRead + AsyncWrite + Unpin>(mut guest: G, resolving: Resolving) {
    while let Ok(Some(query)) = tokio::time::timeout(DNS_TCP_IDLE, read_query(&mut guest)).await {
        let Some(answer) = answered(&resolving, &query).await else {
            return;
        };
        if write_answer(&mut guest, &answer, &resolving.counters)
            .await
            .is_none()
        {
            return;
        }
    }
}

async fn read_query<G: AsyncRead + Unpin>(guest: &mut G) -> Option<Vec<u8>> {
    let mut length = [0u8; 2];
    guest.read_exact(&mut length).await.ok()?;
    let mut query = vec![0u8; usize::from(u16::from_be_bytes(length))];
    guest.read_exact(&mut query).await.ok()?;
    Some(query)
}

async fn answered(resolving: &Resolving, query: &[u8]) -> Option<Vec<u8>> {
    let Some(name) = dns::question_name(query) else {
        resolving.counters.note(DROPPED_MALFORMED);
        return None;
    };
    let Some(_permit) = resolving.allowance.dns_query() else {
        resolving.counters.note(DROPPED_DNS_IN_FLIGHT);
        return None;
    };
    Some(resolved(&resolving.gateway, query, &name).await)
}

async fn write_answer<G: AsyncWrite + Unpin>(
    guest: &mut G,
    answer: &[u8],
    counters: &Counters,
) -> Option<()> {
    let Ok(length) = u16::try_from(answer.len()) else {
        counters.note(DROPPED_DNS_ANSWER);
        return None;
    };
    guest.write_all(&length.to_be_bytes()).await.ok()?;
    guest.write_all(answer).await.ok()
}

async fn carry_tcp<G: AsyncRead + AsyncWrite + Unpin>(mut guest: G, destination: SocketAddr) {
    let connect = tokio::net::TcpStream::connect(destination);
    // copy_bidirectional shuts each write half down on the other's EOF, and dropping both ends closes what is left.
    if let Ok(Ok(mut host)) = tokio::time::timeout(TCP_CONNECT_TIMEOUT, connect).await {
        let _ = tokio::io::copy_bidirectional(&mut guest, &mut host).await;
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
    resolving: Resolving,
    mut datagrams: netstack_smoltcp::udp::ReadHalf,
    replies: Sender<UdpReply>,
) {
    let resolver = gateway_resolver(&config);
    let (allowance, counters, relays) = (
        Arc::clone(&resolving.allowance),
        Arc::clone(&resolving.counters),
        resolving.relays.clone(),
    );
    let mut flows: HashMap<(SocketAddr, SocketAddr), Sender<Vec<u8>>> = HashMap::new();
    while let Some((payload, source, destination)) =
        next_usable(&mut datagrams, &counters, &relays.token).await
    {
        if destination == resolver {
            answer_dns(&resolving, payload, source, &replies);
            continue;
        }
        let key = (source, destination);
        let open = match flows.get(&key) {
            Some(open) if !open.is_closed() => Some(open.clone()),
            _ => open_flow(&allowance, &counters, &relays, &mut flows, key, &replies),
        };
        if let Some(open) = open {
            let _ = open.send(payload).await;
        }
    }
}

fn open_flow(
    allowance: &Allowance,
    counters: &Counters,
    relays: &Relays,
    flows: &mut HashMap<(SocketAddr, SocketAddr), Sender<Vec<u8>>>,
    key: (SocketAddr, SocketAddr),
    replies: &Sender<UdpReply>,
) -> Option<Sender<Vec<u8>>> {
    flows.retain(|_, open| !open.is_closed());
    let Some(permit) = allowance.udp_flow() else {
        counters.note(DROPPED_UDP_FLOWS);
        return None;
    };
    let (tx, rx) = channel(CHANNEL_DEPTH);
    let replies = replies.clone();
    relays.carry(async move {
        let _permit = permit;
        carry_udp(key.0, key.1, rx, replies).await;
    });
    flows.insert(key, tx.clone());
    Some(tx)
}

fn answer_dns(
    resolving: &Resolving,
    query: Vec<u8>,
    source: SocketAddr,
    replies: &Sender<UdpReply>,
) {
    let Some(name) = dns::question_name(&query) else {
        resolving.counters.note(DROPPED_MALFORMED);
        return;
    };
    let Some(permit) = resolving.allowance.dns_query() else {
        resolving.counters.note(DROPPED_DNS_IN_FLIGHT);
        return;
    };
    let resolver = SocketAddr::new(resolving.gateway.address, dns::PORT);
    let relaying = resolving.clone();
    let replies = replies.clone();
    resolving.relays.carry(async move {
        let _permit = permit;
        relay_query(relaying, query, name, source, resolver, replies).await;
    });
}

async fn relay_query(
    resolving: Resolving,
    query: Vec<u8>,
    name: String,
    source: SocketAddr,
    resolver: SocketAddr,
    replies: Sender<UdpReply>,
) {
    let answer = resolved(&resolving.gateway, &query, &name).await;
    let Some(answer) = fit_to_guest(&query, answer) else {
        resolving.counters.note(DROPPED_DNS_ANSWER);
        return;
    };
    let _ = replies.send((answer, resolver, source)).await;
}

/// One question put to the host's own resolvers. A failure nobody answered makes the next query read the host's configuration again.
async fn resolved(gateway: &Gateway, query: &[u8], name: &str) -> Vec<u8> {
    let servers = gateway.resolvers.servers_for(name);
    let answer = dns::relay(query, &servers, gateway.upstream.as_ref()).await;
    if answer == dns::servfail(query) {
        gateway.resolvers.stale();
    }
    answer
}

/// RFC 2181 §9: an answer the guest cannot take over UDP goes back as its question with TC set, so the guest asks again over TCP.
fn fit_to_guest(query: &[u8], answer: Vec<u8>) -> Option<Vec<u8>> {
    if answer.len() <= udp_limit_of(query) {
        return Some(answer);
    }
    let mut fitted = Message::from_vec(&answer).ok()?.truncate();
    if let Ok(asked) = Message::from_vec(query)
        && let Some(edns) = asked.edns
    {
        fitted.set_edns(edns);
    }
    fitted.to_vec().ok()
}

/// What the guest said it can take, never more than one datagram of this link carries.
fn udp_limit_of(query: &[u8]) -> usize {
    Message::from_vec(query)
        .map_or(DNS_UDP_MINIMUM, |asked| usize::from(asked.max_payload()))
        .min(DNS_UDP_CEILING)
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

/// The link is IPv4-only, so every flow the boundary lets through is carried from an IPv4 socket.
async fn host_socket(destination: SocketAddr) -> std::io::Result<tokio::net::UdpSocket> {
    let socket = tokio::net::UdpSocket::bind("0.0.0.0:0").await?;
    socket.connect(destination).await?;
    Ok(socket)
}

enum Step {
    Out(Vec<u8>),
    In(usize),
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
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const GUEST_MAC: [u8; 6] = [0x02, 0x11, 0x22, 0x33, 0x44, 0x55];
    const SUBNET: Ipv4Addr = Ipv4Addr::new(192, 168, 127, 0);
    const PATIENCE: Duration = Duration::from_secs(10);

    /// The one nameserver the test guest's questions reach, standing in for the host's own.
    const NAMESERVER: &str = "203.0.113.53:53";

    struct TestSources;

    impl dns::Sources for TestSources {
        fn scopes(&self) -> Vec<dns::Scope> {
            vec![dns::Scope {
                suffix: None,
                servers: vec![NAMESERVER.parse().unwrap()],
            }]
        }
    }

    struct TestUpstream;

    impl Upstream for TestUpstream {
        fn over_udp(
            &self,
            _server: SocketAddr,
            query: Vec<u8>,
        ) -> BoxFuture<'static, std::io::Result<Vec<u8>>> {
            let answered = answer_of(&query, true);
            Box::pin(async move { answered })
        }

        fn over_tcp(
            &self,
            _server: SocketAddr,
            query: Vec<u8>,
        ) -> BoxFuture<'static, std::io::Result<Vec<u8>>> {
            let answered = answer_of(&query, false);
            Box::pin(async move { answered })
        }
    }

    /// The name whose UDP answer this stand-in nameserver truncates, so the guest's query is asked again over TCP.
    const TRUNCATED: &str = "big.test.";

    /// The name this stand-in nameserver answers with more than any guest takes over UDP.
    const LARGE: &str = "large.test.";

    /// How many addresses the large answer carries; 120 of them put it past 1800 bytes.
    const LARGE_RECORDS: u32 = 120;

    fn address_record(at: u32) -> hickory_proto::rr::Record {
        hickory_proto::rr::Record::from_rdata(
            hickory_proto::rr::Name::from_ascii("example.test.").unwrap(),
            60,
            hickory_proto::rr::RData::A(hickory_proto::rr::rdata::A(Ipv4Addr::from(
                0x5d_b8_d8_00 + at,
            ))),
        )
    }

    /// `example.test`, `big.test` and `large.test` resolve; every other name is one this stand-in nameserver never answers.
    fn answer_of(query: &[u8], truncate: bool) -> std::io::Result<Vec<u8>> {
        let request = hickory_proto::op::Message::from_vec(query)
            .map_err(|_| std::io::Error::other("not a question"))?;
        let name = request.queries[0].name().to_string();
        if name != "example.test." && name != TRUNCATED && name != LARGE {
            return Err(std::io::Error::other("no such name"));
        }
        let mut reply = hickory_proto::op::Message::new(
            request.metadata.id,
            hickory_proto::op::MessageType::Response,
            hickory_proto::op::OpCode::Query,
        );
        reply.metadata.truncation = truncate && name == TRUNCATED;
        reply.queries = request.queries;
        reply.answers = match name.as_str() {
            LARGE => (0..LARGE_RECORDS).map(address_record).collect(),
            _ => vec![address_record(34)],
        };
        reply.to_vec().map_err(std::io::Error::other)
    }

    /// One question on the wire, with the EDNS payload size the guest advertises when it advertises one.
    fn question_of(
        name: &str,
        kind: hickory_proto::rr::RecordType,
        id: u16,
        edns: Option<u16>,
    ) -> Vec<u8> {
        let mut question = hickory_proto::op::Message::new(
            id,
            hickory_proto::op::MessageType::Query,
            hickory_proto::op::OpCode::Query,
        );
        question.add_query(hickory_proto::op::Query::query(
            hickory_proto::rr::Name::from_ascii(format!("{name}.")).unwrap(),
            kind,
        ));
        if let Some(payload) = edns {
            let mut advertised = hickory_proto::op::Edns::new();
            advertised.set_max_payload(payload);
            question.set_edns(advertised);
        }
        question.to_vec().unwrap()
    }

    fn gateway_of(subnet: Ipv4Addr) -> Gateway {
        Gateway {
            address: IpAddr::V4(Ipv4Addr::new(
                subnet.octets()[0],
                subnet.octets()[1],
                subnet.octets()[2],
                1,
            )),
            resolvers: Arc::new(dns::Resolvers::new(
                Arc::new(TestSources),
                dns::REFRESH_AFTER,
            )),
            upstream: Arc::new(TestUpstream),
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
        next_port: u16,
        device: GuestDevice,
        iface: Interface,
        sockets: SocketSet<'static>,
        dhcp: SocketHandle,
        running: Option<Running>,
    }

    #[derive(Debug, PartialEq, Eq)]
    struct Lease {
        address: IpCidr,
        router: Option<Ipv4Address>,
        dns: Vec<Ipv4Address>,
    }

    impl FakeGuest {
        fn on(boundary: Boundary) -> Self {
            Self::limited(boundary, Limits::default())
        }

        fn limited(boundary: Boundary, limits: Limits) -> Self {
            let (to_stack, from_guest) = channel(CHANNEL_DEPTH);
            let (to_guest, from_stack) = channel(CHANNEL_DEPTH);
            let running = start(
                Config {
                    limits,
                    ..Config::for_subnet(SUBNET, boundary)
                },
                gateway_of(SUBNET),
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
                next_port: 6000,
                device,
                iface,
                sockets,
                dhcp,
                running: Some(running),
            }
        }

        fn adopt<F: Future<Output = ()> + Send + 'static>(&mut self, task: F) {
            self.running.as_mut().expect("the run is up").adopt(task);
        }

        fn relays(&self) -> Relays {
            self.running.as_ref().expect("the run is up").relays.clone()
        }

        fn stop(&mut self) {
            self.running = None;
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

        async fn send_only(&mut self, handle: SocketHandle, to: SocketAddr, payload: &[u8]) {
            self.sockets
                .get_mut::<udp::Socket>(handle)
                .send_slice(payload, IpEndpoint::from((to.ip(), to.port())))
                .expect("the datagram is sent");
            for _ in 0..500 {
                self.settle().await;
            }
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

        async fn question(
            &mut self,
            gateway: Ipv4Addr,
            name: &str,
            kind: hickory_proto::rr::RecordType,
        ) -> Vec<u8> {
            self.advertising(gateway, name, kind, None).await
        }

        async fn advertising(
            &mut self,
            gateway: Ipv4Addr,
            name: &str,
            kind: hickory_proto::rr::RecordType,
            edns: Option<u16>,
        ) -> Vec<u8> {
            let server = SocketAddr::new(IpAddr::V4(gateway), dns::PORT);
            let handle = self.bind_udp(self.next_port);
            self.next_port += 1;
            self.exchange(handle, server, &question_of(name, kind, 0x1234, edns))
                .await
        }

        async fn ask(&mut self, name: &str, gateway: Ipv4Addr) -> hickory_proto::op::Message {
            let answer = self
                .question(gateway, name, hickory_proto::rr::RecordType::A)
                .await;
            hickory_proto::op::Message::from_vec(&answer).expect("a DNS answer")
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

        /// One length-prefixed exchange on a TCP connection the guest opens, read until the answer the prefix promises is whole.
        async fn over_tcp(
            &mut self,
            destination: SocketAddr,
            local_port: u16,
            sent: &[u8],
        ) -> Vec<u8> {
            let handle = self.open(destination, local_port);
            assert!(
                self.wait_until(handle, PATIENCE, |socket| socket.may_send())
                    .await,
                "the gateway never opened the connection"
            );
            self.sockets
                .get_mut::<tcp::Socket>(handle)
                .send_slice(sent)
                .expect("the guest sends its query");
            let deadline = tokio::time::Instant::now() + PATIENCE;
            let mut back: Vec<u8> = Vec::new();
            loop {
                self.settle().await;
                let socket = self.sockets.get_mut::<tcp::Socket>(handle);
                if socket.can_recv() {
                    socket
                        .recv(|data| {
                            back.extend_from_slice(data);
                            (data.len(), ())
                        })
                        .unwrap();
                }
                if back.len() >= 2
                    && back.len() >= 2 + usize::from(u16::from_be_bytes([back[0], back[1]]))
                {
                    return back;
                }
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "the gateway never answered over TCP, got {} bytes",
                    back.len()
                );
            }
        }

        async fn wait_until(
            &mut self,
            handle: SocketHandle,
            within: Duration,
            ready: impl Fn(&tcp::Socket) -> bool,
        ) -> bool {
            let deadline = tokio::time::Instant::now() + within;
            let mut reached = false;
            while !reached && tokio::time::Instant::now() < deadline {
                self.settle().await;
                reached = ready(self.sockets.get::<tcp::Socket>(handle));
            }
            reached
        }
    }

    fn config(boundary: Boundary) -> Config {
        Config::for_subnet(SUBNET, boundary)
    }

    /// The gateway a test asks its questions of, with the counters and the allowance the test judges by.
    fn resolving(limits: Limits, counters: &Arc<Counters>, relays: &Relays) -> Resolving {
        Resolving {
            gateway: Arc::new(gateway_of(SUBNET)),
            allowance: Arc::new(Allowance::of(limits)),
            counters: Arc::clone(counters),
            relays: relays.clone(),
        }
    }

    fn admissions(limits: Limits) -> Arc<Admissions> {
        Arc::new(Admissions::new(Arc::new(Allowance::of(limits))))
    }

    /// One packet judged by a run that has no flow open yet, which is what every classification test but the admission ones asks about.
    fn decide(config: &Config, packet: &[u8]) -> Ingress {
        classify(config, &admissions(Limits::default()), packet)
    }

    type Refusal3 = (SocketAddr, Refusal, Vec<u8>);

    fn refused_by(ingress: Ingress) -> Option<Refusal3> {
        match ingress {
            Ingress::Refused {
                destination,
                refusal,
                answer,
            } => Some((destination, refusal, answer)),
            _ => None,
        }
    }

    fn leased_by(ingress: Ingress) -> Option<dhcp::Reply> {
        match ingress {
            Ingress::Dhcp(reply) => Some(reply),
            _ => None,
        }
    }

    fn answered_by(ingress: Ingress) -> Option<Vec<u8>> {
        match ingress {
            Ingress::Answer(packet) => Some(packet),
            _ => None,
        }
    }

    fn udp_packet(destination: &str, payload: &[u8]) -> Vec<u8> {
        let to: std::net::SocketAddrV4 = destination.parse().unwrap();
        let builder = PacketBuilder::ipv4([192, 168, 127, 2], to.ip().octets(), PACKET_TTL)
            .udp(45_000, to.port());
        let mut packet = Vec::with_capacity(builder.size(payload.len()));
        builder.write(&mut packet, payload).unwrap();
        packet
    }

    fn tcp_packet(destination: &str, syn: bool) -> Vec<u8> {
        let to: std::net::SocketAddrV4 = destination.parse().unwrap();
        let builder = PacketBuilder::ipv4([192, 168, 127, 2], to.ip().octets(), PACKET_TTL).tcp(
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

        let reply = leased_by(decide(&config(Boundary::around(SUBNET, 24)), &packet))
            .expect("the guest's own gateway leases its address");

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
            decide(
                &config(Boundary::around(SUBNET, 24)),
                &udp_packet("192.168.127.1:53", b"q")
            ),
            Ingress::Forward,
            "DNS is the one service the gateway offers"
        );
    }

    #[test]
    fn a_datagram_for_any_other_gateway_port_is_dropped_without_a_reply() {
        let (destination, refusal, answer) = refused_by(decide(
            &config(Boundary::around(SUBNET, 24)),
            &udp_packet("192.168.127.1:8080", b"x"),
        ))
        .expect("there is no control API on this gateway");

        assert_eq!(destination, "192.168.127.1:8080".parse().unwrap());
        assert_eq!(refusal, Refusal::GuestSubnet);
        let unreachable = answer;
        let (header, rest) = Ipv4Header::from_slice(&unreachable).unwrap();
        assert_eq!(header.protocol, IpNumber::ICMP);
        assert_eq!(
            Ipv4Addr::from(header.destination),
            Ipv4Addr::new(192, 168, 127, 2)
        );
        let (icmp, quoted) = etherparse::Icmpv4Header::from_slice(rest).unwrap();
        assert_eq!(
            icmp.icmp_type,
            etherparse::Icmpv4Type::DestinationUnreachable(DestUnreachableHeader::Port)
        );
        let (quoted_ip, quoted_rest) = Ipv4Header::from_slice(quoted).unwrap();
        assert_eq!(
            Ipv4Addr::from(quoted_ip.destination),
            Ipv4Addr::new(192, 168, 127, 1),
            "the quote lets the guest match the refusal to the datagram it sent"
        );
        let (quoted_udp, _) = UdpHeader::from_slice(quoted_rest).unwrap();
        assert_eq!(quoted_udp.destination_port, 8080);
    }

    #[test]
    fn a_syn_to_a_refused_destination_is_answered_with_a_reset() {
        let (_, refusal, answer) = refused_by(decide(
            &config(Boundary::around(SUBNET, 24)),
            &tcp_packet("127.0.0.1:8080", true),
        ))
        .expect("a refused connection fails at once");
        assert_eq!(refusal, Refusal::Loopback);
        let reset = answer;

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

    fn flow(source: &str, destination: &str) -> Flow {
        (source.parse().unwrap(), destination.parse().unwrap())
    }

    fn reset_of(ingress: Ingress) -> Option<(Vec<u8>, &'static str)> {
        match ingress {
            Ingress::OverLimit { answer, counted } => Some((answer, counted)),
            _ => None,
        }
    }

    #[test]
    fn a_syn_the_guest_repeats_is_dropped_so_one_flow_allocates_once() {
        let admissions = admissions(Limits::default());
        let config = config(Boundary::around(SUBNET, 24));
        let syn = tcp_packet("93.184.216.34:443", true);

        assert_eq!(classify(&config, &admissions, &syn), Ingress::Forward);
        for _ in 0..2 {
            assert_eq!(
                classify(&config, &admissions, &syn),
                Ingress::Dropped(DROPPED_TCP_REPEATED),
                "the stack allocates a socket per SYN it sees, so it sees one"
            );
        }
        assert_eq!(admissions.held(), 1);
    }

    #[test]
    fn a_syn_over_the_flow_bound_is_reset_before_the_stack_allocates_for_it() {
        let admissions = admissions(Limits {
            tcp_flows: 1,
            ..Limits::default()
        });
        let config = config(Boundary::around(SUBNET, 24));
        assert_eq!(
            classify(&config, &admissions, &tcp_packet("93.184.216.34:443", true)),
            Ingress::Forward
        );

        let over = classify(&config, &admissions, &tcp_packet("93.184.216.35:443", true));

        let (answer, counted) =
            reset_of(over).expect("the guest is told at once that it may hold no more");
        assert_eq!(counted, DROPPED_TCP_FLOWS);
        let (_, rest) = Ipv4Header::from_slice(&answer).unwrap();
        let (tcp, _) = TcpHeader::from_slice(rest).unwrap();
        assert!(tcp.rst);
        assert_eq!(admissions.held(), 1, "and nothing was allocated for it");

        let counters = Counters::default();
        let sent_back = received_by(
            &config,
            &admissions,
            &counters,
            &tcp_packet("93.184.216.36:443", true),
        );

        let Outcome::Frame(frame) = sent_back else {
            panic!("the reset goes back to the guest over its own link");
        };
        let (_, refused) = etherparse::Ethernet2Header::from_slice(&frame).unwrap();
        let (_, segment) = Ipv4Header::from_slice(refused).unwrap();
        assert!(TcpHeader::from_slice(segment).unwrap().0.rst);
        assert_eq!(counters.seen(DROPPED_TCP_FLOWS), 1);
    }

    #[test]
    fn a_flow_whose_relay_has_ended_gives_its_place_back_to_a_later_syn() {
        let admissions = admissions(Limits {
            tcp_flows: 1,
            ..Limits::default()
        });
        let carried = flow("192.168.127.2:45000", "93.184.216.34:443");
        assert_eq!(admissions.admit(carried), Admission::Fresh);
        let claim = admissions.claim(carried).expect("the stream is the flow's");
        assert_eq!(
            admissions.admit(flow("192.168.127.2:45001", "93.184.216.34:443")),
            Admission::Full,
            "the one flow this guest may hold is held"
        );

        drop(claim);

        assert_eq!(admissions.held(), 0, "the permit goes back with the place");
        assert_eq!(admissions.admit(carried), Admission::Fresh);
    }

    #[test]
    fn a_stream_of_a_flow_that_ended_before_it_arrived_is_not_claimed_twice() {
        let admissions = admissions(Limits::default());
        let carried = flow("192.168.127.2:45000", "93.184.216.34:443");
        admissions.admit(carried);
        let claim = admissions.claim(carried).expect("the stream is the flow's");

        assert!(
            admissions.claim(carried).is_none(),
            "one admitted flow is one stream, and a second claim may not release the first"
        );
        assert_eq!(admissions.held(), 1);
        drop(claim);
        assert_eq!(admissions.held(), 0);
        assert!(admissions.claim(carried).is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn a_flow_the_guest_never_finished_opening_leaves_and_takes_its_stream_with_it() {
        let admissions = admissions(Limits::default());
        let counters = Arc::new(Counters::default());
        let half_open = flow("192.168.127.2:45000", "93.184.216.34:443");
        let carried = flow("192.168.127.2:45001", "93.184.216.34:443");
        admissions.admit(half_open);
        admissions.admit(carried);
        admissions.established(carried);
        let relaying = admissions
            .claim(half_open)
            .expect("the stream is the flow's");
        let cancelled = relaying.cancel.clone();
        let sweep = tokio::spawn(expire_half_open(
            Arc::clone(&admissions),
            Arc::clone(&counters),
        ));

        for _ in 0..16 {
            tokio::time::advance(TCP_HALF_OPEN_SWEEP).await;
            tokio::task::yield_now().await;
        }

        assert!(
            cancelled.is_cancelled(),
            "the relay of a flow that never opened ends, so its stream is dropped"
        );
        assert_eq!(counters.seen(DROPPED_TCP_HALF_OPEN), 1);
        assert_eq!(
            admissions.held(),
            1,
            "the flow whose handshake the guest finished is left alone"
        );
        sweep.abort();
    }

    /// The state a spawned loop reaches on its own, waited for rather than read at once.
    async fn eventually(what: &str, mut reached: impl FnMut() -> bool) {
        let deadline = tokio::time::Instant::now() + PATIENCE;
        while !reached() {
            assert!(tokio::time::Instant::now() < deadline, "{what}");
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_stream_whose_flow_has_already_ended_is_dropped_rather_than_carried() {
        let admissions = admissions(Limits::default());
        let counters = Arc::new(Counters::default());
        let relays = Relays::new();
        let (guest, _test_side) = tokio::io::duplex(64);
        let arriving = flow("192.168.127.2:45000", "93.184.216.34:443");

        let accepting = tokio::spawn(accept_tcp(
            Arc::clone(&admissions),
            resolving(Limits::default(), &counters, &relays),
            gateway_resolver(&config(Boundary::Permissive)),
            futures_util::stream::iter(vec![(guest, arriving.0, arriving.1)]),
        ));
        eventually("the stream was never judged", || {
            counters.seen(DROPPED_TCP_UNADMITTED) == 1
        })
        .await;
        relays.token.cancel();

        accepting.await.expect("the accept loop ends with the run");
        assert_eq!(admissions.held(), 0);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_relay_that_ends_releases_the_flow_it_carried() {
        let host = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = host.local_addr().unwrap();
        let accepted = tokio::spawn(async move { host.accept().await.is_ok() });
        let admissions = admissions(Limits::default());
        let relays = Relays::new();
        let (guest, test_side) = tokio::io::duplex(64);
        let carried = (
            "192.168.127.2:45000".parse::<SocketAddr>().unwrap(),
            address,
        );
        admissions.admit(carried);
        let accepting = tokio::spawn(accept_tcp(
            Arc::clone(&admissions),
            resolving(Limits::default(), &Arc::new(Counters::default()), &relays),
            gateway_resolver(&config(Boundary::Permissive)),
            futures_util::stream::iter(vec![(guest, carried.0, carried.1)]),
        ));

        assert!(
            tokio::time::timeout(PATIENCE, accepted)
                .await
                .expect("the host is connected to")
                .unwrap(),
            "the host saw the flow"
        );
        drop(test_side);
        eventually("the flow never left the table", || admissions.held() == 0).await;

        relays.token.cancel();
        accepting.await.expect("the accept loop ends with the run");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_syn_the_guest_sends_three_times_opens_one_connection_on_the_host() {
        let host = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = host.local_addr().unwrap();
        let accepts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = Arc::clone(&accepts);
        tokio::spawn(async move {
            loop {
                let (stream, _) = host.accept().await.unwrap();
                counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                std::mem::forget(stream);
            }
        });
        let mut guest = FakeGuest::on(Boundary::Permissive);
        guest.lease().await;

        let syn = super::super::link::wrap(
            GUEST_MAC,
            GATEWAY_MAC,
            etherparse::EtherType::IPV4,
            &tcp_packet(&address.to_string(), true),
        );
        for _ in 0..3 {
            guest.device.outbound.send(syn.clone()).await.unwrap();
        }
        for _ in 0..500 {
            guest.settle().await;
        }

        assert_eq!(
            accepts.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "a retransmitted SYN is one flow, not three"
        );
    }

    #[test]
    fn a_later_packet_of_a_refused_connection_is_dropped_without_another_reset() {
        let (_, _, answer) = refused_by(decide(
            &config(Boundary::around(SUBNET, 24)),
            &tcp_packet("127.0.0.1:8080", false),
        ))
        .expect("every packet of a refused flow stays refused");

        let (_, rest) = Ipv4Header::from_slice(&answer).unwrap();
        let (tcp, _) = TcpHeader::from_slice(rest).unwrap();
        assert!(
            tcp.rst,
            "every packet of a refused flow is reset, not only the SYN"
        );
        assert!(
            !tcp.ack,
            "a segment that carries an acknowledgement is reset from its own"
        );
        assert_eq!(
            tcp.sequence_number, 1,
            "the ACK number of the segment that came in"
        );
    }

    #[test]
    fn an_allowed_destination_is_forwarded_to_the_stack() {
        let boundary = Boundary::around(SUBNET, 24);
        let carried = || decide(&config(boundary), &tcp_packet("93.184.216.34:443", true));

        assert_eq!(carried(), Ingress::Forward);
        assert_eq!(refused_by(carried()), None, "nothing was refused");
        assert_eq!(leased_by(carried()), None, "nothing was leased");
        assert_eq!(answered_by(carried()), None, "nothing was answered here");
        assert_eq!(
            decide(&config(boundary), &udp_packet("1.1.1.1:443", b"x")),
            Ingress::Forward
        );
    }

    fn echo_request(destination: [u8; 4], payload: &[u8]) -> Vec<u8> {
        let builder = PacketBuilder::ipv4([192, 168, 127, 2], destination, PACKET_TTL)
            .icmpv4_echo_request(7, 9);
        let mut packet = Vec::with_capacity(builder.size(payload.len()));
        builder.write(&mut packet, payload).unwrap();
        packet
    }

    #[test]
    fn a_ping_at_the_gateway_is_answered_by_the_gateway() {
        let packet = echo_request([192, 168, 127, 1], b"are you there");

        let reply = answered_by(decide(&config(Boundary::around(SUBNET, 24)), &packet))
            .expect("a guest can tell a dead link from a refused destination");

        let (header, rest) = Ipv4Header::from_slice(&reply).unwrap();
        assert_eq!(
            Ipv4Addr::from(header.source),
            Ipv4Addr::new(192, 168, 127, 1)
        );
        assert_eq!(
            Ipv4Addr::from(header.destination),
            Ipv4Addr::new(192, 168, 127, 2)
        );
        let (icmp, payload) = etherparse::Icmpv4Header::from_slice(rest).unwrap();
        assert_eq!(
            icmp.icmp_type,
            etherparse::Icmpv4Type::EchoReply(etherparse::IcmpEchoHeader { id: 7, seq: 9 })
        );
        assert_eq!(payload, b"are you there");
    }

    #[test]
    fn a_ping_at_anything_but_the_gateway_is_dropped_rather_than_carried() {
        let packet = echo_request([93, 184, 216, 34], b"");

        assert_eq!(
            decide(&config(Boundary::around(SUBNET, 24)), &packet),
            Ingress::Dropped(DROPPED_PROTOCOL)
        );
    }

    #[test]
    fn an_icmp_message_that_is_not_an_echo_request_is_dropped() {
        let builder = PacketBuilder::ipv4([192, 168, 127, 2], [192, 168, 127, 1], PACKET_TTL)
            .icmpv4_echo_reply(1, 1);
        let mut packet = Vec::with_capacity(builder.size(0));
        builder.write(&mut packet, &[]).unwrap();

        assert_eq!(
            decide(&config(Boundary::around(SUBNET, 24)), &packet),
            Ingress::Dropped(DROPPED_PROTOCOL)
        );
    }

    #[test]
    fn a_protocol_this_gateway_does_not_carry_is_dropped_and_counted() {
        let mut packet = tcp_packet("93.184.216.34:443", true);
        packet[9] = IpNumber::IPV6_ROUTE_HEADER.0;

        assert_eq!(
            decide(&config(Boundary::around(SUBNET, 24)), &packet),
            Ingress::Dropped(DROPPED_PROTOCOL)
        );
    }

    #[test]
    fn a_fragment_is_dropped_rather_than_reassembled() {
        let mut packet = udp_packet("1.1.1.1:443", b"the first half");
        packet[6] |= 0x20;

        assert_eq!(
            decide(&config(Boundary::around(SUBNET, 24)), &packet),
            Ingress::Dropped(DROPPED_FRAGMENT)
        );

        let mut later = udp_packet("1.1.1.1:443", b"the second half");
        later[6] = 0;
        later[7] = 2;
        assert_eq!(
            decide(&config(Boundary::around(SUBNET, 24)), &later),
            Ingress::Dropped(DROPPED_FRAGMENT)
        );
    }

    #[test]
    fn bytes_that_are_not_a_packet_this_gateway_can_read_decide_nothing() {
        let boundary = Boundary::around(SUBNET, 24);
        let malformed = Ingress::Dropped(DROPPED_MALFORMED);
        assert_eq!(decide(&config(boundary), &[0x45, 0x00]), malformed);

        let mut truncated_udp = udp_packet("1.1.1.1:53", b"x");
        truncated_udp.truncate(22);
        assert_eq!(decide(&config(boundary), &truncated_udp), malformed);

        let mut truncated_tcp = tcp_packet("1.1.1.1:443", true);
        truncated_tcp.truncate(24);
        assert_eq!(decide(&config(boundary), &truncated_tcp), malformed);

        let mut truncated_icmp = echo_request([192, 168, 127, 1], b"");
        truncated_icmp.truncate(21);
        assert_eq!(decide(&config(boundary), &truncated_icmp), malformed);

        let not_dhcp = udp_packet("255.255.255.255:67", b"not a lease request");
        assert_eq!(decide(&config(boundary), &not_dhcp), malformed);

        for cut in 0..Ipv4Header::MIN_LEN + UdpHeader::LEN {
            let mut short = udp_packet("1.1.1.1:53", b"a datagram");
            short.truncate(cut);
            assert_eq!(
                decide(&config(boundary), &short),
                malformed,
                "nothing shorter than its own headers is carried ({cut} bytes)"
            );
        }

        for cut in 0..64 {
            let mut short = tcp_packet("1.1.1.1:443", true);
            short.truncate(cut);
            decide(&config(boundary), &short);

            let mut arp = vec![0u8; cut];
            arp.iter_mut()
                .enumerate()
                .for_each(|(at, byte)| *byte = at as u8);
            Link::new(GATEWAY_MAC, Ipv4Addr::new(192, 168, 127, 1)).receive(&arp);
        }
    }

    /// One packet judged the way a run judges it, from the link inwards.
    fn received(config: &Config, packet: &[u8]) -> Outcome {
        received_by(
            config,
            &admissions(Limits::default()),
            &Counters::default(),
            packet,
        )
    }

    fn received_by(
        config: &Config,
        admissions: &Admissions,
        counters: &Counters,
        packet: &[u8],
    ) -> Outcome {
        ingress(
            config,
            &Arc::new(Mutex::new(Refusals::default())),
            counters,
            admissions,
            &Link::new(GATEWAY_MAC, config.lease.gateway),
            packet,
        )
    }

    #[test]
    fn a_datagram_that_declares_more_bytes_than_it_carries_is_dropped_before_the_stack_reads_it() {
        let mut packet = udp_packet("1.1.1.1:53", b"a query");
        let length = Ipv4Header::MIN_LEN + 4;
        let declared = u16::from_be_bytes([packet[length], packet[length + 1]]) + 100;
        packet[length..length + 2].copy_from_slice(&declared.to_be_bytes());

        assert_eq!(
            received(&config(Boundary::around(SUBNET, 24)), &packet),
            Outcome::Dropped(DROPPED_MALFORMED),
            "the stack ends its UDP stream on a datagram it cannot parse, so it never gets one"
        );
    }

    #[test]
    fn a_packet_that_declares_more_bytes_than_the_frame_carries_is_dropped() {
        let mut packet = udp_packet("1.1.1.1:53", b"a query");
        let declared = u16::try_from(packet.len() + 100).unwrap();
        packet[2..4].copy_from_slice(&declared.to_be_bytes());

        assert_eq!(
            received(&config(Boundary::around(SUBNET, 24)), &packet),
            Outcome::Dropped(DROPPED_MALFORMED)
        );
    }

    #[test]
    fn a_frame_padded_to_the_ethernet_minimum_is_carried_at_its_own_length() {
        let packet = udp_packet("1.1.1.1:53", b"q");
        let mut padded = packet.clone();
        padded.resize(60, 0);

        assert_eq!(
            received(&config(Boundary::around(SUBNET, 24)), &padded),
            Outcome::Packet(packet),
            "the padding of a short frame is not payload"
        );
    }

    #[test]
    fn the_engine_reads_a_segment_exactly_as_the_stack_would() {
        let whole = tcp_packet("93.184.216.34:443", true);
        let (_, segment) = Ipv4Header::from_slice(&whole).unwrap();
        for offset in [0x00u8, 0x40, 0x50, 0x60, 0xf0] {
            for cut in 0..segment.len() + 4 {
                let mut mutated = segment[..segment.len().min(cut)].to_vec();
                if mutated.len() > 12 {
                    mutated[12] = offset;
                }
                assert_eq!(
                    TcpHeader::from_slice(&mutated).is_err(),
                    smoltcp::wire::TcpPacket::new_checked(&mutated[..]).is_err(),
                    "a segment this engine reads is one the stack reads ({offset:#x}, {cut} bytes)"
                );
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_stream_that_hands_nothing_over_is_read_on_rather_than_taken_as_its_end() {
        let counters = Counters::default();
        let token = CancellationToken::new();
        let mut unusable = 0;
        let mut items = futures_util::stream::poll_fn(move |_| {
            unusable += 1;
            std::task::Poll::Ready((unusable > UNUSABLE_BEFORE_WAITING + 2).then_some(unusable))
        });

        let item = next_usable(&mut items, &counters, &token).await;

        assert_eq!(item, Some(UNUSABLE_BEFORE_WAITING + 3));
        assert_eq!(
            counters.seen(DROPPED_UNUSABLE),
            u64::try_from(UNUSABLE_BEFORE_WAITING).unwrap(),
            "what the stack would not hand over is counted, and the reader waits after the first few"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_reader_of_a_stream_that_hands_nothing_over_ends_with_its_run() {
        let counters = Counters::default();
        let token = CancellationToken::new();
        let mut nothing = futures_util::stream::poll_fn(|_| std::task::Poll::Ready(None::<u8>));
        let cancelling = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(UNUSABLE_WAIT * 4).await;
            cancelling.cancel();
        });

        assert_eq!(next_usable(&mut nothing, &counters, &token).await, None);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_malformed_datagram_does_not_end_udp_or_dns_for_the_rest_of_the_run() {
        let mut guest = FakeGuest::on(Boundary::Permissive);
        guest.lease().await;
        let gateway = Ipv4Addr::new(192, 168, 127, 1);

        for mut packet in [
            {
                let mut packet = udp_packet("1.1.1.1:53", b"a query");
                let length = Ipv4Header::MIN_LEN + 4;
                let declared = u16::from_be_bytes([packet[length], packet[length + 1]]) + 100;
                packet[length..length + 2].copy_from_slice(&declared.to_be_bytes());
                packet
            },
            {
                let mut packet = udp_packet("1.1.1.1:53", b"a query");
                let declared = u16::try_from(packet.len() + 100).unwrap();
                packet[2..4].copy_from_slice(&declared.to_be_bytes());
                packet
            },
            {
                let mut packet = tcp_packet("93.184.216.34:443", true);
                packet[Ipv4Header::MIN_LEN + 12] = 0xf0;
                packet
            },
        ] {
            packet.truncate(packet.len());
            let frame = super::super::link::wrap(
                GUEST_MAC,
                GATEWAY_MAC,
                etherparse::EtherType::IPV4,
                &packet,
            );
            guest.device.outbound.send(frame).await.unwrap();
            guest.settle().await;
        }

        let answer = guest.ask("example.test", gateway).await;

        assert_eq!(
            answer.answers[0].data,
            hickory_proto::rr::RData::A(hickory_proto::rr::rdata::A(
                "93.184.216.34".parse().unwrap()
            )),
            "one datagram the stack could not read may not end DNS for the run"
        );
    }

    #[test]
    fn the_gateway_carries_a_connection_to_its_resolver_and_refuses_every_other_port() {
        let boundary = Boundary::around(SUBNET, 24);

        assert_eq!(
            decide(&config(boundary), &tcp_packet("192.168.127.1:53", true)),
            Ingress::Forward,
            "DNS over TCP is the one service this gateway offers"
        );
        let (destination, refusal, _) = refused_by(decide(
            &config(boundary),
            &tcp_packet("192.168.127.1:80", true),
        ))
        .expect("there is no control API on this gateway");
        assert_eq!(destination, "192.168.127.1:80".parse().unwrap());
        assert_eq!(refusal, Refusal::GuestSubnet);
    }

    #[test]
    fn an_answer_the_guest_can_take_goes_back_as_it_came() {
        let query = question_of("example.test", hickory_proto::rr::RecordType::A, 7, None);
        let answer = answer_of(&query, false).unwrap();

        assert_eq!(fit_to_guest(&query, answer.clone()), Some(answer));
    }

    #[test]
    fn an_answer_too_big_for_a_guest_that_advertised_nothing_keeps_its_question_and_sets_tc() {
        let query = question_of("large.test", hickory_proto::rr::RecordType::A, 7, None);
        let whole = answer_of(&query, false).unwrap();
        assert!(
            whole.len() > 1800,
            "the stand-in answers with {} bytes",
            whole.len()
        );

        let fitted = fit_to_guest(&query, whole).expect("the guest gets an answer it can take");

        assert!(fitted.len() <= DNS_UDP_MINIMUM, "{} bytes", fitted.len());
        let fitted = Message::from_vec(&fitted).unwrap();
        assert!(
            fitted.metadata.truncation,
            "so the guest asks again over TCP"
        );
        assert!(fitted.answers.is_empty());
        assert!(fitted.authorities.is_empty());
        assert!(fitted.additionals.is_empty());
        assert_eq!(fitted.queries[0].name().to_string(), "large.test.");
        assert_eq!(fitted.metadata.id, 7);
    }

    #[test]
    fn an_answer_too_big_for_the_link_is_truncated_however_much_the_guest_asked_for() {
        let query = question_of(
            "large.test",
            hickory_proto::rr::RecordType::A,
            7,
            Some(4096),
        );
        let whole = answer_of(&query, false).unwrap();

        let fitted = fit_to_guest(&query, whole).expect("the guest gets an answer it can take");

        assert!(
            fitted.len() <= DNS_UDP_CEILING,
            "no answer over {DNS_UDP_CEILING} bytes fits one frame of this link, and this one is {}",
            fitted.len()
        );
        let fitted = Message::from_vec(&fitted).unwrap();
        assert!(fitted.metadata.truncation);
        assert!(
            fitted.edns.is_some(),
            "the OPT record the guest sent comes back, so its payload size still holds"
        );
    }

    #[test]
    fn an_answer_this_gateway_cannot_read_and_cannot_send_is_dropped() {
        let query = question_of("example.test", hickory_proto::rr::RecordType::A, 7, None);

        assert_eq!(fit_to_guest(&query, vec![0xff; 600]), None);
    }

    #[test]
    fn a_guest_that_asks_with_bytes_that_are_not_a_question_is_answered_by_its_own_limit() {
        assert_eq!(udp_limit_of(b"not a question"), DNS_UDP_MINIMUM);
    }

    async fn asked_over_tcp(asking: &mut tokio::io::DuplexStream, query: &[u8]) -> Vec<u8> {
        let length = u16::try_from(query.len()).unwrap();
        asking.write_all(&length.to_be_bytes()).await.unwrap();
        asking.write_all(query).await.unwrap();
        let mut back = [0u8; 2];
        asking.read_exact(&mut back).await.unwrap();
        let mut answer = vec![0u8; usize::from(u16::from_be_bytes(back))];
        asking.read_exact(&mut answer).await.unwrap();
        answer
    }

    #[tokio::test]
    async fn the_gateway_answers_every_query_of_one_connection_and_closes_with_the_guest() {
        let counters = Arc::new(Counters::default());
        let (guest, mut asking) = tokio::io::duplex(8192);
        let serving = tokio::spawn(serve_dns_over_tcp(
            guest,
            resolving(Limits::default(), &counters, &Relays::new()),
        ));

        for id in [1u16, 2] {
            let query = question_of("example.test", hickory_proto::rr::RecordType::A, id, None);
            let answer = asked_over_tcp(&mut asking, &query).await;
            assert_eq!(Message::from_vec(&answer).unwrap().metadata.id, id);
        }
        drop(asking);

        tokio::time::timeout(PATIENCE, serving)
            .await
            .expect("the connection ends when the guest closes it")
            .unwrap();
    }

    #[tokio::test]
    async fn a_connection_that_asks_what_is_not_a_question_is_closed() {
        let counters = Arc::new(Counters::default());
        let (guest, mut asking) = tokio::io::duplex(64);
        let serving = tokio::spawn(serve_dns_over_tcp(
            guest,
            resolving(Limits::default(), &counters, &Relays::new()),
        ));

        asking.write_all(&[0, 5]).await.unwrap();
        asking.write_all(b"hello").await.unwrap();

        tokio::time::timeout(PATIENCE, serving)
            .await
            .expect("the gateway does not wait on a connection it will not answer")
            .unwrap();
        assert_eq!(counters.seen(DROPPED_MALFORMED), 1);
    }

    #[tokio::test]
    async fn a_connection_over_the_guests_query_limit_is_closed() {
        let counters = Arc::new(Counters::default());
        let (guest, mut asking) = tokio::io::duplex(4096);
        let serving = tokio::spawn(serve_dns_over_tcp(
            guest,
            resolving(
                Limits {
                    dns_in_flight: 0,
                    ..Limits::default()
                },
                &counters,
                &Relays::new(),
            ),
        ));

        let query = question_of("example.test", hickory_proto::rr::RecordType::A, 1, None);
        asking
            .write_all(&u16::try_from(query.len()).unwrap().to_be_bytes())
            .await
            .unwrap();
        asking.write_all(&query).await.unwrap();

        tokio::time::timeout(PATIENCE, serving)
            .await
            .expect("a guest over its limit is not left waiting")
            .unwrap();
        assert_eq!(counters.seen(DROPPED_DNS_IN_FLIGHT), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn a_connection_the_guest_leaves_idle_is_closed() {
        let counters = Arc::new(Counters::default());
        let (guest, _asking) = tokio::io::duplex(64);

        let serving = tokio::spawn(serve_dns_over_tcp(
            guest,
            resolving(Limits::default(), &counters, &Relays::new()),
        ));
        tokio::time::sleep(DNS_TCP_IDLE * 2).await;

        assert!(serving.is_finished(), "an idle connection is not held open");
    }

    #[tokio::test]
    async fn an_answer_no_length_prefix_can_carry_is_dropped_rather_than_sent() {
        let counters = Counters::default();
        let (mut guest, _asking) = tokio::io::duplex(64);

        let written = write_answer(&mut guest, &vec![0u8; 70_000], &counters).await;

        assert_eq!(written, None);
        assert_eq!(counters.seen(DROPPED_DNS_ANSWER), 1);
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
        let counters = Arc::new(Counters::default());

        answer_dns(
            &resolving(Limits::default(), &counters, &Relays::new()),
            b"not a question".to_vec(),
            "192.168.127.2:5353".parse().unwrap(),
            &replies,
        );

        assert!(heard.try_recv().is_err());
        assert_eq!(counters.seen(DROPPED_MALFORMED), 1);
    }

    #[tokio::test]
    async fn a_guest_that_asks_more_questions_than_it_may_has_the_rest_dropped() {
        let (replies, mut heard) = channel(1);
        let counters = Arc::new(Counters::default());
        let asking = resolving(
            Limits {
                dns_in_flight: 0,
                ..Limits::default()
            },
            &counters,
            &Relays::new(),
        );
        let query = {
            let mut message = hickory_proto::op::Message::new(
                1,
                hickory_proto::op::MessageType::Query,
                hickory_proto::op::OpCode::Query,
            );
            message.add_query(hickory_proto::op::Query::query(
                hickory_proto::rr::Name::from_ascii("example.test.").unwrap(),
                hickory_proto::rr::RecordType::A,
            ));
            message.to_vec().unwrap()
        };

        answer_dns(
            &asking,
            query,
            "192.168.127.2:5353".parse().unwrap(),
            &replies,
        );

        assert!(heard.try_recv().is_err());
        assert_eq!(counters.seen(DROPPED_DNS_IN_FLIGHT), 1);
    }

    #[test]
    fn a_guest_gets_no_more_flows_than_its_limits_allow() {
        let allowance = Allowance::of(Limits::default());

        let tcp: Vec<_> = (0..MAX_TCP_FLOWS)
            .filter_map(|_| allowance.tcp_flow())
            .collect();
        let udp: Vec<_> = (0..MAX_UDP_FLOWS)
            .filter_map(|_| allowance.udp_flow())
            .collect();
        let dns: Vec<_> = (0..MAX_DNS_IN_FLIGHT)
            .filter_map(|_| allowance.dns_query())
            .collect();

        assert_eq!(tcp.len(), MAX_TCP_FLOWS);
        assert_eq!(udp.len(), MAX_UDP_FLOWS);
        assert_eq!(dns.len(), MAX_DNS_IN_FLIGHT);
        assert!(
            allowance.tcp_flow().is_none(),
            "the 1025th TCP flow is refused"
        );
        assert!(
            allowance.udp_flow().is_none(),
            "the 513th UDP flow is refused"
        );
        assert!(
            allowance.dns_query().is_none(),
            "the 257th query is refused"
        );

        drop(tcp);
        assert!(
            allowance.tcp_flow().is_some(),
            "a flow that ended gives its place back"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dropping_the_guard_aborts_every_task_of_the_run() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let host = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut opening = [0u8; 5];
            stream.read_exact(&mut opening).await.unwrap();
            let mut after = Vec::new();
            stream.read_to_end(&mut after).await.unwrap();
            after
        });

        let held = Arc::new(());
        let mut guest = FakeGuest::on(Boundary::Permissive);
        guest.lease().await;
        let carried = Arc::clone(&held);
        guest.adopt(async move {
            let _carried = carried;
            std::future::pending::<()>().await;
        });
        let flow = guest.open(address, 7000);
        assert!(
            guest
                .wait_until(flow, PATIENCE, |socket| socket.may_send())
                .await,
            "the guest's flow is carried to the host"
        );
        guest
            .sockets
            .get_mut::<tcp::Socket>(flow)
            .send_slice(b"ping!")
            .expect("the guest sends on its open flow");
        for _ in 0..100 {
            guest.settle().await;
        }
        let relays = guest.relays();

        guest.stop();

        let after = tokio::time::timeout(PATIENCE, host)
            .await
            .expect("the host's side of the relayed flow closes with the run")
            .unwrap();
        assert!(after.is_empty(), "and nothing more is copied to it");
        tokio::time::timeout(PATIENCE, relays.tracker.wait())
            .await
            .expect("the relay that carried the flow has ended");
        let unheld = tokio::time::timeout(PATIENCE, async {
            loop {
                tokio::time::sleep(Duration::from_millis(2)).await;
                if Arc::strong_count(&held) == 1 {
                    break;
                }
            }
        })
        .await;
        assert!(
            unheld.is_ok(),
            "every task the run owned has been aborted, so nothing still holds what it carried"
        );
        assert!(
            guest.device.outbound.send(Vec::new()).await.is_err(),
            "and nothing reads the link"
        );
    }

    #[test]
    fn a_guest_that_scans_the_boundary_stops_growing_the_refusal_log_but_not_the_count() {
        let refused: Refused = Arc::new(Mutex::new(Refusals::default()));
        let counters = Counters::default();
        let scanned = MAX_LOGGED_REFUSALS + 16;

        for port in 0..scanned {
            let destination = SocketAddr::new(
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                u16::try_from(port + 1).unwrap(),
            );
            note_refusal(&refused, &counters, destination, Refusal::Loopback);
        }
        note_refusal(
            &refused,
            &counters,
            SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 1),
            Refusal::Loopback,
        );

        let refusals = refused.lock().expect("refusals poisoned");
        assert_eq!(
            refusals.logged.len(),
            MAX_LOGGED_REFUSALS,
            "a scan may not grow the set of destinations the run remembers"
        );
        assert!(
            refusals.capped,
            "and the run says once that it counts the rest without logging them"
        );
        assert_eq!(
            counters.seen(Refusal::Loopback.reason()),
            u64::try_from(scanned + 1).unwrap(),
            "every refusal is counted, logged or not"
        );
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

        let gateway = Ipv4Addr::new(192, 168, 127, 1);
        let answer = guest.ask("example.test", gateway).await;
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

        assert_eq!(
            guest
                .ask("nowhere.test", gateway)
                .await
                .metadata
                .response_code,
            hickory_proto::op::ResponseCode::ServFail,
            "a name the host cannot resolve is a failure the guest can see"
        );
        let relayed = guest
            .question(gateway, "example.test", hickory_proto::rr::RecordType::MX)
            .await;
        let relayed = hickory_proto::op::Message::from_vec(&relayed).unwrap();
        assert_eq!(
            relayed.queries[0].query_type(),
            hickory_proto::rr::RecordType::MX,
            "every question type is relayed, not only the ones a gateway could answer itself"
        );
        assert_eq!(relayed.metadata.id, 0x1234);

        let whole = guest
            .question(gateway, "big.test", hickory_proto::rr::RecordType::A)
            .await;
        let whole = hickory_proto::op::Message::from_vec(&whole).unwrap();
        assert!(
            !whole.metadata.truncation,
            "an answer too big for UDP is asked again over TCP, so the guest gets all of it"
        );

        let handle = guest.open(address, 40_000);
        assert!(
            guest
                .wait_until(handle, PATIENCE, |socket| socket.may_send())
                .await,
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
            guest
                .wait_until(handle, PATIENCE, |socket| !socket.may_recv())
                .await,
            "the host closing its end closes the guest's"
        );

        guest.sockets.get_mut::<tcp::Socket>(handle).close();
        assert!(
            guest
                .wait_until(handle, PATIENCE, |socket| {
                    socket.state() == tcp::State::Closed
                })
                .await,
            "the guest closing its own end ends the connection"
        );
        assert_eq!(&served.await.unwrap(), b"ping!", "the host heard the guest");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_answer_the_guest_cannot_take_over_udp_comes_back_truncated() {
        let mut guest = FakeGuest::on(Boundary::around(SUBNET, 24));
        guest.lease().await;
        let gateway = Ipv4Addr::new(192, 168, 127, 1);

        let plain = guest
            .advertising(
                gateway,
                "large.test",
                hickory_proto::rr::RecordType::A,
                None,
            )
            .await;
        let advertised = guest
            .advertising(
                gateway,
                "large.test",
                hickory_proto::rr::RecordType::A,
                Some(4096),
            )
            .await;

        assert!(plain.len() <= DNS_UDP_MINIMUM, "{} bytes", plain.len());
        assert!(
            advertised.len() <= DNS_UDP_CEILING,
            "{} bytes",
            advertised.len()
        );
        for answer in [&plain, &advertised] {
            let answer = Message::from_vec(answer).unwrap();
            assert!(
                answer.metadata.truncation,
                "so the guest asks again over TCP"
            );
            assert!(answer.answers.is_empty());
            assert_eq!(answer.queries[0].name().to_string(), "large.test.");
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn the_same_question_over_tcp_to_the_gateway_is_answered_whole() {
        let mut guest = FakeGuest::on(Boundary::around(SUBNET, 24));
        guest.lease().await;
        let resolver = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 127, 1)), dns::PORT);
        let query = question_of("large.test", hickory_proto::rr::RecordType::A, 0x1234, None);
        let mut asked = u16::try_from(query.len()).unwrap().to_be_bytes().to_vec();
        asked.extend_from_slice(&query);

        let back = guest.over_tcp(resolver, 42_000, &asked).await;

        let length = usize::from(u16::from_be_bytes([back[0], back[1]]));
        assert!(length > 1800, "the whole answer is {length} bytes");
        let whole = Message::from_vec(&back[2..2 + length]).expect("a DNS answer");
        assert!(!whole.metadata.truncation);
        assert_eq!(whole.answers.len(), usize::try_from(LARGE_RECORDS).unwrap());
        assert_eq!(whole.metadata.id, 0x1234);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_guest_over_its_flow_limits_reaches_the_host_no_further() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let carried = listener.local_addr().unwrap();
        let accepts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = Arc::clone(&accepts);
        tokio::spawn(async move {
            loop {
                let (stream, _) = listener.accept().await.unwrap();
                counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                std::mem::forget(stream);
            }
        });
        let echo = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let echoed = echo.local_addr().unwrap();
        let datagrams = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = Arc::clone(&datagrams);
        tokio::spawn(async move {
            let mut heard = [0u8; 16];
            loop {
                let (read, from) = echo.recv_from(&mut heard).await.unwrap();
                counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                echo.send_to(&heard[..read], from).await.unwrap();
            }
        });

        let mut guest = FakeGuest::limited(
            Boundary::Permissive,
            Limits {
                tcp_flows: 1,
                udp_flows: 1,
                ..Limits::default()
            },
        );
        guest.lease().await;

        let first = guest.open(carried, 41_000);
        assert!(
            guest
                .wait_until(first, PATIENCE, |socket| socket.may_send())
                .await,
            "the first flow is carried"
        );
        let second = guest.open(carried, 41_001);
        guest
            .wait_until(second, Duration::from_secs(2), |_| false)
            .await;

        assert_eq!(
            accepts.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "a guest over its TCP limit reaches the host no further"
        );

        let flow = guest.bind_udp(41_100);
        assert_eq!(guest.exchange(flow, echoed, b"first").await, b"first");
        let over = guest.bind_udp(41_101);
        guest.send_only(over, echoed, b"second").await;

        assert_eq!(
            datagrams.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "a guest over its UDP limit reaches the host no further"
        );
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
                guest
                    .wait_until(handle, PATIENCE, |socket| {
                        socket.state() == tcp::State::Closed
                    })
                    .await,
                "{destination} must be refused at once, not opened and not left to time out"
            );
        }
    }
}
