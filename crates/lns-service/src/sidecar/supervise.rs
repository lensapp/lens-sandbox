//! Holding a run's sidecars: start each one, keep them until the run says stop, then power every started guest off.

use std::os::fd::RawFd;
use std::time::Duration;

use anyhow::Result;
use tokio::sync::mpsc::UnboundedReceiver;

use super::Sidecar;
use crate::log;

const CA_BUDGET: Duration = Duration::from_secs(60);

/// A started sidecar guest, seen only as something that can be powered off.
pub(crate) trait Guest {
    fn stop(self) -> impl Future<Output = ()> + Send;
}

/// What a run can start its sidecars with, so the supervision loop never names a VMM.
pub(crate) trait Guests {
    type Guest: Guest;

    fn start(
        &self,
        sidecar: &Sidecar,
        service_rx: Vec<UnboundedReceiver<RawFd>>,
        proxy_ca: Option<&str>,
    ) -> impl Future<Output = Result<Self::Guest>> + Send;
}

/// A shutdown is read between starts, never by cancelling one, because dropping a `start` in flight would detach a guest that is already booting.
pub(crate) async fn supervise(
    guests: impl Guests,
    sidecars: Vec<Sidecar>,
    service_rx: Vec<Vec<UnboundedReceiver<RawFd>>>,
    proxy_ca: Option<crate::relay::ProxyCaAsk>,
    mut shutdown: tokio::sync::oneshot::Receiver<()>,
) {
    let ca = run_proxy_ca(&sidecars, proxy_ca).await;
    let mut running = Vec::new();
    let mut asked_to_stop = false;
    for (sidecar, service_rx) in sidecars.iter().zip(service_rx) {
        asked_to_stop = !matches!(
            shutdown.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        );
        if asked_to_stop {
            break;
        }
        match guests.start(sidecar, service_rx, ca.as_deref()).await {
            Ok(started) => running.push(started),
            Err(e) => log::warn!("sidecar {} did not start: {e:#}", sidecar.id),
        }
    }
    if !asked_to_stop {
        let _ = shutdown.await;
    }
    for sidecar in running {
        sidecar.stop().await;
    }
}

