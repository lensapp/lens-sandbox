//! The credential-rule source of truth lives in `~/.lns-credentials.json`, not `lns-policy.yaml`.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::approval_flow::protocol::{
    CredentialDecision, CredentialDecisionKind, CredentialPending, HostFrame,
};
use crate::credential_flow::providers::DefProvider;
use crate::credential_flow::store::{CredentialEntry, CredentialStateFile, CredentialStore};

pub type FrameSink = mpsc::UnboundedSender<HostFrame>;

/// Invoked after a state-changing decision so a follow-up `Policy` frame arms the matching injection.
pub type PolicyEmitter = Box<dyn Fn(&CredentialStateFile) + Send + Sync>;

/// Invoked when an un-connected catalog integration is accepted, to connect it live (allow its routes + persist `integrations:`).
pub type ConnectEmitter = Box<dyn Fn(&str) + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialPendingPrompt {
    pub id: String,
    pub credential_id: String,
    pub action: String,
}

/// Abstracts the desktop notification surface so tests can drive prompts without the real system.
pub trait CredentialNotifier: Send + Sync {
    fn present(&self, pending: &CredentialPendingPrompt);
    fn dismiss(&self, id: &str);
    fn inform(&self, message: &str);
    fn clear_informs(&self);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionOutcome {
    Resolved,
    UnknownId,
}

/// `Allow`/`Deny` persist a rule to the store; `Timeout` persists nothing (S12).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialDecisionRequest {
    Allow(CredentialEntry),
    Deny,
    Timeout,
}

struct PendingEntry {
    prompt_id: String,
    request_ids: Vec<String>,
    deadline: Instant,
}

pub struct CredentialSession {
    state: Mutex<CredentialStateFile>,
    pending: Mutex<HashMap<String, PendingEntry>>,
    notifier: Arc<dyn CredentialNotifier>,
    store: Arc<dyn CredentialStore>,
    sink: FrameSink,
    policy_emitter: PolicyEmitter,
    timeout: Duration,
    custom_providers: Arc<Vec<DefProvider>>,
    connectable: HashSet<String>,
    connect: ConnectEmitter,
    oauth_configs: HashMap<String, crate::oauth::OauthConfig>,
    device_flow: Option<Arc<dyn crate::oauth::DeviceFlow>>,
    clock: Option<Arc<dyn crate::oauth::Clock>>,
}

impl CredentialSession {
    pub fn new(
        state: CredentialStateFile,
        notifier: Arc<dyn CredentialNotifier>,
        store: Arc<dyn CredentialStore>,
        sink: FrameSink,
        timeout: Duration,
    ) -> Self {
        Self::with_policy_emitter(state, notifier, store, sink, timeout, Box::new(|_| {}))
    }

    pub fn with_policy_emitter(
        state: CredentialStateFile,
        notifier: Arc<dyn CredentialNotifier>,
        store: Arc<dyn CredentialStore>,
        sink: FrameSink,
        timeout: Duration,
        policy_emitter: PolicyEmitter,
    ) -> Self {
        Self {
            state: Mutex::new(state),
            pending: Mutex::new(HashMap::new()),
            notifier,
            store,
            sink,
            policy_emitter,
            timeout,
            custom_providers: Arc::new(Vec::new()),
            connectable: HashSet::new(),
            connect: Box::new(|_| {}),
            oauth_configs: HashMap::new(),
            device_flow: None,
            clock: None,
        }
    }

    /// Wires the device-flow engine and per-integration oauth configs so an accepted oauth prompt can run an interactive sign-in.
    pub fn with_oauth(
        mut self,
        oauth_configs: HashMap<String, crate::oauth::OauthConfig>,
        device_flow: Arc<dyn crate::oauth::DeviceFlow>,
        clock: Arc<dyn crate::oauth::Clock>,
    ) -> Self {
        self.oauth_configs = oauth_configs;
        self.device_flow = Some(device_flow);
        self.clock = Some(clock);
        self
    }

    /// Captures the run's custom providers once at construction so a mid-run policy edit can't retroactively change a running workload's placeholder set.
    pub fn with_custom_providers(mut self, custom_providers: Arc<Vec<DefProvider>>) -> Self {
        self.custom_providers = custom_providers;
        self
    }

    /// Marks the catalog integrations that aren't connected yet: detecting one offers to connect, and accepting runs `connect` to allow its routes live.
    pub fn with_connect_emitter(
        mut self,
        connectable: HashSet<String>,
        connect: ConnectEmitter,
    ) -> Self {
        self.connectable = connectable;
        self.connect = connect;
        self
    }

    pub fn custom_providers(&self) -> &[DefProvider] {
        &self.custom_providers
    }

    pub fn current_state(&self) -> CredentialStateFile {
        self.state.lock().expect("state mutex poisoned").clone()
    }

    /// Concurrent requests for the same provider share one card; the decision later fans out to every held request (S11).
    pub fn submit_pending(&self, req: CredentialPending, now: Instant) {
        // A standing Deny rule fails the held request at the boundary without re-prompting (S7).
        if self.is_denied(&req.credential_id) {
            self.send_decision_frame(&req.id, CredentialDecisionKind::Deny);
            return;
        }
        let mut pending = self.pending.lock().expect("pending mutex poisoned");
        if let Some(entry) = pending.get_mut(&req.credential_id) {
            if !entry.request_ids.contains(&req.id) {
                entry.request_ids.push(req.id);
            }
            return;
        }
        pending.insert(
            req.credential_id.clone(),
            PendingEntry {
                prompt_id: req.id.clone(),
                request_ids: vec![req.id.clone()],
                deadline: now + self.timeout,
            },
        );
        drop(pending);
        let action = if self.connectable.contains(&req.credential_id) {
            format!("connect to {}", req.credential_id)
        } else {
            req.action
        };
        self.notifier.present(&CredentialPendingPrompt {
            id: req.id,
            credential_id: req.credential_id,
            action,
        });
    }

    fn is_denied(&self, credential_id: &str) -> bool {
        matches!(
            self.state
                .lock()
                .expect("state mutex poisoned")
                .get(credential_id),
            Some(CredentialEntry::Deny)
        )
    }

