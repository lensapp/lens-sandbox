use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::approval_flow::protocol::{
    Credential, Decision, HostFrame, PolicyMessage, RequestDecision, RequestPending,
};
use lns_policy::integrations::TokenFallback;
use lns_policy::{Policy, PolicyStore, RouteRule};

pub type FrameSink = mpsc::UnboundedSender<HostFrame>;

/// Supplies the registry credentials bundled into every emitted `Policy` frame so a network decision is never read upstream as "drop all credentials".
pub type CredentialsProvider = Box<dyn Fn() -> Vec<Credential> + Send + Sync>;

/// A connectable integration whose routes aren't allowed yet, so a held request to one of its `patterns` offers to connect it before the plain allow/deny.
pub struct OfferableIntegration {
    pub id: String,
    pub display_name: String,
    pub patterns: Vec<String>,
    pub token_fallback: Option<TokenFallback>,
}

/// Connects an integration interactively and reports whether it is now connected; injected so the approval flow can offer a connect without owning the credential machinery. `connect_with_token` arms a pasted token instead of running the interactive sign-in.
pub trait IntegrationConnector: Send + Sync {
    fn connect<'a>(&'a self, id: &'a str) -> futures_util::future::BoxFuture<'a, bool>;
    fn connect_with_token<'a>(
        &'a self,
        id: &'a str,
        value: String,
    ) -> futures_util::future::BoxFuture<'a, bool>;
}

pub trait Notifier: Send + Sync {
    fn present(&self, pending: &PendingPrompt);
    fn dismiss(&self, id: &str);
    fn inform(&self, message: &str);
    fn clear_informs(&self);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingPrompt {
    pub id: String,
    pub host: String,
    pub action: String,
    /// Some(display name) when `host` matches a connectable integration, so the card offers to connect it before the plain allow/deny.
    pub offer: Option<String>,
    /// Some when the offered integration declares a token fallback, so the offer card can also reveal "use a token instead".
    pub token_fallback: Option<TokenFallback>,
}

#[derive(Debug)]
struct PendingEntry {
    host: String,
    deadline: Instant,
    offer_integration_id: Option<String>,
}

pub struct ApprovalSession {
    policy: Mutex<Policy>,
    pending: Mutex<HashMap<String, PendingEntry>>,
    notifier: Arc<dyn Notifier>,
    store: Arc<dyn PolicyStore>,
    sink: FrameSink,
    timeout: Duration,
    credentials_provider: OnceLock<CredentialsProvider>,
    offerable: Vec<OfferableIntegration>,
    connector: OnceLock<Arc<dyn IntegrationConnector>>,
    connecting: Mutex<HashSet<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionOutcome {
    Resolved,
    UnknownId,
}

impl ApprovalSession {
    pub fn new(
        policy: Policy,
        notifier: Arc<dyn Notifier>,
        store: Arc<dyn PolicyStore>,
        sink: FrameSink,
        timeout: Duration,
    ) -> Self {
        Self {
            policy: Mutex::new(policy),
            pending: Mutex::new(HashMap::new()),
            notifier,
            store,
            sink,
            timeout,
            credentials_provider: OnceLock::new(),
            offerable: Vec::new(),
            connector: OnceLock::new(),
            connecting: Mutex::new(HashSet::new()),
        }
    }

    /// Captures the run's connectable integrations so a held request to one of their domains offers to connect before the plain allow/deny.
    pub fn with_offers(mut self, offerable: Vec<OfferableIntegration>) -> Self {
        self.offerable = offerable;
        self
    }

    /// Installs the credentials closure once at boot; idempotent, the first provider wins.
    pub fn set_credentials_provider(&self, provider: CredentialsProvider) {
        let _ = self.credentials_provider.set(provider);
    }

    /// Installs the integration connector once the credential subsystem exists; idempotent, the first wins.
    pub fn set_connector(&self, connector: Arc<dyn IntegrationConnector>) {
        let _ = self.connector.set(connector);
    }

    fn offer_for_host(&self, host: &str) -> Option<&OfferableIntegration> {
        self.offerable
            .iter()
            .find(|i| i.patterns.iter().any(|p| host_matches_pattern(p, host)))
    }

    /// The (id, display name, token fallback) to offer for `host`, or `None` when nothing matches or the integration is already connected this run.
    fn offer_id_and_name_for(&self, host: &str) -> Option<(String, String, Option<TokenFallback>)> {
        let integ = self.offer_for_host(host)?;
        let already_connected = self
            .policy
            .lock()
            .expect("policy mutex poisoned")
            .integrations
            .iter()
            .any(|i| i == &integ.id);
        (!already_connected).then(|| {
            (
                integ.id.clone(),
                integ.display_name.clone(),
                integ.token_fallback.clone(),
            )
        })
    }

    fn is_connecting(&self, id: &str) -> bool {
        self.connecting
            .lock()
            .expect("connecting mutex poisoned")
            .contains(id)
    }

    fn current_credentials(&self) -> Option<Vec<Credential>> {
        self.credentials_provider.get().map(|p| p())
    }

    pub fn current_policy(&self) -> Policy {
        self.policy.lock().expect("policy mutex poisoned").clone()
    }

    pub fn submit_pending(&self, req: RequestPending, now: Instant) {
        let matched = self.offer_id_and_name_for(&req.host);
        let offer_integration_id = matched.as_ref().map(|(id, ..)| id.clone());
        // While a connect for this integration is in flight, hold the request silently and let that connect's batch release it — a fresh offer card would only cover the sign-in card.
        let coalesced = offer_integration_id
            .as_deref()
            .is_some_and(|id| self.is_connecting(id));
        let mut pending = self.pending.lock().expect("pending mutex poisoned");
        if pending.contains_key(&req.id) {
            return;
        }
        pending.insert(
            req.id.clone(),
            PendingEntry {
                host: req.host.clone(),
                deadline: now + self.timeout,
                offer_integration_id,
            },
        );
        drop(pending);
        if coalesced {
            return;
        }
        let (offer, token_fallback) = match matched {
            Some((_, name, fallback)) => (Some(name), fallback),
            None => (None, None),
        };
        self.notifier.present(&PendingPrompt {
            id: req.id,
            host: req.host,
            action: req.action,
            offer,
            token_fallback,
        });
    }

