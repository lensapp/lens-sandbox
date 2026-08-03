//! vsock↔vsock: accept a connection one guest opened to the host and splice it into a port inside another guest.

use std::os::fd::{FromRawFd, OwnedFd, RawFd};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc::UnboundedReceiver;

use crate::vm::GuestDialer;
use lns_session::{FORWARD_PORT, ForwardHeader, encode_frame};

type Stream = tokio::net::UnixStream;

const DIAL_TIMEOUT: Duration = Duration::from_secs(30);

/// Bridge every connection arriving on `source_rx` into `target`'s `target_port`; a failed connection is dropped rather than closing the channel for the next.
pub fn spawn(
    label: String,
    mut source_rx: UnboundedReceiver<RawFd>,
    target: Arc<dyn GuestDialer>,
    target_port: u16,
) {
    tokio::spawn(async move {
        while let Some(fd) = source_rx.recv().await {
            let target = target.clone();
            let label = label.clone();
            tokio::spawn(async move {
                if let Err(e) = bridge_one(fd, target.as_ref(), target_port).await {
                    crate::log::debug!("sidecar bridge {label} connection ended: {e}");
                }
            });
        }
    });
}

async fn bridge_one(source_fd: RawFd, target: &dyn GuestDialer, target_port: u16) -> Result<()> {
    let mut source = stream_from_fd(source_fd)?;
    let target_fd = target.connect(FORWARD_PORT, DIAL_TIMEOUT).await?;
    let mut sink = stream_from_fd(target_fd)?;
    sink.write_all(&encode_frame(&ForwardHeader {
        container_port: target_port,
    })?)
    .await?;
    tokio::io::copy_bidirectional(&mut source, &mut sink).await?;
    Ok(())
}

