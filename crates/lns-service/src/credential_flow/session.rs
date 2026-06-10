//! The credential-rule source of truth lives in `~/.lns-credentials.json`, not `lns-policy.yaml`.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::approval_flow::protocol::{
    CredentialDecision, CredentialDecisionKind, CredentialInjection, CredentialPending, HostFrame,
};
use crate::credential_flow::providers::{self, DefProvider, Provider};
use crate::credential_flow::store::{CredentialEntry, CredentialStateFile, CredentialStore};
use lns_policy::integrations::TokenFallback;

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
    /// Some(display name) when this is an oauth integration to connect via a browser sign-in, so the card offers "Connect to <name>" instead of a value field.
    pub oauth_display_name: Option<String>,
    /// Some when the integration declares a token fallback, so the consent card can also reveal "use a token instead".
    pub token_fallback: Option<TokenFallback>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignInPrompt {
    pub credential_id: String,
    pub display_name: String,
    pub user_code: String,
    pub verification_uri: String,
    /// Some when the integration declares a token fallback, so the sign-in card can offer "use a token instead" to a user blocked from the browser dance.
    pub token_fallback: Option<TokenFallback>,
}

/// Abstracts the desktop notification surface so tests can drive prompts without the real system.
pub trait CredentialNotifier: Send + Sync {
    fn present(&self, pending: &CredentialPendingPrompt);
    fn dismiss(&self, id: &str);
    fn inform(&self, message: &str);
    fn clear_informs(&self);
    /// Presents the device-flow verification step; firing `cancel` aborts the in-flight sign-in (or pivots it to a pasted token). The default renders it as an inform and drops `cancel`, so notifiers without a sign-in card still surface the code.
    fn present_sign_in(
        &self,
        prompt: &SignInPrompt,
        cancel: tokio::sync::oneshot::Sender<crate::oauth::SignInPivot>,
    ) {
        let _ = cancel;
        self.inform(&format!(
            "To connect {}, open {} and enter code {}",
            prompt.display_name, prompt.verification_uri, prompt.user_code
        ));
    }
    /// Removes the sign-in card for `credential_id` once its flow resolves; the default is a no-op for notifiers that don't model one.
    fn dismiss_sign_in(&self, credential_id: &str) {
        let _ = credential_id;
    }
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
    oauth_display_names: HashMap<String, String>,
    token_fallbacks: HashMap<String, TokenFallback>,
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
            oauth_display_names: HashMap::new(),
            token_fallbacks: HashMap::new(),
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

    /// Per-id user-facing labels (e.g. `github_oauth` → "GitHub") for connect prompts and the sign-in card; ids absent here fall back to the id itself.
    pub fn with_oauth_display_names(mut self, display_names: HashMap<String, String>) -> Self {
        self.oauth_display_names = display_names;
        self
    }

    /// Per-id token fallbacks so a consent or sign-in card for an integration that declares one can offer "use a token instead".
    pub fn with_token_fallbacks(mut self, token_fallbacks: HashMap<String, TokenFallback>) -> Self {
        self.token_fallbacks = token_fallbacks;
        self
    }

