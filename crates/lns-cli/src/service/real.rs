use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result};
use lns_ipc::{Request, Response, StatusInfo, decode_frame, encode_frame};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::time::{Instant, sleep, timeout};

use super::client::{BoxFuture, ServiceClient};

const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

trait Pinger: Send + Sync {
    fn ping(&self) -> BoxFuture<'_, bool>;
}

trait ChildProbe: Send {
    fn try_wait(&mut self) -> Result<Option<String>>;
}

struct RealPinger<'a> {
    socket: &'a Path,
}

impl Pinger for RealPinger<'_> {
    fn ping(&self) -> BoxFuture<'_, bool> {
        Box::pin(try_ping_impl(self.socket))
    }
}

struct RealChildProbe<'a> {
    child: &'a mut Child,
}

impl ChildProbe for RealChildProbe<'_> {
    fn try_wait(&mut self) -> Result<Option<String>> {
        Ok(self
            .child
            .try_wait()
            .context("checking lns-service child status")?
            .map(|s| format!("{s}")))
    }
}

pub struct RealServiceClient {
    socket: PathBuf,
    service_bin: PathBuf,
}

impl RealServiceClient {
    pub fn new(socket: PathBuf, service_bin: PathBuf) -> Self {
        Self {
            socket,
            service_bin,
        }
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }
}

pub(crate) async fn send_request(socket: &Path, request: &Request) -> Option<Response> {
    let mut stream = UnixStream::connect(socket).await.ok()?;
    let frame = encode_frame(request).ok()?;
    stream.write_all(&frame).await.ok()?;
    stream.shutdown().await.ok()?;

    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.ok()?;
    decode_frame(&mut &buf[..]).ok()
}

async fn try_ping_impl(socket: &Path) -> bool {
    matches!(
        timeout(PROBE_TIMEOUT, send_request(socket, &Request::Ping)).await,
        Ok(Some(Response::Pong))
    )
}

