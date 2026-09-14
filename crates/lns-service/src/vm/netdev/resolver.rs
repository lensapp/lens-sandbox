use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use futures_util::future::BoxFuture;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::dns::{Scope, Sources, Upstream, scopes_of_resolv_conf, scopes_of_scutil};

/// A DNS answer over UDP is at most this long; a bigger one arrives truncated and is asked again over TCP.
const UDP_ANSWER_BYTES: usize = 4096;

/// The host's own nameservers, asked over the wire. Nothing is cached here: what the host answers is what the guest gets.
pub struct HostUpstream;

impl Upstream for HostUpstream {
    fn over_udp(
        &self,
        server: SocketAddr,
        query: Vec<u8>,
    ) -> BoxFuture<'static, std::io::Result<Vec<u8>>> {
        Box::pin(over_udp(server, query))
    }

    fn over_tcp(
        &self,
        server: SocketAddr,
        query: Vec<u8>,
    ) -> BoxFuture<'static, std::io::Result<Vec<u8>>> {
        Box::pin(over_tcp(server, query))
    }
}

async fn over_udp(server: SocketAddr, query: Vec<u8>) -> std::io::Result<Vec<u8>> {
    let bind = if server.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let socket = tokio::net::UdpSocket::bind(bind).await?;
    socket.connect(server).await?;
    socket.send(&query).await?;
    let mut answer = vec![0u8; UDP_ANSWER_BYTES];
    let read = socket.recv(&mut answer).await?;
    answer.truncate(read);
    Ok(answer)
}

async fn over_tcp(server: SocketAddr, query: Vec<u8>) -> std::io::Result<Vec<u8>> {
    let mut stream = tokio::net::TcpStream::connect(server).await?;
    let length = u16::try_from(query.len()).map_err(std::io::Error::other)?;
    stream.write_all(&length.to_be_bytes()).await?;
    stream.write_all(&query).await?;
    let mut length = [0u8; 2];
    stream.read_exact(&mut length).await?;
    let mut answer = vec![0u8; usize::from(u16::from_be_bytes(length))];
    stream.read_exact(&mut answer).await?;
    Ok(answer)
}

/// The host's resolver list: `/etc/resolv.conf`, plus the per-domain resolvers `scutil --dns` names on macOS.
pub struct HostSources {
    resolv_conf: PathBuf,
    scutil: Option<PathBuf>,
}

impl HostSources {
    pub fn new(resolv_conf: impl Into<PathBuf>, scutil: Option<&Path>) -> Self {
        Self {
            resolv_conf: resolv_conf.into(),
            scutil: scutil.map(Path::to_path_buf),
        }
    }
}

impl Sources for HostSources {
    fn scopes(&self) -> BoxFuture<'_, Vec<Scope>> {
        Box::pin(async move {
            let resolv_conf = tokio::fs::read_to_string(&self.resolv_conf)
                .await
                .unwrap_or_default();
            let mut scopes = scopes_of_resolv_conf(&resolv_conf);
            scopes.extend(scopes_of_scutil(
                &scutil_output(self.scutil.as_deref()).await,
            ));
            scopes
        })
    }
}