    pub fn record_decision(&self, id: &str, decision: Decision) -> DecisionOutcome {
        let Some(host) = self.remove_pending(id) else {
            return DecisionOutcome::UnknownId;
        };
        self.notifier.dismiss(id);
        self.send_decision_frame(id, decision);
        if let Some(rule) = rule_for_always_decision(&host, decision) {
            self.apply_persistent_rule(rule);
        }
        DecisionOutcome::Resolved
    }

    fn remove_pending(&self, id: &str) -> Option<String> {
        self.pending
            .lock()
            .expect("pending mutex poisoned")
            .remove(id)
            .map(|entry| entry.host)
    }

    /// Accepts a held request's integration offer via the interactive connect (oauth sign-in or a straight credential connect). See [`Self::connect_offer_with`].
    pub async fn connect_offer(&self, id: &str) -> DecisionOutcome {
        self.connect_offer_with(id, None).await
    }

    /// Accepts a held request's integration offer by arming a pasted token instead of the interactive connect. See [`Self::connect_offer_with`].
    pub async fn connect_offer_with_token(&self, id: &str, value: String) -> DecisionOutcome {
        self.connect_offer_with(id, Some(value)).await
    }

    /// Drives one connect for the offered integration and releases **every** held request for it — allow-once on success, deny-once closed on failure or a missing connector. A second card for the same integration coalesces onto the in-flight connect instead of starting another. `token` selects the pasted-token connect over the interactive one.
    async fn connect_offer_with(&self, id: &str, token: Option<String>) -> DecisionOutcome {
        let Some(integration_id) = self.offer_integration_of(id) else {
            return DecisionOutcome::UnknownId;
        };
        if !self.begin_connecting(&integration_id) {
            // Another card already started this connect; its batch will release this request too.
            self.notifier.dismiss(id);
            return DecisionOutcome::Resolved;
        }
        // Hide every offer card for this integration so the sign-in card isn't covered; the requests stay held for the batch release.
        for request_id in self.offer_request_ids(&integration_id) {
            self.notifier.dismiss(&request_id);
        }
        let connected = match self.connector.get() {
            Some(connector) => match token {
                Some(value) => connector.connect_with_token(&integration_id, value).await,
                None => connector.connect(&integration_id).await,
            },
            None => false,
        };
        self.finish_connecting(&integration_id);
        let decision = if connected {
            Decision::AllowOnce
        } else {
            Decision::DenyOnce
        };
        for request_id in self.drain_offer_requests(&integration_id) {
            self.send_decision_frame(&request_id, decision);
        }
        DecisionOutcome::Resolved
    }

    /// The integration a held entry offers, if any; does not remove the entry.
    fn offer_integration_of(&self, id: &str) -> Option<String> {
        self.pending
            .lock()
            .expect("pending mutex poisoned")
            .get(id)?
            .offer_integration_id
            .clone()
    }

    /// Marks `id` as connecting; returns false when a connect was already in flight.
    fn begin_connecting(&self, id: &str) -> bool {
        self.connecting
            .lock()
            .expect("connecting mutex poisoned")
            .insert(id.to_string())
    }

    fn finish_connecting(&self, id: &str) {
        self.connecting
            .lock()
            .expect("connecting mutex poisoned")
            .remove(id);
    }