/// The CA the run's proxy generated, asked for only once and only when some sidecar routes its egress through it.
async fn run_proxy_ca(
    sidecars: &[Sidecar],
    proxy_ca: Option<crate::relay::ProxyCaAsk>,
) -> Option<String> {
    if !sidecars.iter().any(|s| s.egress_via_proxy) {
        return None;
    }
    match proxy_ca?.get(CA_BUDGET).await {
        Ok(pem) => Some(pem),
        Err(e) => {
            log::warn!(
                "no proxy CA for the run's sidecars, so their egress cannot be inspected, only tunnelled: {e:#}"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    #[derive(Clone, Default)]
    struct Log(Arc<Mutex<Vec<String>>>);

    impl Log {
        fn push(&self, entry: impl Into<String>) {
            self.0.lock().expect("log poisoned").push(entry.into());
        }

        fn entries(&self) -> Vec<String> {
            self.0.lock().expect("log poisoned").clone()
        }
    }

    struct FakeGuest {
        id: String,
        log: Log,
    }

    impl Guest for FakeGuest {
        async fn stop(self) {
            self.log.push(format!("stop {}", self.id));
        }
    }

    struct FakeGuests {
        log: Log,
        refuse: Vec<String>,
        ca_seen: Arc<Mutex<Vec<Option<String>>>>,
    }

    impl FakeGuests {
        fn new(log: Log) -> Self {
            Self {
                log,
                refuse: Vec::new(),
                ca_seen: Arc::default(),
            }
        }

        fn refusing(log: Log, id: &str) -> Self {
            Self {
                refuse: vec![id.to_string()],
                ..Self::new(log)
            }
        }
    }

    impl Guests for FakeGuests {
        type Guest = FakeGuest;

        async fn start(
            &self,
            sidecar: &Sidecar,
            _service_rx: Vec<UnboundedReceiver<RawFd>>,
            proxy_ca: Option<&str>,
        ) -> Result<FakeGuest> {
            self.log.push(format!("start {}", sidecar.id));
            self.ca_seen
                .lock()
                .expect("ca log poisoned")
                .push(proxy_ca.map(str::to_string));
            if self.refuse.contains(&sidecar.id) {
                anyhow::bail!("no rootfs");
            }
            Ok(FakeGuest {
                id: sidecar.id.clone(),
                log: self.log.clone(),
            })
        }
    }

    fn sidecar(id: &str, egress_via_proxy: bool) -> Sidecar {
        Sidecar {
            id: id.to_string(),
            image: "some.example/some-image:1".to_string(),
            argv: Vec::new(),
            env: Vec::new(),
            cpus: 2,
            memory_mib: 1024,
            volumes: Vec::new(),
            expose: Vec::new(),
            egress_via_proxy,
        }
    }

    fn channels(count: usize) -> Vec<Vec<UnboundedReceiver<RawFd>>> {
        (0..count).map(|_| Vec::new()).collect()
    }

    #[tokio::test]
    async fn every_started_sidecar_is_powered_off_when_the_run_stops() {
        let log = Log::default();
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let held = tokio::spawn(supervise(
            FakeGuests::new(log.clone()),
            vec![sidecar("first", false), sidecar("second", false)],
            channels(2),
            None,
            stop_rx,
        ));
        tokio::task::yield_now().await;
        assert!(!held.is_finished(), "sidecars are held until the run stops");
        stop_tx.send(()).expect("supervise is still waiting");
        held.await.expect("supervise ends with the run");
        assert_eq!(
            log.entries(),
            ["start first", "start second", "stop first", "stop second"]
        );
    }

    #[tokio::test]
    async fn a_sidecar_that_does_not_start_leaves_the_others_running() {
        let log = Log::default();
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let held = tokio::spawn(supervise(
            FakeGuests::refusing(log.clone(), "first"),
            vec![sidecar("first", false), sidecar("second", false)],
            channels(2),
            None,
            stop_rx,
        ));
        tokio::task::yield_now().await;
        stop_tx.send(()).expect("supervise is still waiting");
        held.await.expect("a refused sidecar is not a failed run");
        assert_eq!(
            log.entries(),
            ["start first", "start second", "stop second"]
        );
    }

    #[tokio::test]
    async fn a_shutdown_that_arrives_mid_sequence_starts_no_further_sidecar() {
        let log = Log::default();
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        stop_tx.send(()).expect("supervise has not started yet");
        supervise(
            FakeGuests::new(log.clone()),
            vec![sidecar("first", false)],
            channels(1),
            None,
            stop_rx,
        )
        .await;
        assert!(log.entries().is_empty(), "got: {:?}", log.entries());
    }

    async fn started_with(sidecar: Sidecar, answered: crate::relay::ProxyCaState) -> CaRun {
        let (frame_tx, frame_rx) = tokio::sync::mpsc::unbounded_channel();
        let guests = FakeGuests::new(Log::default());
        let ca_seen = guests.ca_seen.clone();
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let held = tokio::spawn(supervise(
            guests,
            vec![sidecar],
            channels(1),
            Some(crate::relay::test_proxy_ca_ask(frame_tx, answered)),
            stop_rx,
        ));
        tokio::task::yield_now().await;
        stop_tx.send(()).expect("supervise is still waiting");
        held.await.expect("supervise ends with the run");
        CaRun {
            asks: frame_rx,
            seen: ca_seen.lock().expect("ca log poisoned").clone(),
        }
    }

    struct CaRun {
        asks: tokio::sync::mpsc::UnboundedReceiver<crate::approval_flow::protocol::HostFrame>,
        seen: Vec<Option<String>>,
    }

    #[tokio::test]
    async fn a_run_whose_sidecars_want_no_proxy_never_asks_for_the_ca() {
        let mut run =
            started_with(sidecar("first", false), crate::relay::ProxyCaState::Pending).await;
        assert!(run.asks.try_recv().is_err(), "no ask reaches the relay");
        assert_eq!(run.seen, [None]);
    }

    #[tokio::test]
    async fn a_proxied_sidecar_starts_with_the_ca_the_run_answered_with() {
        let run = started_with(
            sidecar("first", true),
            crate::relay::ProxyCaState::Available("-----BEGIN CERTIFICATE-----".to_string()),
        )
        .await;
        assert_eq!(run.seen, [Some("-----BEGIN CERTIFICATE-----".to_string())]);
    }

    #[tokio::test]
    async fn a_proxied_sidecar_starts_without_a_ca_when_the_run_has_none() {
        let mut run = started_with(
            sidecar("first", true),
            crate::relay::ProxyCaState::Unavailable,
        )
        .await;
        assert!(run.asks.try_recv().is_ok(), "a proxied sidecar asks once");
        assert_eq!(run.seen, [None]);
    }
}