fn stream_from_fd(fd: RawFd) -> Result<Stream> {
    // SAFETY: the listener/connector transferred ownership of this AF_VSOCK fd; OwnedFd closes it on drop.
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    let std_stream = std::os::unix::net::UnixStream::from(owned);
    std_stream.set_nonblocking(true)?;
    Ok(Stream::from_std(std_stream)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context as _;
    use std::os::fd::IntoRawFd;
    use std::sync::Mutex;
    use tokio::io::AsyncReadExt;

    /// Hands out one end of a socketpair per `connect`, so a test can act as the target guest.
    struct FakeGuest {
        ends: Mutex<Vec<std::os::unix::net::UnixStream>>,
        ports: Mutex<Vec<u32>>,
    }

    impl FakeGuest {
        fn with(ends: Vec<std::os::unix::net::UnixStream>) -> Arc<Self> {
            Arc::new(Self {
                ends: Mutex::new(ends),
                ports: Mutex::new(Vec::new()),
            })
        }
    }

    impl GuestDialer for FakeGuest {
        fn connect(
            &self,
            port: u32,
            _timeout: Duration,
        ) -> futures_util::future::BoxFuture<'_, Result<RawFd>> {
            self.ports.lock().expect("ports").push(port);
            let end = self.ends.lock().expect("ends").pop();
            Box::pin(async move {
                let end = end.context("no socketpair left: connect called more than expected")?;
                Ok(end.into_raw_fd())
            })
        }
    }

    fn pair() -> (
        std::os::unix::net::UnixStream,
        std::os::unix::net::UnixStream,
    ) {
        std::os::unix::net::UnixStream::pair().expect("socketpair")
    }

    #[tokio::test]
    async fn a_bridged_connection_announces_the_target_port_before_relaying() {
        // The target broker demultiplexes on this header alone.
        let (target_host_end, target_guest_end) = pair();
        let guest = FakeGuest::with(vec![target_host_end]);
        let (source, _source_peer) = pair();

        let probe = guest.clone();
        let bridged =
            tokio::spawn(
                async move { bridge_one(source.into_raw_fd(), probe.as_ref(), 2375).await },
            );

        target_guest_end.set_nonblocking(true).expect("nonblocking");
        let mut service = Stream::from_std(target_guest_end).expect("stream");
        let expected = encode_frame(&ForwardHeader {
            container_port: 2375,
        })
        .expect("frame");
        let mut got = vec![0u8; expected.len()];
        service.read_exact(&mut got).await.expect("read header");

        assert_eq!(got, expected);
        assert_eq!(
            guest.ports.lock().expect("ports").as_slice(),
            &[FORWARD_PORT],
            "delivery reuses the broker's forward port rather than a new listener"
        );
        bridged.abort();
    }

    #[tokio::test]
    async fn bytes_cross_the_bridge_in_both_directions() {
        let (target_host_end, target_guest_end) = pair();
        let guest = FakeGuest::with(vec![target_host_end]);
        let (source, source_peer) = pair();

        let header_len = encode_frame(&ForwardHeader { container_port: 1 })
            .expect("frame")
            .len();
        let bridged =
            tokio::spawn(async move { bridge_one(source.into_raw_fd(), guest.as_ref(), 1).await });

        source_peer.set_nonblocking(true).expect("nonblocking");
        let mut client = Stream::from_std(source_peer).expect("client");
        target_guest_end.set_nonblocking(true).expect("nonblocking");
        let mut service = Stream::from_std(target_guest_end).expect("service");

        let mut header = vec![0u8; header_len];
        service.read_exact(&mut header).await.expect("header");

        client.write_all(b"ping").await.expect("write up");
        let mut up = [0u8; 4];
        service.read_exact(&mut up).await.expect("read up");
        assert_eq!(&up, b"ping");

        service.write_all(b"pong").await.expect("write down");
        let mut down = [0u8; 4];
        client.read_exact(&mut down).await.expect("read down");
        assert_eq!(&down, b"pong");

        bridged.abort();
    }

    #[tokio::test]
    async fn a_bridge_ends_cleanly_once_both_sides_stop_writing() {
        // A hung-up client is the ordinary end of a relayed connection, not a failure.
        let (target_host_end, target_guest_end) = pair();
        let guest = FakeGuest::with(vec![target_host_end]);
        let (source, source_peer) = pair();

        let header_len = encode_frame(&ForwardHeader { container_port: 7 })
            .expect("frame")
            .len();
        let bridged =
            tokio::spawn(async move { bridge_one(source.into_raw_fd(), guest.as_ref(), 7).await });

        source_peer.set_nonblocking(true).expect("nonblocking");
        let mut client = Stream::from_std(source_peer).expect("client");
        target_guest_end.set_nonblocking(true).expect("nonblocking");
        let mut service = Stream::from_std(target_guest_end).expect("service");
        let mut header = vec![0u8; header_len];
        service.read_exact(&mut header).await.expect("header");

        client.shutdown().await.expect("client hangs up");
        service.shutdown().await.expect("service answers EOF");

        bridged
            .await
            .expect("join")
            .expect("a closed connection is not an error");
    }

    #[tokio::test]
    async fn a_refused_dial_fails_that_connection_only() {
        let guest = FakeGuest::with(Vec::new());
        let (source, _source_peer) = pair();

        let err = bridge_one(source.into_raw_fd(), guest.as_ref(), 2375)
            .await
            .expect_err("no target to dial");

        assert!(
            format!("{err:#}").contains("connect called more than expected"),
            "the dial failure must surface: {err:#}"
        );
    }

    #[tokio::test]
    async fn one_failed_connection_does_not_close_the_bridge_for_the_next() {
        // A sidecar that refuses one dial must not cost the run its channel.
        let (target_host_end, target_guest_end) = pair();
        let guest = FakeGuest::with(vec![target_host_end]);
        let (fd_tx, fd_rx) = tokio::sync::mpsc::unbounded_channel();

        spawn("test/api".into(), fd_rx, guest.clone(), 2375);

        let (dead, dead_peer) = pair();
        drop(dead_peer);
        fd_tx.send(dead.into_raw_fd()).expect("send dead fd");
        let (live, _live_peer) = pair();
        fd_tx.send(live.into_raw_fd()).expect("send live fd");

        target_guest_end.set_nonblocking(true).expect("nonblocking");
        let mut service = Stream::from_std(target_guest_end).expect("service");
        let mut header = vec![
            0u8;
            encode_frame(&ForwardHeader {
                container_port: 2375
            })
            .expect("frame")
            .len()
        ];
        tokio::time::timeout(Duration::from_secs(5), service.read_exact(&mut header))
            .await
            .expect("the second connection still reaches the target")
            .expect("header");
    }
}