    fn offer_request_ids(&self, integration_id: &str) -> Vec<String> {
        self.pending
            .lock()
            .expect("pending mutex poisoned")
            .iter()
            .filter(|(_, e)| e.offer_integration_id.as_deref() == Some(integration_id))
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Removes and returns every held request offering `integration_id`.
    fn drain_offer_requests(&self, integration_id: &str) -> Vec<String> {
        let mut pending = self.pending.lock().expect("pending mutex poisoned");
        let ids: Vec<String> = pending
            .iter()
            .filter(|(_, e)| e.offer_integration_id.as_deref() == Some(integration_id))
            .map(|(id, _)| id.clone())
            .collect();
        for id in &ids {
            pending.remove(id);
        }
        ids
    }

    pub fn tick_timeouts(&self, now: Instant) -> usize {
        let connecting = self
            .connecting
            .lock()
            .expect("connecting mutex poisoned")
            .clone();
        let expired: Vec<String> = {
            let pending = self.pending.lock().expect("pending mutex poisoned");
            pending
                .iter()
                .filter(|(_, entry)| entry.deadline <= now)
                // A request offering an integration that's mid sign-in must not be swept; its connect releases it.
                .filter(|(_, entry)| {
                    entry
                        .offer_integration_id
                        .as_deref()
                        .is_none_or(|id| !connecting.contains(id))
                })
                .map(|(id, _)| id.clone())
                .collect()
        };
        expired.iter().filter(|id| self.timeout_one(id)).count()
    }

    fn timeout_one(&self, id: &str) -> bool {
        if self.remove_pending(id).is_none() {
            return false;
        }
        self.notifier.dismiss(id);
        self.send_decision_frame(id, Decision::Timeout);
        true
    }

    pub fn apply_external_policy(&self, new_policy: Policy) {
        *self.policy.lock().expect("policy mutex poisoned") = new_policy.clone();
        let credentials = self.current_credentials();
        let _ = self.sink.send(HostFrame::Policy(PolicyMessage {
            network: Some(new_policy.network),
            credentials,
        }));
    }

    pub fn withdraw_run(&self) {
        let ids: Vec<String> = {
            let mut pending = self.pending.lock().expect("pending mutex poisoned");
            pending.drain().map(|(id, _)| id).collect()
        };
        for id in &ids {
            self.notifier.dismiss(id);
        }
        self.notifier.clear_informs();
    }

    fn send_decision_frame(&self, id: &str, decision: Decision) {
        let _ = self.sink.send(HostFrame::RequestDecision(RequestDecision {
            id: id.to_string(),
            decision,
        }));
    }

    fn apply_persistent_rule(&self, rule: RouteRule) {
        let snapshot = {
            let mut policy = self.policy.lock().expect("policy mutex poisoned");
            policy.add_rule(rule);
            policy.clone()
        };
        let credentials = self.current_credentials();
        let _ = self.sink.send(HostFrame::Policy(PolicyMessage {
            network: Some(snapshot.network.clone()),
            credentials,
        }));
        if let Err(e) = self.store.save(&snapshot) {
            self.notifier.inform(&format!(
                "policy rule applied in-memory but not persisted: {e}"
            ));
        }
    }

    /// Connects an integration live: records the id under `integrations:` and persists only that, while the routes are applied to the in-memory policy and emitted so a held request sees them — boot re-derives the routes from the catalog, so persisting them would leave a residual allow that `disconnect` can't revoke.
    pub fn connect_integration(&self, id: &str, routes: Vec<RouteRule>) {
        let (to_persist, live_network) = {
            let mut policy = self.policy.lock().expect("policy mutex poisoned");
            policy.connect(id);
            let to_persist = policy.clone();
            policy.network.allowed_routes.extend(routes);
            (to_persist, policy.network.clone())
        };
        let credentials = self.current_credentials();
        let _ = self.sink.send(HostFrame::Policy(PolicyMessage {
            network: Some(live_network),
            credentials,
        }));
        if let Err(e) = self.store.save(&to_persist) {
            self.notifier.inform(&format!(
                "integration connected in-memory but not persisted: {e}"
            ));
        }
    }
}

/// Matches a request host against an integration route pattern: exact, or a `*.suffix` wildcard covering the apex and any subdomain.
fn host_matches_pattern(pattern: &str, host: &str) -> bool {
    match pattern.strip_prefix("*.") {
        Some(suffix) => host == suffix || host.ends_with(&format!(".{suffix}")),
        None => pattern == host,
    }
}

fn rule_for_always_decision(host: &str, decision: Decision) -> Option<RouteRule> {
    match decision {
        Decision::AllowAlways => Some(RouteRule::allow_host(host)),
        Decision::DenyAlways => Some(RouteRule::deny_host(host)),
        Decision::AllowOnce | Decision::DenyOnce | Decision::Timeout => None,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use lns_policy::{RouteRule, Verdict};
    use std::io;
    use std::sync::Mutex as StdMutex;

    #[derive(Default)]
    pub(crate) struct RecordingNotifier {
        pub(crate) presented: StdMutex<Vec<PendingPrompt>>,
        pub(crate) dismissed: StdMutex<Vec<String>>,
        pub(crate) informed: StdMutex<Vec<String>>,
        pub(crate) informs_cleared: StdMutex<usize>,
    }

    impl Notifier for RecordingNotifier {
        fn present(&self, p: &PendingPrompt) {
            self.presented.lock().unwrap().push(p.clone());
        }
        fn dismiss(&self, id: &str) {
            self.dismissed.lock().unwrap().push(id.to_string());
        }
        fn inform(&self, m: &str) {
            self.informed.lock().unwrap().push(m.to_string());
        }
        fn clear_informs(&self) {
            *self.informs_cleared.lock().unwrap() += 1;
        }
    }

    #[derive(Default)]
    pub(crate) struct CapturingStore {
        pub(crate) saves: StdMutex<Vec<Policy>>,
        next_err: StdMutex<Option<io::Error>>,
    }

    impl CapturingStore {
        pub(crate) fn fail_next(&self, kind: io::ErrorKind, msg: &'static str) {
            *self.next_err.lock().unwrap() = Some(io::Error::new(kind, msg));
        }
    }

    impl PolicyStore for CapturingStore {
        fn save(&self, policy: &Policy) -> io::Result<()> {
            if let Some(e) = self.next_err.lock().unwrap().take() {
                return Err(e);
            }
            self.saves.lock().unwrap().push(policy.clone());
            Ok(())
        }
    }

    type Fixture = (
        ApprovalSession,
        Arc<RecordingNotifier>,
        Arc<CapturingStore>,
        mpsc::UnboundedReceiver<HostFrame>,
    );

    const TEST_TIMEOUT: Duration = Duration::from_secs(30);

    fn fixture() -> Fixture {
        fixture_with_timeout(TEST_TIMEOUT)
    }

    fn fixture_with_timeout(timeout: Duration) -> Fixture {
        let notifier = Arc::new(RecordingNotifier::default());
        let store = Arc::new(CapturingStore::default());
        let (tx, rx) = mpsc::unbounded_channel();
        let session = ApprovalSession::new(
            Policy::default(),
            notifier.clone(),
            store.clone(),
            tx,
            timeout,
        );
        (session, notifier, store, rx)
    }

    fn pending(id: &str, host: &str) -> RequestPending {
        RequestPending {
            id: id.into(),
            host: host.into(),
            action: format!("CONNECT {host}:443"),
            reason: "policy-ambiguous".into(),
        }
    }

    fn decision_frame(rx: &mut mpsc::UnboundedReceiver<HostFrame>) -> RequestDecision {
        match rx.try_recv().expect("expected a frame") {
            HostFrame::RequestDecision(d) => d,
            other => panic!("expected RequestDecision, got {other:?}"),
        }
    }

    fn policy_frame(rx: &mut mpsc::UnboundedReceiver<HostFrame>) -> PolicyMessage {
        match rx.try_recv().expect("expected a frame") {
            HostFrame::Policy(p) => p,
            other => panic!("expected Policy, got {other:?}"),
        }
    }

    #[test]
    #[should_panic(expected = "expected RequestDecision")]
    fn decision_frame_rejects_a_non_decision_frame() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        tx.send(HostFrame::Policy(PolicyMessage::default()))
            .unwrap();
        let _ = decision_frame(&mut rx);
    }

    #[test]
    #[should_panic(expected = "expected Policy")]
    fn policy_frame_rejects_a_non_policy_frame() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        tx.send(HostFrame::RequestDecision(RequestDecision {
            id: "x".into(),
            decision: Decision::AllowOnce,
        }))
        .unwrap();
        let _ = policy_frame(&mut rx);
    }

    #[test]
    fn submit_pending_presents_notification_with_destination() {
        let (s, n, _store, _rx) = fixture();
        s.submit_pending(pending("r1", "api.linear.app"), Instant::now());
        let p = n.presented.lock().unwrap();
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].id, "r1");
        assert_eq!(p[0].host, "api.linear.app");
        assert_eq!(p[0].action, "CONNECT api.linear.app:443");
    }

