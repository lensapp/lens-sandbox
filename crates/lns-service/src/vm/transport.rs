use std::os::fd::RawFd;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use futures_util::future::BoxFuture;

/// Dialling a port inside a guest; all a relay needs, so it never gets the power to stop the guest it dials into.
pub trait GuestDialer: Send + Sync {
    fn connect(&self, port: u32, timeout: Duration) -> BoxFuture<'_, Result<RawFd>>;
}

/// A connected guest VM, abstracted over the host VMM (Vz on macOS, Cloud Hypervisor on Linux).
pub trait GuestTransport: GuestDialer {
    fn request_stop(&self);
}

/// Stops a guest whose transport arrives after its caller gave up, because dropping the receiver would leave the guest running with nobody left to stop it.
pub fn stop_when_reachable(connector_rx: tokio::sync::oneshot::Receiver<Arc<dyn GuestTransport>>) {
    tokio::spawn(async move {
        if let Ok(connector) = connector_rx.await {
            crate::log::debug!("stopping a guest that became reachable after its boot budget");
            connector.request_stop();
        }
    });
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
    use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

    struct FakeTransport {
        stopped: UnboundedSender<()>,
    }

    impl FakeTransport {
        fn new() -> (Self, UnboundedReceiver<()>) {
            let (stopped, stops) = unbounded_channel();
            (Self { stopped }, stops)
        }
    }

    impl GuestDialer for FakeTransport {
        fn connect(&self, _port: u32, _timeout: Duration) -> BoxFuture<'_, Result<RawFd>> {
            Box::pin(async { Ok(42) })
        }
    }

    impl GuestTransport for FakeTransport {
        fn request_stop(&self) {
            let _ = self.stopped.send(());
        }
    }

    #[test]
    fn dropping_the_guard_requests_stop() {
        let (transport, mut stops) = FakeTransport::new();
        {
            let _guard = VmStopGuard::new(Arc::new(transport));
        }
        stops
            .try_recv()
            .expect("the stop guard must signal the VM to stop when dropped");
    }

    #[tokio::test]
    async fn a_guest_that_becomes_reachable_after_the_caller_gave_up_is_still_stopped() {
        let (transport, mut stops) = FakeTransport::new();
        let (connector_tx, connector_rx) = tokio::sync::oneshot::channel();
        stop_when_reachable(connector_rx);

        connector_tx
            .send(Arc::new(transport) as Arc<dyn GuestTransport>)
            .map_err(|_| "the waiter hung up")
            .expect("send the late transport");

        stops.recv().await.expect("the late guest is powered off");
    }

    #[tokio::test]
    async fn connect_delegates_to_the_transport() {
        let (transport, _stops) = FakeTransport::new();
        let fd = transport
            .connect(1029, Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(fd, 42);
    }
}
