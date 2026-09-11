pub mod pattern;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pattern::{MIB, pattern_chunk};

const READ_TIMEOUT: Duration = Duration::from_secs(60);
const ACCEPT_POLL: Duration = Duration::from_millis(20);
const COPY_CHUNK: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Role {
    Sink,
    Source,
    Bidirectional,
    HalfCloseReply,
    HostHalfClose,
    Reset,
    Echo,
    UdpEcho,
    Witness,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Sink => "sink",
            Role::Source => "source",
            Role::Bidirectional => "bidirectional",
            Role::HalfCloseReply => "half-close-reply",
            Role::HostHalfClose => "host-half-close",
            Role::Reset => "reset",
            Role::Echo => "echo",
            Role::UdpEcho => "udp-echo",
            Role::Witness => "witness",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Sizes {
    pub source_bytes: u64,
    pub bidirectional_bytes: u64,
    pub half_close_reply_bytes: u64,
    pub host_half_close_bytes: u64,
    pub reset_after_bytes: u64,
}

impl Default for Sizes {
    fn default() -> Self {
        Self {
            source_bytes: 100 * MIB,
            bidirectional_bytes: 100 * MIB,
            half_close_reply_bytes: MIB,
            host_half_close_bytes: 10 * MIB,
            reset_after_bytes: 5 * MIB,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ConnRecord {
    pub id: u64,
    pub role: String,
    pub port: u16,
    pub peer: String,
    pub bytes_in: u64,
    pub bytes_out: u64,
    pub sha_in: Option<String>,
    pub sha_out: Option<String>,
    pub saw_eof: bool,
    pub reset_sent: bool,
    pub opened_ms: u64,
    pub closed_ms: Option<u64>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct UdpRecord {
    pub peer: String,
    pub len: usize,
    pub echoed: bool,
    pub at_ms: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct FixtureReport {
    pub bind: String,
    pub ports: BTreeMap<String, u16>,
    pub connections: Vec<ConnRecord>,
    pub datagrams: Vec<UdpRecord>,
    pub witness_accepts: u64,
}

impl FixtureReport {
    #[cfg(test)]
    pub fn last_on(&self, port: u16) -> Option<&ConnRecord> {
        self.connections.iter().rev().find(|c| c.port == port)
    }
}

struct Shared {
    report: Mutex<FixtureReport>,
    stop: AtomicBool,
    next_id: AtomicU64,
    start: Instant,
    sizes: Sizes,
}

impl Shared {
    fn elapsed_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }

    fn open(&self, role: Role, port: u16, peer: SocketAddr) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let record = ConnRecord {
            id,
            role: role.as_str().to_string(),
            port,
            peer: peer.to_string(),
            opened_ms: self.elapsed_ms(),
            ..ConnRecord::default()
        };
        self.with_report(|r| r.connections.push(record.clone()));
        id
    }

    fn update(&self, id: u64, f: impl FnOnce(&mut ConnRecord)) {
        self.with_report(|r| {
            if let Some(record) = r.connections.iter_mut().find(|c| c.id == id) {
                f(record);
            }
        });
    }

    fn with_report<T>(&self, f: impl FnOnce(&mut FixtureReport) -> T) -> T {
        let mut guard = match self.report.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        f(&mut guard)
    }

    fn stopping(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }
}

pub struct Fixtures {
    shared: Arc<Shared>,
    ports: BTreeMap<String, u16>,
    bind: Ipv4Addr,
}

impl Fixtures {
    pub fn start(bind: Ipv4Addr, base_port: u16, sizes: Sizes) -> Result<Self> {
        let shared = Arc::new(Shared {
            report: Mutex::new(FixtureReport::default()),
            stop: AtomicBool::new(false),
            next_id: AtomicU64::new(1),
            start: Instant::now(),
            sizes,
        });

        let tcp_roles = [
            Role::Sink,
            Role::Source,
            Role::Bidirectional,
            Role::HalfCloseReply,
            Role::HostHalfClose,
            Role::Reset,
            Role::Echo,
        ];

        let mut ports = BTreeMap::new();
        for (index, role) in tcp_roles.into_iter().enumerate() {
            let port = offset_port(base_port, index)?;
            let listener = TcpListener::bind(SocketAddrV4::new(bind, port))
                .with_context(|| format!("bind {role:?} on {bind}:{port}"))?;
            let port = listener.local_addr()?.port();
            ports.insert(role.as_str().to_string(), port);
            spawn_tcp(listener, role, port, Arc::clone(&shared));
        }

        let udp_port = offset_port(base_port, tcp_roles.len())?;
        let udp = UdpSocket::bind(SocketAddrV4::new(bind, udp_port))
            .with_context(|| format!("bind udp echo on {bind}:{udp_port}"))?;
        let udp_port = udp.local_addr()?.port();
        ports.insert(Role::UdpEcho.as_str().to_string(), udp_port);
        spawn_udp(udp, Arc::clone(&shared));

        let witness_port = offset_port(base_port, tcp_roles.len() + 1)?;
        let witness = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, witness_port))
            .with_context(|| format!("bind witness on 127.0.0.1:{witness_port}"))?;
        let witness_port = witness.local_addr()?.port();
        ports.insert(Role::Witness.as_str().to_string(), witness_port);
        spawn_tcp(witness, Role::Witness, witness_port, Arc::clone(&shared));

        shared.with_report(|r| {
            r.bind = bind.to_string();
            r.ports = ports.clone();
        });

        Ok(Self {
            shared,
            ports,
            bind,
        })
    }

    pub fn bind(&self) -> Ipv4Addr {
        self.bind
    }

    pub fn port(&self, role: Role) -> u16 {
        self.ports[role.as_str()]
    }

    pub fn report(&self) -> FixtureReport {
        self.shared.with_report(|r| r.clone())
    }

    pub fn write_report(&self, path: &std::path::Path) -> Result<()> {
        let json = serde_json::to_string_pretty(&self.report())?;
        std::fs::write(path, json).with_context(|| format!("write {}", path.display()))
    }

    pub fn shutdown(&self) {
        self.shared.stop.store(true, Ordering::SeqCst);
    }
}

impl Drop for Fixtures {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn offset_port(base: u16, index: usize) -> Result<u16> {
    if base == 0 {
        return Ok(0);
    }
    let port = base as usize + index;
    if port > u16::MAX as usize {
        bail!("base port {base} leaves no room for {} ports", index + 1);
    }
    Ok(port as u16)
}

pub fn refuse_unsuitable_bind(addr: Ipv4Addr) -> Result<()> {
    if addr.is_loopback() {
        bail!("--bind needs a LAN address: a guest cannot reach the host's loopback");
    }
    if addr.is_unspecified() {
        bail!("--bind needs one LAN address, not 0.0.0.0");
    }
    if addr.octets()[0] == 192 && addr.octets()[1] == 168 && addr.octets()[2] == 127 {
        bail!("--bind must stay off the guest subnet 192.168.127.0/24");
    }
    Ok(())
}

fn spawn_tcp(listener: TcpListener, role: Role, port: u16, shared: Arc<Shared>) {
    std::thread::spawn(move || {
        if listener.set_nonblocking(true).is_err() {
            return;
        }
        while !shared.stopping() {
            match listener.accept() {
                Ok((stream, peer)) => {
                    let shared = Arc::clone(&shared);
                    std::thread::spawn(move || {
                        let _ = stream.set_nonblocking(false);
                        let id = shared.open(role, port, peer);
                        let outcome = serve(role, stream, &shared, id);
                        shared.update(id, |record| {
                            record.closed_ms = Some(shared.elapsed_ms());
                            if let Err(err) = outcome {
                                record.error = Some(err.to_string());
                            }
                        });
                    });
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(ACCEPT_POLL)
                }
                Err(_) => break,
            }
        }
    });
}

fn serve(role: Role, mut stream: TcpStream, shared: &Shared, id: u64) -> Result<()> {
    stream.set_read_timeout(Some(READ_TIMEOUT))?;
    match role {
        Role::Sink => drain(&mut stream, shared, id),
        Role::Source => {
            send_pattern(&mut stream, shared.sizes.source_bytes, shared, id)?;
            Ok(())
        }
        Role::Bidirectional => bidirectional(stream, shared, id),
        Role::HalfCloseReply => {
            drain(&mut stream, shared, id)?;
            send_pattern(&mut stream, shared.sizes.half_close_reply_bytes, shared, id)
        }
        Role::HostHalfClose => {
            send_pattern(&mut stream, shared.sizes.host_half_close_bytes, shared, id)?;
            stream.shutdown(std::net::Shutdown::Write)?;
            drain(&mut stream, shared, id)
        }
        Role::Reset => {
            send_pattern(&mut stream, shared.sizes.reset_after_bytes, shared, id)?;
            set_linger_zero(&stream)?;
            shared.update(id, |record| record.reset_sent = true);
            drop(stream);
            Ok(())
        }
        Role::Echo => echo(&mut stream, shared, id),
        Role::Witness => {
            shared.with_report(|r| r.witness_accepts += 1);
            let body = b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\nREACHED!";
            stream.write_all(body)?;
            shared.update(id, |record| record.bytes_out = body.len() as u64);
            Ok(())
        }
        Role::UdpEcho => Ok(()),
    }
}

fn set_linger_zero(stream: &TcpStream) -> Result<()> {
    use std::os::fd::AsRawFd;
    let linger = libc::linger {
        l_onoff: 1,
        l_linger: 0,
    };
    // SAFETY: the fd outlives the call and linger is a live, correctly sized SO_LINGER value.
    let outcome = unsafe {
        libc::setsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_LINGER,
            std::ptr::from_ref(&linger).cast(),
            std::mem::size_of::<libc::linger>() as libc::socklen_t,
        )
    };
    if outcome != 0 {
        bail!("SO_LINGER: {}", std::io::Error::last_os_error());
    }
    Ok(())
}

fn drain(stream: &mut TcpStream, shared: &Shared, id: u64) -> Result<()> {
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; COPY_CHUNK];
    let mut total = 0u64;
    loop {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        total += read as u64;
        shared.update(id, |record| record.bytes_in = total);
    }
    let digest = hex::encode(hasher.finalize());
    shared.update(id, |record| {
        record.bytes_in = total;
        record.sha_in = Some(digest);
        record.saw_eof = true;
    });
    Ok(())
}

fn send_pattern(stream: &mut TcpStream, len: u64, shared: &Shared, id: u64) -> Result<()> {
    let mut hasher = Sha256::new();
    let mut sent = 0u64;
    let already = shared.with_report(|r| {
        r.connections
            .iter()
            .find(|c| c.id == id)
            .map(|c| c.bytes_out)
            .unwrap_or(0)
    });
    while sent < len {
        let take = std::cmp::min(COPY_CHUNK as u64, len - sent) as usize;
        let chunk = pattern_chunk(sent, take);
        stream.write_all(&chunk)?;
        hasher.update(&chunk);
        sent += take as u64;
        shared.update(id, |record| record.bytes_out = already + sent);
    }
    stream.flush()?;
    let digest = hex::encode(hasher.finalize());
    shared.update(id, |record| {
        record.bytes_out = already + sent;
        record.sha_out = Some(digest);
    });
    Ok(())
}

fn bidirectional(stream: TcpStream, shared: &Shared, id: u64) -> Result<()> {
    let mut writer = stream.try_clone()?;
    let mut reader = stream;
    let len = shared.sizes.bidirectional_bytes;
    std::thread::scope(|scope| -> Result<()> {
        let write_side = scope.spawn(|| send_pattern(&mut writer, len, shared, id));
        drain(&mut reader, shared, id)?;
        match write_side.join() {
            Ok(result) => result,
            Err(_) => bail!("the fixture's sending side panicked"),
        }
    })
}

fn echo(stream: &mut TcpStream, shared: &Shared, id: u64) -> Result<()> {
    let mut buffer = vec![0u8; COPY_CHUNK];
    let mut in_hasher = Sha256::new();
    let mut total = 0u64;
    loop {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        in_hasher.update(&buffer[..read]);
        stream.write_all(&buffer[..read])?;
        total += read as u64;
        shared.update(id, |record| {
            record.bytes_in = total;
            record.bytes_out = total;
        });
    }
    let digest = hex::encode(in_hasher.finalize());
    shared.update(id, |record| {
        record.saw_eof = true;
        record.sha_in = Some(digest.clone());
        record.sha_out = Some(digest);
    });
    Ok(())
}

fn spawn_udp(socket: UdpSocket, shared: Arc<Shared>) {
    std::thread::spawn(move || {
        if socket
            .set_read_timeout(Some(Duration::from_millis(200)))
            .is_err()
        {
            return;
        }
        let mut buffer = vec![0u8; 65535];
        while !shared.stopping() {
            match socket.recv_from(&mut buffer) {
                Ok((len, peer)) => {
                    let echoed = socket.send_to(&buffer[..len], peer).is_ok();
                    let record = UdpRecord {
                        peer: peer.to_string(),
                        len,
                        echoed,
                        at_ms: shared.elapsed_ms(),
                    };
                    shared.with_report(|r| r.datagrams.push(record));
                }
                Err(ref e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(_) => break,
            }
        }
    });
}

#[cfg(test)]
mod tests;
