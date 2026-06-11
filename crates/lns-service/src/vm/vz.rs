use std::future::Future;
use std::os::fd::RawFd;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

mod ffi;
pub use ffi::VmStopGuard;
pub use ffi::VsockConnector;
pub use ffi::VzBackend;

pub(crate) trait ConnectOnce {
    fn connect_once(&self, port: u32) -> impl Future<Output = Result<RawFd>> + Send;
}

const CONNECT_INITIAL_DELAY: Duration = Duration::from_millis(10);

// Capped low because every ms spent past guest readiness is added to every single run's boot time, while a refused vsock connect costs near nothing.
const CONNECT_MAX_DELAY: Duration = Duration::from_millis(100);

pub(crate) async fn connect_with<C: ConnectOnce + Sync>(
    connector: &C,
    port: u32,
    timeout: Duration,
) -> Result<RawFd> {
    let deadline = Instant::now() + timeout;
    let mut delay = CONNECT_INITIAL_DELAY;
    loop {
        match connector.connect_once(port).await {
            Ok(fd) => return Ok(fd),
            Err(e) => {
                if Instant::now() >= deadline {
                    return Err(e).with_context(|| {
                        format!("connect_to_guest_port({port}) timed out after {timeout:?}")
                    });
                }
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(CONNECT_MAX_DELAY);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicI32, Ordering};

    struct FakeConnect {
        fails_before_ok: AtomicI32,
        always_fail: bool,
        fd: RawFd,
    }

    impl ConnectOnce for FakeConnect {
        fn connect_once(&self, _port: u32) -> impl Future<Output = Result<RawFd>> + Send {
            let remaining = self.fails_before_ok.fetch_sub(1, Ordering::SeqCst);
            let always_fail = self.always_fail;
            let fd = self.fd;
            async move {
                if always_fail || remaining > 0 {
                    Err(anyhow::anyhow!("scripted connect failure"))
                } else {
                    Ok(fd)
                }
            }
        }
    }

    #[tokio::test]
    async fn connect_with_returns_fd_on_first_success() {
        let fake = FakeConnect {
            fails_before_ok: AtomicI32::new(0),
            always_fail: false,
            fd: 5,
        };
        let fd = connect_with(&fake, 9, Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(fd, 5);
    }

    #[tokio::test]
    async fn connect_with_times_out_after_the_deadline() {
        let fake = FakeConnect {
            fails_before_ok: AtomicI32::new(0),
            always_fail: true,
            fd: 5,
        };
        let err = connect_with(&fake, 9, Duration::ZERO).await.unwrap_err();
        assert!(format!("{err:#}").contains("timed out"), "got {err:#}");
    }

    #[tokio::test(start_paused = true)]
    async fn connect_with_retries_after_a_transient_failure() {
        let fake = FakeConnect {
            fails_before_ok: AtomicI32::new(1),
            always_fail: false,
            fd: 7,
        };
        let fd = connect_with(&fake, 9, Duration::from_secs(60))
            .await
            .unwrap();
        assert_eq!(fd, 7);
    }

    #[tokio::test(start_paused = true)]
    async fn connect_with_polls_tightly_enough_to_catch_readiness_fast() {
        let fake = FakeConnect {
            fails_before_ok: AtomicI32::new(6),
            always_fail: false,
            fd: 3,
        };
        let started = tokio::time::Instant::now();
        let fd = connect_with(&fake, 9, Duration::from_secs(60))
            .await
            .unwrap();
        assert_eq!(fd, 3);
        let waited = started.elapsed();
        assert_eq!(
            waited,
            Duration::from_millis(10 + 20 + 40 + 80 + 100 + 100),
            "backoff schedule regressed — a guest ready after the 6th probe \
             must be reached in 350ms, not the old 1.27s",
        );
    }
}