async fn wait_for_ready_with(
    pinger: &dyn Pinger,
    child: &mut dyn ChildProbe,
    socket: &Path,
    total_timeout: Duration,
) -> Result<bool> {
    let deadline = Instant::now() + total_timeout;
    loop {
        if pinger.ping().await {
            return Ok(true);
        }
        if let Some(exit_desc) = child.try_wait()? {
            let log_path = socket
                .parent()
                .map(|p| p.join("service.log").display().to_string())
                .unwrap_or_else(|| "service.log".to_string());
            anyhow::bail!(
                "lns-service exited before becoming ready ({exit_desc}). See {log_path} for details."
            );
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        sleep(POLL_INTERVAL).await;
    }
}

async fn poll_until_ready_with(pinger: &dyn Pinger, total_timeout: Duration) -> bool {
    let deadline = Instant::now() + total_timeout;
    loop {
        if pinger.ping().await {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        sleep(POLL_INTERVAL).await;
    }
}

async fn wait_for_stopped_with(pinger: &dyn Pinger, total_timeout: Duration) -> bool {
    let deadline = Instant::now() + total_timeout;
    while Instant::now() < deadline {
        if !pinger.ping().await {
            return true;
        }
        sleep(POLL_INTERVAL).await;
    }
    false
}

async fn send_cancel_impl(socket: &Path, run_id: String) {
    let attempt = async {
        let mut stream = UnixStream::connect(socket).await.ok()?;
        let frame = encode_frame(&Request::CancelRun { run_id }).ok()?;
        stream.write_all(&frame).await.ok()?;
        let _ = lns_ipc::read_frame_bytes_async(&mut stream).await;
        Some(())
    };
    let _ = attempt.await;
}

fn spawn_service_impl(socket: &Path, service_bin: &Path) -> Result<Child> {
    let log_file = socket.parent().and_then(|p| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(p.join("service.log"))
            .ok()
    });

    let mut cmd = Command::new(service_bin);
    cmd.env("LNS_SOCKET_PATH", socket);
    cmd.stdin(Stdio::null());

    if let Some(f) = log_file {
        if let Ok(f2) = f.try_clone() {
            cmd.stdout(f);
            cmd.stderr(f2);
        } else {
            cmd.stdout(f);
        }
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: pre_exec runs in the forked child; setsid() is async-signal-safe.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    cmd.spawn().context("spawn lns-service")
}

impl ServiceClient for RealServiceClient {
    fn ping(&self) -> BoxFuture<'_, bool> {
        Box::pin(try_ping_impl(&self.socket))
    }

    fn status(&self) -> BoxFuture<'_, Option<StatusInfo>> {
        Box::pin(async move {
            match send_request(&self.socket, &Request::Status).await {
                Some(Response::Status(info)) => Some(info),
                _ => None,
            }
        })
    }

    fn shutdown(&self) -> BoxFuture<'_, Option<()>> {
        Box::pin(async move {
            send_request(&self.socket, &Request::Shutdown)
                .await
                .map(|_| ())
        })
    }

    fn start_and_wait_for_ready(&self, total_timeout: Duration) -> BoxFuture<'_, Result<bool>> {
        Box::pin(async move {
            let mut child = spawn_service_impl(&self.socket, &self.service_bin)?;
            let pinger = RealPinger {
                socket: &self.socket,
            };
            let mut probe = RealChildProbe { child: &mut child };
            wait_for_ready_with(&pinger, &mut probe, &self.socket, total_timeout).await
        })
    }

    fn wait_for_ready(&self, total_timeout: Duration) -> BoxFuture<'_, bool> {
        Box::pin(async move {
            let pinger = RealPinger {
                socket: &self.socket,
            };
            poll_until_ready_with(&pinger, total_timeout).await
        })
    }

    fn wait_for_stopped(&self, total_timeout: Duration) -> BoxFuture<'_, bool> {
        Box::pin(async move {
            let pinger = RealPinger {
                socket: &self.socket,
            };
            wait_for_stopped_with(&pinger, total_timeout).await
        })
    }

    fn cancel_run(&self, run_id: String) -> BoxFuture<'_, ()> {
        Box::pin(send_cancel_impl(&self.socket, run_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::time::Duration;

    struct FakePinger {
        returns: Mutex<VecDeque<bool>>,
    }

    impl FakePinger {
        fn scripted(rets: impl IntoIterator<Item = bool>) -> Self {
            Self {
                returns: Mutex::new(rets.into_iter().collect()),
            }
        }
    }

    impl Pinger for FakePinger {
        fn ping(&self) -> BoxFuture<'_, bool> {
            let ret = self
                .returns
                .lock()
                .unwrap()
                .pop_front()
                .expect("FakePinger: no scripted return");
            Box::pin(async move { ret })
        }
    }

    struct FakeChild {
        returns: VecDeque<Result<Option<String>>>,
    }

    impl FakeChild {
        fn scripted(rets: impl IntoIterator<Item = Result<Option<String>>>) -> Self {
            Self {
                returns: rets.into_iter().collect(),
            }
        }
    }

    impl ChildProbe for FakeChild {
        fn try_wait(&mut self) -> Result<Option<String>> {
            self.returns
                .pop_front()
                .expect("FakeChild: no scripted return")
        }
    }

    #[tokio::test(start_paused = true)]
    async fn ready_immediate_ping_success() {
        let pinger = FakePinger::scripted([true]);
        let mut child = FakeChild::scripted([]);
        let socket = PathBuf::from("/tmp/test.sock");

        let result =
            wait_for_ready_with(&pinger, &mut child, &socket, Duration::from_millis(100)).await;

        assert!(result.unwrap());
    }

    #[tokio::test(start_paused = true)]
    async fn ready_timeout_after_failed_pings() {
        let pinger = FakePinger::scripted([false, false, false]);
        let mut child = FakeChild::scripted([Ok(None), Ok(None), Ok(None)]);
        let socket = PathBuf::from("/tmp/test.sock");

        let result =
            wait_for_ready_with(&pinger, &mut child, &socket, Duration::from_millis(100)).await;

        assert!(!result.unwrap());
    }

    #[tokio::test(start_paused = true)]
    async fn ready_bails_on_child_exit() {
        let pinger = FakePinger::scripted([false]);
        let mut child = FakeChild::scripted([Ok(Some("exit status: 1".into()))]);
        let socket = PathBuf::from("/tmp/dir/test.sock");

        let err = wait_for_ready_with(&pinger, &mut child, &socket, Duration::from_millis(500))
            .await
            .expect_err("should bail on child exit");

        let msg = format!("{err:#}");
        assert!(
            msg.contains("exited before becoming ready"),
            "unexpected: {msg}"
        );
        assert!(msg.contains("exit status: 1"), "unexpected: {msg}");
        assert!(msg.contains("service.log"), "unexpected: {msg}");
    }

    #[tokio::test(start_paused = true)]
    async fn ready_succeeds_before_deadline() {
        let pinger = FakePinger::scripted([false, true]);
        let mut child = FakeChild::scripted([Ok(None)]);
        let socket = PathBuf::from("/tmp/test.sock");

        let result =
            wait_for_ready_with(&pinger, &mut child, &socket, Duration::from_millis(200)).await;

        assert!(result.unwrap());
    }

    #[tokio::test(start_paused = true)]
    async fn ready_poll_returns_true_as_soon_as_ping_succeeds() {
        let pinger = FakePinger::scripted([false, false, true]);

        let ready = poll_until_ready_with(&pinger, Duration::from_millis(500)).await;

        assert!(ready);
    }

    #[tokio::test(start_paused = true)]
    async fn ready_poll_times_out_without_spawning_when_ping_never_succeeds() {
        let pinger = FakePinger::scripted([false, false, false]);

        let ready = poll_until_ready_with(&pinger, Duration::from_millis(100)).await;

        assert!(!ready);
    }

    #[tokio::test(start_paused = true)]
    async fn stopped_immediate_when_not_responding() {
        let pinger = FakePinger::scripted([false]);

        let result = wait_for_stopped_with(&pinger, Duration::from_millis(100)).await;

        assert!(result);
    }

    #[tokio::test(start_paused = true)]
    async fn stopped_timeout_when_still_responding() {
        let pinger = FakePinger::scripted([true, true, true]);

        let result = wait_for_stopped_with(&pinger, Duration::from_millis(100)).await;

        assert!(!result);
    }
}