    #[test]
    fn duplicate_pending_id_does_not_present_a_second_notification() {
        let (s, n, _store, _rx) = fixture();
        s.submit_pending(pending("r1", "api.linear.app"), Instant::now());
        s.submit_pending(pending("r1", "api.linear.app"), Instant::now());
        assert_eq!(n.presented.lock().unwrap().len(), 1);
    }

    #[test]
    fn allow_once_emits_frame_dismisses_notification_and_leaves_policy_alone() {
        let (s, n, store, mut rx) = fixture();
        s.submit_pending(pending("r1", "api.linear.app"), Instant::now());

        let outcome = s.record_decision("r1", Decision::AllowOnce);

        assert_eq!(outcome, DecisionOutcome::Resolved);
        assert_eq!(n.dismissed.lock().unwrap().as_slice(), &["r1".to_string()]);
        assert_eq!(decision_frame(&mut rx).decision, Decision::AllowOnce);
        assert!(
            rx.try_recv().is_err(),
            "no Policy frame should have been pushed"
        );
        assert!(store.saves.lock().unwrap().is_empty());
        assert_eq!(s.current_policy(), Policy::default());
    }

    #[test]
    fn deny_once_emits_frame_without_mutating_policy() {
        let (s, _n, store, mut rx) = fixture();
        s.submit_pending(pending("r1", "api.linear.app"), Instant::now());

        let before = s.current_policy();
        s.record_decision("r1", Decision::DenyOnce);
        let after = s.current_policy();

        assert_eq!(before, after);
        assert_eq!(decision_frame(&mut rx).decision, Decision::DenyOnce);
        assert!(rx.try_recv().is_err());
        assert!(store.saves.lock().unwrap().is_empty());
    }

