use std::os::fd::RawFd;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use futures_util::future::BoxFuture;

/// A connected guest VM, abstracted over the host VMM (Vz on macOS, Cloud Hypervisor on Linux).
pub trait GuestTransport: Send + Sync {
    fn connect(&self, port: u32, timeout: Duration) -> BoxFuture<'_, Result<RawFd>>;
    fn request_stop(&self);
}

pub struct VmStopGuard {
    transport: Arc<dyn GuestTransport>,
}

impl VmStopGuard {
    pub fn new(transport: Arc<dyn GuestTransport>) -> Self {
        Self { transport }
    }
}

impl Drop for VmStopGuard {
    fn drop(&mut self) {
        self.transport.request_stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    struct FakeTransport {
        stopped: Arc<AtomicBool>,
    }

    impl GuestTransport for FakeTransport {
        fn connect(&self, _port: u32, _timeout: Duration) -> BoxFuture<'_, Result<RawFd>> {
            Box::pin(async { Ok(42) })
        }
        fn request_stop(&self) {
            self.stopped.store(true, Ordering::SeqCst);
        }
    }

    #[test]
    fn dropping_the_guard_requests_stop() {
        let stopped = Arc::new(AtomicBool::new(false));
        let transport = Arc::new(FakeTransport {
            stopped: stopped.clone(),
        });
        {
            let _guard = VmStopGuard::new(transport);
        }
        assert!(
            stopped.load(Ordering::SeqCst),
            "the stop guard must signal the VM to stop when dropped"
        );
    }

    #[tokio::test]
    async fn connect_delegates_to_the_transport() {
        let transport = FakeTransport {
            stopped: Arc::new(AtomicBool::new(false)),
        };
        let fd = transport
            .connect(1029, Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(fd, 42);
    }
}
