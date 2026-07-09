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
pub(crate) struct StopFlagTransport {
    pub(crate) stopped: Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(test)]
impl StopFlagTransport {
    pub(crate) fn guard() -> (VmStopGuard, Arc<std::sync::atomic::AtomicBool>) {
        let stopped = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let guard = VmStopGuard::new(Arc::new(Self {
            stopped: stopped.clone(),
        }));
        (guard, stopped)
    }
}

#[cfg(test)]
impl GuestTransport for StopFlagTransport {
    fn connect(&self, _port: u32, _timeout: Duration) -> BoxFuture<'_, Result<RawFd>> {
        Box::pin(async { Ok(42) })
    }
    fn request_stop(&self) {
        self.stopped
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn dropping_the_guard_requests_stop() {
        let (guard, stopped) = StopFlagTransport::guard();
        drop(guard);
        assert!(
            stopped.load(Ordering::SeqCst),
            "the stop guard must signal the VM to stop when dropped"
        );
    }

    #[tokio::test]
    async fn connect_delegates_to_the_transport() {
        let transport = StopFlagTransport {
            stopped: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        let fd = transport
            .connect(1029, Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(fd, 42);
    }
}
