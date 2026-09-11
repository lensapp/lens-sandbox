use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use futures_util::future::BoxFuture;

use super::dns::{self, Resolver};

const FORWARD_TIMEOUT: Duration = Duration::from_secs(5);

const REPLY_BYTES: usize = 4096;

/// The host's own resolver. `lookup` goes through `getaddrinfo`, so a VPN's or a scoped resolver's answer is the one the guest gets.
pub struct SystemResolver {
    upstream: Option<SocketAddr>,
    timeout: Duration,
}

impl SystemResolver {
    pub fn from_resolv_conf(contents: &str) -> Self {
        Self {
            upstream: dns::first_nameserver(contents),
            timeout: FORWARD_TIMEOUT,
        }
    }
}

impl Resolver for SystemResolver {
    fn lookup(&self, name: String) -> BoxFuture<'static, std::io::Result<Vec<IpAddr>>> {
        Box::pin(async move {
            let found = tokio::net::lookup_host((name.as_str(), 0)).await?;
            Ok(found.map(|address| address.ip()).collect())
        })
    }

    fn forward(&self, query: Vec<u8>) -> BoxFuture<'static, std::io::Result<Vec<u8>>> {
        let upstream = self.upstream;
        let timeout = self.timeout;
        Box::pin(async move {
            let upstream = upstream
                .ok_or_else(|| std::io::Error::other("no nameserver in /etc/resolv.conf"))?;
            tokio::time::timeout(timeout, ask(upstream, query))
                .await
                .unwrap_or_else(|_| Err(std::io::Error::other("the nameserver did not answer")))
        })
    }
}

async fn ask(upstream: SocketAddr, query: Vec<u8>) -> std::io::Result<Vec<u8>> {
    let bind = if upstream.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    let socket = tokio::net::UdpSocket::bind(bind).await?;
    socket.connect(upstream).await?;
    socket.send(&query).await?;
    let mut reply = vec![0u8; REPLY_BYTES];
    let read = socket.recv(&mut reply).await?;
    reply.truncate(read);
    Ok(reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_upstream_is_the_first_nameserver_of_the_hosts_resolv_conf() {
        let resolver = SystemResolver::from_resolv_conf("nameserver 9.9.9.9\n");
        assert_eq!(resolver.upstream, Some("9.9.9.9:53".parse().unwrap()));
        assert_eq!(resolver.timeout, FORWARD_TIMEOUT);
    }

    #[tokio::test]
    async fn a_literal_address_resolves_to_itself_without_asking_anyone() {
        let found = SystemResolver::from_resolv_conf("")
            .lookup("127.0.0.1".to_string())
            .await
            .expect("a literal needs no nameserver");
        assert_eq!(found, vec![IpAddr::from([127, 0, 0, 1])]);
    }

    #[tokio::test]
    async fn a_name_the_host_cannot_even_read_is_an_error_not_an_empty_answer() {
        let err = SystemResolver::from_resolv_conf("")
            .lookup("a\0b".to_string())
            .await
            .expect_err("a name with a NUL byte never reaches a nameserver");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[tokio::test]
    async fn a_forward_reaches_the_nameserver_and_brings_its_answer_back() {
        let nameserver = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let address = nameserver.local_addr().unwrap();
        tokio::spawn(async move {
            let mut heard = [0u8; 512];
            let (read, from) = nameserver.recv_from(&mut heard).await.unwrap();
            assert_eq!(&heard[..read], b"question");
            nameserver.send_to(b"answer", from).await.unwrap();
        });

        let resolver = SystemResolver::from_resolv_conf(&format!("nameserver {}\n", address.ip()));
        let resolver = SystemResolver {
            upstream: Some(address),
            ..resolver
        };

        assert_eq!(
            resolver.forward(b"question".to_vec()).await.unwrap(),
            b"answer"
        );
    }

    #[tokio::test]
    async fn a_nameserver_that_never_answers_gives_up_rather_than_holding_the_guest() {
        let silent = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let resolver = SystemResolver {
            upstream: Some(silent.local_addr().unwrap()),
            timeout: Duration::from_millis(20),
        };

        let err = resolver
            .forward(b"question".to_vec())
            .await
            .expect_err("a question with no answer must not hang the guest");
        assert!(format!("{err}").contains("did not answer"), "{err}");
    }

    #[tokio::test]
    async fn a_host_with_no_nameserver_at_all_says_so() {
        let err = SystemResolver::from_resolv_conf("")
            .forward(b"question".to_vec())
            .await
            .expect_err("there is nothing to forward to");
        assert!(format!("{err}").contains("no nameserver"), "{err}");
    }
}
