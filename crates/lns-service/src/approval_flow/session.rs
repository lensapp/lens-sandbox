use std::collections::HashMap;
use std::sync::Arc;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::approval_flow::protocol::{
    Credential, Decision, HostFrame, PolicyMessage, RequestDecision, RequestPending,
};
use lns_policy::{Policy, PolicyStore, RouteRule};

pub type FrameSink = mpsc::UnboundedSender<HostFrame>;

/// Supplies the registry credentials bundled into every emitted `Policy` frame so a network decision is never read upstream as "drop all credentials".
pub type CredentialsProvider = Box<dyn Fn() -> Vec<Credential> + Send + Sync>;

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
}

#[derive(Debug)]
struct PendingEntry {
    host: String,
    deadline: Instant,
}

pub struct ApprovalSession {
    policy: Mutex<Policy>,
    pending: Mutex<HashMap<String, PendingEntry>>,
    notifier: Arc<dyn Notifier>,
    store: Arc<dyn PolicyStore>,
    sink: FrameSink,
    timeout: Duration,
    credentials_provider: OnceLock<CredentialsProvider>,
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
        }
    }

    /// Installs the credentials closure once at boot; idempotent, the first provider wins.
    pub fn set_credentials_provider(&self, provider: CredentialsProvider) {
        let _ = self.credentials_provider.set(provider);
    }

    fn current_credentials(&self) -> Option<Vec<Credential>> {
        self.credentials_provider.get().map(|p| p())
    }

    pub fn current_policy(&self) -> Policy {
        self.policy.lock().expect("policy mutex poisoned").clone()
    }

    pub fn submit_pending(&self, req: RequestPending, now: Instant) {
        let mut pending = self.pending.lock().expect("pending mutex poisoned");
        if pending.contains_key(&req.id) {
            return;
        }
        pending.insert(
            req.id.clone(),
            PendingEntry {
                host: req.host.clone(),
                deadline: now + self.timeout,
            },
        );
        drop(pending);
        self.notifier.present(&PendingPrompt {
            id: req.id,
            host: req.host,
            action: req.action,
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

    pub fn tick_timeouts(&self, now: Instant) -> usize {
        let expired: Vec<String> = {
            let pending = self.pending.lock().expect("pending mutex poisoned");
            pending
                .iter()
                .filter(|(_, entry)| entry.deadline <= now)
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
}