    #[test]
    fn allow_always_adds_allow_rule_persists_and_emits_policy_frame() {
        let (s, _n, store, mut rx) = fixture();
        s.submit_pending(pending("r1", "api.linear.app"), Instant::now());

        s.record_decision("r1", Decision::AllowAlways);

        let routes = s.current_policy().network.allowed_routes;
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].match_pattern, "api.linear.app");
        assert_eq!(routes[0].verdict, Verdict::Allow);

        assert_eq!(decision_frame(&mut rx).decision, Decision::AllowAlways);
        let pushed = policy_frame(&mut rx);
        assert_eq!(
            pushed.network.unwrap().allowed_routes[0].match_pattern,
            "api.linear.app"
        );

        let saves = store.saves.lock().unwrap();
        assert_eq!(saves.len(), 1);
        assert_eq!(
            saves[0].network.allowed_routes[0].match_pattern,
            "api.linear.app"
        );
    }

    #[test]
    fn deny_always_adds_deny_rule_persists_and_emits_policy_frame() {
        let (s, _n, store, mut rx) = fixture();
        s.submit_pending(pending("r1", "evil.example"), Instant::now());

        s.record_decision("r1", Decision::DenyAlways);

        let routes = s.current_policy().network.allowed_routes;
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].verdict, Verdict::Deny);

        assert_eq!(decision_frame(&mut rx).decision, Decision::DenyAlways);
        let pushed = policy_frame(&mut rx);
        assert_eq!(
            pushed.network.unwrap().allowed_routes[0].verdict,
            Verdict::Deny
        );

        assert_eq!(store.saves.lock().unwrap().len(), 1);
    }

    #[test]
    fn allow_always_with_failed_persist_keeps_rule_in_memory_and_informs_developer() {
        let (s, n, store, mut rx) = fixture();
        s.submit_pending(pending("r1", "api.linear.app"), Instant::now());
        store.fail_next(io::ErrorKind::PermissionDenied, "disk full");

        s.record_decision("r1", Decision::AllowAlways);

        let routes = s.current_policy().network.allowed_routes;
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].verdict, Verdict::Allow);

        assert_eq!(decision_frame(&mut rx).decision, Decision::AllowAlways);
        assert!(policy_frame(&mut rx).network.is_some());

        assert!(store.saves.lock().unwrap().is_empty());
        let informed = n.informed.lock().unwrap();
        assert_eq!(informed.len(), 1);
        assert!(
            informed[0].contains("could not be persisted") || informed[0].contains("not persisted")
        );
    }

    #[test]
    fn record_decision_for_unknown_id_is_unknownid_and_no_frame() {
        let (s, _n, _store, mut rx) = fixture();
        let outcome = s.record_decision("never-submitted", Decision::AllowOnce);
        assert_eq!(outcome, DecisionOutcome::UnknownId);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn record_decision_twice_returns_unknownid_the_second_time() {
        let (s, _n, _store, mut rx) = fixture();
        s.submit_pending(pending("r1", "api.linear.app"), Instant::now());
        assert_eq!(
            s.record_decision("r1", Decision::AllowOnce),
            DecisionOutcome::Resolved
        );
        assert_eq!(
            s.record_decision("r1", Decision::AllowOnce),
            DecisionOutcome::UnknownId
        );
        assert!(rx.try_recv().is_ok());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn tick_timeouts_before_deadline_sweeps_nothing() {
        let (s, n, _store, mut rx) = fixture_with_timeout(Duration::from_secs(30));
        let t0 = Instant::now();
        s.submit_pending(pending("r1", "api.linear.app"), t0);

        let expired = s.tick_timeouts(t0 + Duration::from_secs(5));

        assert_eq!(expired, 0);
        assert!(rx.try_recv().is_err());
        assert!(n.dismissed.lock().unwrap().is_empty());
    }

    #[test]
    fn tick_timeouts_after_deadline_dismisses_and_emits_timeout_decision() {
        let (s, n, _store, mut rx) = fixture_with_timeout(Duration::from_secs(10));
        let t0 = Instant::now();
        s.submit_pending(pending("r1", "api.linear.app"), t0);

        let expired = s.tick_timeouts(t0 + Duration::from_secs(11));

        assert_eq!(expired, 1);
        assert_eq!(n.dismissed.lock().unwrap().as_slice(), &["r1".to_string()]);
        assert_eq!(decision_frame(&mut rx).decision, Decision::Timeout);
    }

    #[test]
    fn tick_timeouts_leaves_unexpired_entries_alone() {
        let (s, _n, _store, mut rx) = fixture_with_timeout(Duration::from_secs(10));
        let t0 = Instant::now();
        s.submit_pending(pending("r1", "a"), t0);
        s.submit_pending(pending("r2", "b"), t0 + Duration::from_secs(20));

        let expired = s.tick_timeouts(t0 + Duration::from_secs(11));

        assert_eq!(expired, 1);
        let frame = decision_frame(&mut rx);
        assert_eq!(frame.id, "r1");
        assert_eq!(frame.decision, Decision::Timeout);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn timeout_one_emits_timeout_and_dismisses_when_pending_is_present() {
        let (s, n, _store, mut rx) = fixture();
        s.submit_pending(pending("r1", "api.linear.app"), Instant::now());

        let acted = s.timeout_one("r1");

        assert!(acted);
        assert_eq!(decision_frame(&mut rx).decision, Decision::Timeout);
        assert_eq!(n.dismissed.lock().unwrap().as_slice(), &["r1".to_string()]);
    }

    #[test]
    fn timeout_one_is_noop_when_a_decision_already_resolved_the_id() {
        let (s, n, _store, mut rx) = fixture();
        s.submit_pending(pending("r1", "api.linear.app"), Instant::now());
        s.record_decision("r1", Decision::AllowOnce);

        let acted = s.timeout_one("r1");

        assert!(!acted, "sweep must not act on an already-resolved id");
        let f = decision_frame(&mut rx);
        assert_eq!(f.decision, Decision::AllowOnce);
        assert!(
            rx.try_recv().is_err(),
            "no second decision frame for the same id"
        );
        assert_eq!(n.dismissed.lock().unwrap().len(), 1);
    }

    #[test]
    fn tick_timeouts_return_count_reflects_only_actually_swept_entries() {
        let (s, _n, _store, mut rx) = fixture_with_timeout(Duration::from_secs(1));
        let t0 = Instant::now();
        s.submit_pending(pending("r1", "a"), t0);
        s.submit_pending(pending("r2", "b"), t0);

        s.record_decision("r2", Decision::AllowOnce);
        let _ = rx.try_recv();

        let swept = s.tick_timeouts(t0 + Duration::from_secs(2));

        assert_eq!(swept, 1, "only r1 should have been swept");
        let f = decision_frame(&mut rx);
        assert_eq!(f.id, "r1");
        assert_eq!(f.decision, Decision::Timeout);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn record_decision_after_timeout_returns_unknownid() {
        let (s, _n, _store, mut rx) = fixture_with_timeout(Duration::from_secs(1));
        let t0 = Instant::now();
        s.submit_pending(pending("r1", "a"), t0);
        s.tick_timeouts(t0 + Duration::from_secs(2));
        let _ = rx.try_recv();

        let outcome = s.record_decision("r1", Decision::AllowOnce);

        assert_eq!(outcome, DecisionOutcome::UnknownId);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn withdraw_run_dismisses_all_pending_without_emitting_frames() {
        let (s, n, _store, mut rx) = fixture();
        s.submit_pending(pending("r1", "a"), Instant::now());
        s.submit_pending(pending("r2", "b"), Instant::now());

        s.withdraw_run();

        let dismissed = n.dismissed.lock().unwrap();
        assert_eq!(dismissed.len(), 2);
        assert!(dismissed.contains(&"r1".to_string()));
        assert!(dismissed.contains(&"r2".to_string()));
        assert!(
            rx.try_recv().is_err(),
            "withdraw should not emit decision frames"
        );
    }

    #[test]
    fn record_decision_after_withdraw_returns_unknownid() {
        let (s, _n, _store, _rx) = fixture();
        s.submit_pending(pending("r1", "a"), Instant::now());
        s.withdraw_run();
        assert_eq!(
            s.record_decision("r1", Decision::AllowOnce),
            DecisionOutcome::UnknownId
        );
    }

    #[test]
    fn withdraw_run_clears_notifier_informs_so_window_does_not_stay_pinned() {
        let (s, n, _store, _rx) = fixture();
        s.submit_pending(pending("r1", "api.linear.app"), Instant::now());
        n.informed
            .lock()
            .unwrap()
            .push("policy rule could not be persisted: disk full".into());

        s.withdraw_run();

        assert_eq!(
            *n.informs_cleared.lock().unwrap(),
            1,
            "withdraw_run must call clear_informs exactly once"
        );
    }

    #[test]
    fn apply_external_policy_replaces_in_memory_and_emits_policy_frame() {
        let (s, _n, _store, mut rx) = fixture();
        let mut updated = Policy::default();
        updated.add_rule(RouteRule::allow_host("api.linear.app"));

        s.apply_external_policy(updated.clone());

        assert_eq!(s.current_policy(), updated);
        let pushed = policy_frame(&mut rx);
        assert_eq!(
            pushed.network.unwrap().allowed_routes[0].match_pattern,
            "api.linear.app"
        );
    }

    #[test]
    fn apply_external_policy_emits_no_decision_frames() {
        let (s, _n, _store, mut rx) = fixture();
        s.apply_external_policy(Policy::default());
        let frame = rx.try_recv().expect("expected a policy frame");
        assert!(matches!(frame, HostFrame::Policy(_)));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn allow_always_policy_frame_bundles_registry_credentials_when_provider_set() {
        let (s, _n, _store, mut rx) = fixture();
        s.set_credentials_provider(Box::new(|| {
            vec![Credential {
                id: "github".into(),
                env_var: Some("GITHUB_TOKEN".into()),
                placeholder: Some("ghp_LNSPLACEHOLDER0000000000000000000000".into()),
                injections: Vec::new(),
            }]
        }));

        s.submit_pending(pending("r1", "api.linear.app"), Instant::now());
        s.record_decision("r1", Decision::AllowAlways);

        let _ = decision_frame(&mut rx);
        let pushed = policy_frame(&mut rx);
        let creds = pushed
            .credentials
            .expect("Policy frame must carry credentials when provider is set");
        assert_eq!(creds.len(), 1);
        assert_eq!(creds[0].id, "github");
    }

    #[test]
    fn apply_external_policy_bundles_registry_credentials_when_provider_set() {
        let (s, _n, _store, mut rx) = fixture();
        s.set_credentials_provider(Box::new(|| {
            vec![Credential {
                id: "openai".into(),
                env_var: Some("OPENAI_API_KEY".into()),
                placeholder: Some("sk-LNSPLACEHOLDER0000000000000000000000000000000000".into()),
                injections: Vec::new(),
            }]
        }));

        let mut updated = Policy::default();
        updated.add_rule(RouteRule::allow_host("api.linear.app"));
        s.apply_external_policy(updated);

        let pushed = policy_frame(&mut rx);
        let creds = pushed
            .credentials
            .expect("hot-swap Policy must carry credentials when provider is set");
        assert_eq!(creds.len(), 1);
        assert_eq!(creds[0].id, "openai");
    }

    #[test]
    fn policy_frames_omit_credentials_when_no_provider_installed() {
        let (s, _n, _store, mut rx) = fixture();
        s.apply_external_policy(Policy::default());
        let pushed = policy_frame(&mut rx);
        assert!(pushed.credentials.is_none(), "creds must be absent");
    }

    fn single_credential_provider(id: &'static str) -> CredentialsProvider {
        Box::new(move || {
            vec![Credential {
                id: id.into(),
                env_var: None,
                placeholder: None,
                injections: Vec::new(),
            }]
        })
    }

    #[test]
    fn set_credentials_provider_is_idempotent() {
        let (s, _n, _store, mut rx) = fixture();
        s.set_credentials_provider(single_credential_provider("first"));
        s.set_credentials_provider(single_credential_provider("second"));
        s.apply_external_policy(Policy::default());
        let pushed = policy_frame(&mut rx);
        let creds = pushed.credentials.expect("credentials present");
        assert_eq!(creds.len(), 1);
        assert_eq!(
            creds[0].id, "first",
            "OnceLock keeps the first provider; second set call is a no-op"
        );
    }

    #[test]
    fn rule_for_always_decision_maps_allow_and_deny_only() {
        assert!(rule_for_always_decision("h", Decision::AllowOnce).is_none());
        assert!(rule_for_always_decision("h", Decision::DenyOnce).is_none());
        assert!(rule_for_always_decision("h", Decision::Timeout).is_none());

        let allow = rule_for_always_decision("h", Decision::AllowAlways).unwrap();
        assert_eq!(allow.verdict, Verdict::Allow);
        assert_eq!(allow.match_pattern, "h");

        let deny = rule_for_always_decision("h", Decision::DenyAlways).unwrap();
        assert_eq!(deny.verdict, Verdict::Deny);
    }

    #[test]
    fn connect_integration_persists_only_the_id_but_emits_the_route_live() {
        let (s, _n, store, mut rx) = fixture();
        s.connect_integration("gitlab", vec![RouteRule::allow_host("gitlab.com")]);
        let saves = store.saves.lock().unwrap();
        assert_eq!(saves.len(), 1, "the connection is persisted once");
        assert_eq!(saves[0].integrations, ["gitlab"]);
        assert!(
            !saves[0]
                .network
                .allowed_routes
                .iter()
                .any(|r| r.match_pattern == "gitlab.com"),
            "the route must not be baked into the file — boot re-derives it from the catalog, so persisting it would survive `disconnect`"
        );
        drop(saves);
        let v = serde_json::to_value(rx.try_recv().expect("policy frame")).unwrap();
        assert_eq!(v["type"], "policy");
        assert_eq!(
            v["network"]["allowedRoutes"][0]["match"], "gitlab.com",
            "the live frame still carries the route so a held request can proceed"
        );
    }

    #[test]
    fn connect_integration_informs_when_persist_fails() {
        let (s, n, store, _rx) = fixture();
        store.fail_next(io::ErrorKind::PermissionDenied, "disk full");
        s.connect_integration("gitlab", vec![RouteRule::allow_host("gitlab.com")]);
        let informed = n.informed.lock().unwrap();
        assert_eq!(informed.len(), 1);
        assert!(informed[0].contains("not persisted"), "got: {:?}", informed);
    }

    struct FakeConnector {
        result: bool,
        connected: StdMutex<Vec<String>>,
        connected_with_token: StdMutex<Vec<(String, String)>>,
    }

    impl FakeConnector {
        fn new(result: bool) -> Self {
            Self {
                result,
                connected: StdMutex::new(Vec::new()),
                connected_with_token: StdMutex::new(Vec::new()),
            }
        }
    }

    impl IntegrationConnector for FakeConnector {
        fn connect<'a>(&'a self, id: &'a str) -> futures_util::future::BoxFuture<'a, bool> {
            Box::pin(async move {
                self.connected.lock().unwrap().push(id.to_string());
                self.result
            })
        }
        fn connect_with_token<'a>(
            &'a self,
            id: &'a str,
            value: String,
        ) -> futures_util::future::BoxFuture<'a, bool> {
            Box::pin(async move {
                self.connected_with_token
                    .lock()
                    .unwrap()
                    .push((id.to_string(), value));
                self.result
            })
        }
    }

    fn offerable(id: &str, name: &str, pattern: &str) -> OfferableIntegration {
        OfferableIntegration {
            id: id.into(),
            display_name: name.into(),
            patterns: vec![pattern.into()],
            token_fallback: None,
        }
    }

    fn offerable_with_fallback(id: &str, name: &str, pattern: &str) -> OfferableIntegration {
        OfferableIntegration {
            token_fallback: Some(TokenFallback {
                help: Some("https://example.com/pat".into()),
            }),
            ..offerable(id, name, pattern)
        }
    }

    fn offer_session(
        offers: Vec<OfferableIntegration>,
        connector: Option<Arc<FakeConnector>>,
    ) -> (
        ApprovalSession,
        Arc<RecordingNotifier>,
        mpsc::UnboundedReceiver<HostFrame>,
    ) {
        let notifier = Arc::new(RecordingNotifier::default());
        let store = Arc::new(CapturingStore::default());
        let (tx, rx) = mpsc::unbounded_channel();
        let session =
            ApprovalSession::new(Policy::default(), notifier.clone(), store, tx, TEST_TIMEOUT)
                .with_offers(offers);
        if let Some(c) = connector {
            session.set_connector(c);
        }
        (session, notifier, rx)
    }

    #[test]
    fn submit_pending_with_a_matching_offer_presents_the_integration_display_name() {
        let (s, n, _rx) = offer_session(
            vec![offerable("github_oauth", "GitHub", "api.github.com")],
            None,
        );
        s.submit_pending(pending("r1", "api.github.com"), Instant::now());
        assert_eq!(
            n.presented.lock().unwrap()[0].offer.as_deref(),
            Some("GitHub"),
            "a held request to an integration domain offers to connect it"
        );
    }

    #[test]
    fn submit_pending_without_a_matching_offer_presents_no_offer() {
        let (s, n, _rx) = offer_session(
            vec![offerable("github_oauth", "GitHub", "api.github.com")],
            None,
        );
        s.submit_pending(pending("r1", "example.com"), Instant::now());
        assert_eq!(n.presented.lock().unwrap()[0].offer, None);
    }

    #[test]
    fn submit_pending_surfaces_the_offered_integrations_token_fallback() {
        let (s, n, _rx) = offer_session(
            vec![offerable_with_fallback(
                "github_oauth",
                "GitHub",
                "api.github.com",
            )],
            None,
        );
        s.submit_pending(pending("r1", "api.github.com"), Instant::now());
        let presented = n.presented.lock().unwrap();
        assert_eq!(presented[0].offer.as_deref(), Some("GitHub"));
        assert_eq!(
            presented[0].token_fallback,
            Some(TokenFallback {
                help: Some("https://example.com/pat".into()),
            }),
            "an offer for an integration that declares a token fallback carries it to the card"
        );
    }

    #[test]
    fn submit_pending_carries_no_token_fallback_for_an_offer_that_declares_none() {
        let (s, n, _rx) = offer_session(
            vec![offerable("github_oauth", "GitHub", "api.github.com")],
            None,
        );
        s.submit_pending(pending("r1", "api.github.com"), Instant::now());
        assert_eq!(n.presented.lock().unwrap()[0].token_fallback, None);
    }

    #[tokio::test]
    async fn connect_offer_with_token_arms_via_the_token_and_releases_the_held_request_allow_once()
    {
        let connector = Arc::new(FakeConnector::new(true));
        let (s, n, mut rx) = offer_session(
            vec![offerable_with_fallback(
                "github_oauth",
                "GitHub",
                "api.github.com",
            )],
            Some(connector.clone()),
        );
        s.submit_pending(pending("r1", "api.github.com"), Instant::now());

        let outcome = s.connect_offer_with_token("r1", "ghp_pasted".into()).await;

        assert_eq!(outcome, DecisionOutcome::Resolved);
        assert_eq!(
            connector.connected_with_token.lock().unwrap().as_slice(),
            &[("github_oauth".to_string(), "ghp_pasted".to_string())],
            "the pasted token drives the token connect, not the interactive one"
        );
        assert!(
            connector.connected.lock().unwrap().is_empty(),
            "the interactive connect must not run when a token is pasted"
        );
        assert_eq!(decision_frame(&mut rx).decision, Decision::AllowOnce);
        assert_eq!(n.dismissed.lock().unwrap().as_slice(), &["r1".to_string()]);
    }

    #[tokio::test]
    async fn connect_offer_with_token_for_a_non_offer_id_is_unknownid() {
        let connector = Arc::new(FakeConnector::new(true));
        let (s, _n, _rx) = offer_session(vec![], Some(connector.clone()));
        s.submit_pending(pending("r1", "example.com"), Instant::now());
        assert_eq!(
            s.connect_offer_with_token("r1", "ghp_pasted".into()).await,
            DecisionOutcome::UnknownId
        );
        assert!(connector.connected_with_token.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn connect_offer_success_releases_the_held_request_with_allow_once() {
        let connector = Arc::new(FakeConnector::new(true));
        let (s, n, mut rx) = offer_session(
            vec![offerable("github_oauth", "GitHub", "api.github.com")],
            Some(connector.clone()),
        );
        s.submit_pending(pending("r1", "api.github.com"), Instant::now());

        let outcome = s.connect_offer("r1").await;

        assert_eq!(outcome, DecisionOutcome::Resolved);
        assert_eq!(
            connector.connected.lock().unwrap().as_slice(),
            &["github_oauth".to_string()],
            "accepting the offer drives a connect of the matched integration"
        );
        assert_eq!(decision_frame(&mut rx).decision, Decision::AllowOnce);
        assert_eq!(n.dismissed.lock().unwrap().as_slice(), &["r1".to_string()]);
    }

    #[tokio::test]
    async fn connect_offer_failure_releases_the_held_request_deny_once_closed() {
        let connector = Arc::new(FakeConnector::new(false));
        let (s, _n, mut rx) = offer_session(
            vec![offerable("github_oauth", "GitHub", "api.github.com")],
            Some(connector),
        );
        s.submit_pending(pending("r1", "api.github.com"), Instant::now());

        s.connect_offer("r1").await;

        assert_eq!(
            decision_frame(&mut rx).decision,
            Decision::DenyOnce,
            "a failed sign-in fails the held request closed"
        );
    }

    #[tokio::test]
    async fn connect_offer_without_a_connector_fails_closed() {
        let (s, _n, mut rx) = offer_session(
            vec![offerable("github_oauth", "GitHub", "api.github.com")],
            None,
        );
        s.submit_pending(pending("r1", "api.github.com"), Instant::now());

        s.connect_offer("r1").await;

        assert_eq!(decision_frame(&mut rx).decision, Decision::DenyOnce);
    }

    #[tokio::test]
    async fn connect_offer_for_an_id_that_carries_no_offer_is_unknownid() {
        let connector = Arc::new(FakeConnector::new(true));
        let (s, _n, mut rx) = offer_session(vec![], Some(connector.clone()));
        s.submit_pending(pending("r1", "example.com"), Instant::now());

        assert_eq!(s.connect_offer("r1").await, DecisionOutcome::UnknownId);
        assert!(
            connector.connected.lock().unwrap().is_empty(),
            "a plain network request must not be treated as an offer"
        );
        assert!(
            rx.try_recv().is_err(),
            "no decision frame for a non-offer id"
        );
    }

    #[tokio::test]
    async fn connect_offer_for_an_unknown_id_is_unknownid() {
        let (s, _n, _rx) = offer_session(
            vec![offerable("github_oauth", "GitHub", "api.github.com")],
            None,
        );
        assert_eq!(s.connect_offer("never").await, DecisionOutcome::UnknownId);
    }

    #[tokio::test]
    async fn connect_offer_releases_every_held_request_for_the_integration() {
        let connector = Arc::new(FakeConnector::new(true));
        let (s, n, mut rx) = offer_session(
            vec![offerable("github_oauth", "GitHub", "api.github.com")],
            Some(connector.clone()),
        );
        s.submit_pending(pending("r1", "api.github.com"), Instant::now());
        s.submit_pending(pending("r2", "api.github.com"), Instant::now());

        s.connect_offer("r1").await;

        assert_eq!(
            connector.connected.lock().unwrap().as_slice(),
            &["github_oauth".to_string()],
            "one sign-in serves every held request for the integration"
        );
        let a = decision_frame(&mut rx);
        let b = decision_frame(&mut rx);
        assert_eq!(a.decision, Decision::AllowOnce);
        assert_eq!(b.decision, Decision::AllowOnce);
        let mut released = vec![a.id, b.id];
        released.sort();
        assert_eq!(released, vec!["r1".to_string(), "r2".to_string()]);
        assert!(rx.try_recv().is_err(), "no third frame");
        let dismissed = n.dismissed.lock().unwrap();
        assert!(
            dismissed.contains(&"r1".to_string()) && dismissed.contains(&"r2".to_string()),
            "both cards are dismissed so no second offer is ever shown"
        );
    }

    #[test]
    fn submit_pending_coalesces_a_request_while_its_integration_is_connecting() {
        let (s, n, _rx) = offer_session(
            vec![offerable("github_oauth", "GitHub", "api.github.com")],
            None,
        );
        s.begin_connecting("github_oauth");
        s.submit_pending(pending("r1", "api.github.com"), Instant::now());
        assert!(
            n.presented.lock().unwrap().is_empty(),
            "a request arriving mid sign-in raises no new card"
        );
        assert_eq!(
            s.offer_request_ids("github_oauth"),
            vec!["r1".to_string()],
            "but it is still held, to be released by the in-flight connect"
        );
    }

    #[tokio::test]
    async fn connect_offer_for_a_sibling_while_connecting_does_not_start_a_second_connect() {
        let connector = Arc::new(FakeConnector::new(true));
        let (s, _n, mut rx) = offer_session(
            vec![offerable("github_oauth", "GitHub", "api.github.com")],
            Some(connector.clone()),
        );
        s.begin_connecting("github_oauth");
        s.submit_pending(pending("r2", "api.github.com"), Instant::now());

        let outcome = s.connect_offer("r2").await;

        assert_eq!(outcome, DecisionOutcome::Resolved);
        assert!(
            connector.connected.lock().unwrap().is_empty(),
            "a second sign-in must not run while one is in flight"
        );
        assert!(
            rx.try_recv().is_err(),
            "the in-flight connect releases r2, not this duplicate click"
        );
        assert_eq!(
            s.offer_request_ids("github_oauth"),
            vec!["r2".to_string()],
            "r2 stays held for the in-flight connect's batch"
        );
    }

    #[test]
    fn submit_pending_does_not_offer_an_already_connected_integration() {
        let mut policy = Policy::default();
        policy.connect("github_oauth");
        let notifier = Arc::new(RecordingNotifier::default());
        let store = Arc::new(CapturingStore::default());
        let (tx, _rx) = mpsc::unbounded_channel();
        let s = ApprovalSession::new(policy, notifier.clone(), store, tx, TEST_TIMEOUT)
            .with_offers(vec![offerable("github_oauth", "GitHub", "api.github.com")]);
        s.submit_pending(pending("r1", "api.github.com"), Instant::now());
        assert_eq!(
            notifier.presented.lock().unwrap()[0].offer,
            None,
            "a connected integration is not re-offered"
        );
    }

    #[test]
    fn an_offer_held_during_a_connect_is_not_swept_by_the_timeout_ticker() {
        let (s, _n, mut rx) = offer_session(
            vec![offerable("github_oauth", "GitHub", "api.github.com")],
            None,
        );
        let t0 = Instant::now();
        s.submit_pending(pending("r1", "api.github.com"), t0);
        s.begin_connecting("github_oauth");
        assert_eq!(
            s.tick_timeouts(t0 + TEST_TIMEOUT * 2),
            0,
            "a connecting offer must not time out under the sign-in"
        );
        s.finish_connecting("github_oauth");
        assert_eq!(
            s.tick_timeouts(t0 + TEST_TIMEOUT * 2),
            1,
            "once the connect ends the request can time out"
        );
        assert_eq!(decision_frame(&mut rx).decision, Decision::Timeout);
    }

    #[test]
    fn host_matches_pattern_exact_matches_only_the_same_host() {
        assert!(host_matches_pattern("api.github.com", "api.github.com"));
        assert!(!host_matches_pattern("api.github.com", "github.com"));
        assert!(!host_matches_pattern(
            "api.github.com",
            "evil-api.github.com"
        ));
    }

    #[test]
    fn host_matches_pattern_wildcard_covers_apex_and_subdomains_only() {
        assert!(host_matches_pattern("*.huggingface.co", "huggingface.co"));
        assert!(host_matches_pattern(
            "*.huggingface.co",
            "cdn.huggingface.co"
        ));
        assert!(!host_matches_pattern(
            "*.huggingface.co",
            "evilhuggingface.co"
        ));
        assert!(!host_matches_pattern(
            "*.huggingface.co",
            "huggingface.co.evil.com"
        ));
    }
}
