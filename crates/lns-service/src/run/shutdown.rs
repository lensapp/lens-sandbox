use std::time::Duration;

use anyhow::Result;
use tokio::task::JoinHandle;

use crate::forward::ForwardGuard;
use crate::log;
use crate::vm::VmStopGuard;

// Host ports are released synchronously so a back-to-back `lns run -p` can rebind them, then the VM stop wait detaches — the workload's exit must not wait on guest poweroff.
pub(crate) fn detach_after_session(
    forwards: ForwardGuard,
    grace: Duration,
    vm_task: JoinHandle<Result<()>>,
    stop_guard: VmStopGuard,
) -> JoinHandle<()> {
    drop(forwards);
    tokio::spawn(async move {
        if let Err(e) = stop_vm_within_grace(grace, vm_task).await {
            log::warn!("vm teardown after session exit: {e:#}");
        }
        drop(stop_guard);
    })
}

async fn stop_vm_within_grace(grace: Duration, mut vm_task: JoinHandle<Result<()>>) -> Result<()> {
    let sleep = tokio::time::sleep(grace);
    tokio::pin!(sleep);
    tokio::select! {
        _ = &mut sleep => {
            log::debug!("vm did not stop within grace period; proceeding");
        }
        r = &mut vm_task => {
            r??;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::forward::{ForwardError, ForwardSpec, PortForwarder, establish, plan};
    use crate::vm::GuestTransport;
    use lns_ipc::{PortPublish, Protocol};
    use std::net::SocketAddr;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    fn init_tracing_capture() {
        use std::sync::Once;
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            let subscriber = tracing_subscriber::fmt()
                .with_writer(std::io::sink)
                .with_ansi(false)
                .with_max_level(tracing::Level::TRACE)
                .finish();
            tracing::subscriber::set_global_default(subscriber).ok();
        });
    }

    #[derive(Default)]
    struct TimestampForwarder {
        bound: Mutex<Vec<SocketAddr>>,
        unbind_instants: Mutex<Vec<tokio::time::Instant>>,
    }

    impl TimestampForwarder {
        fn unbind_instants(&self) -> Vec<tokio::time::Instant> {
            self.unbind_instants.lock().unwrap().clone()
        }
    }

    impl PortForwarder for TimestampForwarder {
        fn bind(&self, spec: &ForwardSpec) -> Result<(), ForwardError> {
            self.bound.lock().unwrap().push(spec.bind);
            Ok(())
        }

        fn unbind(&self, _bind: SocketAddr) {
            self.unbind_instants
                .lock()
                .unwrap()
                .push(tokio::time::Instant::now());
        }
    }

    struct FakeTransport {
        stopped: Arc<AtomicBool>,
    }

    impl GuestTransport for FakeTransport {
        fn connect(
            &self,
            _port: u32,
            _timeout: Duration,
        ) -> futures_util::future::BoxFuture<'_, Result<std::os::fd::RawFd>> {
            Box::pin(async { Ok(42) })
        }
        fn request_stop(&self) {
            self.stopped.store(true, Ordering::SeqCst);
        }
    }

    fn stop_guard() -> (VmStopGuard, Arc<AtomicBool>) {
        let stopped = Arc::new(AtomicBool::new(false));
        let guard = VmStopGuard::new(Arc::new(FakeTransport {
            stopped: stopped.clone(),
        }));
        (guard, stopped)
    }

    fn pp(host_port: u16) -> PortPublish {
        PortPublish {
            host_ip: "127.0.0.1".parse().unwrap(),
            host_port,
            container_port: host_port,
            protocol: Protocol::Tcp,
        }
    }

    #[tokio::test(start_paused = true)]
    async fn ports_are_released_synchronously_before_the_detached_wait() {
        let fake = Arc::new(TimestampForwarder::default());
        let specs = plan(&[pp(3003)]);
        let guard = establish(fake.clone(), &specs).unwrap();
        let t0 = tokio::time::Instant::now();
        let (stop_guard, _stopped) = stop_guard();

        // VM task that never completes — the caller must still get its ports back immediately.
        let vm_task = tokio::spawn(std::future::pending::<Result<()>>());
        let teardown = detach_after_session(guard, Duration::from_secs(2), vm_task, stop_guard);

        let instants = fake.unbind_instants();
        assert_eq!(instants.len(), 1);
        assert_eq!(
            instants[0], t0,
            "unbind must happen inside the detach call, before any await"
        );
        teardown.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn detached_teardown_stops_the_vm_after_the_grace_period() {
        init_tracing_capture();
        let fake = Arc::new(TimestampForwarder::default());
        let guard = establish(fake.clone(), &plan(&[pp(8080)])).unwrap();
        let t0 = tokio::time::Instant::now();
        let (stop_guard, stopped) = stop_guard();

        let vm_task = tokio::spawn(std::future::pending::<Result<()>>());
        let teardown = detach_after_session(guard, Duration::from_secs(2), vm_task, stop_guard);
        teardown.await.unwrap();

        assert!(
            stopped.load(Ordering::SeqCst),
            "a hung guest must still get a stop request after the grace period"
        );
        assert_eq!(
            tokio::time::Instant::now().duration_since(t0),
            Duration::from_secs(2),
            "the stop request waits out the full grace for a hung guest"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn detached_teardown_completes_early_when_the_vm_exits_within_grace() {
        let fake = Arc::new(TimestampForwarder::default());
        let guard = establish(fake.clone(), &plan(&[pp(8081)])).unwrap();
        let t0 = tokio::time::Instant::now();
        let (stop_guard, stopped) = stop_guard();

        let vm_task = tokio::spawn(async {
            tokio::time::sleep(Duration::from_millis(100)).await;
            Ok(())
        });
        let teardown = detach_after_session(guard, Duration::from_secs(2), vm_task, stop_guard);
        teardown.await.unwrap();

        assert!(stopped.load(Ordering::SeqCst), "guard dropped after wait");
        assert_eq!(
            tokio::time::Instant::now().duration_since(t0),
            Duration::from_millis(100),
            "teardown returns when the VM exits, not after full grace"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn detached_teardown_survives_a_panicked_vm_task_and_still_stops_the_vm() {
        init_tracing_capture();
        let fake = Arc::new(TimestampForwarder::default());
        let guard = establish(fake.clone(), &plan(&[pp(9090)])).unwrap();
        let (stop_guard, stopped) = stop_guard();

        let vm_task = tokio::spawn(async { panic!("vm exploded") });
        tokio::task::yield_now().await;

        let teardown = detach_after_session(guard, Duration::from_secs(2), vm_task, stop_guard);
        teardown.await.unwrap();

        assert_eq!(fake.unbind_instants().len(), 1, "ports released on panic");
        assert!(
            stopped.load(Ordering::SeqCst),
            "the stop guard must fire even when the vm task panicked"
        );
    }
}