    fn display_name_for(&self, credential_id: &str) -> String {
        self.oauth_display_names
            .get(credential_id)
            .cloned()
            .unwrap_or_else(|| credential_id.to_string())
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
        // Already armed for this host: the gate is a propagation race (the guest released the connection before applying the armed injection), not a fresh consent. Allow it — the guest re-injects on Allow — instead of re-prompting (and re-running a sign-in).
        if self.is_armed_for_request(&req.credential_id, &req.action) {
            self.send_decision_frame(&req.id, CredentialDecisionKind::Allow);
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
        let oauth_display_name = self
            .oauth_configs
            .contains_key(&req.credential_id)
            .then(|| self.display_name_for(&req.credential_id));
        let token_fallback = self.token_fallbacks.get(&req.credential_id).cloned();
        let action = if self.connectable.contains(&req.credential_id) {
            format!("connect to {}", req.credential_id)
        } else {
            req.action
        };
        self.notifier.present(&CredentialPendingPrompt {
            id: req.id,
            credential_id: req.credential_id,
            action,
            token_fallback,
            oauth_display_name,
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

    /// True when `credential_id` already holds a usable value and injects into the host named in `action`, so a gate for it is a propagation race the host can safely allow rather than re-prompt. A request to a host the credential does not inject into (a real leak attempt) returns false and still prompts.
    fn is_armed_for_request(&self, credential_id: &str, action: &str) -> bool {
        if !self.has_armed_value(credential_id) {
            return false;
        }
        request_host(action).is_some_and(|host| self.injects_for_host(credential_id, host))
    }

    fn has_armed_value(&self, credential_id: &str) -> bool {
        match self
            .state
            .lock()
            .expect("state mutex poisoned")
            .get(credential_id)
        {
            Some(CredentialEntry::Oauth { access_token, .. }) => !access_token.is_empty(),
            Some(CredentialEntry::Stored { value }) => !value.is_empty(),
            _ => false,
        }
    }

    fn injects_for_host(&self, credential_id: &str, host: &str) -> bool {
        self.custom_providers
            .iter()
            .map(|p| p as &dyn Provider)
            .chain(providers::ALL.iter().map(|p| *p as &dyn Provider))
            .find(|p| p.id() == credential_id)
            .is_some_and(|p| {
                p.unarmed_injections()
                    .iter()
                    .any(|inj| injection_targets_host(inj, host))
            })
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
        if self.run_oauth_connect(&credential_id).await {
            for request_id in &request_ids {
                self.send_decision_frame(request_id, CredentialDecisionKind::Allow);
            }
        } else {
            self.fail_held(&request_ids);
        }
        DecisionOutcome::Resolved
    }

    /// Connects integration `id` outside the held-credential flow (e.g. accepting a network offer): a device sign-in for an oauth id, a straight route-allow for a plain connectable id; returns whether it is now connected.
    pub async fn connect_integration_now(&self, id: &str) -> bool {
        if self.oauth_configs.contains_key(id) {
            let ok = self.run_oauth_connect(id).await;
            if ok {
                // The same connect arms the token, so any placeholder card already held for this integration is satisfied too — don't ask for it separately.
                self.release_armed_holds(id);
            }
            return ok;
        }
        if self.connectable.contains(id) {
            (self.connect)(id);
            return true;
        }
        false
    }

    /// Connects integration `id` from a network offer using a pasted token instead of the interactive sign-in: arms the value in its slot, allows its routes (if connectable), and releases any placeholder card already held for it. Always reports connected.
    pub fn connect_integration_with_token(&self, id: &str, value: String) -> bool {
        self.arm_connected(id, CredentialEntry::Stored { value });
        self.release_armed_holds(id);
        true
    }

    /// Allows and dismisses a held credential prompt for `credential_id` once another surface (a network offer) has armed the integration; a no-op when nothing is held for it.
    fn release_armed_holds(&self, credential_id: &str) {
        let entry = self
            .pending
            .lock()
            .expect("pending mutex poisoned")
            .remove(credential_id);
        let Some(entry) = entry else {
            return;
        };
        self.notifier.dismiss(&entry.prompt_id);
        for request_id in &entry.request_ids {
            self.send_decision_frame(request_id, CredentialDecisionKind::Allow);
        }
    }

    /// Runs the device sign-in for an oauth integration and, on success, arms the token set and connects it live; returns whether a token was obtained. A missing config, denial, expiry, cancel, or error yields false.
    async fn run_oauth_connect(&self, credential_id: &str) -> bool {
        let (Some(cfg), Some(flow), Some(clock)) = (
            self.oauth_configs.get(credential_id),
            self.device_flow.as_ref(),
            self.clock.as_ref(),
        ) else {
            return false;
        };
        let display_name = self.display_name_for(credential_id);
        let token_fallback = self.token_fallbacks.get(credential_id).cloned();
        let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<crate::oauth::SignInPivot>();
        let card_id = credential_id.to_string();
        let present = move |code: &crate::oauth::DeviceCode| {
            self.notifier.present_sign_in(
                &SignInPrompt {
                    credential_id: card_id,
                    display_name,
                    user_code: code.user_code.clone(),
                    verification_uri: code.verification_uri.clone(),
                    token_fallback,
                },
                cancel_tx,
            );
        };
        let cancel = async move {
            // A dropped sender means the notifier has no cancel surface, so the sign-in runs to its natural end.
            match cancel_rx.await {
                Ok(pivot) => pivot,
                Err(_) => std::future::pending().await,
            }
        };
        let result = crate::oauth::run_device_flow(flow.as_ref(), cfg, present, cancel).await;
        self.notifier.dismiss_sign_in(credential_id);
        match result {
            Ok(crate::oauth::SignIn::Completed(token)) => {
                let entry = crate::oauth::entry_from_token(clock.as_ref(), &token);
                self.arm_connected(credential_id, entry);
                true
            }
            // Pivoting to a pasted token mid-flow arms it in the same credential slot, exactly as a completed sign-in would.
            Ok(crate::oauth::SignIn::Token(value)) => {
                self.arm_connected(credential_id, CredentialEntry::Stored { value });
                true
            }
            Ok(
                crate::oauth::SignIn::Denied
                | crate::oauth::SignIn::Expired
                | crate::oauth::SignIn::Cancelled,
            ) => false,
            Err(e) => {
                self.notifier
                    .inform(&format!("sign-in to {credential_id} failed: {e:#}"));
                false
            }
        }
    }

    /// Connects the integration live (if it isn't already) and arms `entry` in its credential slot — the shared tail of every successful connect, whether by completed sign-in or pasted token.
    fn arm_connected(&self, credential_id: &str, entry: CredentialEntry) {
        if self.connectable.contains(credential_id) {
            (self.connect)(credential_id);
        }
        self.apply_persistent_entry(credential_id.to_string(), entry);
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

/// The host an outbound request targets, parsed from a gate `action` like `GET api.github.com/x` or `CONNECT api.github.com:443`.
fn request_host(action: &str) -> Option<&str> {
    let target = action.split_whitespace().nth(1)?;
    let host = target.split(['/', ':']).next()?;
    (!host.is_empty()).then_some(host)
}

fn injection_targets_host(inj: &CredentialInjection, host: &str) -> bool {
    match inj {
        CredentialInjection::Header { domain, .. } => domain == host,
        CredentialInjection::UriPlaceholder { domain, .. } => domain == host,
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
        sign_ins: StdMutex<Vec<SignInPrompt>>,
        dismissed_sign_ins: StdMutex<Vec<String>>,
        pivot_next: StdMutex<Option<crate::oauth::SignInPivot>>,
        dismissed: StdMutex<Vec<String>>,
        informed: StdMutex<Vec<String>>,
        informs_cleared: StdMutex<usize>,
    }

    impl RecordingNotifier {
        fn cancel_next_sign_in(&self) {
            *self.pivot_next.lock().unwrap() = Some(crate::oauth::SignInPivot::Cancel);
        }
        fn use_token_next_sign_in(&self, value: &str) {
            *self.pivot_next.lock().unwrap() =
                Some(crate::oauth::SignInPivot::UseToken(value.to_string()));
        }
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
        fn present_sign_in(
            &self,
            p: &SignInPrompt,
            cancel: tokio::sync::oneshot::Sender<crate::oauth::SignInPivot>,
        ) {
            self.sign_ins.lock().unwrap().push(p.clone());
            if let Some(pivot) = self.pivot_next.lock().unwrap().take() {
                let _ = cancel.send(pivot);
            }
            // Otherwise drop the sender: connect_oauth treats that as "no cancel surface" and runs to completion.
        }
        fn dismiss_sign_in(&self, credential_id: &str) {
            self.dismissed_sign_ins
                .lock()
                .unwrap()
                .push(credential_id.to_string());
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
    fn submit_pending_for_a_non_oauth_id_carries_no_oauth_display_name() {
        let (s, n, _store, _rx, _connected) = fixture_connectable(&["gitlab"]);
        s.submit_pending(pending("c1", "gitlab"), Instant::now());
        assert_eq!(
            n.presented.lock().unwrap()[0].oauth_display_name,
            None,
            "a credential connectable is not a browser sign-in"
        );
    }

    #[test]
    fn submit_pending_for_an_oauth_id_flavors_the_prompt_with_its_display_name() {
        let (s, n, _store, _rx, _c) = oauth_fixture(FakeFlow::polling(vec![]));
        s.submit_pending(pending("c1", "github_oauth"), Instant::now());
        assert_eq!(
            n.presented.lock().unwrap()[0].oauth_display_name,
            Some("GitHub".to_string())
        );
    }

    #[test]
    fn submit_pending_surfaces_the_token_fallback_on_the_consent_prompt() {
        let (s, n, _store, _rx, _c) = oauth_fixture(FakeFlow::polling(vec![]));
        s.submit_pending(pending("c1", "github_oauth"), Instant::now());
        assert_eq!(
            n.presented.lock().unwrap()[0].token_fallback,
            Some(TokenFallback {
                help: Some("https://example.com/pat".into()),
            }),
            "a consent card for an integration with a token fallback offers the pivot"
        );
    }

    #[test]
    fn submit_pending_carries_no_token_fallback_for_an_id_that_declares_none() {
        let (s, n, _store, _rx, _connected) = fixture_connectable(&["gitlab"]);
        s.submit_pending(pending("c1", "gitlab"), Instant::now());
        assert_eq!(n.presented.lock().unwrap()[0].token_fallback, None);
    }

    #[test]
    fn submit_pending_for_an_oauth_id_without_a_configured_name_falls_back_to_the_id() {
        let notifier = Arc::new(RecordingNotifier::default());
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut configs = HashMap::new();
        configs.insert(
            "github_oauth".to_string(),
            crate::oauth::OauthConfig {
                client_id: "Iv1.test".into(),
                scopes: vec![],
                device_authorization_endpoint: "https://example.com/device/code".into(),
                token_endpoint: "https://example.com/oauth/token".into(),
            },
        );
        let s = CredentialSession::new(
            CredentialStateFile::new(),
            notifier.clone(),
            Arc::new(CapturingStore::default()),
            tx,
            TEST_TIMEOUT,
        )
        .with_oauth(configs, FakeFlow::polling(vec![]), Arc::new(FixedClock(0)));
        s.submit_pending(pending("c1", "github_oauth"), Instant::now());
        assert_eq!(
            notifier.presented.lock().unwrap()[0].oauth_display_name,
            Some("github_oauth".to_string()),
            "absent a configured name, the id is the fallback label"
        );
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
        .with_oauth(configs, flow, Arc::new(FixedClock(1000)))
        .with_oauth_display_names(HashMap::from([(
            "github_oauth".to_string(),
            "GitHub".to_string(),
        )]))
        .with_token_fallbacks(HashMap::from([(
            "github_oauth".to_string(),
            TokenFallback {
                help: Some("https://example.com/pat".into()),
            },
        )]));
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
        let sign_ins = n.sign_ins.lock().unwrap();
        assert_eq!(sign_ins.len(), 1, "the verification step is presented once");
        assert_eq!(sign_ins[0].display_name, "GitHub");
        assert_eq!(sign_ins[0].user_code, "WXYZ-1234");
        assert_eq!(sign_ins[0].verification_uri, "https://example.com/device");
        drop(sign_ins);
        assert_eq!(
            n.dismissed_sign_ins.lock().unwrap().as_slice(),
            &["github_oauth".to_string()],
            "the sign-in card is dismissed once the flow resolves"
        );
    }

    #[tokio::test]
    async fn connect_oauth_pivots_to_a_pasted_token_arms_stored_connects_and_releases_held_requests()
     {
        let (s, n, store, mut rx, connected) = oauth_fixture(FakeFlow::polling(vec![]));
        n.use_token_next_sign_in("ghp_pasted");
        s.submit_pending(pending("c1", "github_oauth"), Instant::now());
        s.submit_pending(pending("c2", "github_oauth"), Instant::now());

        let outcome = s.connect_oauth("c1").await;

        assert_eq!(outcome, DecisionOutcome::Resolved);
        assert_eq!(
            connected.lock().unwrap().as_slice(),
            &["github_oauth".to_string()],
            "pivoting to a token still connects the integration live"
        );
        assert_eq!(
            store
                .saves
                .lock()
                .unwrap()
                .last()
                .unwrap()
                .get("github_oauth"),
            Some(&CredentialEntry::Stored {
                value: "ghp_pasted".into(),
            }),
            "the pasted token is armed as a Stored value in the integration's slot"
        );
        let f1 = decision_frame(&mut rx);
        let f2 = decision_frame(&mut rx);
        assert_eq!(f1.decision, CredentialDecisionKind::Allow);
        assert_eq!(f2.decision, CredentialDecisionKind::Allow);
        assert_eq!(vec![f1.id, f2.id], vec!["c1".to_string(), "c2".to_string()]);
        assert_eq!(
            n.dismissed_sign_ins.lock().unwrap().as_slice(),
            &["github_oauth".to_string()],
            "the sign-in card is dismissed once the pivot resolves the flow"
        );
    }

    #[tokio::test]
    async fn the_sign_in_card_carries_the_integrations_declared_token_fallback() {
        let (s, n, _store, _rx, _connected) =
            oauth_fixture(FakeFlow::polling(vec![crate::oauth::PollOutcome::Token(
                oauth_token(3600),
            )]));
        s.submit_pending(pending("c1", "github_oauth"), Instant::now());
        s.connect_oauth("c1").await;
        let sign_ins = n.sign_ins.lock().unwrap();
        assert_eq!(
            sign_ins[0].token_fallback,
            Some(TokenFallback {
                help: Some("https://example.com/pat".into()),
            }),
            "a sign-in card for an integration with a token fallback offers the pivot"
        );
    }

    #[tokio::test]
    async fn connect_oauth_cancelled_fails_held_requests_and_dismisses_the_card() {
        let (s, n, store, mut rx, connected) = oauth_fixture(FakeFlow::polling(vec![]));
        n.cancel_next_sign_in();
        s.submit_pending(pending("c1", "github_oauth"), Instant::now());

        s.connect_oauth("c1").await;

        assert!(
            connected.lock().unwrap().is_empty(),
            "a cancelled sign-in connects nothing"
        );
        assert!(
            store.saves.lock().unwrap().is_empty(),
            "a cancelled sign-in arms nothing"
        );
        assert_eq!(
            decision_frame(&mut rx).decision,
            CredentialDecisionKind::Deny
        );
        assert_eq!(
            n.dismissed_sign_ins.lock().unwrap().as_slice(),
            &["github_oauth".to_string()]
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

    #[tokio::test]
    async fn connect_integration_now_completes_an_oauth_sign_in_arms_and_connects() {
        let (s, n, _store, _rx, connected) =
            oauth_fixture(FakeFlow::polling(vec![crate::oauth::PollOutcome::Token(
                oauth_token(3600),
            )]));

        let ok = s.connect_integration_now("github_oauth").await;

        assert!(ok, "a completed sign-in reports connected");
        assert_eq!(
            connected.lock().unwrap().as_slice(),
            &["github_oauth".to_string()]
        );
        assert_eq!(
            s.current_state().get("github_oauth"),
            Some(&CredentialEntry::Oauth {
                access_token: "gho_access".into(),
                refresh_token: "ghr_refresh".into(),
                expires_at: 1000 + 3600,
            }),
            "the obtained token set is armed"
        );
        assert_eq!(n.sign_ins.lock().unwrap().len(), 1);
        assert_eq!(
            n.dismissed_sign_ins.lock().unwrap().as_slice(),
            &["github_oauth".to_string()]
        );
    }

    #[tokio::test]
    async fn connect_integration_now_returns_false_when_the_oauth_sign_in_is_denied() {
        let (s, _n, store, _rx, connected) =
            oauth_fixture(FakeFlow::polling(vec![crate::oauth::PollOutcome::Denied]));

        let ok = s.connect_integration_now("github_oauth").await;

        assert!(!ok);
        assert!(connected.lock().unwrap().is_empty());
        assert!(store.saves.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn connect_integration_now_connects_a_plain_credential_integration_without_a_sign_in() {
        let (s, n, _store, _rx, connected) = fixture_connectable(&["gitlab"]);

        let ok = s.connect_integration_now("gitlab").await;

        assert!(ok);
        assert_eq!(
            connected.lock().unwrap().as_slice(),
            &["gitlab".to_string()]
        );
        assert!(
            n.sign_ins.lock().unwrap().is_empty(),
            "a credential connect shows no device sign-in"
        );
    }

    #[tokio::test]
    async fn connect_integration_now_returns_false_for_an_unknown_id() {
        let (s, _n, _store, _rx, _c) = oauth_fixture(FakeFlow::polling(vec![]));
        assert!(!s.connect_integration_now("nope").await);
    }

    #[tokio::test]
    async fn connect_integration_now_also_releases_a_held_credential_prompt_for_the_same_integration()
     {
        let (s, n, _store, mut rx, _connected) =
            oauth_fixture(FakeFlow::polling(vec![crate::oauth::PollOutcome::Token(
                oauth_token(3600),
            )]));
        // A placeholder card for github_oauth is already held (the "use your GitHub access" prompt).
        s.submit_pending(pending("cred1", "github_oauth"), Instant::now());

        let ok = s.connect_integration_now("github_oauth").await;

        assert!(ok);
        let frame = decision_frame(&mut rx);
        assert_eq!(frame.id, "cred1");
        assert_eq!(
            frame.decision,
            CredentialDecisionKind::Allow,
            "arming the integration via a network offer also releases its held placeholder request"
        );
        assert!(
            n.dismissed.lock().unwrap().contains(&"cred1".to_string()),
            "and dismisses the duplicate placeholder card"
        );
    }

    #[test]
    fn connect_integration_with_token_arms_stored_connects_live_and_releases_held_requests() {
        let (s, n, store, mut rx, connected) = fixture_connectable(&["gitlab"]);
        // A placeholder card for gitlab is already held.
        s.submit_pending(pending("cred1", "gitlab"), Instant::now());

        let ok = s.connect_integration_with_token("gitlab", "glpat_pasted".into());

        assert!(ok, "a pasted token connects the integration");
        assert_eq!(
            connected.lock().unwrap().as_slice(),
            &["gitlab".to_string()],
            "the integration's routes are allowed live"
        );
        assert_eq!(
            store.saves.lock().unwrap().last().unwrap().get("gitlab"),
            Some(&CredentialEntry::Stored {
                value: "glpat_pasted".into(),
            }),
            "the pasted token is armed as a Stored value"
        );
        let frame = decision_frame(&mut rx);
        assert_eq!(frame.id, "cred1");
        assert_eq!(
            frame.decision,
            CredentialDecisionKind::Allow,
            "the held placeholder request is released once the token arms the slot"
        );
        assert!(n.dismissed.lock().unwrap().contains(&"cred1".to_string()));
    }

    fn armed_session(
        entries: Vec<(&str, CredentialEntry)>,
        custom: Vec<DefProvider>,
    ) -> (
        CredentialSession,
        Arc<RecordingNotifier>,
        mpsc::UnboundedReceiver<HostFrame>,
    ) {
        let notifier = Arc::new(RecordingNotifier::default());
        let store = Arc::new(CapturingStore::default());
        let (tx, rx) = mpsc::unbounded_channel();
        let mut state = CredentialStateFile::new();
        for (id, entry) in entries {
            state.insert(id.into(), entry);
        }
        let session = CredentialSession::new(state, notifier.clone(), store, tx, TEST_TIMEOUT)
            .with_custom_providers(Arc::new(custom));
        (session, notifier, rx)
    }

    fn gh_oauth_provider() -> DefProvider {
        use lns_policy::providers::{InjectionDef, InjectionKind, ProviderDef};
        DefProvider::new(ProviderDef {
            id: "github_oauth".into(),
            env_var: "GH_TOKEN".into(),
            placeholder: "gho_LNSPLACEHOLDER".into(),
            injections: vec![InjectionDef {
                kind: InjectionKind::TokenHeader,
                domain: "api.github.com".into(),
                header: None,
            }],
        })
    }

    fn armed_oauth(access_token: &str) -> CredentialEntry {
        CredentialEntry::Oauth {
            access_token: access_token.into(),
            refresh_token: String::new(),
            expires_at: 9_999_999_999,
        }
    }

    fn gate(credential_id: &str, action: &str) -> CredentialPending {
        CredentialPending {
            id: "c1".into(),
            credential_id: credential_id.into(),
            action: action.into(),
            reason: "placeholder-unauthorized".into(),
        }
    }

    #[test]
    fn submit_pending_auto_allows_an_armed_credential_on_a_host_it_injects_into() {
        let (s, n, mut rx) = armed_session(
            vec![("github_oauth", armed_oauth("gho_real"))],
            vec![gh_oauth_provider()],
        );
        s.submit_pending(gate("github_oauth", "GET api.github.com/"), Instant::now());
        assert!(
            n.presented.lock().unwrap().is_empty(),
            "an armed credential gated on a host it injects into is a propagation race, not a fresh consent — auto-allow without re-prompting"
        );
        assert_eq!(
            decision_frame(&mut rx).decision,
            CredentialDecisionKind::Allow
        );
    }

    #[test]
    fn submit_pending_still_prompts_an_armed_credential_on_a_host_it_does_not_inject_into() {
        let (s, n, _rx) = armed_session(
            vec![("github_oauth", armed_oauth("gho_real"))],
            vec![gh_oauth_provider()],
        );
        s.submit_pending(gate("github_oauth", "GET evil.example/"), Instant::now());
        assert_eq!(
            n.presented.lock().unwrap().len(),
            1,
            "sending the placeholder to a host with no injection is a real leak attempt and must still prompt"
        );
    }

    #[test]
    fn submit_pending_auto_allows_an_armed_builtin_stored_credential_on_its_host() {
        let (s, n, mut rx) = armed_session(
            vec![(
                "github",
                CredentialEntry::Stored {
                    value: "ghp_real".into(),
                },
            )],
            vec![],
        );
        s.submit_pending(gate("github", "GET api.github.com/"), Instant::now());
        assert!(n.presented.lock().unwrap().is_empty());
        assert_eq!(
            decision_frame(&mut rx).decision,
            CredentialDecisionKind::Allow
        );
    }

    #[test]
    fn submit_pending_prompts_when_the_armed_oauth_value_is_empty() {
        let (s, n, _rx) = armed_session(
            vec![("github_oauth", armed_oauth(""))],
            vec![gh_oauth_provider()],
        );
        s.submit_pending(gate("github_oauth", "GET api.github.com/"), Instant::now());
        assert_eq!(
            n.presented.lock().unwrap().len(),
            1,
            "an empty token is not a usable armed value"
        );
    }

    #[test]
    fn submit_pending_prompts_when_the_action_has_no_parseable_host() {
        let (s, n, _rx) = armed_session(
            vec![("github_oauth", armed_oauth("gho_real"))],
            vec![gh_oauth_provider()],
        );
        s.submit_pending(gate("github_oauth", "malformed"), Instant::now());
        assert_eq!(n.presented.lock().unwrap().len(), 1);
    }

    #[test]
    fn submit_pending_prompts_for_an_armed_credential_with_no_known_provider() {
        let (s, n, _rx) = armed_session(
            vec![("mystery", CredentialEntry::Stored { value: "v".into() })],
            vec![],
        );
        s.submit_pending(gate("mystery", "GET api.mystery.com/"), Instant::now());
        assert_eq!(
            n.presented.lock().unwrap().len(),
            1,
            "without a provider we can't know its injection domains, so we prompt"
        );
    }

    #[test]
    fn request_host_parses_method_target_forms() {
        assert_eq!(request_host("GET api.github.com/x"), Some("api.github.com"));
        assert_eq!(
            request_host("CONNECT api.github.com:443"),
            Some("api.github.com")
        );
        assert_eq!(request_host("POST github.com/graphql"), Some("github.com"));
        assert_eq!(request_host("malformed"), None);
        assert_eq!(request_host("GET /onlypath"), None);
    }

    #[test]
    fn injection_targets_host_matches_header_and_uri_placeholder_domains() {
        let header = CredentialInjection::Header {
            domain: "api.github.com".into(),
            header: "Authorization".into(),
            value: "token x".into(),
        };
        assert!(injection_targets_host(&header, "api.github.com"));
        assert!(!injection_targets_host(&header, "evil.example"));
        let uri = CredentialInjection::UriPlaceholder {
            domain: "api.telegram.org".into(),
            value: "v".into(),
        };
        assert!(injection_targets_host(&uri, "api.telegram.org"));
        assert!(!injection_targets_host(&uri, "evil.example"));
    }
}
