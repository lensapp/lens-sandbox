use lns_openshell_spike::{Error, Gate, dns_wire};
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::os::fd::AsRawFd;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, UdpSocket};
use tokio::sync::mpsc;

pub fn start(
    namespace: &std::fs::File,
    gate: Arc<Gate>,
    audit: mpsc::Sender<String>,
) -> Result<tokio::task::JoinHandle<Result<(), Error>>, Error> {
    let servers = crate::dns_config::servers(&std::fs::read_to_string("/etc/resolv.conf")?)?;
    let upstream = SocketAddr::from((servers[0], 53));
    let namespace = namespace.try_clone()?;
    let (udp, tcp) = std::thread::spawn(move || bind(namespace, &servers))
        .join()
        .map_err(|_| "DNS setup thread panicked")??;
    let udp = Arc::new(UdpSocket::from_std(udp)?);
    let tcp = TcpListener::from_std(tcp)?;
    Ok(tokio::spawn(serve(udp, tcp, upstream, gate, audit)))
}

fn bind(
    namespace: std::fs::File,
    servers: &[Ipv4Addr],
) -> io::Result<(std::net::UdpSocket, std::net::TcpListener)> {
    // SAFETY: only this short-lived setup thread enters the broker-owned namespace.
    if unsafe { libc::setns(namespace.as_raw_fd(), libc::CLONE_NEWNET) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let udp = std::net::UdpSocket::bind("0.0.0.0:53")?;
    for (index, address) in servers.iter().enumerate() {
        add_alias(udp.as_raw_fd(), index, *address)?;
    }
    let tcp = std::net::TcpListener::bind("0.0.0.0:53")?;
    udp.set_nonblocking(true)?;
    tcp.set_nonblocking(true)?;
    Ok((udp, tcp))
}

fn add_alias(fd: i32, index: usize, address: Ipv4Addr) -> io::Result<()> {
    // SAFETY: zeroed ifreq is valid initial storage for an interface ioctl.
    let mut request: libc::ifreq = unsafe { std::mem::zeroed() };
    for (target, byte) in request
        .ifr_name
        .iter_mut()
        .zip(format!("lo:dns{index}").bytes())
    {
        *target = byte as libc::c_char;
    }
    let address = libc::sockaddr_in {
        sin_family: libc::AF_INET as _,
        sin_port: 0,
        sin_addr: libc::in_addr {
            s_addr: u32::from_ne_bytes(address.octets()),
        },
        sin_zero: [0; 8],
    };
    // SAFETY: sockaddr_in and sockaddr have equal sizes and the union accepts sockaddr storage.
    request.ifr_ifru.ifru_addr =
        unsafe { std::mem::transmute::<libc::sockaddr_in, libc::sockaddr>(address) };
    // SAFETY: the live datagram descriptor and initialized request are valid for SIOCSIFADDR.
    if unsafe { libc::ioctl(fd, libc::SIOCSIFADDR as _, &request) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

async fn serve(
    udp: Arc<UdpSocket>,
    tcp: TcpListener,
    upstream: SocketAddr,
    gate: Arc<Gate>,
    audit: mpsc::Sender<String>,
) -> Result<(), Error> {
    let mut tasks = tokio::task::JoinSet::new();
    let mut packet = [0; 4096];
    loop {
        tokio::select! {
            received = udp.recv_from(&mut packet) => {
                let (length, peer) = received?;
                if tasks.len() >= 32 { continue; }
                let bytes = packet[..length].to_vec();
                let (udp, gate, audit) = (udp.clone(), gate.clone(), audit.clone());
                tasks.spawn(async move {
                    if let Some(reply) = reply(&bytes, upstream, &gate, &audit).await {
                        let _ = udp.send_to(&reply, peer).await;
                    }
                });
            }
            accepted = tcp.accept() => {
                let (mut socket, _) = accepted?;
                if tasks.len() >= 32 { continue; }
                let (gate, audit) = (gate.clone(), audit.clone());
                tasks.spawn(async move {
                    let _ = tokio::time::timeout(Duration::from_secs(5), async {
                        let length = socket.read_u16().await? as usize;
                        if length > 4096 { return Ok::<_, io::Error>(()); }
                        let mut bytes = vec![0;length];
                        socket.read_exact(&mut bytes).await?;
                        if let Some(reply) = reply(&bytes, upstream, &gate, &audit).await {
                            socket.write_u16(reply.len() as u16).await?;
                            socket.write_all(&reply).await?;
                        }
                        Ok(())
                    }).await;
                });
            }
            completed = tasks.join_next(), if !tasks.is_empty() => { completed.ok_or("DNS task set closed")??; }
        }
    }
}

async fn reply(
    packet: &[u8],
    upstream: SocketAddr,
    gate: &Gate,
    audit: &mpsc::Sender<String>,
) -> Option<Vec<u8>> {
    let generation = gate.engine.current_generation();
    let reply = dns_wire::answer(
        packet,
        |host| {
            generation == gate.engine.current_generation()
                && gate.dns_eligible(host).unwrap_or(false)
        },
        |host| async move {
            tokio::time::timeout(
                Duration::from_secs(3),
                openshell_supervisor_network::run::resolve_dns_upstream(
                    upstream,
                    &host,
                    false,
                    Duration::from_secs(2),
                ),
            )
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| error.to_string())
        },
    )
    .await?;
    if let Some(host) = reply.denied {
        let _ = audit.try_send(host);
    }
    Some(reply.bytes)
}
