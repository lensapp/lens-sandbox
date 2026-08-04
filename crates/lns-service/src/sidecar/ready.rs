//! A sidecar is ready when the port it exposes speaks, not when its guest boots. The nudge is HTTP-shaped, so a service that stays silent until spoken to in its own binary protocol is not recognised yet.

use std::os::fd::{FromRawFd, OwnedFd, RawFd};
use std::time::Duration;

use anyhow::{Result, bail};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::vm::GuestDialer;
use lns_session::{FORWARD_PORT, ForwardHeader, encode_frame};

const DIAL_TIMEOUT: Duration = Duration::from_secs(5);
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const RETRY_DELAY: Duration = Duration::from_secs(1);

/// Dial `port` inside the guest until it answers or `budget` runs out.
pub async fn wait(guest: &dyn GuestDialer, port: u16, budget: Duration) -> Result<()> {
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        if answers(guest, port).await {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            bail!("the sidecar never answered on port {port} within {budget:?}");
        }
        tokio::time::sleep(RETRY_DELAY).await;
    }
}

/// A reply provoked and a byte read back, under its own deadline, because a guest that accepts and then says nothing would hold the wait open past its budget.
async fn answers(guest: &dyn GuestDialer, port: u16) -> bool {
    matches!(
        tokio::time::timeout(PROBE_TIMEOUT, probe(guest, port)).await,
        Ok(Ok(true))
    )
}

async fn probe(guest: &dyn GuestDialer, port: u16) -> Result<bool> {
    let fd = guest.connect(FORWARD_PORT, DIAL_TIMEOUT).await?;
    let mut stream = stream_from_fd(fd)?;
    stream
        .write_all(&encode_frame(&ForwardHeader {
            container_port: port,
        })?)
        .await?;
    stream.write_all(b"\r\n\r\n").await?;
    let mut byte = [0u8; 1];
    Ok(stream.read(&mut byte).await? > 0)
}

fn stream_from_fd(fd: RawFd) -> Result<tokio::net::UnixStream> {
    // SAFETY: the connector transferred ownership of this AF_VSOCK fd; OwnedFd closes it on drop.
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    let std_stream = std::os::unix::net::UnixStream::from(owned);
    std_stream.set_nonblocking(true)?;
    Ok(tokio::net::UnixStream::from_std(std_stream)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context as _;
    use std::os::fd::IntoRawFd;
    use std::sync::Mutex;

    /// Refuses its first `refusals` dials, then hands out one end of a socketpair per dial.
    struct FakeGuest {
        refusals: Mutex<usize>,
        ends: Mutex<Vec<std::os::unix::net::UnixStream>>,
    }

    impl FakeGuest {
        fn with(ends: Vec<std::os::unix::net::UnixStream>) -> Self {
            Self {
                refusals: Mutex::new(0),
                ends: Mutex::new(ends),
            }
        }

        fn refusing(refusals: usize, ends: Vec<std::os::unix::net::UnixStream>) -> Self {
            Self {
                refusals: Mutex::new(refusals),
                ends: Mutex::new(ends),
            }
        }
    }

    impl GuestDialer for FakeGuest {
        fn connect(
            &self,
            _port: u32,
            _timeout: Duration,
        ) -> futures_util::future::BoxFuture<'_, Result<RawFd>> {
            let mut refusals = self.refusals.lock().expect("refusals");
            if *refusals > 0 {
                *refusals -= 1;
                return Box::pin(async { bail!("the guest is not listening yet") });
            }
            let end = self.ends.lock().expect("ends").pop();
            Box::pin(async move {
                let end = end.context("no socketpair left: dialled more than expected")?;
                Ok(end.into_raw_fd())
            })
        }
    }

    /// Long enough that a loaded machine cannot turn "it became ready" into a timeout; the test that pins the deadline passes ZERO instead.
    const GENEROUS: Duration = Duration::from_secs(120);

    fn pair() -> (
        std::os::unix::net::UnixStream,
        std::os::unix::net::UnixStream,
    ) {
        std::os::unix::net::UnixStream::pair().expect("socketpair")
    }

    /// Read the forward header the probe writes, answer, then stay open until the probe hangs up — a service that closed mid-probe would look unreachable, which is not what this fake is for.
    fn answer(end: std::os::unix::net::UnixStream) {
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            let mut end = end;
            let mut header = vec![
                0u8;
                encode_frame(&ForwardHeader {
                    container_port: 2375
                })
                .expect("frame")
                .len()
            ];
            let _ = end.read_exact(&mut header);
            let _ = end.write_all(b"OK");
            let _ = end.read_to_end(&mut Vec::new());
        });
    }

    #[tokio::test]
    async fn a_service_that_answers_is_ready() {
        let (host_end, guest_end) = pair();
        answer(guest_end);
        let guest = FakeGuest::with(vec![host_end]);

        wait(&guest, 2375, GENEROUS)
            .await
            .expect("a port that answers is ready");
    }

    #[tokio::test]
    async fn the_probe_announces_the_port_it_is_checking() {
        // The broker demultiplexes on this header alone, so a probe without it would reach nothing.
        let (host_end, guest_end) = pair();
        let guest = FakeGuest::with(vec![host_end]);
        let probing = tokio::spawn(async move { wait(&guest, 2375, GENEROUS).await });

        guest_end.set_nonblocking(true).expect("nonblocking");
        let mut service = tokio::net::UnixStream::from_std(guest_end).expect("stream");
        let expected = encode_frame(&ForwardHeader {
            container_port: 2375,
        })
        .expect("frame");
        let mut got = vec![0u8; expected.len()];
        service.read_exact(&mut got).await.expect("header");

        assert_eq!(got, expected);
        probing.abort();
    }

    #[tokio::test]
    async fn a_port_that_never_answers_names_itself_and_the_budget() {
        let (host_end, guest_end) = pair();
        drop(guest_end);
        let guest = FakeGuest::with(vec![host_end]);

        let err = wait(&guest, 2375, Duration::ZERO)
            .await
            .expect_err("a dead port is not ready");

        assert!(format!("{err:#}").contains("2375"), "got: {err:#}");
    }

    #[tokio::test]
    async fn a_refused_dial_is_retried_rather_than_reported_as_ready() {
        // The guest refuses until its broker is up; that is the ordinary first second of a boot.
        let (host_end, guest_end) = pair();
        answer(guest_end);
        let guest = FakeGuest::refusing(1, vec![host_end]);

        wait(&guest, 2375, GENEROUS)
            .await
            .expect("the retry after the refusal finds the service");
    }

    #[tokio::test(start_paused = true)]
    async fn a_service_that_accepts_but_never_answers_is_not_ready_yet() {
        // Without its own deadline this probe would hold the wait open long past the budget the caller set.
        let (host_end, guest_end) = pair();
        let guest = FakeGuest::with(vec![host_end]);

        let err = wait(&guest, 2375, Duration::ZERO)
            .await
            .expect_err("silence is not readiness");

        assert!(format!("{err:#}").contains("2375"), "got: {err:#}");
        drop(guest_end);
    }
}