/// `kill_on_drop`: a read the caller gave up on takes its `scutil` child with it rather than leaving one behind per refresh.
async fn scutil_output(scutil: Option<&Path>) -> String {
    let Some(scutil) = scutil else {
        return String::new();
    };
    let read = tokio::process::Command::new(scutil)
        .arg("--dns")
        .kill_on_drop(true)
        .output()
        .await;
    match read {
        Ok(output) => String::from_utf8_lossy(&output.stdout).into_owned(),
        Err(_) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::dns::PORT;
    use super::*;
    use std::net::IpAddr;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    fn server(address: &str, port: u16) -> SocketAddr {
        SocketAddr::new(address.parse::<IpAddr>().unwrap(), port)
    }

    fn resolv_conf_in(dir: &Path) -> PathBuf {
        let path = dir.join("resolv.conf");
        std::fs::write(&path, "nameserver 9.9.9.9\n").unwrap();
        path
    }

    #[tokio::test]
    async fn a_udp_query_brings_the_servers_answer_back() {
        let nameserver = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let address = nameserver.local_addr().unwrap();
        tokio::spawn(async move {
            let mut heard = [0u8; 512];
            let (read, from) = nameserver.recv_from(&mut heard).await.unwrap();
            assert_eq!(&heard[..read], b"question");
            nameserver.send_to(b"answer", from).await.unwrap();
        });

        let answer = HostUpstream
            .over_udp(address, b"question".to_vec())
            .await
            .expect("the nameserver answers");

        assert_eq!(answer, b"answer");
    }

    #[tokio::test]
    async fn an_ipv6_nameserver_is_asked_over_ipv6() {
        let asked = HostUpstream.over_udp(server("::1", 9), b"question".to_vec());

        let answered = tokio::time::timeout(Duration::from_millis(200), asked).await;

        assert!(
            answered.is_err() || answered.is_ok_and(|inner| inner.is_err()),
            "the discard port answers nothing, and a host without IPv6 cannot even bind"
        );
    }

    #[tokio::test]
    async fn a_tcp_query_is_length_prefixed_both_ways() {
        let nameserver = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = nameserver.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = nameserver.accept().await.unwrap();
            let mut length = [0u8; 2];
            stream.read_exact(&mut length).await.unwrap();
            let mut query = vec![0u8; usize::from(u16::from_be_bytes(length))];
            stream.read_exact(&mut query).await.unwrap();
            assert_eq!(query, b"question");
            stream.write_all(&6u16.to_be_bytes()).await.unwrap();
            stream.write_all(b"answer").await.unwrap();
        });

        let answer = HostUpstream
            .over_tcp(address, b"question".to_vec())
            .await
            .expect("the nameserver answers over TCP too");

        assert_eq!(answer, b"answer");
    }

    #[tokio::test]
    async fn a_tcp_server_that_is_not_there_is_an_error_rather_than_a_wait() {
        let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = closed.local_addr().unwrap();
        drop(closed);

        let answered = HostUpstream.over_tcp(address, b"question".to_vec()).await;

        assert!(answered.is_err());
    }

    #[tokio::test]
    async fn the_host_sources_read_resolv_conf() {
        let dir = tempfile::tempdir().unwrap();

        let scopes = HostSources::new(resolv_conf_in(dir.path()), None)
            .scopes()
            .await;

        assert_eq!(scopes.len(), 1);
        assert_eq!(scopes[0].suffix, None);
        assert_eq!(scopes[0].servers, vec![server("9.9.9.9", PORT)]);
    }

    #[tokio::test]
    async fn a_host_with_no_resolv_conf_at_all_names_no_scope() {
        let dir = tempfile::tempdir().unwrap();

        let scopes = HostSources::new(dir.path().join("absent"), None)
            .scopes()
            .await;

        assert_eq!(scopes, Vec::new());
    }

    #[tokio::test]
    async fn the_per_domain_resolvers_of_the_host_are_added_to_the_default_ones() {
        let dir = tempfile::tempdir().unwrap();
        let scutil = dir.path().join("scutil");
        std::fs::write(
            &scutil,
            "#!/bin/sh\nprintf 'resolver #1\\n  domain : corp.internal\\n  nameserver[0] : 10.0.0.53\\n'\n",
        )
        .unwrap();
        std::fs::set_permissions(&scutil, std::fs::Permissions::from_mode(0o755)).unwrap();

        let scopes = HostSources::new(resolv_conf_in(dir.path()), Some(&scutil))
            .scopes()
            .await;

        assert_eq!(scopes.len(), 2);
        assert_eq!(scopes[1].suffix.as_deref(), Some("corp.internal"));
        assert_eq!(scopes[1].servers, vec![server("10.0.0.53", PORT)]);
    }

    #[tokio::test]
    async fn a_host_where_scutil_cannot_run_still_has_its_default_resolvers() {
        let dir = tempfile::tempdir().unwrap();
        let absent = dir.path().join("absent");

        let scopes = HostSources::new(resolv_conf_in(dir.path()), Some(&absent))
            .scopes()
            .await;

        assert_eq!(scopes.len(), 1);
    }
}