    /// Emits the armed `Policy` frame before the `CredentialDecision`s so the MITM has the injection in hand before it releases each held request.
    pub fn record_decision(&self, id: &str, request: CredentialDecisionRequest) -> DecisionOutcome {
        let Some((credential_id, request_ids)) = self.remove_pending(id) else {
            return DecisionOutcome::UnknownId;
        };
        self.notifier.dismiss(id);
        let kind = decision_kind_of(&request);
        // Accepting an un-connected catalog integration connects it live (routes) before the value is armed, so the held request sees both.
        let connect_now = matches!(request, CredentialDecisionRequest::Allow(_))
            && self.connectable.contains(&credential_id);
        if let Some(entry) = persistent_entry(request) {
            if connect_now {
                (self.connect)(&credential_id);
            }
            self.apply_persistent_entry(credential_id, entry);
        }
        for request_id in &request_ids {
            self.send_decision_frame(request_id, kind);
        }
        DecisionOutcome::Resolved
    }

    /// True when a held prompt belongs to an oauth integration, so its acceptance must drive a device sign-in rather than a static value decision.
    pub fn is_oauth_prompt(&self, prompt_id: &str) -> bool {
        let pending = self.pending.lock().expect("pending mutex poisoned");
        pending
            .iter()
            .any(|(id, e)| e.prompt_id == prompt_id && self.oauth_configs.contains_key(id))
    }

    /// Drives a device sign-in for an accepted oauth prompt: on success connects the integration live and arms the token set, releasing held requests; denial, expiry, or error fails them closed without persisting (the next use re-prompts).
    pub async fn connect_oauth(&self, prompt_id: &str) -> DecisionOutcome {
        let Some((credential_id, request_ids)) = self.remove_pending(prompt_id) else {
            return DecisionOutcome::UnknownId;
        };
        self.notifier.dismiss(prompt_id);
        let (Some(cfg), Some(flow), Some(clock)) = (
            self.oauth_configs.get(&credential_id),
            self.device_flow.as_ref(),
            self.clock.as_ref(),
        ) else {
            self.fail_held(&request_ids);
            return DecisionOutcome::Resolved;
        };
        let present = |code: &crate::oauth::DeviceCode| {
            self.notifier.inform(&format!(
                "To connect {credential_id}, open {} and enter code {}",
                code.verification_uri, code.user_code
            ));
        };
        match crate::oauth::run_device_flow(flow.as_ref(), cfg, present).await {
            Ok(crate::oauth::SignIn::Completed(token)) => {
                let entry = crate::oauth::entry_from_token(clock.as_ref(), &token);
                if self.connectable.contains(&credential_id) {
                    (self.connect)(&credential_id);
                }
                self.apply_persistent_entry(credential_id, entry);
                for request_id in &request_ids {
                    self.send_decision_frame(request_id, CredentialDecisionKind::Allow);
                }
            }
            Ok(crate::oauth::SignIn::Denied) | Ok(crate::oauth::SignIn::Expired) => {
                self.fail_held(&request_ids);
            }
            Err(e) => {
                self.notifier
                    .inform(&format!("sign-in to {credential_id} failed: {e:#}"));
                self.fail_held(&request_ids);
            }
        }
        DecisionOutcome::Resolved
    }

    fn fail_held(&self, request_ids: &[String]) {
        for request_id in request_ids {
            self.send_decision_frame(request_id, CredentialDecisionKind::Deny);
        }
    }

    /// Keyed by `prompt_id` because decisions and timeouts arrive against the card id, not the provider.
    fn remove_pending(&self, prompt_id: &str) -> Option<(String, Vec<String>)> {
        let mut pending = self.pending.lock().expect("pending mutex poisoned");
        let credential_id = pending
            .iter()
            .find(|(_, e)| e.prompt_id == prompt_id)
            .map(|(id, _)| id.clone())?;
        let entry = pending.remove(&credential_id).expect("just located entry");
        Some((credential_id, entry.request_ids))
    }

    fn send_decision_frame(&self, id: &str, decision: CredentialDecisionKind) {
        let _ = self
            .sink
            .send(HostFrame::CredentialDecision(CredentialDecision {
                id: id.to_string(),
                decision,
            }));
    }

    fn apply_persistent_entry(&self, credential_id: String, entry: CredentialEntry) {
        let snapshot = {
            let mut state = self.state.lock().expect("state mutex poisoned");
            state.insert(credential_id, entry);
            state.clone()
        };
        if let Err(e) = self.store.save(&snapshot) {
            self.notifier.inform(&format!(
                "credential rule applied in-memory but not persisted: {e}"
            ));
        }
        // Policy frame goes out even on a failed write so the held request that triggered the decision isn't stalled (S14).
        (self.policy_emitter)(&snapshot);
    }

    /// Expired prompts emit a `Timeout` decision so the MITM fails every held request closed (S12).
    pub fn tick_timeouts(&self, now: Instant) -> usize {
        let expired: Vec<String> = {
            let pending = self.pending.lock().expect("pending mutex poisoned");
            pending
                .values()
                .filter(|e| e.deadline <= now)
                .map(|e| e.prompt_id.clone())
                .collect()
        };
        expired
            .iter()
            .filter(|prompt_id| self.timeout_one(prompt_id))
            .count()
    }

    fn timeout_one(&self, prompt_id: &str) -> bool {
        let Some((_, request_ids)) = self.remove_pending(prompt_id) else {
            return false;
        };
        self.notifier.dismiss(prompt_id);
        for request_id in &request_ids {
            self.send_decision_frame(request_id, CredentialDecisionKind::Timeout);
        }
        true
    }

    /// Also emits a fresh Policy frame so the MITM picks up the new arming/revocation (S10).
    pub fn apply_external_state(&self, new_state: CredentialStateFile) {
        *self.state.lock().expect("state mutex poisoned") = new_state.clone();
        (self.policy_emitter)(&new_state);
    }

