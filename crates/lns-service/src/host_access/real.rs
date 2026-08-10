use std::os::fd::{FromRawFd, OwnedFd, RawFd};

use tokio::net::UnixStream;
use tokio::sync::mpsc::UnboundedReceiver;

use super::HostSocketSpec;

/// Serves one forward for the life of the run: each fd the guest opens on this port becomes its own connection to the host socket, so two concurrent signs never share a stream.
pub fn serve(spec: HostSocketSpec, mut fds: UnboundedReceiver<RawFd>) {
    tokio::spawn(async move {
        while let Some(fd) = fds.recv().await {
            let source = spec.host_source.clone();
            let id = spec.id.clone();
            tokio::spawn(async move {
                if let Err(e) = bridge_one(fd, &source).await {
                    crate::log::debug!("host-socket forward {id} connection ended: {e}");
                }
            });
        }
    });
}

async fn bridge_one(fd: RawFd, host_source: &str) -> anyhow::Result<()> {
    let mut guest = stream_from_fd(fd)?;
    let mut host = UnixStream::connect(host_source).await?;
    tokio::io::copy_bidirectional(&mut guest, &mut host).await?;
    Ok(())
}

fn stream_from_fd(fd: RawFd) -> anyhow::Result<UnixStream> {
    // SAFETY: the accept loop transfers ownership of the fd; OwnedFd closes it on drop.
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    let std_stream = std::os::unix::net::UnixStream::from(owned);
    std_stream.set_nonblocking(true)?;
    Ok(UnixStream::from_std(std_stream)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::os::fd::IntoRawFd;

    fn spec(host_source: &str) -> HostSocketSpec {
        HostSocketSpec {
            id: "some-access".into(),
            host_source: host_source.into(),
            port: super::super::FIRST_FORWARD_PORT,
        }
    }

    /// A stand-in for the host agent: echoes what it is asked, so the test can prove bytes reach it and answers come back.
    fn spawn_echo(path: std::path::PathBuf) -> std::thread::JoinHandle<()> {
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        std::thread::spawn(move || {
            while let Ok((mut conn, _)) = listener.accept() {
                std::thread::spawn(move || {
                    let mut buf = [0u8; 64];
                    while let Ok(n) = conn.read(&mut buf) {
                        if n == 0 || conn.write_all(&buf[..n]).is_err() {
                            break;
                        }
                    }
                });
            }
        })
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn each_guest_connection_reaches_the_host_socket_and_answers_come_back() {
        let dir = tempfile::TempDir::new().unwrap();
        let agent = dir.path().join("agent.sock");
        let _server = spawn_echo(agent.clone());

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<RawFd>();
        serve(spec(agent.to_str().unwrap()), rx);

        // Two independent guest-side connections, to prove a forward is not one shared stream.
        let mut ends = Vec::new();
        for _ in 0..2 {
            let (guest_end, service_end) = std::os::unix::net::UnixStream::pair().unwrap();
            tx.send(service_end.into_raw_fd()).unwrap();
            ends.push(guest_end);
        }
        for (index, end) in ends.iter_mut().enumerate() {
            let payload = format!("sign-{index}");
            end.write_all(payload.as_bytes()).unwrap();
            let mut buf = vec![0u8; payload.len()];
            end.read_exact(&mut buf).unwrap();
            assert_eq!(String::from_utf8(buf).unwrap(), payload);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_finished_exchange_delivers_the_answer_and_closes_both_ends() {
        let dir = tempfile::TempDir::new().unwrap();
        let agent = dir.path().join("agent.sock");
        let listener = std::os::unix::net::UnixListener::bind(&agent).unwrap();
        std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let mut asked = Vec::new();
            // Reads until the workload's half-close propagates, answers once, then hangs up — a full agent exchange.
            let _ = conn.read_to_end(&mut asked);
            let _ = conn.write_all(b"signed");
        });

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<RawFd>();
        serve(spec(agent.to_str().unwrap()), rx);

        let (mut guest_end, service_end) = std::os::unix::net::UnixStream::pair().unwrap();
        tx.send(service_end.into_raw_fd()).unwrap();
        guest_end.write_all(b"sign").unwrap();
        guest_end
            .shutdown(std::net::Shutdown::Write)
            .expect("half-close, the way a client signals end of request");

        let answer = tokio::task::spawn_blocking(move || {
            let mut answer = Vec::new();
            guest_end.read_to_end(&mut answer).map(|_| answer)
        })
        .await
        .unwrap()
        .expect("the answer must arrive and the stream must then close");
        assert_eq!(
            answer, b"signed",
            "the reply must reach the workload before the proxy releases both ends"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_connection_whose_host_socket_is_gone_closes_without_taking_the_run_down() {
        let dir = tempfile::TempDir::new().unwrap();
        let missing = dir.path().join("never-created.sock");

        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<RawFd>();
        serve(spec(missing.to_str().unwrap()), rx);

        let (mut guest_end, service_end) = std::os::unix::net::UnixStream::pair().unwrap();
        tx.send(service_end.into_raw_fd()).unwrap();
        guest_end.write_all(b"sign").ok();
        let mut buf = [0u8; 8];
        // The guest sees a closed socket, which is what gpg reports as an agent error; the service stays up to serve the next connection.
        let read = tokio::task::spawn_blocking(move || guest_end.read(&mut buf))
            .await
            .unwrap();
        assert!(matches!(read, Ok(0) | Err(_)), "got {read:?}");
        assert!(!tx.is_closed(), "the forward must keep serving");
    }
}