    /// Sends no decision frames because on WS drop the MITM is gone with the workload (S13).
    pub fn withdraw_run(&self) {
        let prompt_ids: Vec<String> = {
            let mut pending = self.pending.lock().expect("pending mutex poisoned");
            pending.drain().map(|(_, e)| e.prompt_id).collect()
        };
        for prompt_id in &prompt_ids {
            self.notifier.dismiss(prompt_id);
        }
        self.notifier.clear_informs();
    }
}

fn decision_kind_of(request: &CredentialDecisionRequest) -> CredentialDecisionKind {
    match request {
        CredentialDecisionRequest::Allow(_) => CredentialDecisionKind::Allow,
        CredentialDecisionRequest::Deny => CredentialDecisionKind::Deny,
        CredentialDecisionRequest::Timeout => CredentialDecisionKind::Timeout,
    }
}

fn persistent_entry(request: CredentialDecisionRequest) -> Option<CredentialEntry> {
    match request {
        CredentialDecisionRequest::Allow(entry) => Some(entry),
        CredentialDecisionRequest::Deny => Some(CredentialEntry::Deny),
        CredentialDecisionRequest::Timeout => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use std::sync::Mutex as StdMutex;

    #[derive(Default)]
    struct RecordingNotifier {
        presented: StdMutex<Vec<CredentialPendingPrompt>>,
        dismissed: StdMutex<Vec<String>>,
        informed: StdMutex<Vec<String>>,
        informs_cleared: StdMutex<usize>,
    }

    impl CredentialNotifier for RecordingNotifier {
        fn present(&self, p: &CredentialPendingPrompt) {
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
    struct CapturingStore {
        saves: StdMutex<Vec<CredentialStateFile>>,
        next_err: StdMutex<Option<io::Error>>,
    }

    impl CapturingStore {
        fn fail_next(&self, kind: io::ErrorKind, msg: &'static str) {
            *self.next_err.lock().unwrap() = Some(io::Error::new(kind, msg));
        }
    }

    impl CredentialStore for CapturingStore {
        fn load(&self) -> io::Result<CredentialStateFile> {
            Ok(CredentialStateFile::new())
        }
        fn save(&self, state: &CredentialStateFile) -> io::Result<()> {
            if let Some(e) = self.next_err.lock().unwrap().take() {
                return Err(e);
            }
            self.saves.lock().unwrap().push(state.clone());
            Ok(())
        }
    }

    #[test]
    fn capturing_store_load_returns_empty_state() {
        // The session never re-loads, so the fixture's `load` is pinned directly rather than through a scenario.
        let store = CapturingStore::default();
        assert!(store.load().unwrap().is_empty());
    }

    type Fixture = (
        CredentialSession,
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
        let session = CredentialSession::new(
            CredentialStateFile::new(),
            notifier.clone(),
            store.clone(),
            tx,
            timeout,
        );
        (session, notifier, store, rx)
    }

    fn pending(id: &str, credential_id: &str) -> CredentialPending {
        CredentialPending {
            id: id.into(),
            credential_id: credential_id.into(),
            action: format!("use of {credential_id} placeholder"),
            reason: "placeholder-unauthorized".into(),
        }
    }

    fn decision_frame(rx: &mut mpsc::UnboundedReceiver<HostFrame>) -> CredentialDecision {
        let v = serde_json::to_value(rx.try_recv().expect("expected a frame")).unwrap();
        assert_eq!(v["type"], "credential_decision", "got {v}");
        serde_json::from_value(v).expect("CredentialDecision must round-trip through JSON")
    }

    #[test]
    fn submit_pending_presents_notification_with_credential_metadata() {
        let (s, n, _store, _rx) = fixture();
        s.submit_pending(pending("c1", "github"), Instant::now());
        let p = n.presented.lock().unwrap();
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].id, "c1");
        assert_eq!(p[0].credential_id, "github");
        assert_eq!(p[0].action, "use of github placeholder");
    }

    #[test]
    fn duplicate_pending_id_does_not_present_a_second_notification() {
        let (s, n, _store, _rx) = fixture();
        s.submit_pending(pending("c1", "github"), Instant::now());
        s.submit_pending(pending("c1", "github"), Instant::now());
        assert_eq!(n.presented.lock().unwrap().len(), 1);
    }

    #[test]
    fn concurrent_requests_for_one_provider_present_a_single_card() {
        let (s, n, _store, _rx) = fixture();
        s.submit_pending(pending("c1", "github"), Instant::now());
        s.submit_pending(pending("c2", "github"), Instant::now());
        let presented = n.presented.lock().unwrap();
        assert_eq!(presented.len(), 1, "same provider must raise one card");
        assert_eq!(presented[0].id, "c1");
    }

    #[test]
    fn decision_on_a_coalesced_card_resolves_every_held_request() {
        let (s, n, store, mut rx) = fixture();
        s.submit_pending(pending("c1", "github"), Instant::now());
        s.submit_pending(pending("c2", "github"), Instant::now());

        let outcome = s.record_decision(
            "c1",
            CredentialDecisionRequest::Allow(CredentialEntry::HostDetect),
        );

        assert_eq!(outcome, DecisionOutcome::Resolved);
        let first = decision_frame(&mut rx);
        let second = decision_frame(&mut rx);
        assert_eq!(first.decision, CredentialDecisionKind::Allow);
        assert_eq!(second.decision, CredentialDecisionKind::Allow);
        assert_eq!(
            vec![first.id, second.id],
            vec!["c1".to_string(), "c2".to_string()]
        );
        assert!(
            rx.try_recv().is_err(),
            "no frame beyond the two held requests"
        );
        assert_eq!(
            store.saves.lock().unwrap().len(),
            1,
            "one provider decision persists exactly one rule"
        );
        assert_eq!(n.dismissed.lock().unwrap().as_slice(), &["c1".to_string()]);
    }

    #[test]
    fn timeout_on_a_coalesced_card_fails_every_held_request_closed() {
        let (s, _n, _store, mut rx) = fixture_with_timeout(Duration::from_secs(10));
        let t0 = Instant::now();
        s.submit_pending(pending("c1", "github"), t0);
        s.submit_pending(pending("c2", "github"), t0);

        let swept = s.tick_timeouts(t0 + Duration::from_secs(11));

        assert_eq!(swept, 1, "one coalesced card sweeps once");
        let first = decision_frame(&mut rx);
        let second = decision_frame(&mut rx);
        assert_eq!(first.decision, CredentialDecisionKind::Timeout);
        assert_eq!(second.decision, CredentialDecisionKind::Timeout);
        assert_eq!(
            vec![first.id, second.id],
            vec!["c1".to_string(), "c2".to_string()]
        );
        assert!(
            rx.try_recv().is_err(),
            "no frame beyond the two held requests"
        );
    }

    #[test]
    fn submit_pending_with_standing_deny_rule_fails_request_without_prompting() {
        let notifier = Arc::new(RecordingNotifier::default());
        let store = Arc::new(CapturingStore::default());
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut state = CredentialStateFile::new();
        state.insert("github".into(), CredentialEntry::Deny);
        let s = CredentialSession::new(state, notifier.clone(), store.clone(), tx, TEST_TIMEOUT);

        s.submit_pending(pending("c1", "github"), Instant::now());

        assert!(
            notifier.presented.lock().unwrap().is_empty(),
            "a standing Deny rule must not raise a card"
        );
        let frame = decision_frame(&mut rx);
        assert_eq!(frame.id, "c1");
        assert_eq!(frame.decision, CredentialDecisionKind::Deny);
        assert!(
            store.saves.lock().unwrap().is_empty(),
            "the Deny rule already exists, so nothing new is persisted"
        );
    }

    #[test]
    fn allow_with_host_detect_persists_host_detect_kind_and_emits_decision_frame() {
        let (s, n, store, mut rx) = fixture();
        s.submit_pending(pending("c1", "github"), Instant::now());

        let outcome = s.record_decision(
            "c1",
            CredentialDecisionRequest::Allow(CredentialEntry::HostDetect),
        );

        assert_eq!(outcome, DecisionOutcome::Resolved);
        assert_eq!(n.dismissed.lock().unwrap().as_slice(), &["c1".to_string()]);
        let frame = decision_frame(&mut rx);
        assert_eq!(frame.id, "c1");
        assert_eq!(frame.decision, CredentialDecisionKind::Allow);
        let saves = store.saves.lock().unwrap();
        assert_eq!(saves.len(), 1);
        assert_eq!(saves[0].get("github"), Some(&CredentialEntry::HostDetect));
        assert_eq!(
            s.current_state().get("github"),
            Some(&CredentialEntry::HostDetect)
        );
    }

    #[test]
    fn allow_with_stored_persists_typed_value_verbatim() {
        let (s, _n, store, mut rx) = fixture();
        s.submit_pending(pending("c1", "openai"), Instant::now());

        s.record_decision(
            "c1",
            CredentialDecisionRequest::Allow(CredentialEntry::Stored {
                value: "sk-real-token".into(),
            }),
        );

        let saves = store.saves.lock().unwrap();
        assert_eq!(
            saves[0].get("openai"),
            Some(&CredentialEntry::Stored {
                value: "sk-real-token".into(),
            })
        );
        assert_eq!(
            decision_frame(&mut rx).decision,
            CredentialDecisionKind::Allow
        );
    }

    #[test]
    fn deny_persists_deny_rule_and_emits_deny_decision_frame() {
        let (s, _n, store, mut rx) = fixture();
        s.submit_pending(pending("c1", "linear"), Instant::now());

        s.record_decision("c1", CredentialDecisionRequest::Deny);

        let saves = store.saves.lock().unwrap();
        assert_eq!(saves[0].get("linear"), Some(&CredentialEntry::Deny));
        assert_eq!(
            decision_frame(&mut rx).decision,
            CredentialDecisionKind::Deny
        );
        assert_eq!(
            s.current_state().get("linear"),
            Some(&CredentialEntry::Deny)
        );
    }

    #[test]
    fn timeout_request_emits_decision_frame_but_does_not_persist() {
        let (s, n, store, mut rx) = fixture();
        s.submit_pending(pending("c1", "github"), Instant::now());

        s.record_decision("c1", CredentialDecisionRequest::Timeout);

        assert_eq!(
            decision_frame(&mut rx).decision,
            CredentialDecisionKind::Timeout
        );
        assert_eq!(n.dismissed.lock().unwrap().as_slice(), &["c1".to_string()]);
        assert!(
            store.saves.lock().unwrap().is_empty(),
            "Timeout must not persist a rule"
        );
        assert!(s.current_state().is_empty());
    }

    #[test]
    fn allow_with_failed_persist_keeps_rule_in_memory_and_informs_developer() {
        let (s, n, store, mut rx) = fixture();
        s.submit_pending(pending("c1", "github"), Instant::now());
        store.fail_next(io::ErrorKind::PermissionDenied, "disk full");

        s.record_decision(
            "c1",
            CredentialDecisionRequest::Allow(CredentialEntry::HostDetect),
        );

        assert_eq!(
            s.current_state().get("github"),
            Some(&CredentialEntry::HostDetect)
        );
        let kind = decision_frame(&mut rx).decision;
        assert_eq!(kind, CredentialDecisionKind::Allow);
        assert!(store.saves.lock().unwrap().is_empty());
        let informed = n.informed.lock().unwrap();
        assert_eq!(informed.len(), 1);
        assert!(informed[0].contains("not persisted"));
    }

    #[test]
    fn record_decision_for_unknown_id_is_unknownid_and_no_frame() {
        let (s, _n, _store, mut rx) = fixture();
        let outcome = s.record_decision("never-submitted", CredentialDecisionRequest::Deny);
        assert_eq!(outcome, DecisionOutcome::UnknownId);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn record_decision_twice_returns_unknownid_the_second_time() {
        let (s, _n, _store, mut rx) = fixture();
        s.submit_pending(pending("c1", "github"), Instant::now());
        assert_eq!(
            s.record_decision("c1", CredentialDecisionRequest::Deny),
            DecisionOutcome::Resolved
        );
        assert_eq!(
            s.record_decision("c1", CredentialDecisionRequest::Deny),
            DecisionOutcome::UnknownId
        );
        assert!(rx.try_recv().is_ok());
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn tick_timeouts_before_deadline_sweeps_nothing() {
        let (s, n, _store, mut rx) = fixture_with_timeout(Duration::from_secs(30));
        let t0 = Instant::now();
        s.submit_pending(pending("c1", "github"), t0);

        let expired = s.tick_timeouts(t0 + Duration::from_secs(5));

        assert_eq!(expired, 0);
        assert!(rx.try_recv().is_err());
        assert!(n.dismissed.lock().unwrap().is_empty());
    }

    #[test]
    fn tick_timeouts_after_deadline_dismisses_and_emits_timeout_decision() {
        let (s, n, _store, mut rx) = fixture_with_timeout(Duration::from_secs(10));
        let t0 = Instant::now();
        s.submit_pending(pending("c1", "github"), t0);

        let expired = s.tick_timeouts(t0 + Duration::from_secs(11));

        assert_eq!(expired, 1);
        assert_eq!(n.dismissed.lock().unwrap().as_slice(), &["c1".to_string()]);
        assert_eq!(
            decision_frame(&mut rx).decision,
            CredentialDecisionKind::Timeout
        );
    }

    #[test]
    fn tick_timeouts_leaves_unexpired_entries_alone() {
        let (s, _n, _store, mut rx) = fixture_with_timeout(Duration::from_secs(10));
        let t0 = Instant::now();
        s.submit_pending(pending("c1", "github"), t0);
        s.submit_pending(pending("c2", "openai"), t0 + Duration::from_secs(20));

        let expired = s.tick_timeouts(t0 + Duration::from_secs(11));

        assert_eq!(expired, 1);
        let f = decision_frame(&mut rx);
        assert_eq!(f.id, "c1");
        assert_eq!(f.decision, CredentialDecisionKind::Timeout);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn timeout_one_is_noop_when_a_decision_already_resolved_the_id() {
        // A concurrent record_decision can remove the id in the snapshot-then-sweep window; without the guard the sweep would emit a second decision frame.
        let (s, n, _store, mut rx) = fixture();
        s.submit_pending(pending("c1", "github"), Instant::now());
        s.record_decision("c1", CredentialDecisionRequest::Deny);

        let acted = s.timeout_one("c1");

        assert!(!acted, "sweep must not act on an already-resolved id");
        let f = decision_frame(&mut rx);
        assert_eq!(f.decision, CredentialDecisionKind::Deny);
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
        s.submit_pending(pending("c1", "github"), t0);
        s.submit_pending(pending("c2", "openai"), t0);

        s.record_decision("c2", CredentialDecisionRequest::Deny);
        let _ = rx.try_recv();

        let swept = s.tick_timeouts(t0 + Duration::from_secs(2));

        assert_eq!(swept, 1, "only c1 should have been swept");
        let f = decision_frame(&mut rx);
        assert_eq!(f.id, "c1");
        assert_eq!(f.decision, CredentialDecisionKind::Timeout);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn timeout_one_emits_timeout_and_dismisses_when_pending_is_present() {
        let (s, n, _store, mut rx) = fixture();
        s.submit_pending(pending("c1", "github"), Instant::now());

        let acted = s.timeout_one("c1");

        assert!(acted);
        assert_eq!(
            decision_frame(&mut rx).decision,
            CredentialDecisionKind::Timeout
        );
        assert_eq!(n.dismissed.lock().unwrap().as_slice(), &["c1".to_string()]);
    }

    #[test]
    fn withdraw_run_dismisses_all_pending_without_emitting_frames() {
        let (s, n, _store, mut rx) = fixture();
        s.submit_pending(pending("c1", "github"), Instant::now());
        s.submit_pending(pending("c2", "openai"), Instant::now());

        s.withdraw_run();

        let dismissed = n.dismissed.lock().unwrap();
        assert_eq!(dismissed.len(), 2);
        assert!(dismissed.contains(&"c1".to_string()));
        assert!(dismissed.contains(&"c2".to_string()));
        assert!(
            rx.try_recv().is_err(),
            "withdraw must not emit decision frames"
        );
    }

    #[test]
    fn record_decision_after_withdraw_returns_unknownid() {
        let (s, _n, _store, _rx) = fixture();
        s.submit_pending(pending("c1", "github"), Instant::now());
        s.withdraw_run();
        assert_eq!(
            s.record_decision("c1", CredentialDecisionRequest::Deny),
            DecisionOutcome::UnknownId
        );
    }

    #[test]
    fn withdraw_run_clears_notifier_informs_so_window_does_not_stay_pinned() {
        let (s, n, _store, _rx) = fixture();
        s.submit_pending(pending("c1", "github"), Instant::now());
        n.informed
            .lock()
            .unwrap()
            .push("credential rule could not be persisted: disk full".into());

        s.withdraw_run();

        assert_eq!(
            *n.informs_cleared.lock().unwrap(),
            1,
            "withdraw_run must call clear_informs exactly once"
        );
    }

    #[test]
    fn apply_external_state_replaces_in_memory_state() {
        let (s, _n, _store, mut rx) = fixture();
        let mut updated = CredentialStateFile::new();
        updated.insert("github".into(), CredentialEntry::Deny);

        s.apply_external_state(updated.clone());

        assert_eq!(s.current_state(), updated);
        // No decision frame: S10 is a host-side hot-swap, not a workload-driven event.
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn record_decision_after_timeout_returns_unknownid() {
        let (s, _n, _store, mut rx) = fixture_with_timeout(Duration::from_secs(1));
        let t0 = Instant::now();
        s.submit_pending(pending("c1", "github"), t0);
        s.tick_timeouts(t0 + Duration::from_secs(2));
        let _ = rx.try_recv();

        let outcome = s.record_decision("c1", CredentialDecisionRequest::Deny);

        assert_eq!(outcome, DecisionOutcome::UnknownId);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn decision_kind_of_maps_each_request_variant() {
        assert_eq!(
            decision_kind_of(&CredentialDecisionRequest::Allow(CredentialEntry::Deny)),
            CredentialDecisionKind::Allow
        );
        assert_eq!(
            decision_kind_of(&CredentialDecisionRequest::Deny),
            CredentialDecisionKind::Deny
        );
        assert_eq!(
            decision_kind_of(&CredentialDecisionRequest::Timeout),
            CredentialDecisionKind::Timeout
        );
    }

    #[derive(Default)]
    struct CapturingEmitter {
        snapshots: StdMutex<Vec<CredentialStateFile>>,
    }

    #[test]
    fn record_decision_calls_policy_emitter_with_post_decision_snapshot() {
        let notifier = Arc::new(RecordingNotifier::default());
        let store = Arc::new(CapturingStore::default());
        let (tx, _rx) = mpsc::unbounded_channel();
        let captured = Arc::new(CapturingEmitter::default());
        let captured_clone = captured.clone();
        let session = CredentialSession::with_policy_emitter(
            CredentialStateFile::new(),
            notifier,
            store,
            tx,
            Duration::from_secs(30),
            Box::new(move |state| {
                captured_clone.snapshots.lock().unwrap().push(state.clone());
            }),
        );
        session.submit_pending(pending("c1", "github"), Instant::now());
        session.record_decision(
            "c1",
            CredentialDecisionRequest::Allow(CredentialEntry::HostDetect),
        );

        let snaps = captured.snapshots.lock().unwrap();
        assert_eq!(snaps.len(), 1);
        assert_eq!(snaps[0].get("github"), Some(&CredentialEntry::HostDetect));
    }

    #[test]
    fn record_decision_emits_policy_frame_before_decision_frame_on_allow() {
        let notifier = Arc::new(RecordingNotifier::default());
        let store = Arc::new(CapturingStore::default());
        let (tx, mut rx) = mpsc::unbounded_channel();
        let tx_for_emitter = tx.clone();
        let session = CredentialSession::with_policy_emitter(
            CredentialStateFile::new(),
            notifier,
            store,
            tx,
            Duration::from_secs(30),
            Box::new(move |_state| {
                use crate::approval_flow::protocol::PolicyMessage;
                let _ = tx_for_emitter.send(HostFrame::Policy(PolicyMessage::default()));
            }),
        );
        session.submit_pending(pending("c1", "github"), Instant::now());
        session.record_decision(
            "c1",
            CredentialDecisionRequest::Allow(CredentialEntry::HostDetect),
        );

        let first = rx.try_recv().expect("first frame");
        assert!(
            matches!(first, HostFrame::Policy(_)),
            "Policy must precede CredentialDecision on the wire, got {first:?} first"
        );
        let second = rx.try_recv().expect("second frame");
        assert!(
            matches!(second, HostFrame::CredentialDecision(_)),
            "expected CredentialDecision after Policy, got {second:?}"
        );
    }

    #[test]
    fn record_decision_calls_policy_emitter_even_when_store_save_fails() {
        let notifier = Arc::new(RecordingNotifier::default());
        let store = Arc::new(CapturingStore::default());
        store.fail_next(io::ErrorKind::PermissionDenied, "disk full");
        let (tx, _rx) = mpsc::unbounded_channel();
        let captured = Arc::new(CapturingEmitter::default());
        let captured_clone = captured.clone();
        let session = CredentialSession::with_policy_emitter(
            CredentialStateFile::new(),
            notifier,
            store,
            tx,
            Duration::from_secs(30),
            Box::new(move |state| {
                captured_clone.snapshots.lock().unwrap().push(state.clone());
            }),
        );
        session.submit_pending(pending("c1", "github"), Instant::now());
        session.record_decision("c1", CredentialDecisionRequest::Deny);

        let snaps = captured.snapshots.lock().unwrap();
        assert_eq!(
            snaps.len(),
            1,
            "policy_emitter must be called even when store.save fails"
        );
    }

    #[test]
    fn timeout_does_not_invoke_policy_emitter() {
        // The emitter's only call site is reached when `persistent_entry` is `Some`, so asserting the `None` mapping pins this without dead closure-instrumentation code (S12).
        assert_eq!(persistent_entry(CredentialDecisionRequest::Timeout), None);
    }

    #[test]
    fn apply_external_state_invokes_policy_emitter_with_new_state() {
        let notifier = Arc::new(RecordingNotifier::default());
        let store = Arc::new(CapturingStore::default());
        let (tx, _rx) = mpsc::unbounded_channel();
        let captured = Arc::new(CapturingEmitter::default());
        let captured_clone = captured.clone();
        let session = CredentialSession::with_policy_emitter(
            CredentialStateFile::new(),
            notifier,
            store,
            tx,
            Duration::from_secs(30),
            Box::new(move |state| {
                captured_clone.snapshots.lock().unwrap().push(state.clone());
            }),
        );
        let mut next = CredentialStateFile::new();
        next.insert("openai".into(), CredentialEntry::Deny);
        session.apply_external_state(next.clone());

        let snaps = captured.snapshots.lock().unwrap();
        assert_eq!(snaps.len(), 1);
        assert_eq!(snaps[0].get("openai"), Some(&CredentialEntry::Deny));
    }

    #[test]
    fn persistent_entry_drops_timeout_and_preserves_allow_and_deny() {
        assert_eq!(
            persistent_entry(CredentialDecisionRequest::Allow(
                CredentialEntry::HostDetect
            )),
            Some(CredentialEntry::HostDetect)
        );
        assert_eq!(
            persistent_entry(CredentialDecisionRequest::Deny),
            Some(CredentialEntry::Deny)
        );
        assert_eq!(persistent_entry(CredentialDecisionRequest::Timeout), None);
    }

    type ConnectableFixture = (
        CredentialSession,
        Arc<RecordingNotifier>,
        Arc<CapturingStore>,
        mpsc::UnboundedReceiver<HostFrame>,
        Arc<StdMutex<Vec<String>>>,
    );

    fn fixture_connectable(connectable: &[&str]) -> ConnectableFixture {
        let notifier = Arc::new(RecordingNotifier::default());
        let store = Arc::new(CapturingStore::default());
        let (tx, rx) = mpsc::unbounded_channel();
        let connected = Arc::new(StdMutex::new(Vec::new()));
        let connected_cb = connected.clone();
        let set: HashSet<String> = connectable.iter().map(|s| s.to_string()).collect();
        let session = CredentialSession::new(
            CredentialStateFile::new(),
            notifier.clone(),
            store.clone(),
            tx,
            TEST_TIMEOUT,
        )
        .with_connect_emitter(
            set,
            Box::new(move |id| connected_cb.lock().unwrap().push(id.to_string())),
        );
        (session, notifier, store, rx, connected)
    }

    #[test]
    fn submit_pending_flavors_the_action_as_connect_for_a_connectable_id() {
        let (s, n, _store, _rx, _connected) = fixture_connectable(&["gitlab"]);
        s.submit_pending(pending("c1", "gitlab"), Instant::now());
        let p = n.presented.lock().unwrap();
        assert_eq!(p[0].action, "connect to gitlab");
    }

    #[test]
    fn allow_for_a_connectable_id_connects_live_and_records_the_value() {
        let (s, _n, store, _rx, connected) = fixture_connectable(&["gitlab"]);
        s.submit_pending(pending("c1", "gitlab"), Instant::now());
        s.record_decision(
            "c1",
            CredentialDecisionRequest::Allow(CredentialEntry::HostDetect),
        );
        assert_eq!(
            connected.lock().unwrap().as_slice(),
            &["gitlab".to_string()],
            "accepting a connectable id connects it"
        );
        assert_eq!(
            store.saves.lock().unwrap()[0].get("gitlab"),
            Some(&CredentialEntry::HostDetect),
            "the value decision is still recorded"
        );
    }

    #[test]
    fn deny_for_a_connectable_id_does_not_connect() {
        let (s, _n, store, _rx, connected) = fixture_connectable(&["gitlab"]);
        s.submit_pending(pending("c1", "gitlab"), Instant::now());
        s.record_decision("c1", CredentialDecisionRequest::Deny);
        assert!(
            connected.lock().unwrap().is_empty(),
            "denying must not connect the integration"
        );
        assert_eq!(
            store.saves.lock().unwrap()[0].get("gitlab"),
            Some(&CredentialEntry::Deny)
        );
    }

    #[test]
    fn allow_for_a_non_connectable_id_does_not_connect() {
        let (s, _n, _store, _rx, connected) = fixture_connectable(&[]);
        s.submit_pending(pending("c1", "github"), Instant::now());
        s.record_decision(
            "c1",
            CredentialDecisionRequest::Allow(CredentialEntry::HostDetect),
        );
        assert!(
            connected.lock().unwrap().is_empty(),
            "a built-in/connected id must not trigger a connect"
        );
    }

    use std::collections::VecDeque;

    use futures_util::future::BoxFuture;

    struct FakeFlow {
        code_err: bool,
        polls: StdMutex<VecDeque<crate::oauth::PollOutcome>>,
    }
    impl FakeFlow {
        fn polling(polls: Vec<crate::oauth::PollOutcome>) -> Arc<Self> {
            Arc::new(Self {
                code_err: false,
                polls: StdMutex::new(polls.into()),
            })
        }
        fn code_error() -> Arc<Self> {
            Arc::new(Self {
                code_err: true,
                polls: StdMutex::new(VecDeque::new()),
            })
        }
    }
    impl crate::oauth::DeviceFlow for FakeFlow {
        fn request_device_code<'a>(
            &'a self,
            _cfg: &'a crate::oauth::OauthConfig,
        ) -> BoxFuture<'a, anyhow::Result<crate::oauth::DeviceCode>> {
            let err = self.code_err;
            Box::pin(async move {
                if err {
                    anyhow::bail!("device-authorization request failed");
                }
                Ok(crate::oauth::DeviceCode {
                    device_code: "dc".into(),
                    user_code: "WXYZ-1234".into(),
                    verification_uri: "https://example.com/device".into(),
                    interval: Duration::ZERO,
                    expires_in: Duration::from_secs(900),
                })
            })
        }
        fn poll_token<'a>(
            &'a self,
            _cfg: &'a crate::oauth::OauthConfig,
            _device_code: &'a str,
        ) -> BoxFuture<'a, anyhow::Result<crate::oauth::PollOutcome>> {
            Box::pin(async move {
                Ok(self
                    .polls
                    .lock()
                    .unwrap()
                    .pop_front()
                    .expect("poll_token scripted"))
            })
        }
        fn refresh<'a>(
            &'a self,
            _cfg: &'a crate::oauth::OauthConfig,
            _refresh_token: &'a str,
        ) -> BoxFuture<'a, anyhow::Result<crate::oauth::TokenSet>> {
            Box::pin(async move { anyhow::bail!("refresh is not exercised by connect tests") })
        }
    }

    #[tokio::test]
    async fn fake_flow_refresh_is_pinned_directly() {
        // connect_oauth requests-then-polls and never refreshes, so the connect-test fake's refresh arm is exercised directly.
        use crate::oauth::DeviceFlow;
        let flow = FakeFlow::polling(vec![]);
        let cfg = crate::oauth::OauthConfig {
            client_id: "Iv1.test".into(),
            scopes: vec![],
            device_authorization_endpoint: "https://example.com/device/code".into(),
            token_endpoint: "https://example.com/oauth/token".into(),
        };
        let err = flow.refresh(&cfg, "rt").await.unwrap_err();
        assert!(err.to_string().contains("not exercised"), "got: {err}");
    }

    struct FixedClock(u64);
    impl crate::oauth::Clock for FixedClock {
        fn now_unix(&self) -> u64 {
            self.0
        }
    }

    fn oauth_token(expires_in: u64) -> crate::oauth::TokenSet {
        crate::oauth::TokenSet {
            access_token: "gho_access".into(),
            refresh_token: "ghr_refresh".into(),
            expires_in: Duration::from_secs(expires_in),
        }
    }

    fn oauth_fixture(flow: Arc<dyn crate::oauth::DeviceFlow>) -> ConnectableFixture {
        let notifier = Arc::new(RecordingNotifier::default());
        let store = Arc::new(CapturingStore::default());
        let (tx, rx) = mpsc::unbounded_channel();
        let connected = Arc::new(StdMutex::new(Vec::new()));
        let connected_cb = connected.clone();
        let mut configs = HashMap::new();
        configs.insert(
            "github_oauth".to_string(),
            crate::oauth::OauthConfig {
                client_id: "Iv1.test".into(),
                scopes: vec!["repo".into()],
                device_authorization_endpoint: "https://example.com/device/code".into(),
                token_endpoint: "https://example.com/oauth/token".into(),
            },
        );
        let session = CredentialSession::new(
            CredentialStateFile::new(),
            notifier.clone(),
            store.clone(),
            tx,
            TEST_TIMEOUT,
        )
        .with_connect_emitter(
            HashSet::from(["github_oauth".to_string()]),
            Box::new(move |id| connected_cb.lock().unwrap().push(id.to_string())),
        )
        .with_oauth(configs, flow, Arc::new(FixedClock(1000)));
        (session, notifier, store, rx, connected)
    }

    #[test]
    fn is_oauth_prompt_distinguishes_oauth_from_plain_pending() {
        let (s, _n, _store, _rx, _c) = oauth_fixture(FakeFlow::polling(vec![]));
        s.submit_pending(pending("c1", "github_oauth"), Instant::now());
        s.submit_pending(pending("c2", "plain"), Instant::now());
        assert!(s.is_oauth_prompt("c1"));
        assert!(
            !s.is_oauth_prompt("c2"),
            "a non-oauth prompt is not an oauth prompt"
        );
        assert!(
            !s.is_oauth_prompt("nope"),
            "an unknown prompt id is not an oauth prompt"
        );
    }

    #[tokio::test]
    async fn connect_oauth_completes_arms_the_token_connects_live_and_releases_held_requests() {
        let (s, n, store, mut rx, connected) =
            oauth_fixture(FakeFlow::polling(vec![crate::oauth::PollOutcome::Token(
                oauth_token(3600),
            )]));
        s.submit_pending(pending("c1", "github_oauth"), Instant::now());
        s.submit_pending(pending("c2", "github_oauth"), Instant::now());
        let outcome = s.connect_oauth("c1").await;
        assert_eq!(outcome, DecisionOutcome::Resolved);
        assert_eq!(
            connected.lock().unwrap().as_slice(),
            &["github_oauth".to_string()],
            "accepting an oauth prompt connects the integration live"
        );
        assert_eq!(
            store
                .saves
                .lock()
                .unwrap()
                .last()
                .unwrap()
                .get("github_oauth"),
            Some(&CredentialEntry::Oauth {
                access_token: "gho_access".into(),
                refresh_token: "ghr_refresh".into(),
                expires_at: 1000 + 3600,
            }),
            "the obtained token set is armed and persisted"
        );
        let f1 = decision_frame(&mut rx);
        let f2 = decision_frame(&mut rx);
        assert_eq!(f1.decision, CredentialDecisionKind::Allow);
        assert_eq!(f2.decision, CredentialDecisionKind::Allow);
        assert_eq!(vec![f1.id, f2.id], vec!["c1".to_string(), "c2".to_string()]);
        assert!(
            n.informed
                .lock()
                .unwrap()
                .iter()
                .any(|m| m.contains("WXYZ-1234")),
            "the verification code is surfaced to the user"
        );
    }

    #[tokio::test]
    async fn connect_oauth_denied_fails_held_requests_without_arming_or_connecting() {
        let (s, _n, store, mut rx, connected) =
            oauth_fixture(FakeFlow::polling(vec![crate::oauth::PollOutcome::Denied]));
        s.submit_pending(pending("c1", "github_oauth"), Instant::now());
        s.connect_oauth("c1").await;
        assert!(connected.lock().unwrap().is_empty());
        assert!(store.saves.lock().unwrap().is_empty());
        assert_eq!(
            decision_frame(&mut rx).decision,
            CredentialDecisionKind::Deny
        );
    }

    #[tokio::test]
    async fn connect_oauth_expired_fails_held_requests_without_arming() {
        let (s, _n, store, mut rx, _connected) =
            oauth_fixture(FakeFlow::polling(vec![crate::oauth::PollOutcome::Expired]));
        s.submit_pending(pending("c1", "github_oauth"), Instant::now());
        s.connect_oauth("c1").await;
        assert!(store.saves.lock().unwrap().is_empty());
        assert_eq!(
            decision_frame(&mut rx).decision,
            CredentialDecisionKind::Deny
        );
    }

    #[tokio::test]
    async fn connect_oauth_surfaces_a_device_flow_error_and_fails_held_requests() {
        let (s, n, store, mut rx, _connected) = oauth_fixture(FakeFlow::code_error());
        s.submit_pending(pending("c1", "github_oauth"), Instant::now());
        s.connect_oauth("c1").await;
        assert_eq!(
            decision_frame(&mut rx).decision,
            CredentialDecisionKind::Deny
        );
        assert!(store.saves.lock().unwrap().is_empty());
        assert!(
            n.informed
                .lock()
                .unwrap()
                .iter()
                .any(|m| m.contains("failed"))
        );
    }

    #[tokio::test]
    async fn connect_oauth_on_an_unknown_prompt_is_a_noop() {
        let (s, _n, _store, _rx, _c) = oauth_fixture(FakeFlow::polling(vec![]));
        assert_eq!(s.connect_oauth("nope").await, DecisionOutcome::UnknownId);
    }

    #[tokio::test]
    async fn connect_oauth_for_a_prompt_without_an_oauth_config_fails_it_closed() {
        let (s, _n, store, mut rx, _c) = oauth_fixture(FakeFlow::polling(vec![]));
        s.submit_pending(pending("c1", "plain"), Instant::now());
        s.connect_oauth("c1").await;
        assert_eq!(
            decision_frame(&mut rx).decision,
            CredentialDecisionKind::Deny
        );
        assert!(store.saves.lock().unwrap().is_empty());
    }
}
