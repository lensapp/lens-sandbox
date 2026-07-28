//! The credential-rule source of truth lives in `~/.lns-credentials.json`, not `lns-policy.yaml`.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::approval_flow::protocol::{
    CredentialDecision, CredentialDecisionKind, CredentialInjection, CredentialPending, HostFrame,
};
use crate::credential_flow::providers::{DefProvider, Provider};
use crate::credential_flow::store::{CredentialEntry, CredentialStateFile, CredentialStore};
use crate::ledger::LedgerRecorder;
use lns_ipc::{ApprovalKind, AuthKind, Decision, LedgerEvent};
use lns_policy::connectors::TokenFallback;
use lns_policy::grants::{GrantRecord, GrantStore, GrantVerdict, WorkloadIdentity};

pub type FrameSink = mpsc::UnboundedSender<HostFrame>;

/// Invoked after a state-changing decision so a follow-up `Policy` frame arms the matching injection; receives the run's armed ids because a resolved value may only arm for a consented connector.
pub type PolicyEmitter = Box<dyn Fn(&CredentialStateFile, &HashSet<String>) + Send + Sync>;

/// Invoked when an un-connected catalog connector is accepted, to connect it live (allow its routes + persist `connectors:`).
pub type ConnectEmitter = Box<dyn Fn(&str) + Send + Sync>;

/// Opens a URL in the developer's browser; injected so the pkce flow drives the real opener in production and records it in tests.
pub type BrowserOpener = Box<dyn Fn(&str) + Send + Sync>;

/// Mints a fresh PKCE challenge; injected so tests can pin a known verifier and state.
pub type ChallengeGen = Box<dyn Fn() -> crate::oauth::PkceChallenge + Send + Sync>;

/// How long a pkce browser sign-in may run before the held request is failed closed.
pub const PKCE_SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialPendingPrompt {
    pub id: String,
    pub credential_id: String,
    pub action: String,
    /// Some(display name) when this is an oauth connector to connect via a browser sign-in, so the card offers "Connect to <name>" instead of a value field.
    pub oauth_display_name: Option<String>,
    /// Some when the connector declares a token fallback, so the consent card can also reveal "use a token instead".
    pub token_fallback: Option<TokenFallback>,
    pub env_var: Option<String>,
    pub injection_domains: Vec<String>,
    pub is_project_defined: bool,
    /// True when a value is already bound for this connector on this machine, so the card can grant that binding instead of demanding the secret again (or re-running a sign-in).
    pub bound_value_available: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignInPrompt {
    pub credential_id: String,
    pub display_name: String,
    /// Some(code) for a device sign-in the user types; None for a pkce sign-in that redirects through the browser with no code to enter.
    pub user_code: Option<String>,
    pub verification_uri: String,
    /// Some when the connector declares a token fallback, so the sign-in card can offer "use a token instead" to a user blocked from the browser dance.
    pub token_fallback: Option<TokenFallback>,
    pub env_var: Option<String>,
    pub injection_domains: Vec<String>,
    pub is_project_defined: bool,
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
        let code = prompt.user_code.as_deref().unwrap_or("");
        self.inform(&format!(
            "To connect {}, open {} {code}",
            prompt.display_name, prompt.verification_uri
        ));
    }
    /// Removes the sign-in card for `credential_id` once its flow resolves; the default is a no-op for notifiers that don't model one.
    fn dismiss_sign_in(&self, credential_id: &str) {
        let _ = credential_id;
    }
    /// Signals that an accepted consent's connect has resolved (either way), so any surface holding the card's slot can release it; default no-op.
    fn connect_finished(&self, display_name: &str) {
        let _ = display_name;
    }
    /// Drops every in-flight connect placeholder on run teardown so a sign-in that never resolves can't keep the window pinned; default no-op.
    fn clear_all_connecting(&self) {}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionOutcome {
    Resolved,
    UnknownId,
}

/// `Allow` binds a new value on this machine and `AllowBound` grants the one already bound; both are this workload's consent. `Deny` records a per-workload decline and `Timeout` records nothing (S12).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialDecisionRequest {
    Allow(CredentialEntry),
    AllowBound,
    Deny,
    Timeout,
}

struct PendingEntry {
    prompt_id: String,
    request_ids: Vec<String>,
    deadline: Instant,
}

struct GrantBinding {
    project: String,
    workload: WorkloadIdentity,
    store: Arc<dyn GrantStore>,
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
    bundled_ids: HashSet<String>,
    connectable: HashSet<String>,
    armed: Mutex<HashSet<String>>,
    slot_ids: HashSet<String>,
    grant_binding: Option<GrantBinding>,
    run_grants: Mutex<HashMap<String, GrantRecord>>,
    connect: ConnectEmitter,
    oauth_configs: HashMap<String, crate::oauth::OauthConfig>,
    oauth_display_names: HashMap<String, String>,
    token_fallbacks: HashMap<String, TokenFallback>,
    device_flow: Option<Arc<dyn crate::oauth::DeviceFlow>>,
    userinfo_fetcher: Option<Arc<dyn crate::oauth::UserInfoFetcher>>,
    clock: Option<Arc<dyn crate::oauth::Clock>>,
    pkce_configs: HashMap<String, crate::oauth::PkceConfig>,
    pkce_flow: Option<Arc<dyn crate::oauth::AuthCodeFlow>>,
    callback_listener: Option<Arc<dyn crate::oauth::CallbackListener>>,
    open_browser: BrowserOpener,
    pkce_challenge_gen: ChallengeGen,
    pkce_timeout: Duration,
    ledger: OnceLock<Arc<dyn LedgerRecorder>>,
}

impl CredentialSession {
    pub fn new(
        state: CredentialStateFile,
        notifier: Arc<dyn CredentialNotifier>,
        store: Arc<dyn CredentialStore>,
        sink: FrameSink,
        timeout: Duration,
    ) -> Self {
        Self::with_policy_emitter(state, notifier, store, sink, timeout, Box::new(|_, _| {}))
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
            bundled_ids: HashSet::new(),
            connectable: HashSet::new(),
            armed: Mutex::new(HashSet::new()),
            slot_ids: HashSet::new(),
            grant_binding: None,
            run_grants: Mutex::new(HashMap::new()),
            connect: Box::new(|_| {}),
            oauth_configs: HashMap::new(),
            oauth_display_names: HashMap::new(),
            token_fallbacks: HashMap::new(),
            device_flow: None,
            userinfo_fetcher: None,
            clock: None,
            pkce_configs: HashMap::new(),
            pkce_flow: None,
            callback_listener: None,
            open_browser: Box::new(|_| {}),
            pkce_challenge_gen: Box::new(crate::oauth::PkceChallenge::generate),
            pkce_timeout: PKCE_SIGN_IN_TIMEOUT,
            ledger: OnceLock::new(),
        }
    }

    pub fn set_ledger_recorder(&self, recorder: Arc<dyn LedgerRecorder>) {
        let _ = self.ledger.set(recorder);
    }

    /// Wires the device-flow engine and per-connector oauth configs so an accepted oauth prompt can run an interactive sign-in.
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

    /// Wires the userinfo fetcher so an accepted device sign-in can resolve the signed-in account; absent it, oauth connections record no account.
    pub fn with_userinfo_fetcher(
        mut self,
        fetcher: Arc<dyn crate::oauth::UserInfoFetcher>,
    ) -> Self {
        self.userinfo_fetcher = Some(fetcher);
        self
    }

    /// Wires the PKCE engine, per-connector pkce configs, the loopback callback listener, the browser opener, and the challenge generator so an accepted pkce prompt can run a browser-redirect sign-in.
    pub fn with_pkce(
        mut self,
        pkce_configs: HashMap<String, crate::oauth::PkceConfig>,
        pkce_flow: Arc<dyn crate::oauth::AuthCodeFlow>,
        callback_listener: Arc<dyn crate::oauth::CallbackListener>,
        open_browser: BrowserOpener,
        challenge_gen: ChallengeGen,
        timeout: Duration,
    ) -> Self {
        self.pkce_configs = pkce_configs;
        self.pkce_flow = Some(pkce_flow);
        self.callback_listener = Some(callback_listener);
        self.open_browser = open_browser;
        self.pkce_challenge_gen = challenge_gen;
        self.pkce_timeout = timeout;
        self
    }

    /// Per-id user-facing labels (e.g. `some-oauth` → "GitHub") for connect prompts and the sign-in card; ids absent here fall back to the id itself.
    pub fn with_oauth_display_names(mut self, display_names: HashMap<String, String>) -> Self {
        self.oauth_display_names = display_names;
        self
    }

    /// Per-id token fallbacks so a consent or sign-in card for a connector that declares one can offer "use a token instead".
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

    /// True when the connector authenticates by an interactive sign-in — a device grant or a pkce browser redirect — rather than a pasted value.
    fn is_signin(&self, credential_id: &str) -> bool {
        self.oauth_configs.contains_key(credential_id)
            || self.pkce_configs.contains_key(credential_id)
    }

    /// Captures the run's custom providers once at construction so a mid-run policy edit can't retroactively change a running workload's placeholder set.
    pub fn with_custom_providers(mut self, custom_providers: Arc<Vec<DefProvider>>) -> Self {
        self.custom_providers = custom_providers;
        self
    }

    pub fn with_bundled_ids(mut self, bundled_ids: HashSet<String>) -> Self {
        self.bundled_ids = bundled_ids;
        self
    }

    /// The ids consented for this run at launch — the overlay's connected connectors and the definition's credential slots; only these may arm a machine-stored value, and a live connect grants more.
    pub fn with_armed_ids(mut self, armed: HashSet<String>) -> Self {
        self.armed = Mutex::new(armed);
        self
    }

    /// The run's artifact-declared credential slot ids, which `lns-policy.yaml` has no authority over — a policy reload retains their live arming rather than revoking it, whether the slot was granted at boot or consented at first use.
    pub fn with_slot_ids(mut self, slots: HashSet<String>) -> Self {
        self.slot_ids = slots;
        self
    }

    /// Binds this run's grant context so an accepted or declined credential decision is remembered per (project, workload, connector) in the machine-local sidecar; run-local memory is seeded with this workload's own sidecar records so a prior deny suppresses the offer, and without a binding a decision is remembered for the run alone.
    pub fn with_grants(
        mut self,
        project: String,
        workload: WorkloadIdentity,
        store: Arc<dyn GrantStore>,
    ) -> Self {
        let key = workload.key();
        let seeded = store
            .load()
            .unwrap_or_default()
            .grants
            .into_iter()
            .filter(|g| g.project == project && g.workload == key)
            .map(|g| (g.connector.clone(), g))
            .collect();
        self.run_grants = Mutex::new(seeded);
        self.grant_binding = Some(GrantBinding {
            project,
            workload,
            store,
        });
        self
    }

    pub fn armed_ids(&self) -> HashSet<String> {
        self.armed.lock().expect("armed mutex poisoned").clone()
    }

    /// Reconcile the armed set against a reloaded policy without erasing this run's live consent: keep an id the reload still lists or that names a credential slot (a live connect isn't self-revoked by its own persist, and the policy file can't revoke an artifact slot), re-arm the ids the reloaded policy grants, and drop everything else (a `lns connector disconnect`) — so a reload can revoke arming but never re-arm a connector this workload never granted.
    pub fn reconcile_armed(&self, granted: &[String], reloaded: &[String]) {
        let reloaded_set: HashSet<&str> = reloaded.iter().map(String::as_str).collect();
        let mut armed = self.armed.lock().expect("armed mutex poisoned");
        armed.retain(|id| reloaded_set.contains(id.as_str()) || self.slot_ids.contains(id));
        armed.extend(granted.iter().cloned());
    }

    fn grant_armed(&self, credential_id: &str) {
        self.armed
            .lock()
            .expect("armed mutex poisoned")
            .insert(credential_id.to_string());
    }

    /// True when this workload holds a standing deny grant for the connector whose recorded disclosure still describes it, so a held request fails without re-prompting for the rest of this run and future runs; a redefinition invalidates the deny exactly as it invalidates an allow.
    fn workload_denied(&self, credential_id: &str) -> bool {
        let grant = self
            .run_grants
            .lock()
            .expect("run grants mutex poisoned")
            .get(credential_id)
            .cloned();
        let Some(grant) = grant.filter(|g| g.verdict == GrantVerdict::Deny) else {
            return false;
        };
        if grant.has_no_disclosure() {
            return true;
        }
        match self.provider_disclosure(credential_id) {
            (Some(env_var), domains, _) => grant.matches_disclosure(&env_var, &domains),
            _ => true,
        }
    }

    /// This run's accept/decline as a grant record, unless it is an allow the provider discloses no injection to pin it to (a deny, being a standing "no", needs none); an unbound session's record carries empty keys nothing ever reads — run-local memory keys by connector alone.
    fn grant_record_for(&self, credential_id: &str, verdict: GrantVerdict) -> Option<GrantRecord> {
        let (env_var, injection_domains) = match self.provider_disclosure(credential_id) {
            (Some(env_var), domains, _) => (env_var, domains),
            _ if verdict == GrantVerdict::Deny => (String::new(), Vec::new()),
            _ => return None,
        };
        let (project, workload) = match &self.grant_binding {
            Some(b) => (b.project.clone(), b.workload.key()),
            None => (String::new(), String::new()),
        };
        Some(GrantRecord {
            project,
            workload,
            connector: credential_id.to_string(),
            verdict,
            env_var,
            injection_domains,
        })
    }

    /// Remembers this run's accept/decline for `credential_id` in run-local memory, so a decline suppresses the re-prompt whether or not a sidecar is bound and writable; the returned record is what [`Self::persist_grant`] later carries to the sidecar.
    fn remember_grant_for_the_run(
        &self,
        credential_id: &str,
        verdict: GrantVerdict,
    ) -> Option<GrantRecord> {
        let record = self.grant_record_for(credential_id, verdict)?;
        self.run_grants
            .lock()
            .expect("run grants mutex poisoned")
            .insert(credential_id.to_string(), record.clone());
        Some(record)
    }

    /// Writes a remembered grant to the machine-local sidecar so a later run skips the offer; callers whose decision also writes a value hold this back until that write lands, or the grant would outlive the value it was given for.
    fn persist_grant(&self, record: Option<GrantRecord>) {
        let (Some(record), Some(binding)) = (record, &self.grant_binding) else {
            return;
        };
        if let Err(e) = binding.store.update(&mut |file| {
            file.upsert(record.clone());
            true
        }) {
            self.notifier.inform(&format!(
                "credential decision applied but its grant was not persisted; the next run will ask again: {e}"
            ));
        }
    }

    /// Remembers a decision that writes no value of its own — a decline, or a grant of what is already bound on this machine — so nothing gates its durability.
    fn remember_grant(&self, credential_id: &str, verdict: GrantVerdict) {
        let record = self.remember_grant_for_the_run(credential_id, verdict);
        self.persist_grant(record);
    }

    fn is_armed_id(&self, credential_id: &str) -> bool {
        self.armed
            .lock()
            .expect("armed mutex poisoned")
            .contains(credential_id)
    }

    /// Marks the catalog connectors that aren't connected yet: detecting one offers to connect, and accepting runs `connect` to allow its routes live.
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
            .is_signin(&req.credential_id)
            .then(|| self.display_name_for(&req.credential_id));
        let token_fallback = self.token_fallbacks.get(&req.credential_id).cloned();
        let action = if self.connectable.contains(&req.credential_id) {
            format!("connect to {}", req.credential_id)
        } else {
            req.action
        };
        let (env_var, injection_domains, is_project_defined) =
            self.provider_disclosure(&req.credential_id);
        // Only a value bound in the store counts: a host-detect entry resolves to the very value the card's detected-value offer already spends.
        let bound_value_available = self.has_armed_value(&req.credential_id);
        self.notifier.present(&CredentialPendingPrompt {
            id: req.id,
            credential_id: req.credential_id,
            action,
            token_fallback,
            oauth_display_name,
            env_var,
            injection_domains,
            is_project_defined,
            bound_value_available,
        });
    }

    fn is_denied(&self, credential_id: &str) -> bool {
        self.workload_denied(credential_id)
            || matches!(
                self.state
                    .lock()
                    .expect("state mutex poisoned")
                    .get(credential_id),
                Some(CredentialEntry::Deny)
            )
    }

    fn provider_disclosure(&self, credential_id: &str) -> (Option<String>, Vec<String>, bool) {
        self.custom_providers
            .iter()
            .find(|p| p.id() == credential_id)
            .map(|p| {
                let (env_var, domains) = p.disclosure_snapshot();
                let is_project = !self.bundled_ids.contains(p.id());
                (Some(env_var), domains, is_project)
            })
            .unwrap_or((None, Vec::new(), false))
    }

    /// True when `credential_id` is consented for this run, already holds a usable value, and injects into the host named in `action`, so a gate for it is a propagation race the host can safely allow rather than re-prompt. An unconsented id (a declared or connectable connector with a machine-stored value) still prompts, and so does a host the credential does not inject into (a real leak attempt).
    fn is_armed_for_request(&self, credential_id: &str, action: &str) -> bool {
        if !self.is_armed_id(credential_id) || !self.has_armed_value(credential_id) {
            return false;
        }
        request_host(action).is_some_and(|host| self.injects_for_host(credential_id, host))
    }

    fn has_armed_value(&self, credential_id: &str) -> bool {
        lns_policy::credentials::has_armed_entry(
            &self.state.lock().expect("state mutex poisoned"),
            credential_id,
        )
    }

    /// True when the wire expansion would arm this id: a stored/oauth value, or a host-detect entry whose host source resolves to a non-empty value — the same signal the policy emitter uses, so the release tracks what actually landed on the wire.
    fn has_resolvable_value(&self, credential_id: &str) -> bool {
        let host_detect = matches!(
            self.state
                .lock()
                .expect("state mutex poisoned")
                .get(credential_id),
            Some(CredentialEntry::HostDetect)
        );
        if host_detect {
            return crate::credential_flow::registry::detect_for_with(
                credential_id,
                &self.custom_providers,
            )
            .is_some_and(|v| !v.is_empty());
        }
        self.has_armed_value(credential_id)
    }

    fn injects_for_host(&self, credential_id: &str, host: &str) -> bool {
        self.custom_providers
            .iter()
            .map(|p| p as &dyn Provider)
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
        match &request {
            CredentialDecisionRequest::Allow(_) | CredentialDecisionRequest::AllowBound => {
                self.record_credential_approval(&credential_id, Decision::Allow)
            }
            CredentialDecisionRequest::Deny => {
                self.record_credential_approval(&credential_id, Decision::Deny)
            }
            CredentialDecisionRequest::Timeout => {}
        }
        match request {
            CredentialDecisionRequest::Allow(entry) => {
                // Answering the first-use card Allow is this run's consent, so arm the id whether or not it was connectable; a value never arms without a card being answered.
                self.grant_armed(&credential_id);
                let grant = self.remember_grant_for_the_run(&credential_id, GrantVerdict::Allow);
                // Apply and emit the armed value before the connect releases network holds, so a released request never reaches the placeholder gate ahead of its injection.
                if self.apply_persistent_entry(credential_id.clone(), entry) {
                    self.persist_grant(grant);
                }
                if self.connectable.contains(&credential_id) {
                    (self.connect)(&credential_id);
                }
            }
            // Granting the binding already on this machine is consent to spend it, not a decision to rebind: the stored entry is left exactly as `lns connector connect` wrote it and only the armed set changes.
            CredentialDecisionRequest::AllowBound => {
                self.grant_armed(&credential_id);
                self.remember_grant(&credential_id, GrantVerdict::Allow);
                self.record_bound_credential_event(&credential_id);
                self.emit_armed_state();
                if self.connectable.contains(&credential_id) {
                    (self.connect)(&credential_id);
                }
            }
            // A declined card is remembered per-workload in the grant sidecar, not as a machine-wide credential Deny that would block the connector for every project.
            CredentialDecisionRequest::Deny => {
                self.remember_grant(&credential_id, GrantVerdict::Deny);
            }
            CredentialDecisionRequest::Timeout => {}
        }
        for request_id in &request_ids {
            self.send_decision_frame(request_id, kind);
        }
        DecisionOutcome::Resolved
    }

    /// True when a held prompt belongs to an oauth connector, so its acceptance must drive a device sign-in rather than a static value decision.
    pub fn is_oauth_prompt(&self, prompt_id: &str) -> bool {
        let pending = self.pending.lock().expect("pending mutex poisoned");
        pending
            .iter()
            .any(|(id, e)| e.prompt_id == prompt_id && self.is_signin(id))
    }

    /// Drives a device sign-in for an accepted oauth prompt: on success connects the connector live and arms the token set, releasing held requests; denial, expiry, or error fails them closed without persisting (the next use re-prompts).
    pub async fn connect_oauth(&self, prompt_id: &str) -> DecisionOutcome {
        let Some((credential_id, request_ids)) = self.remove_pending(prompt_id) else {
            return DecisionOutcome::UnknownId;
        };
        self.notifier.dismiss(prompt_id);
        let connected = self.run_signin_connect(&credential_id).await;
        self.notifier
            .connect_finished(&self.display_name_for(&credential_id));
        self.record_credential_approval(
            &credential_id,
            if connected {
                Decision::Allow
            } else {
                Decision::Deny
            },
        );
        if connected {
            for request_id in &request_ids {
                self.send_decision_frame(request_id, CredentialDecisionKind::Allow);
            }
        } else {
            self.fail_held(&request_ids);
        }
        DecisionOutcome::Resolved
    }

    /// Connects connector `id` outside the held-credential flow (e.g. accepting a network offer): a device sign-in for an oauth id, a straight route-allow for a plain connectable id; returns whether it is now connected.
    pub async fn connect_connector_now(&self, id: &str) -> bool {
        if self.is_signin(id) {
            let ok = self.run_signin_connect(id).await;
            if ok {
                // The same connect arms the token, so any placeholder card already held for this connector is satisfied too — don't ask for it separately.
                self.release_armed_holds(id);
            }
            return ok;
        }
        if self.connectable.contains(id) {
            self.grant_armed(id);
            self.remember_grant(id, GrantVerdict::Allow);
            (self.connect)(id);
            // The connect armed a value the wire expansion resolves (stored, oauth, or a host-detect that the host supplies), so a held value card asks an answered question; with nothing resolvable the card stays — that decision is still owed.
            if self.has_resolvable_value(id) {
                self.release_armed_holds(id);
            }
            return true;
        }
        false
    }

    /// Connects connector `id` from a network offer using a pasted token instead of the interactive sign-in: arms the value in its slot, allows its routes (if connectable), and releases any placeholder card already held for it. Always reports connected.
    pub fn connect_connector_with_token(&self, id: &str, value: String) -> bool {
        self.arm_connected(id, CredentialEntry::Stored { value });
        self.release_armed_holds(id);
        true
    }

    /// Allows and dismisses a held credential prompt for `credential_id` once another surface (a network offer) has armed the connector; a no-op when nothing is held for it.
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

    /// Routes an accepted interactive sign-in to its flow: the pkce browser redirect when one is wired for `credential_id`, otherwise the device grant.
    async fn run_signin_connect(&self, credential_id: &str) -> bool {
        if let (Some(cfg), Some(flow), Some(listener)) = (
            self.pkce_configs.get(credential_id),
            self.pkce_flow.as_ref(),
            self.callback_listener.as_ref(),
        ) {
            return self
                .run_pkce_connect(credential_id, cfg, flow.as_ref(), listener.as_ref())
                .await;
        }
        self.run_oauth_connect(credential_id).await
    }

    /// Runs the pkce browser-redirect sign-in and, on success, arms the obtained key as a durable credential and connects the connector live; returns whether a key was obtained. Cancel, timeout, or error yields false.
    async fn run_pkce_connect(
        &self,
        credential_id: &str,
        cfg: &crate::oauth::PkceConfig,
        flow: &dyn crate::oauth::AuthCodeFlow,
        listener: &dyn crate::oauth::CallbackListener,
    ) -> bool {
        let display_name = self.display_name_for(credential_id);
        let token_fallback = self.token_fallbacks.get(credential_id).cloned();
        let (env_var, injection_domains, is_project_defined) =
            self.provider_disclosure(credential_id);
        let challenge = (self.pkce_challenge_gen)();
        let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<crate::oauth::SignInPivot>();
        let card_id = credential_id.to_string();
        let on_authorization_url = move |auth_url: &str| {
            self.notifier.present_sign_in(
                &SignInPrompt {
                    credential_id: card_id,
                    display_name,
                    user_code: None,
                    verification_uri: auth_url.to_string(),
                    token_fallback,
                    env_var,
                    injection_domains,
                    is_project_defined,
                },
                cancel_tx,
            );
            (self.open_browser)(auth_url);
        };
        let cancel = async move {
            // A dropped sender means the notifier has no cancel surface, so the sign-in runs to its natural end.
            if cancel_rx.await.is_err() {
                std::future::pending::<()>().await;
            }
        };
        let result = crate::oauth::run_pkce_flow(
            flow,
            listener,
            cfg,
            &challenge,
            on_authorization_url,
            cancel,
            self.pkce_timeout,
        )
        .await;
        self.notifier.dismiss_sign_in(credential_id);
        match result {
            Ok(crate::oauth::PkceSignIn::Completed(key)) => {
                self.arm_connected(credential_id, CredentialEntry::Stored { value: key });
                true
            }
            Ok(crate::oauth::PkceSignIn::Cancelled | crate::oauth::PkceSignIn::TimedOut) => false,
            Err(e) => {
                self.notifier
                    .inform(&format!("sign-in to {credential_id} failed: {e:#}"));
                false
            }
        }
    }

    /// Runs the device sign-in for an oauth connector and, on success, arms the token set and connects it live; returns whether a token was obtained. A missing config, denial, expiry, cancel, or error yields false.
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
        let (env_var, injection_domains, is_project_defined) =
            self.provider_disclosure(credential_id);
        let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<crate::oauth::SignInPivot>();
        let card_id = credential_id.to_string();
        let present = move |code: &crate::oauth::DeviceCode| {
            self.notifier.present_sign_in(
                &SignInPrompt {
                    credential_id: card_id,
                    display_name,
                    user_code: Some(code.user_code.clone()),
                    verification_uri: code.verification_uri.clone(),
                    token_fallback,
                    env_var,
                    injection_domains,
                    is_project_defined,
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
                let token = self.resolve_account(cfg, token).await;
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

    async fn resolve_account(
        &self,
        cfg: &crate::oauth::OauthConfig,
        token: crate::oauth::TokenSet,
    ) -> crate::oauth::TokenSet {
        match &self.userinfo_fetcher {
            Some(fetcher) => crate::oauth::resolve_account(fetcher.as_ref(), cfg, token).await,
            None => token,
        }
    }

    /// Connects the connector live (if it isn't already) and arms `entry` in its credential slot — the shared tail of every successful connect, whether by completed sign-in or pasted token. The value is applied and emitted before the connect, because the connect releases network holds and the released request must not reach the placeholder gate before the armed injection is in hand.
    fn arm_connected(&self, credential_id: &str, entry: CredentialEntry) {
        self.grant_armed(credential_id);
        let grant = self.remember_grant_for_the_run(credential_id, GrantVerdict::Allow);
        if self.apply_persistent_entry(credential_id.to_string(), entry) {
            self.persist_grant(grant);
        }
        if self.connectable.contains(credential_id) {
            (self.connect)(credential_id);
        }
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

    /// Re-emits the wire snapshot for an unchanged store, so a decision that only widened the armed set still reaches the boundary with the injection in hand.
    fn emit_armed_state(&self) {
        let snapshot = self.state.lock().expect("state mutex poisoned").clone();
        (self.policy_emitter)(&snapshot, &self.armed_ids());
    }

    /// Reports whether the entry reached the store, so a caller can hold back a grant that would otherwise outlive the value it was given for.
    fn apply_persistent_entry(&self, credential_id: String, entry: CredentialEntry) -> bool {
        self.record_credential_event(&credential_id, &entry);
        let snapshot = {
            let mut state = self.state.lock().expect("state mutex poisoned");
            state.insert(credential_id, entry);
            state.clone()
        };
        let persisted = match self.store.save(&snapshot) {
            Ok(()) => true,
            Err(e) => {
                self.notifier.inform(&format!(
                    "credential rule applied in-memory but not persisted: {e}"
                ));
                false
            }
        };
        // Policy frame goes out even on a failed write so the held request that triggered the decision isn't stalled (S14).
        (self.policy_emitter)(&snapshot, &self.armed_ids());
        persisted
    }

    /// Records the ledger event for the value already bound, so granting an existing binding leaves the same audit trail as a decision that binds one.
    fn record_bound_credential_event(&self, credential_id: &str) {
        let bound = self
            .state
            .lock()
            .expect("state mutex poisoned")
            .get(credential_id)
            .cloned();
        if let Some(entry) = bound {
            self.record_credential_event(credential_id, &entry);
        }
    }

    fn record_credential_approval(&self, credential_id: &str, decision: Decision) {
        let Some(recorder) = self.ledger.get() else {
            return;
        };
        recorder.record(LedgerEvent::Approval {
            kind: ApprovalKind::Credential,
            target: credential_id.to_string(),
            decision,
            reason: None,
            connector: Some(credential_id.to_string()),
        });
    }

    fn record_credential_event(&self, credential_id: &str, entry: &CredentialEntry) {
        let Some(recorder) = self.ledger.get() else {
            return;
        };
        if let Some(event) = self.credential_event(credential_id, entry) {
            recorder.record(event);
        }
    }

    fn credential_event(
        &self,
        credential_id: &str,
        entry: &CredentialEntry,
    ) -> Option<LedgerEvent> {
        match entry {
            CredentialEntry::Stored { value } if !value.is_empty() => {
                let (_, domains, _) = self.provider_disclosure(credential_id);
                Some(LedgerEvent::CredentialUse {
                    connector: credential_id.to_string(),
                    auth: AuthKind::Apikey,
                    fp: Some(lns_ipc::fingerprint(value)),
                    dest: domains,
                })
            }
            CredentialEntry::Oauth {
                access_token,
                expires_at,
                scopes,
                account,
                ..
            } if !access_token.is_empty() => Some(LedgerEvent::Connection {
                connector: credential_id.to_string(),
                auth: AuthKind::Oauth,
                account: account.clone(),
                scopes: if scopes.is_empty() {
                    self.scopes_for(credential_id)
                } else {
                    scopes.clone()
                },
                expires: (*expires_at != 0)
                    .then(|| crate::time_fmt::rfc3339_from_unix(*expires_at)),
            }),
            _ => None,
        }
    }

    fn scopes_for(&self, credential_id: &str) -> Vec<String> {
        self.oauth_configs
            .get(credential_id)
            .map(|cfg| cfg.scopes.clone())
            .unwrap_or_default()
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
        let Some((credential_id, request_ids)) = self.remove_pending(prompt_id) else {
            return false;
        };
        self.notifier.dismiss(prompt_id);
        // A consent clicked into a sign-in right as it expired leaves a placeholder no later sign-in will resolve; take it down with the prompt.
        if self.is_signin(&credential_id) {
            self.notifier
                .connect_finished(&self.display_name_for(&credential_id));
        }
        for request_id in &request_ids {
            self.send_decision_frame(request_id, CredentialDecisionKind::Timeout);
        }
        true
    }

    /// Also emits a fresh Policy frame so the MITM picks up the new arming/revocation (S10).
    pub fn apply_external_state(&self, new_state: CredentialStateFile) {
        *self.state.lock().expect("state mutex poisoned") = new_state.clone();
        (self.policy_emitter)(&new_state, &self.armed_ids());
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
        self.notifier.clear_all_connecting();
    }
}

fn decision_kind_of(request: &CredentialDecisionRequest) -> CredentialDecisionKind {
    match request {
        CredentialDecisionRequest::Allow(_) | CredentialDecisionRequest::AllowBound => {
            CredentialDecisionKind::Allow
        }
        CredentialDecisionRequest::Deny => CredentialDecisionKind::Deny,
        CredentialDecisionRequest::Timeout => CredentialDecisionKind::Timeout,
    }
}

/// The host an outbound request targets, parsed from a gate `action` like `POST https://api.some-oauth.example/x` or `CONNECT api.some-oauth.example:443`.
fn request_host(action: &str) -> Option<&str> {
    let target = action.split_whitespace().nth(1)?;
    let authority = target.split_once("://").map_or(target, |(_, rest)| rest);
    let host = authority.split(['/', ':']).next()?;
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
    use crate::approval_flow::session::tests::CapturingRecorder;
    use lns_policy::grants::WorkloadGrantFile;
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
        connects_finished: StdMutex<Vec<String>>,
        all_connecting_cleared: StdMutex<usize>,
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
        fn connect_finished(&self, display_name: &str) {
            self.connects_finished
                .lock()
                .unwrap()
                .push(display_name.to_string());
        }
        fn clear_all_connecting(&self) {
            *self.all_connecting_cleared.lock().unwrap() += 1;
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
    fn arming_a_stored_value_records_a_credential_use_with_a_fingerprint() {
        let (s, _n, _store, _rx) = fixture();
        let recorder = Arc::new(CapturingRecorder::default());
        s.set_ledger_recorder(recorder.clone());
        s.apply_persistent_entry(
            "some-provider".into(),
            CredentialEntry::Stored {
                value: "sk-secret".into(),
            },
        );
        assert_eq!(
            *recorder.events.lock().unwrap().first().expect("one event"),
            LedgerEvent::CredentialUse {
                connector: "some-provider".into(),
                auth: AuthKind::Apikey,
                fp: Some(lns_ipc::fingerprint("sk-secret")),
                dest: vec![],
            }
        );
    }

    #[test]
    fn arming_an_oauth_token_records_a_connection_with_its_expiry() {
        let (s, _n, _store, _rx) = fixture();
        let recorder = Arc::new(CapturingRecorder::default());
        s.set_ledger_recorder(recorder.clone());
        s.apply_persistent_entry(
            "some-oauth".into(),
            CredentialEntry::Oauth {
                scopes: Vec::new(),
                account: None,
                access_token: "tok".into(),
                refresh_token: "r".into(),
                expires_at: 1_735_689_600,
            },
        );
        assert_eq!(
            *recorder.events.lock().unwrap().first().expect("one event"),
            LedgerEvent::Connection {
                connector: "some-oauth".into(),
                auth: AuthKind::Oauth,
                account: None,
                scopes: vec![],
                expires: Some(crate::time_fmt::rfc3339_from_unix(1_735_689_600)),
            }
        );
    }

    #[test]
    fn an_oauth_grant_with_no_known_expiry_omits_the_expires_field() {
        let (s, _n, _store, _rx) = fixture();
        let recorder = Arc::new(CapturingRecorder::default());
        s.set_ledger_recorder(recorder.clone());
        s.apply_persistent_entry(
            "some-oauth".into(),
            CredentialEntry::Oauth {
                scopes: Vec::new(),
                account: None,
                access_token: "tok".into(),
                refresh_token: "r".into(),
                expires_at: 0,
            },
        );
        assert_eq!(
            *recorder.events.lock().unwrap().first().expect("one event"),
            LedgerEvent::Connection {
                connector: "some-oauth".into(),
                auth: AuthKind::Oauth,
                account: None,
                scopes: vec![],
                expires: None,
            },
            "a 0 expiry means long-lived, so the ledger omits expires rather than recording the epoch"
        );
    }

    #[test]
    fn a_denied_credential_records_nothing() {
        let (s, _n, _store, _rx) = fixture();
        let recorder = Arc::new(CapturingRecorder::default());
        s.set_ledger_recorder(recorder.clone());
        s.apply_persistent_entry("some-provider".into(), CredentialEntry::Deny);
        assert!(recorder.events.lock().unwrap().is_empty());
    }

    #[test]
    fn an_oauth_connection_carries_the_connectors_configured_scopes() {
        let notifier = Arc::new(RecordingNotifier::default());
        let (tx, _rx) = mpsc::unbounded_channel();
        let configs = HashMap::from([(
            "some-oauth".to_string(),
            crate::oauth::OauthConfig {
                userinfo_endpoint: None,
                account_field: None,
                client_id: "Iv1.test".into(),
                client_secret: String::new(),
                scopes: vec!["repo".into(), "read:org".into()],
                device_authorization_endpoint: "https://example.com/device/code".into(),
                token_endpoint: "https://example.com/oauth/token".into(),
            },
        )]);
        let s = CredentialSession::new(
            CredentialStateFile::new(),
            notifier,
            Arc::new(CapturingStore::default()),
            tx,
            TEST_TIMEOUT,
        )
        .with_oauth(configs, FakeFlow::polling(vec![]), Arc::new(FixedClock(0)));
        let recorder = Arc::new(CapturingRecorder::default());
        s.set_ledger_recorder(recorder.clone());
        s.apply_persistent_entry(
            "some-oauth".into(),
            CredentialEntry::Oauth {
                scopes: Vec::new(),
                account: None,
                access_token: "tok".into(),
                refresh_token: "r".into(),
                expires_at: 1_735_689_600,
            },
        );
        assert_eq!(
            *recorder.events.lock().unwrap().first().expect("one event"),
            LedgerEvent::Connection {
                connector: "some-oauth".into(),
                auth: AuthKind::Oauth,
                account: None,
                scopes: vec!["repo".into(), "read:org".into()],
                expires: Some(crate::time_fmt::rfc3339_from_unix(1_735_689_600)),
            }
        );
    }

    #[test]
    fn an_oauth_connection_records_its_granted_scopes_and_resolved_account() {
        let (s, _n, _store, _rx) = fixture();
        let recorder = Arc::new(CapturingRecorder::default());
        s.set_ledger_recorder(recorder.clone());
        s.apply_persistent_entry(
            "some-oauth".into(),
            CredentialEntry::Oauth {
                access_token: "tok".into(),
                refresh_token: "r".into(),
                expires_at: 1_735_689_600,
                scopes: vec!["repo".into(), "read:org".into()],
                account: Some("@hchen".into()),
            },
        );
        assert_eq!(
            *recorder.events.lock().unwrap().first().expect("one event"),
            LedgerEvent::Connection {
                connector: "some-oauth".into(),
                auth: AuthKind::Oauth,
                account: Some("@hchen".into()),
                scopes: vec!["repo".into(), "read:org".into()],
                expires: Some(crate::time_fmt::rfc3339_from_unix(1_735_689_600)),
            }
        );
    }

    #[test]
    fn a_credential_allow_records_a_credential_approval() {
        let (s, _n, _store, _rx) = fixture();
        let recorder = Arc::new(CapturingRecorder::default());
        s.set_ledger_recorder(recorder.clone());
        s.submit_pending(pending("c1", "some-provider"), Instant::now());
        s.record_decision(
            "c1",
            CredentialDecisionRequest::Allow(CredentialEntry::HostDetect),
        );
        assert_eq!(
            *recorder.events.lock().unwrap().first().expect("one event"),
            LedgerEvent::Approval {
                kind: ApprovalKind::Credential,
                target: "some-provider".into(),
                decision: Decision::Allow,
                reason: None,
                connector: Some("some-provider".into()),
            }
        );
    }

    #[test]
    fn allowing_a_first_use_card_arms_an_ungranted_connector_for_the_run() {
        let notifier = Arc::new(RecordingNotifier::default());
        let store = Arc::new(CapturingStore::default());
        let (tx, _rx) = mpsc::unbounded_channel();
        let session = CredentialSession::new(
            CredentialStateFile::new(),
            notifier,
            store,
            tx,
            TEST_TIMEOUT,
        )
        .with_custom_providers(Arc::new(vec![some_provider()]));
        assert!(
            !session.armed_ids().contains("some-provider"),
            "an applied-but-ungranted id starts unarmed"
        );
        session.submit_pending(pending("c1", "some-provider"), Instant::now());
        session.record_decision(
            "c1",
            CredentialDecisionRequest::Allow(CredentialEntry::Stored {
                value: "some-real".into(),
            }),
        );
        assert!(
            session.armed_ids().contains("some-provider"),
            "answering the first-use card Allow is this run's consent, so the id must arm even though it was neither connectable nor granted at boot"
        );
        assert!(
            wire_injections(&session, "some-provider")
                .iter()
                .any(|i| matches!(
                    i,
                    CredentialInjection::Header { value, .. } if value.contains("some-real")
                )),
            "the consented value reaches the wire after Allow"
        );
    }

    #[derive(Default)]
    struct CapturingGrantStore {
        saved: StdMutex<lns_policy::grants::WorkloadGrantFile>,
    }
    impl GrantStore for CapturingGrantStore {
        fn load(&self) -> std::io::Result<lns_policy::grants::WorkloadGrantFile> {
            Ok(self.saved.lock().unwrap().clone())
        }
        fn save(&self, s: &lns_policy::grants::WorkloadGrantFile) -> std::io::Result<()> {
            *self.saved.lock().unwrap() = s.clone();
            Ok(())
        }
    }

    struct FailingGrantStore;
    impl GrantStore for FailingGrantStore {
        fn load(&self) -> std::io::Result<lns_policy::grants::WorkloadGrantFile> {
            Ok(lns_policy::grants::WorkloadGrantFile::default())
        }
        fn save(&self, _: &lns_policy::grants::WorkloadGrantFile) -> std::io::Result<()> {
            Err(std::io::Error::other("sidecar unwritable"))
        }
    }

    fn grants_session(
        store: Arc<dyn GrantStore>,
        workload: WorkloadIdentity,
        custom: Vec<DefProvider>,
    ) -> CredentialSession {
        let (tx, _rx) = mpsc::unbounded_channel();
        CredentialSession::new(
            CredentialStateFile::new(),
            Arc::new(RecordingNotifier::default()),
            Arc::new(CapturingStore::default()),
            tx,
            TEST_TIMEOUT,
        )
        .with_custom_providers(Arc::new(custom))
        .with_grants("proj".into(), workload, store)
    }

    #[test]
    fn a_consent_allow_persists_an_allow_grant_to_the_sidecar() {
        let store = Arc::new(CapturingGrantStore::default());
        let workload = WorkloadIdentity::Definition {
            dir: "/proj".into(),
        };
        let session = grants_session(store.clone(), workload.clone(), vec![some_provider()]);
        session.submit_pending(pending("c1", "some-provider"), Instant::now());
        session.record_decision(
            "c1",
            CredentialDecisionRequest::Allow(CredentialEntry::Stored {
                value: "some-real".into(),
            }),
        );
        let persisted = store.load().unwrap();
        let grant = persisted
            .lookup("proj", &workload, "some-provider")
            .expect("the consent must persist a grant so the next run skips the offer");
        assert_eq!(grant.verdict, lns_policy::grants::GrantVerdict::Allow);
        assert_eq!(grant.env_var, "SOME_TOKEN");
        assert_eq!(grant.injection_domains, ["api.some-provider.example"]);
    }

    #[test]
    fn a_grant_persist_failure_informs_the_user_but_still_arms_for_the_run() {
        let notifier = Arc::new(RecordingNotifier::default());
        let (tx, _rx) = mpsc::unbounded_channel();
        let session = CredentialSession::new(
            CredentialStateFile::new(),
            notifier.clone(),
            Arc::new(CapturingStore::default()),
            tx,
            TEST_TIMEOUT,
        )
        .with_custom_providers(Arc::new(vec![some_provider()]))
        .with_grants(
            "proj".into(),
            WorkloadIdentity::Definition {
                dir: "/proj".into(),
            },
            Arc::new(FailingGrantStore),
        );
        session.submit_pending(pending("c1", "some-provider"), Instant::now());
        session.record_decision(
            "c1",
            CredentialDecisionRequest::Allow(CredentialEntry::Stored {
                value: "some-real".into(),
            }),
        );
        assert!(
            session.armed_ids().contains("some-provider"),
            "consent still arms for this run even when the sidecar can't be written"
        );
        assert!(
            notifier
                .informed
                .lock()
                .unwrap()
                .iter()
                .any(|m| m.contains("not persisted")),
            "the user is told the grant did not persist so the re-prompt next run isn't a surprise"
        );
    }

    fn grants_session_with_store(
        grants: Arc<dyn GrantStore>,
        workload: WorkloadIdentity,
        store: Arc<CapturingStore>,
    ) -> CredentialSession {
        let (tx, _rx) = mpsc::unbounded_channel();
        CredentialSession::new(
            CredentialStateFile::new(),
            Arc::new(RecordingNotifier::default()),
            store,
            tx,
            TEST_TIMEOUT,
        )
        .with_custom_providers(Arc::new(vec![some_provider()]))
        .with_connect_emitter(
            HashSet::from(["some-provider".to_string()]),
            Box::new(|_| {}),
        )
        .with_grants("proj".into(), workload, grants)
    }

    #[test]
    fn a_consent_whose_value_cannot_be_written_persists_no_grant() {
        let grants = Arc::new(CapturingGrantStore::default());
        let workload = WorkloadIdentity::Definition {
            dir: "/proj".into(),
        };
        let store = Arc::new(CapturingStore::default());
        let session = grants_session_with_store(grants.clone(), workload.clone(), store.clone());
        store.fail_next(io::ErrorKind::PermissionDenied, "denied");
        session.submit_pending(pending("c1", "some-provider"), Instant::now());

        session.record_decision(
            "c1",
            CredentialDecisionRequest::Allow(CredentialEntry::Stored {
                value: "some-new".into(),
            }),
        );

        assert!(
            session.armed_ids().contains("some-provider"),
            "the run the developer consented to still arms from the in-memory value"
        );
        assert!(
            grants
                .load()
                .unwrap()
                .lookup("proj", &workload, "some-provider")
                .is_none(),
            "a grant outliving the value it was given for would silently arm whatever stale value the next run loads from disk, with no card"
        );
    }

    #[test]
    fn a_connect_whose_token_cannot_be_written_persists_no_grant() {
        let grants = Arc::new(CapturingGrantStore::default());
        let workload = WorkloadIdentity::Definition {
            dir: "/proj".into(),
        };
        let store = Arc::new(CapturingStore::default());
        let session = grants_session_with_store(grants.clone(), workload.clone(), store.clone());
        store.fail_next(io::ErrorKind::PermissionDenied, "denied");

        session.connect_connector_with_token("some-provider", "some-new".into());

        assert!(
            grants
                .load()
                .unwrap()
                .lookup("proj", &workload, "some-provider")
                .is_none(),
            "the pasted-token tail must hold its grant back too, or the next run arms the stale on-disk value under it"
        );
    }

    #[test]
    fn a_consent_for_an_undisclosed_id_persists_no_grant() {
        let store = Arc::new(CapturingGrantStore::default());
        let session = grants_session(
            store.clone(),
            WorkloadIdentity::Definition {
                dir: "/proj".into(),
            },
            vec![],
        );
        session.submit_pending(pending("c1", "some-provider"), Instant::now());
        session.record_decision(
            "c1",
            CredentialDecisionRequest::Allow(CredentialEntry::Stored {
                value: "some-real".into(),
            }),
        );
        assert!(
            store.load().unwrap().grants.is_empty(),
            "with no injection snapshot to pin the grant to, nothing is persisted"
        );
    }

    fn grants_session_connectable(
        store: Arc<dyn GrantStore>,
        workload: WorkloadIdentity,
    ) -> CredentialSession {
        let (tx, _rx) = mpsc::unbounded_channel();
        CredentialSession::new(
            CredentialStateFile::new(),
            Arc::new(RecordingNotifier::default()),
            Arc::new(CapturingStore::default()),
            tx,
            TEST_TIMEOUT,
        )
        .with_custom_providers(Arc::new(vec![some_provider()]))
        .with_connect_emitter(
            HashSet::from(["some-provider".to_string()]),
            Box::new(|_| {}),
        )
        .with_grants("proj".into(), workload, store)
    }

    #[tokio::test]
    async fn a_network_offer_connect_persists_the_grant() {
        let store = Arc::new(CapturingGrantStore::default());
        let workload = WorkloadIdentity::Definition {
            dir: "/proj".into(),
        };
        let session = grants_session_connectable(store.clone(), workload.clone());
        assert!(session.connect_connector_now("some-provider").await);
        assert!(
            store
                .load()
                .unwrap()
                .lookup("proj", &workload, "some-provider")
                .is_some(),
            "accepting a network-offer connect must persist the grant so the next run skips the offer"
        );
    }

    #[test]
    fn a_connect_by_pasted_token_persists_the_grant() {
        let store = Arc::new(CapturingGrantStore::default());
        let workload = WorkloadIdentity::Definition {
            dir: "/proj".into(),
        };
        let session = grants_session_connectable(store.clone(), workload.clone());
        session.connect_connector_with_token("some-provider", "some-real".into());
        assert!(
            store
                .load()
                .unwrap()
                .lookup("proj", &workload, "some-provider")
                .is_some(),
            "connecting with a pasted token (the shared arm_connected tail) must persist the grant"
        );
    }

    #[test]
    fn a_credential_deny_records_a_credential_approval() {
        let (s, _n, _store, _rx) = fixture();
        let recorder = Arc::new(CapturingRecorder::default());
        s.set_ledger_recorder(recorder.clone());
        s.submit_pending(pending("c1", "some-provider"), Instant::now());
        s.record_decision("c1", CredentialDecisionRequest::Deny);
        assert_eq!(
            *recorder.events.lock().unwrap().first().expect("one event"),
            LedgerEvent::Approval {
                kind: ApprovalKind::Credential,
                target: "some-provider".into(),
                decision: Decision::Deny,
                reason: None,
                connector: Some("some-provider".into()),
            }
        );
    }

    #[test]
    fn a_credential_timeout_records_nothing() {
        let (s, _n, _store, _rx) = fixture();
        let recorder = Arc::new(CapturingRecorder::default());
        s.set_ledger_recorder(recorder.clone());
        s.submit_pending(pending("c1", "some-provider"), Instant::now());
        s.record_decision("c1", CredentialDecisionRequest::Timeout);
        assert!(recorder.events.lock().unwrap().is_empty());
    }

    #[test]
    fn submit_pending_presents_notification_with_credential_metadata() {
        let (s, n, _store, _rx) = fixture();
        s.submit_pending(pending("c1", "some-provider"), Instant::now());
        let p = n.presented.lock().unwrap();
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].id, "c1");
        assert_eq!(p[0].credential_id, "some-provider");
        assert_eq!(p[0].action, "use of some-provider placeholder");
    }

    #[test]
    fn duplicate_pending_id_does_not_present_a_second_notification() {
        let (s, n, _store, _rx) = fixture();
        s.submit_pending(pending("c1", "some-provider"), Instant::now());
        s.submit_pending(pending("c1", "some-provider"), Instant::now());
        assert_eq!(n.presented.lock().unwrap().len(), 1);
    }

    #[test]
    fn concurrent_requests_for_one_provider_present_a_single_card() {
        let (s, n, _store, _rx) = fixture();
        s.submit_pending(pending("c1", "some-provider"), Instant::now());
        s.submit_pending(pending("c2", "some-provider"), Instant::now());
        let presented = n.presented.lock().unwrap();
        assert_eq!(presented.len(), 1, "same provider must raise one card");
        assert_eq!(presented[0].id, "c1");
    }

    #[test]
    fn decision_on_a_coalesced_card_resolves_every_held_request() {
        let (s, n, store, mut rx) = fixture();
        s.submit_pending(pending("c1", "some-provider"), Instant::now());
        s.submit_pending(pending("c2", "some-provider"), Instant::now());

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
        s.submit_pending(pending("c1", "some-provider"), t0);
        s.submit_pending(pending("c2", "some-provider"), t0);

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
        state.insert("some-provider".into(), CredentialEntry::Deny);
        let s = CredentialSession::new(state, notifier.clone(), store.clone(), tx, TEST_TIMEOUT);

        s.submit_pending(pending("c1", "some-provider"), Instant::now());

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
        s.submit_pending(pending("c1", "some-provider"), Instant::now());

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
        assert_eq!(
            saves[0].get("some-provider"),
            Some(&CredentialEntry::HostDetect)
        );
        assert_eq!(
            s.current_state().get("some-provider"),
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
    fn a_card_deny_persists_a_per_workload_deny_and_emits_a_deny_frame() {
        let store = Arc::new(CapturingGrantStore::default());
        let workload = WorkloadIdentity::Definition {
            dir: "/proj".into(),
        };
        let (tx, mut rx) = mpsc::unbounded_channel();
        let session = CredentialSession::new(
            CredentialStateFile::new(),
            Arc::new(RecordingNotifier::default()),
            Arc::new(CapturingStore::default()),
            tx,
            TEST_TIMEOUT,
        )
        .with_custom_providers(Arc::new(vec![some_provider()]))
        .with_grants("proj".into(), workload.clone(), store.clone());
        session.submit_pending(pending("c1", "some-provider"), Instant::now());

        session.record_decision("c1", CredentialDecisionRequest::Deny);

        let persisted = store.load().unwrap();
        let grant = persisted
            .lookup("proj", &workload, "some-provider")
            .expect("a declined card persists a per-workload deny grant");
        assert_eq!(grant.verdict, GrantVerdict::Deny);
        assert_eq!(
            decision_frame(&mut rx).decision,
            CredentialDecisionKind::Deny
        );
        assert!(
            !matches!(
                session.current_state().get("some-provider"),
                Some(CredentialEntry::Deny)
            ),
            "a declined card is per-workload, never a machine-wide credential Deny that would block the connector for every project"
        );
    }

    #[test]
    fn a_card_deny_for_an_undisclosed_id_persists_a_standing_deny() {
        let store = Arc::new(CapturingGrantStore::default());
        let workload = WorkloadIdentity::Definition {
            dir: "/proj".into(),
        };
        let session = grants_session(store.clone(), workload.clone(), vec![]);
        session.submit_pending(pending("c1", "some-provider"), Instant::now());
        session.record_decision("c1", CredentialDecisionRequest::Deny);
        let persisted = store.load().unwrap();
        let grant = persisted
            .lookup("proj", &workload, "some-provider")
            .expect("a deny is a standing no, recorded even with no injection to pin it to");
        assert_eq!(grant.verdict, GrantVerdict::Deny);
        assert!(grant.env_var.is_empty());
    }

    #[test]
    fn a_prior_workload_deny_suppresses_the_offer_without_re_prompting() {
        let store = Arc::new(CapturingGrantStore::default());
        let workload = WorkloadIdentity::Definition {
            dir: "/proj".into(),
        };
        let mut prior = WorkloadGrantFile::default();
        prior.upsert(GrantRecord::deny(
            "proj",
            &workload,
            "some-provider",
            "SOME_TOKEN",
            vec!["api.some-provider.example".into()],
        ));
        store.save(&prior).unwrap();
        let notifier = Arc::new(RecordingNotifier::default());
        let (tx, mut rx) = mpsc::unbounded_channel();
        let session = CredentialSession::new(
            CredentialStateFile::new(),
            notifier.clone(),
            Arc::new(CapturingStore::default()),
            tx,
            TEST_TIMEOUT,
        )
        .with_custom_providers(Arc::new(vec![some_provider()]))
        .with_grants("proj".into(), workload, store);
        session.submit_pending(pending("c1", "some-provider"), Instant::now());
        assert_eq!(
            decision_frame(&mut rx).decision,
            CredentialDecisionKind::Deny,
            "a standing per-workload deny fails the request at the boundary"
        );
        assert!(
            notifier.presented.lock().unwrap().is_empty(),
            "no card is shown for a connector this workload already declined"
        );
    }

    fn session_with_prior_grant(
        record: GrantRecord,
    ) -> (CredentialSession, Arc<RecordingNotifier>) {
        let store = Arc::new(CapturingGrantStore::default());
        let mut prior = WorkloadGrantFile::default();
        prior.upsert(record);
        store.save(&prior).unwrap();
        let notifier = Arc::new(RecordingNotifier::default());
        let (tx, _rx) = mpsc::unbounded_channel();
        let session = CredentialSession::new(
            CredentialStateFile::new(),
            notifier.clone(),
            Arc::new(CapturingStore::default()),
            tx,
            TEST_TIMEOUT,
        )
        .with_custom_providers(Arc::new(vec![some_provider()]))
        .with_grants(
            "proj".into(),
            WorkloadIdentity::Definition {
                dir: "/proj".into(),
            },
            store,
        );
        (session, notifier)
    }

    #[test]
    fn a_prior_deny_for_another_workload_does_not_suppress_this_workloads_card() {
        let foreign = GrantRecord::deny(
            "proj",
            &WorkloadIdentity::Definition {
                dir: "/other".into(),
            },
            "some-provider",
            "SOME_TOKEN",
            vec!["api.some-provider.example".into()],
        );
        let (session, notifier) = session_with_prior_grant(foreign);
        session.submit_pending(pending("c1", "some-provider"), Instant::now());
        assert_eq!(
            notifier.presented.lock().unwrap().len(),
            1,
            "another workload's decline is not this workload's answer; the card must still be asked"
        );
    }

    #[test]
    fn a_deny_is_remembered_for_the_run_even_with_no_grant_binding() {
        let (s, n, _store, _rx) = fixture();
        s.submit_pending(pending("c1", "some-provider"), Instant::now());
        s.record_decision("c1", CredentialDecisionRequest::Deny);
        let after_decline = n.presented.lock().unwrap().len();
        s.submit_pending(pending("c2", "some-provider"), Instant::now());
        assert_eq!(
            n.presented.lock().unwrap().len(),
            after_decline,
            "a decline must stop the re-prompt from run-local memory alone; whether a sidecar is bound is not the workload's concern"
        );
    }

    #[test]
    fn a_prior_deny_whose_connector_was_redefined_asks_again() {
        let stale = GrantRecord::deny(
            "proj",
            &WorkloadIdentity::Definition {
                dir: "/proj".into(),
            },
            "some-provider",
            "OLD_TOKEN",
            vec!["api.old.example".into()],
        );
        let (session, notifier) = session_with_prior_grant(stale);
        session.submit_pending(pending("c1", "some-provider"), Instant::now());
        assert_eq!(
            notifier.presented.lock().unwrap().len(),
            1,
            "a deny pinned to a since-changed env var and domain must be re-asked, exactly as a stale allow is re-asked rather than silently inherited"
        );
    }

    #[test]
    fn a_prior_deny_holds_when_the_run_resolves_no_provider_to_compare_against() {
        let store = Arc::new(CapturingGrantStore::default());
        let workload = WorkloadIdentity::Definition {
            dir: "/proj".into(),
        };
        let mut prior = WorkloadGrantFile::default();
        prior.upsert(GrantRecord::deny(
            "proj",
            &workload,
            "some-provider",
            "SOME_TOKEN",
            vec!["api.some-provider.example".into()],
        ));
        store.save(&prior).unwrap();
        let notifier = Arc::new(RecordingNotifier::default());
        let (tx, _rx) = mpsc::unbounded_channel();
        // No custom providers, so this run has no disclosure to check the recorded one against.
        let session = CredentialSession::new(
            CredentialStateFile::new(),
            notifier.clone(),
            Arc::new(CapturingStore::default()),
            tx,
            TEST_TIMEOUT,
        )
        .with_grants("proj".into(), workload, store);
        session.submit_pending(pending("c1", "some-provider"), Instant::now());
        assert!(
            notifier.presented.lock().unwrap().is_empty(),
            "with nothing to compare the recorded disclosure against, the standing no holds rather than failing open into a fresh card"
        );
    }

    #[test]
    fn a_prior_deny_recorded_without_a_disclosure_still_suppresses_the_offer() {
        let unpinned = GrantRecord::deny(
            "proj",
            &WorkloadIdentity::Definition {
                dir: "/proj".into(),
            },
            "some-provider",
            "",
            vec![],
        );
        let (session, notifier) = session_with_prior_grant(unpinned);
        session.submit_pending(pending("c1", "some-provider"), Instant::now());
        assert!(
            notifier.presented.lock().unwrap().is_empty(),
            "a standing no with no disclosure to pin it to has nothing a redefinition could invalidate, so it holds"
        );
    }

    #[test]
    fn a_mid_run_deny_suppresses_the_offer_even_when_the_sidecar_write_failed() {
        let notifier = Arc::new(RecordingNotifier::default());
        let (tx, mut rx) = mpsc::unbounded_channel();
        let session = CredentialSession::new(
            CredentialStateFile::new(),
            notifier.clone(),
            Arc::new(CapturingStore::default()),
            tx,
            TEST_TIMEOUT,
        )
        .with_custom_providers(Arc::new(vec![some_provider()]))
        .with_grants(
            "proj".into(),
            WorkloadIdentity::Definition {
                dir: "/proj".into(),
            },
            Arc::new(FailingGrantStore),
        );
        session.submit_pending(pending("c1", "some-provider"), Instant::now());
        session.record_decision("c1", CredentialDecisionRequest::Deny);
        let cards_after_first_decline = notifier.presented.lock().unwrap().len();
        // The re-request in the same run must be suppressed from run-local memory even though the sidecar write failed.
        session.submit_pending(pending("c2", "some-provider"), Instant::now());
        assert_eq!(
            notifier.presented.lock().unwrap().len(),
            cards_after_first_decline,
            "the re-request is suppressed without a fresh card even though the deny could not be persisted"
        );
        let mut c2_denied = false;
        while let Ok(frame) = rx.try_recv() {
            if let HostFrame::CredentialDecision(d) = frame
                && d.id == "c2"
            {
                c2_denied = d.decision == CredentialDecisionKind::Deny;
            }
        }
        assert!(
            c2_denied,
            "the re-request is failed at the boundary from the run-local deny memory"
        );
    }

    #[test]
    fn timeout_request_emits_decision_frame_but_does_not_persist() {
        let (s, n, store, mut rx) = fixture();
        s.submit_pending(pending("c1", "some-provider"), Instant::now());

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
        s.submit_pending(pending("c1", "some-provider"), Instant::now());
        store.fail_next(io::ErrorKind::PermissionDenied, "disk full");

        s.record_decision(
            "c1",
            CredentialDecisionRequest::Allow(CredentialEntry::HostDetect),
        );

        assert_eq!(
            s.current_state().get("some-provider"),
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
        s.submit_pending(pending("c1", "some-provider"), Instant::now());
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
        s.submit_pending(pending("c1", "some-provider"), t0);

        let expired = s.tick_timeouts(t0 + Duration::from_secs(5));

        assert_eq!(expired, 0);
        assert!(rx.try_recv().is_err());
        assert!(n.dismissed.lock().unwrap().is_empty());
    }

    #[test]
    fn tick_timeouts_after_deadline_dismisses_and_emits_timeout_decision() {
        let (s, n, _store, mut rx) = fixture_with_timeout(Duration::from_secs(10));
        let t0 = Instant::now();
        s.submit_pending(pending("c1", "some-provider"), t0);

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
        s.submit_pending(pending("c1", "some-provider"), t0);
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
        s.submit_pending(pending("c1", "some-provider"), Instant::now());
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
        s.submit_pending(pending("c1", "some-provider"), t0);
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
        s.submit_pending(pending("c1", "some-provider"), Instant::now());

        let acted = s.timeout_one("c1");

        assert!(acted);
        assert_eq!(
            decision_frame(&mut rx).decision,
            CredentialDecisionKind::Timeout
        );
        assert_eq!(n.dismissed.lock().unwrap().as_slice(), &["c1".to_string()]);
        assert!(
            n.connects_finished.lock().unwrap().is_empty(),
            "a plain credential never holds a connect placeholder, so its timeout signals none"
        );
    }

    #[test]
    fn timeout_clears_a_connecting_placeholder_for_an_expired_oauth_consent() {
        let (s, n, _store, _rx, _c) = oauth_fixture(FakeFlow::polling(vec![]));
        let t0 = Instant::now();
        s.submit_pending(pending("c1", "some-oauth"), t0);

        s.tick_timeouts(t0 + TEST_TIMEOUT + Duration::from_secs(1));

        assert_eq!(
            n.connects_finished.lock().unwrap().as_slice(),
            &["GitHub".to_string()],
            "timing out a clicked consent must release the placeholder no later sign-in will resolve"
        );
    }

    #[test]
    fn withdraw_run_dismisses_all_pending_without_emitting_frames() {
        let (s, n, _store, mut rx) = fixture();
        s.submit_pending(pending("c1", "some-provider"), Instant::now());
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
        s.submit_pending(pending("c1", "some-provider"), Instant::now());
        s.withdraw_run();
        assert_eq!(
            s.record_decision("c1", CredentialDecisionRequest::Deny),
            DecisionOutcome::UnknownId
        );
    }

    #[test]
    fn withdraw_run_clears_notifier_informs_so_window_does_not_stay_pinned() {
        let (s, n, _store, _rx) = fixture();
        s.submit_pending(pending("c1", "some-provider"), Instant::now());
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
    fn withdraw_run_clears_connecting_placeholders_so_window_does_not_stay_pinned() {
        let (s, n, _store, _rx) = fixture();
        s.submit_pending(pending("c1", "some-provider"), Instant::now());

        s.withdraw_run();

        assert_eq!(
            *n.all_connecting_cleared.lock().unwrap(),
            1,
            "an in-flight sign-in placeholder must not survive the run that started it"
        );
    }

    #[test]
    fn apply_external_state_replaces_in_memory_state() {
        let (s, _n, _store, mut rx) = fixture();
        let mut updated = CredentialStateFile::new();
        updated.insert("some-provider".into(), CredentialEntry::Deny);

        s.apply_external_state(updated.clone());

        assert_eq!(s.current_state(), updated);
        // No decision frame: S10 is a host-side hot-swap, not a workload-driven event.
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn record_decision_after_timeout_returns_unknownid() {
        let (s, _n, _store, mut rx) = fixture_with_timeout(Duration::from_secs(1));
        let t0 = Instant::now();
        s.submit_pending(pending("c1", "some-provider"), t0);
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
            decision_kind_of(&CredentialDecisionRequest::AllowBound),
            CredentialDecisionKind::Allow,
            "granting the existing binding releases the held request like any other allow"
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
            Box::new(move |state, _armed| {
                captured_clone.snapshots.lock().unwrap().push(state.clone());
            }),
        );
        session.submit_pending(pending("c1", "some-provider"), Instant::now());
        session.record_decision(
            "c1",
            CredentialDecisionRequest::Allow(CredentialEntry::HostDetect),
        );

        let snaps = captured.snapshots.lock().unwrap();
        assert_eq!(snaps.len(), 1);
        assert_eq!(
            snaps[0].get("some-provider"),
            Some(&CredentialEntry::HostDetect)
        );
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
            Box::new(move |_state, _armed| {
                use crate::approval_flow::protocol::PolicyMessage;
                let _ = tx_for_emitter.send(HostFrame::Policy(PolicyMessage::default()));
            }),
        );
        session.submit_pending(pending("c1", "some-provider"), Instant::now());
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
            Box::new(move |state, _armed| {
                captured_clone.snapshots.lock().unwrap().push(state.clone());
            }),
        );
        session.submit_pending(pending("c1", "some-provider"), Instant::now());
        session.record_decision(
            "c1",
            CredentialDecisionRequest::Allow(CredentialEntry::Stored {
                value: "some-real".into(),
            }),
        );

        let snaps = captured.snapshots.lock().unwrap();
        assert_eq!(
            snaps.len(),
            1,
            "policy_emitter must be called even when store.save fails"
        );
    }

    #[test]
    fn timeout_does_not_invoke_policy_emitter() {
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
            Box::new(move |state, _armed| {
                captured_clone.snapshots.lock().unwrap().push(state.clone());
            }),
        );
        session.submit_pending(pending("c1", "some-provider"), Instant::now());
        session.record_decision("c1", CredentialDecisionRequest::Timeout);
        assert!(
            captured.snapshots.lock().unwrap().is_empty(),
            "a timed-out prompt writes nothing and must not touch the wire"
        );
        // Positive control: the same emitter fires on a real decision, so the assertion above is about the timeout and not about an unwired emitter.
        session.submit_pending(pending("c2", "some-provider"), Instant::now());
        session.record_decision(
            "c2",
            CredentialDecisionRequest::Allow(CredentialEntry::HostDetect),
        );
        assert_eq!(captured.snapshots.lock().unwrap().len(), 1);
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
            Box::new(move |state, _armed| {
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
    fn granting_a_connectable_ids_existing_binding_connects_it_live_and_keeps_the_stored_value() {
        let (s, n, store, _rx, connected) = fixture_connectable(&["gitlab"]);
        let bound = CredentialEntry::Stored {
            value: "already-bound".into(),
        };
        let mut machine = CredentialStateFile::new();
        machine.insert("gitlab".to_string(), bound.clone());
        s.apply_external_state(machine);
        s.submit_pending(pending("c1", "gitlab"), Instant::now());
        assert!(
            n.presented.lock().unwrap()[0].bound_value_available,
            "a value bound on this machine must be offerable for reuse on the card"
        );

        s.record_decision("c1", CredentialDecisionRequest::AllowBound);

        assert_eq!(
            connected.lock().unwrap().as_slice(),
            &["gitlab".to_string()],
            "granting the binding still connects the connector live, exactly as a fresh value would"
        );
        assert!(s.armed_ids().contains("gitlab"));
        assert_eq!(
            s.current_state().get("gitlab"),
            Some(&bound),
            "the machine binding is spent, never rewritten"
        );
        assert!(
            store.saves.lock().unwrap().is_empty(),
            "reusing a binding writes nothing to the credential store"
        );
    }

    #[test]
    fn granting_an_existing_binding_records_the_credential_use_it_starts() {
        let (s, _n, _store, _rx, _connected) = fixture_connectable(&["some-provider"]);
        let recorder = Arc::new(CapturingRecorder::default());
        s.set_ledger_recorder(recorder.clone());
        let mut machine = CredentialStateFile::new();
        machine.insert(
            "some-provider".to_string(),
            CredentialEntry::Stored {
                value: "some-already-bound".into(),
            },
        );
        s.apply_external_state(machine);
        s.submit_pending(pending("c1", "some-provider"), Instant::now());

        s.record_decision("c1", CredentialDecisionRequest::AllowBound);

        let events = recorder.events.lock().unwrap();
        assert!(
            events.iter().any(|e| matches!(
                e,
                LedgerEvent::CredentialUse { connector, fp, .. }
                    if connector == "some-provider"
                        && fp == &Some(lns_ipc::fingerprint("some-already-bound"))
            )),
            "granting a bound value is where that secret starts reaching the workload's destinations, so the audit chain owes the same credential-use row a freshly-bound value writes; got: {events:?}"
        );
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
        s.submit_pending(pending("c1", "some-oauth"), Instant::now());
        assert_eq!(
            n.presented.lock().unwrap()[0].oauth_display_name,
            Some("GitHub".to_string())
        );
    }

    #[test]
    fn submit_pending_surfaces_the_token_fallback_on_the_consent_prompt() {
        let (s, n, _store, _rx, _c) = oauth_fixture(FakeFlow::polling(vec![]));
        s.submit_pending(pending("c1", "some-oauth"), Instant::now());
        assert_eq!(
            n.presented.lock().unwrap()[0].token_fallback,
            Some(TokenFallback {
                help: Some("https://example.com/pat".into()),
                command: None,
            }),
            "a consent card for a connector with a token fallback offers the pivot"
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
            "some-oauth".to_string(),
            crate::oauth::OauthConfig {
                userinfo_endpoint: None,
                account_field: None,
                client_id: "Iv1.test".into(),
                client_secret: String::new(),
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
        s.submit_pending(pending("c1", "some-oauth"), Instant::now());
        assert_eq!(
            notifier.presented.lock().unwrap()[0].oauth_display_name,
            Some("some-oauth".to_string()),
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
        let (s, _n, _store, _rx, connected) = fixture_connectable(&["gitlab"]);
        s.submit_pending(pending("c1", "gitlab"), Instant::now());
        s.record_decision("c1", CredentialDecisionRequest::Deny);
        assert!(
            connected.lock().unwrap().is_empty(),
            "denying must not connect the connector"
        );
    }

    #[test]
    fn allow_for_a_non_connectable_id_does_not_connect() {
        let (s, _n, _store, _rx, connected) = fixture_connectable(&[]);
        s.submit_pending(pending("c1", "some-provider"), Instant::now());
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
            userinfo_endpoint: None,
            account_field: None,
            client_id: "Iv1.test".into(),
            client_secret: String::new(),
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
            scopes: Vec::new(),
            account: None,
            access_token: "some-access".into(),
            refresh_token: "some-refresh".into(),
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
            "some-oauth".to_string(),
            crate::oauth::OauthConfig {
                userinfo_endpoint: None,
                account_field: None,
                client_id: "Iv1.test".into(),
                client_secret: String::new(),
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
            HashSet::from(["some-oauth".to_string()]),
            Box::new(move |id| connected_cb.lock().unwrap().push(id.to_string())),
        )
        .with_oauth(configs, flow, Arc::new(FixedClock(1000)))
        .with_oauth_display_names(HashMap::from([(
            "some-oauth".to_string(),
            "GitHub".to_string(),
        )]))
        .with_token_fallbacks(HashMap::from([(
            "some-oauth".to_string(),
            TokenFallback {
                help: Some("https://example.com/pat".into()),
                command: None,
            },
        )]));
        (session, notifier, store, rx, connected)
    }

    #[test]
    fn is_oauth_prompt_distinguishes_oauth_from_plain_pending() {
        let (s, _n, _store, _rx, _c) = oauth_fixture(FakeFlow::polling(vec![]));
        s.submit_pending(pending("c1", "some-oauth"), Instant::now());
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
        s.submit_pending(pending("c1", "some-oauth"), Instant::now());
        s.submit_pending(pending("c2", "some-oauth"), Instant::now());
        let outcome = s.connect_oauth("c1").await;
        assert_eq!(outcome, DecisionOutcome::Resolved);
        assert_eq!(
            connected.lock().unwrap().as_slice(),
            &["some-oauth".to_string()],
            "accepting an oauth prompt connects the connector live"
        );
        assert_eq!(
            store
                .saves
                .lock()
                .unwrap()
                .last()
                .unwrap()
                .get("some-oauth"),
            Some(&CredentialEntry::Oauth {
                scopes: Vec::new(),
                account: None,
                access_token: "some-access".into(),
                refresh_token: "some-refresh".into(),
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
        assert_eq!(sign_ins[0].user_code.as_deref(), Some("WXYZ-1234"));
        assert_eq!(sign_ins[0].verification_uri, "https://example.com/device");
        drop(sign_ins);
        assert_eq!(
            n.dismissed_sign_ins.lock().unwrap().as_slice(),
            &["some-oauth".to_string()],
            "the sign-in card is dismissed once the flow resolves"
        );
    }

    #[tokio::test]
    async fn connect_oauth_pivots_to_a_pasted_token_arms_stored_connects_and_releases_held_requests()
     {
        let (s, n, store, mut rx, connected) = oauth_fixture(FakeFlow::polling(vec![]));
        n.use_token_next_sign_in("some-pasted-token");
        s.submit_pending(pending("c1", "some-oauth"), Instant::now());
        s.submit_pending(pending("c2", "some-oauth"), Instant::now());

        let outcome = s.connect_oauth("c1").await;

        assert_eq!(outcome, DecisionOutcome::Resolved);
        assert_eq!(
            connected.lock().unwrap().as_slice(),
            &["some-oauth".to_string()],
            "pivoting to a token still connects the connector live"
        );
        assert_eq!(
            store
                .saves
                .lock()
                .unwrap()
                .last()
                .unwrap()
                .get("some-oauth"),
            Some(&CredentialEntry::Stored {
                value: "some-pasted-token".into(),
            }),
            "the pasted token is armed as a Stored value in the connector's slot"
        );
        let f1 = decision_frame(&mut rx);
        let f2 = decision_frame(&mut rx);
        assert_eq!(f1.decision, CredentialDecisionKind::Allow);
        assert_eq!(f2.decision, CredentialDecisionKind::Allow);
        assert_eq!(vec![f1.id, f2.id], vec!["c1".to_string(), "c2".to_string()]);
        assert_eq!(
            n.dismissed_sign_ins.lock().unwrap().as_slice(),
            &["some-oauth".to_string()],
            "the sign-in card is dismissed once the pivot resolves the flow"
        );
    }

    #[tokio::test]
    async fn the_sign_in_card_carries_the_connectors_declared_token_fallback() {
        let (s, n, _store, _rx, _connected) =
            oauth_fixture(FakeFlow::polling(vec![crate::oauth::PollOutcome::Token(
                oauth_token(3600),
            )]));
        s.submit_pending(pending("c1", "some-oauth"), Instant::now());
        s.connect_oauth("c1").await;
        let sign_ins = n.sign_ins.lock().unwrap();
        assert_eq!(
            sign_ins[0].token_fallback,
            Some(TokenFallback {
                help: Some("https://example.com/pat".into()),
                command: None,
            }),
            "a sign-in card for a connector with a token fallback offers the pivot"
        );
    }

    #[tokio::test]
    async fn connect_oauth_cancelled_fails_held_requests_and_dismisses_the_card() {
        let (s, n, store, mut rx, connected) = oauth_fixture(FakeFlow::polling(vec![]));
        n.cancel_next_sign_in();
        s.submit_pending(pending("c1", "some-oauth"), Instant::now());

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
            &["some-oauth".to_string()]
        );
    }

    #[tokio::test]
    async fn connect_oauth_denied_fails_held_requests_without_arming_or_connecting() {
        let (s, _n, store, mut rx, connected) =
            oauth_fixture(FakeFlow::polling(vec![crate::oauth::PollOutcome::Denied]));
        s.submit_pending(pending("c1", "some-oauth"), Instant::now());
        s.connect_oauth("c1").await;
        assert!(connected.lock().unwrap().is_empty());
        assert!(store.saves.lock().unwrap().is_empty());
        assert_eq!(
            decision_frame(&mut rx).decision,
            CredentialDecisionKind::Deny
        );
    }

    #[tokio::test]
    async fn connect_oauth_success_records_a_credential_allow() {
        let (s, _n, _store, _rx, _c) =
            oauth_fixture(FakeFlow::polling(vec![crate::oauth::PollOutcome::Token(
                oauth_token(3600),
            )]));
        let recorder = Arc::new(CapturingRecorder::default());
        s.set_ledger_recorder(recorder.clone());
        s.submit_pending(pending("c1", "some-oauth"), Instant::now());
        s.connect_oauth("c1").await;
        let events = recorder.events.lock().unwrap();
        assert_eq!(
            *events.last().expect("an approval after the connection"),
            LedgerEvent::Approval {
                kind: ApprovalKind::Credential,
                target: "some-oauth".into(),
                decision: Decision::Allow,
                reason: None,
                connector: Some("some-oauth".into()),
            }
        );
    }

    #[tokio::test]
    async fn connect_oauth_denied_records_a_credential_deny() {
        let (s, _n, _store, _rx, _c) =
            oauth_fixture(FakeFlow::polling(vec![crate::oauth::PollOutcome::Denied]));
        let recorder = Arc::new(CapturingRecorder::default());
        s.set_ledger_recorder(recorder.clone());
        s.submit_pending(pending("c1", "some-oauth"), Instant::now());
        s.connect_oauth("c1").await;
        assert_eq!(
            *recorder.events.lock().unwrap().first().expect("one event"),
            LedgerEvent::Approval {
                kind: ApprovalKind::Credential,
                target: "some-oauth".into(),
                decision: Decision::Deny,
                reason: None,
                connector: Some("some-oauth".into()),
            }
        );
    }

    #[tokio::test]
    async fn connect_oauth_resolves_the_account_through_the_userinfo_fetcher() {
        struct FakeUserInfo;
        impl crate::oauth::UserInfoFetcher for FakeUserInfo {
            fn fetch<'a>(
                &'a self,
                _url: &'a str,
                _token: &'a str,
            ) -> futures_util::future::BoxFuture<'a, anyhow::Result<Vec<u8>>> {
                Box::pin(async { Ok(br#"{"login":"@hchen"}"#.to_vec()) })
            }
        }
        let notifier = Arc::new(RecordingNotifier::default());
        let (tx, _rx) = mpsc::unbounded_channel();
        let configs = HashMap::from([(
            "some-oauth".to_string(),
            crate::oauth::OauthConfig {
                userinfo_endpoint: Some("https://api.example.test/user".into()),
                account_field: Some("login".into()),
                client_id: "Iv1.test".into(),
                client_secret: String::new(),
                scopes: vec!["repo".into()],
                device_authorization_endpoint: "https://example.com/device/code".into(),
                token_endpoint: "https://example.com/oauth/token".into(),
            },
        )]);
        let s = CredentialSession::new(
            CredentialStateFile::new(),
            notifier,
            Arc::new(CapturingStore::default()),
            tx,
            TEST_TIMEOUT,
        )
        .with_oauth(
            configs,
            FakeFlow::polling(vec![crate::oauth::PollOutcome::Token(oauth_token(3600))]),
            Arc::new(FixedClock(0)),
        )
        .with_userinfo_fetcher(Arc::new(FakeUserInfo));
        let recorder = Arc::new(CapturingRecorder::default());
        s.set_ledger_recorder(recorder.clone());
        s.submit_pending(pending("c1", "some-oauth"), Instant::now());
        s.connect_oauth("c1").await;
        let accounts: Vec<_> = recorder
            .events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|e| match e {
                LedgerEvent::Connection { account, .. } => Some(account.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(accounts, vec![Some("@hchen".to_string())]);
    }

    #[tokio::test]
    async fn connect_oauth_expired_fails_held_requests_without_arming() {
        let (s, _n, store, mut rx, _connected) =
            oauth_fixture(FakeFlow::polling(vec![crate::oauth::PollOutcome::Expired]));
        s.submit_pending(pending("c1", "some-oauth"), Instant::now());
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
        s.submit_pending(pending("c1", "some-oauth"), Instant::now());
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
    async fn connect_oauth_signals_connect_finished_with_the_display_name() {
        let (s, n, _store, _rx, _connected) =
            oauth_fixture(FakeFlow::polling(vec![crate::oauth::PollOutcome::Token(
                oauth_token(3600),
            )]));
        s.submit_pending(pending("c1", "some-oauth"), Instant::now());
        s.connect_oauth("c1").await;
        assert_eq!(
            n.connects_finished.lock().unwrap().as_slice(),
            &["GitHub".to_string()],
            "the resolved connect releases the slot the consent card's placeholder holds"
        );
    }

    #[tokio::test]
    async fn connect_oauth_failure_still_signals_connect_finished() {
        let (s, n, _store, _rx, _connected) = oauth_fixture(FakeFlow::code_error());
        s.submit_pending(pending("c1", "some-oauth"), Instant::now());
        s.connect_oauth("c1").await;
        assert_eq!(
            n.connects_finished.lock().unwrap().as_slice(),
            &["GitHub".to_string()],
            "a failed sign-in must not leave a connecting placeholder behind"
        );
    }

    #[tokio::test]
    async fn connect_oauth_on_an_unknown_prompt_is_a_noop() {
        let (s, n, _store, _rx, _c) = oauth_fixture(FakeFlow::polling(vec![]));
        assert_eq!(s.connect_oauth("nope").await, DecisionOutcome::UnknownId);
        assert!(n.connects_finished.lock().unwrap().is_empty());
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
    async fn connect_connector_now_completes_an_oauth_sign_in_arms_and_connects() {
        let (s, n, _store, _rx, connected) =
            oauth_fixture(FakeFlow::polling(vec![crate::oauth::PollOutcome::Token(
                oauth_token(3600),
            )]));

        let ok = s.connect_connector_now("some-oauth").await;

        assert!(ok, "a completed sign-in reports connected");
        assert_eq!(
            connected.lock().unwrap().as_slice(),
            &["some-oauth".to_string()]
        );
        assert_eq!(
            s.current_state().get("some-oauth"),
            Some(&CredentialEntry::Oauth {
                scopes: Vec::new(),
                account: None,
                access_token: "some-access".into(),
                refresh_token: "some-refresh".into(),
                expires_at: 1000 + 3600,
            }),
            "the obtained token set is armed"
        );
        assert_eq!(n.sign_ins.lock().unwrap().len(), 1);
        assert_eq!(
            n.dismissed_sign_ins.lock().unwrap().as_slice(),
            &["some-oauth".to_string()]
        );
    }

    #[tokio::test]
    async fn connect_connector_now_returns_false_when_the_oauth_sign_in_is_denied() {
        let (s, _n, store, _rx, connected) =
            oauth_fixture(FakeFlow::polling(vec![crate::oauth::PollOutcome::Denied]));

        let ok = s.connect_connector_now("some-oauth").await;

        assert!(!ok);
        assert!(connected.lock().unwrap().is_empty());
        assert!(store.saves.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn connect_connector_now_connects_a_plain_credential_connector_without_a_sign_in() {
        let (s, n, _store, _rx, connected) = fixture_connectable(&["gitlab"]);

        let ok = s.connect_connector_now("gitlab").await;

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
    async fn connect_connector_now_returns_false_for_an_unknown_id() {
        let (s, _n, _store, _rx, _c) = oauth_fixture(FakeFlow::polling(vec![]));
        assert!(!s.connect_connector_now("nope").await);
    }

    #[tokio::test]
    async fn connect_connector_now_also_releases_a_held_credential_prompt_for_the_same_connector() {
        let (s, n, _store, mut rx, _connected) =
            oauth_fixture(FakeFlow::polling(vec![crate::oauth::PollOutcome::Token(
                oauth_token(3600),
            )]));
        // A placeholder card for some-oauth is already held (the "use your GitHub access" prompt).
        s.submit_pending(pending("cred1", "some-oauth"), Instant::now());

        let ok = s.connect_connector_now("some-oauth").await;

        assert!(ok);
        let frame = decision_frame(&mut rx);
        assert_eq!(frame.id, "cred1");
        assert_eq!(
            frame.decision,
            CredentialDecisionKind::Allow,
            "arming the connector via a network offer also releases its held placeholder request"
        );
        assert!(
            n.dismissed.lock().unwrap().contains(&"cred1".to_string()),
            "and dismisses the duplicate placeholder card"
        );
    }

    #[test]
    fn connect_connector_with_token_arms_stored_connects_live_and_releases_held_requests() {
        let (s, n, store, mut rx, connected) = fixture_connectable(&["gitlab"]);
        // A placeholder card for gitlab is already held.
        s.submit_pending(pending("cred1", "gitlab"), Instant::now());

        let ok = s.connect_connector_with_token("gitlab", "glpat_pasted".into());

        assert!(ok, "a pasted token connects the connector");
        assert_eq!(
            connected.lock().unwrap().as_slice(),
            &["gitlab".to_string()],
            "the connector's routes are allowed live"
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

    struct FakePkceFlow {
        result: StdMutex<Option<anyhow::Result<String>>>,
    }
    impl crate::oauth::AuthCodeFlow for FakePkceFlow {
        fn exchange_code<'a>(
            &'a self,
            _cfg: &'a crate::oauth::PkceConfig,
            _code: &'a str,
            _verifier: &'a str,
        ) -> BoxFuture<'a, anyhow::Result<String>> {
            Box::pin(async move {
                self.result
                    .lock()
                    .unwrap()
                    .take()
                    .expect("exchange scripted")
            })
        }
    }
    struct FakePkceListener {
        state: String,
    }
    impl crate::oauth::CallbackListener for FakePkceListener {
        fn bind(&self) -> BoxFuture<'_, anyhow::Result<Box<dyn crate::oauth::CallbackHandle>>> {
            let state = self.state.clone();
            Box::pin(async move {
                Ok(Box::new(FakePkceHandle { state }) as Box<dyn crate::oauth::CallbackHandle>)
            })
        }
    }
    struct FakePkceHandle {
        state: String,
    }
    impl crate::oauth::CallbackHandle for FakePkceHandle {
        fn redirect_url(&self) -> &str {
            "http://localhost:0/callback"
        }
        fn wait(
            self: Box<Self>,
        ) -> BoxFuture<'static, anyhow::Result<crate::oauth::CallbackParams>> {
            Box::pin(async move {
                Ok(crate::oauth::CallbackParams {
                    code: "code".into(),
                    state: self.state,
                })
            })
        }
    }
    fn fixed_pkce_challenge() -> crate::oauth::PkceChallenge {
        crate::oauth::PkceChallenge {
            verifier: "v".into(),
            challenge: "c".into(),
            state: "st".into(),
        }
    }

    type PkceFixture = (
        CredentialSession,
        Arc<RecordingNotifier>,
        Arc<CapturingStore>,
        mpsc::UnboundedReceiver<HostFrame>,
        Arc<StdMutex<Vec<String>>>,
        Arc<StdMutex<Vec<String>>>,
    );

    fn pkce_session(exchange: anyhow::Result<String>) -> PkceFixture {
        let notifier = Arc::new(RecordingNotifier::default());
        let store = Arc::new(CapturingStore::default());
        let (tx, rx) = mpsc::unbounded_channel();
        let connected = Arc::new(StdMutex::new(Vec::new()));
        let connected_cb = connected.clone();
        let opened = Arc::new(StdMutex::new(Vec::new()));
        let opened_cb = opened.clone();
        let configs = HashMap::from([(
            "some-pkce".to_string(),
            crate::oauth::PkceConfig {
                authorization_endpoint: "https://some-pkce.example/auth".into(),
                token_endpoint: "https://some-pkce.example/keys".into(),
                scopes: Vec::new(),
            },
        )]);
        let session = CredentialSession::new(
            CredentialStateFile::new(),
            notifier.clone(),
            store.clone(),
            tx,
            TEST_TIMEOUT,
        )
        .with_connect_emitter(
            HashSet::from(["some-pkce".to_string()]),
            Box::new(move |id| connected_cb.lock().unwrap().push(id.to_string())),
        )
        .with_pkce(
            configs,
            Arc::new(FakePkceFlow {
                result: StdMutex::new(Some(exchange)),
            }),
            Arc::new(FakePkceListener { state: "st".into() }),
            Box::new(move |url| opened_cb.lock().unwrap().push(url.to_string())),
            Box::new(fixed_pkce_challenge),
            Duration::from_secs(30),
        );
        (session, notifier, store, rx, connected, opened)
    }

    #[tokio::test]
    async fn connect_oauth_for_a_pkce_prompt_opens_the_browser_arms_a_stored_key_and_releases() {
        let (s, n, store, mut rx, connected, opened) = pkce_session(Ok("openrouter-key".into()));
        s.submit_pending(pending("c1", "some-pkce"), Instant::now());
        assert!(
            s.is_oauth_prompt("c1"),
            "a pkce prompt routes through the interactive sign-in path"
        );

        s.connect_oauth("c1").await;

        assert_eq!(
            connected.lock().unwrap().as_slice(),
            &["some-pkce".to_string()],
            "a completed pkce sign-in connects the connector live"
        );
        assert_eq!(
            store.saves.lock().unwrap().last().unwrap().get("some-pkce"),
            Some(&CredentialEntry::Stored {
                value: "openrouter-key".into(),
            }),
            "the captured key is armed as a durable Stored value"
        );
        assert_eq!(
            decision_frame(&mut rx).decision,
            CredentialDecisionKind::Allow
        );
        let opened = opened.lock().unwrap();
        assert_eq!(
            opened.len(),
            1,
            "the browser is opened to the authorization URL"
        );
        assert!(opened[0].contains("code_challenge=c"), "got: {opened:?}");
        assert_eq!(
            n.sign_ins.lock().unwrap()[0].user_code,
            None,
            "a pkce sign-in card carries no code to type"
        );
        assert_eq!(
            n.dismissed_sign_ins.lock().unwrap().as_slice(),
            &["some-pkce".to_string()]
        );
    }

    #[tokio::test]
    async fn connect_oauth_for_a_pkce_prompt_surfaces_a_failed_exchange_and_fails_held_requests() {
        let (s, n, store, mut rx, connected, _opened) =
            pkce_session(Err(anyhow::anyhow!("the authorization code was rejected")));
        s.submit_pending(pending("c1", "some-pkce"), Instant::now());

        s.connect_oauth("c1").await;

        assert!(connected.lock().unwrap().is_empty());
        assert!(store.saves.lock().unwrap().is_empty());
        assert_eq!(
            decision_frame(&mut rx).decision,
            CredentialDecisionKind::Deny
        );
        assert!(
            n.informed
                .lock()
                .unwrap()
                .iter()
                .any(|m| m.contains("failed"))
        );
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
        let armed: HashSet<String> = entries.iter().map(|(id, _)| id.to_string()).collect();
        for (id, entry) in entries {
            state.insert(id.into(), entry);
        }
        // Every entry id is consented: these sessions model connectors the run connected, so their values arm.
        let session = CredentialSession::new(state, notifier.clone(), store, tx, TEST_TIMEOUT)
            .with_custom_providers(Arc::new(custom))
            .with_armed_ids(armed);
        (session, notifier, rx)
    }

    fn some_oauth_provider() -> DefProvider {
        use lns_policy::providers::{InjectionDef, InjectionKind, ProviderDef};
        DefProvider::new(ProviderDef {
            id: "some-oauth".into(),
            env_var: "SOME_OAUTH_TOKEN".into(),
            placeholder: "some-oauth-placeholder".into(),
            injections: vec![InjectionDef {
                kind: InjectionKind::TokenHeader,
                domain: "api.some-oauth.example".into(),
                header: None,
            }],
        })
    }

    fn some_provider() -> DefProvider {
        use lns_policy::providers::{InjectionDef, InjectionKind, ProviderDef};
        DefProvider::new(ProviderDef {
            id: "some-provider".into(),
            env_var: "SOME_TOKEN".into(),
            placeholder: "some-placeholder-0000".into(),
            injections: vec![InjectionDef {
                kind: InjectionKind::BearerHeader,
                domain: "api.some-provider.example".into(),
                header: None,
            }],
        })
    }

    fn armed_oauth(access_token: &str) -> CredentialEntry {
        CredentialEntry::Oauth {
            scopes: Vec::new(),
            account: None,
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
            vec![("some-oauth", armed_oauth("some-token"))],
            vec![some_oauth_provider()],
        );
        s.submit_pending(
            gate("some-oauth", "POST https://api.some-oauth.example/issues"),
            Instant::now(),
        );
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
    fn a_live_connect_grants_arming_so_the_next_policy_emit_arms_the_value() {
        let notifier = Arc::new(RecordingNotifier::default());
        let store = Arc::new(CapturingStore::default());
        let (tx, _rx) = mpsc::unbounded_channel();
        let emitted: Arc<StdMutex<Vec<HashSet<String>>>> = Arc::new(StdMutex::new(Vec::new()));
        let emitted_clone = emitted.clone();
        let session = CredentialSession::with_policy_emitter(
            CredentialStateFile::new(),
            notifier,
            store,
            tx,
            TEST_TIMEOUT,
            Box::new(move |_state, armed| {
                emitted_clone.lock().unwrap().push(armed.clone());
            }),
        )
        .with_custom_providers(Arc::new(vec![some_provider()]))
        .with_connect_emitter(
            HashSet::from(["some-provider".to_string()]),
            Box::new(|_| {}),
        );
        assert!(
            !session.armed_ids().contains("some-provider"),
            "a connectable id must start unconsented"
        );
        session.connect_connector_with_token("some-provider", "some-real".into());
        let armed = emitted.lock().unwrap();
        assert!(
            armed.last().is_some_and(|a| a.contains("some-provider")),
            "the connect must grant arming before the policy emit, or the accepted credential stays dead until relaunch: {armed:?}"
        );
    }

    #[test]
    fn submit_pending_still_prompts_an_unconsented_id_despite_a_machine_stored_value() {
        let notifier = Arc::new(RecordingNotifier::default());
        let store = Arc::new(CapturingStore::default());
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut state = CredentialStateFile::new();
        state.insert(
            "some-provider".into(),
            CredentialEntry::Stored {
                value: "some-real".into(),
            },
        );
        let session = CredentialSession::new(state, notifier.clone(), store, tx, TEST_TIMEOUT)
            .with_custom_providers(Arc::new(vec![some_provider()]));
        session.submit_pending(
            gate(
                "some-provider",
                "POST https://api.some-provider.example/issues",
            ),
            Instant::now(),
        );
        assert_eq!(
            notifier.presented.lock().unwrap().len(),
            1,
            "a machine-stored value for a connector this run never consented to is a fresh consent, not a propagation race — it must prompt"
        );
    }

    fn unconsented_session_with(
        entries: Vec<(&str, CredentialEntry)>,
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
            .with_custom_providers(Arc::new(vec![some_provider()]))
            .with_connect_emitter(
                HashSet::from(["some-provider".to_string()]),
                Box::new(|_| {}),
            );
        (session, notifier, rx)
    }

    #[test]
    #[serial_test::serial(env)]
    fn a_host_detect_binding_is_not_also_offered_as_a_bound_value() {
        let _g = crate::test_env::EnvVarGuard::set("SOME_TOKEN", "host-real");
        let (session, notifier, _rx) =
            unconsented_session_with(vec![("some-provider", CredentialEntry::HostDetect)]);
        session.submit_pending(pending("c1", "some-provider"), Instant::now());
        assert!(
            !notifier.presented.lock().unwrap()[0].bound_value_available,
            "a host-detect entry is the same host value the card's own detected-value offer spends, so offering it a second time would put two primary buttons with one effect on the consent card"
        );
    }

    #[test]
    fn a_stored_binding_is_offered_as_a_bound_value() {
        let (session, notifier, _rx) = unconsented_session_with(vec![(
            "some-provider",
            CredentialEntry::Stored {
                value: "some-already-bound".into(),
            },
        )]);
        session.submit_pending(pending("c1", "some-provider"), Instant::now());
        assert!(
            notifier.presented.lock().unwrap()[0].bound_value_available,
            "a value bound on this machine is the card's whole reason to offer a grant instead of demanding the secret again"
        );
    }

    #[tokio::test]
    async fn a_network_offer_connect_releases_a_held_request_once_the_stored_value_arms() {
        let (session, notifier, mut rx) = unconsented_session_with(vec![(
            "some-provider",
            CredentialEntry::Stored {
                value: "some-real".into(),
            },
        )]);
        session.submit_pending(
            gate(
                "some-provider",
                "POST https://api.some-provider.example/issues",
            ),
            Instant::now(),
        );
        assert_eq!(
            notifier.presented.lock().unwrap().len(),
            1,
            "the unconsented hold prompts first"
        );
        assert!(session.connect_connector_now("some-provider").await);
        assert_eq!(
            decision_frame(&mut rx).decision,
            CredentialDecisionKind::Allow,
            "the connect armed the stored value, so the held request must release without a second card"
        );
        assert_eq!(
            notifier.dismissed.lock().unwrap().as_slice(),
            &["c1".to_string()],
            "the value card asks a question the connect already answered"
        );
    }

    /// Builds a session whose policy emitter and connect emitter append to one ordered log, so a test can pin that the armed-credential frame precedes the connect (which releases network holds in production).
    fn frame_order_session(log: Arc<StdMutex<Vec<String>>>) -> CredentialSession {
        let notifier = Arc::new(RecordingNotifier::default());
        let store = Arc::new(CapturingStore::default());
        let (tx, _rx) = mpsc::unbounded_channel();
        let log_policy = log.clone();
        let log_connect = log;
        CredentialSession::with_policy_emitter(
            CredentialStateFile::new(),
            notifier,
            store,
            tx,
            TEST_TIMEOUT,
            Box::new(move |state, _armed| {
                let armed = matches!(
                    state.get("some-provider"),
                    Some(CredentialEntry::Stored { value }) if !value.is_empty()
                );
                log_policy
                    .lock()
                    .unwrap()
                    .push(format!("policy:armed={armed}"));
            }),
        )
        .with_custom_providers(Arc::new(vec![some_provider()]))
        .with_connect_emitter(
            HashSet::from(["some-provider".to_string()]),
            Box::new(move |_| log_connect.lock().unwrap().push("connect".into())),
        )
    }

    fn assert_armed_before_connect(entries: &[String]) {
        let connect_at = entries
            .iter()
            .position(|e| e == "connect")
            .expect("the connect emitter must fire");
        assert!(
            entries[..connect_at].contains(&"policy:armed=true".to_string()),
            "the connect releases network holds in production, so the armed-credential frame must precede it — else the released request hits the placeholder gate before the real value is in hand: {entries:?}"
        );
    }

    #[test]
    fn arm_connected_emits_the_armed_credential_before_the_connect_releases_network_holds() {
        let log = Arc::new(StdMutex::new(Vec::new()));
        let session = frame_order_session(log.clone());
        session.connect_connector_with_token("some-provider", "pasted".into());
        assert_armed_before_connect(&log.lock().unwrap());
    }

    #[test]
    fn accepting_a_held_card_emits_the_armed_credential_before_the_connect() {
        let log = Arc::new(StdMutex::new(Vec::new()));
        let session = frame_order_session(log.clone());
        session.submit_pending(
            gate(
                "some-provider",
                "POST https://api.some-provider.example/issues",
            ),
            Instant::now(),
        );
        session.record_decision(
            "c1",
            CredentialDecisionRequest::Allow(CredentialEntry::Stored {
                value: "typed".into(),
            }),
        );
        assert_armed_before_connect(&log.lock().unwrap());
    }

    fn wire_injections(session: &CredentialSession, id: &str) -> Vec<CredentialInjection> {
        crate::credential_flow::registry::expand_credentials_for_wire_with_custom(
            &session.current_state(),
            session.custom_providers(),
            &session.armed_ids(),
        )
        .into_iter()
        .find(|c| c.id == id)
        .expect("provider on the wire")
        .injections
    }

    #[test]
    fn a_policy_reload_dropping_a_connector_revokes_its_stored_value_arming() {
        let notifier = Arc::new(RecordingNotifier::default());
        let store = Arc::new(CapturingStore::default());
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut state = CredentialStateFile::new();
        state.insert(
            "some-provider".into(),
            CredentialEntry::Stored {
                value: "some-real".into(),
            },
        );
        let session = CredentialSession::new(state, notifier, store, tx, TEST_TIMEOUT)
            .with_custom_providers(Arc::new(vec![some_provider()]))
            .with_armed_ids(HashSet::from(["some-provider".to_string()]));
        assert!(
            wire_injections(&session, "some-provider")
                .iter()
                .any(|i| matches!(
                    i,
                    CredentialInjection::Header { value, .. } if value.contains("some-real")
                )),
            "while connected, the stored value arms"
        );
        // `lns connector disconnect` drops the id from lns-policy.yaml; the reload reconciles with the remaining connectors.
        session.reconcile_armed(&[], &[]);
        assert!(
            wire_injections(&session, "some-provider")
                .iter()
                .all(|i| matches!(
                    i,
                    CredentialInjection::Header { value, .. } if value.is_empty()
                )),
            "after disconnect the stored value must not keep arming, even though a route may still permit the destination"
        );
    }

    #[test]
    fn a_policy_reload_preserves_an_independent_slot_grant() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let session = CredentialSession::new(
            CredentialStateFile::new(),
            Arc::new(RecordingNotifier::default()),
            Arc::new(CapturingStore::default()),
            tx,
            TEST_TIMEOUT,
        )
        .with_slot_ids(HashSet::from(["some-slot".to_string()]))
        .with_armed_ids(HashSet::from([
            "some-slot".to_string(),
            "gitlab".to_string(),
        ]));
        session.reconcile_armed(&[], &["gitlab".to_string()]);
        assert_eq!(
            session.armed_ids(),
            HashSet::from(["gitlab".to_string(), "some-slot".to_string()]),
            "the reloaded connector and the slot grant both hold"
        );
        session.reconcile_armed(&[], &[]);
        assert_eq!(
            session.armed_ids(),
            HashSet::from(["some-slot".to_string()]),
            "a credential slot is an artifact contract, never revoked by a policy edit"
        );
    }

    #[test]
    fn a_slot_consented_at_first_use_survives_an_unrelated_policy_reload() {
        let notifier = Arc::new(RecordingNotifier::default());
        let store = Arc::new(CapturingStore::default());
        let (tx, _rx) = mpsc::unbounded_channel();
        let session = CredentialSession::new(
            CredentialStateFile::new(),
            notifier,
            store,
            tx,
            TEST_TIMEOUT,
        )
        .with_custom_providers(Arc::new(vec![some_provider()]))
        .with_slot_ids(HashSet::from(["some-provider".to_string()]));
        assert!(
            !session.armed_ids().contains("some-provider"),
            "an ungranted slot boots unarmed"
        );
        session.submit_pending(pending("c1", "some-provider"), Instant::now());
        session.record_decision(
            "c1",
            CredentialDecisionRequest::Allow(CredentialEntry::Stored {
                value: "some-real".into(),
            }),
        );
        assert!(
            session.armed_ids().contains("some-provider"),
            "answering the first-use card arms the slot for the run"
        );
        // An unrelated policy write (e.g. an allow-always network rule) triggers the watcher with no slots in the reloaded overlay and no grant on disk.
        session.reconcile_armed(&[], &[]);
        assert!(
            session.armed_ids().contains("some-provider"),
            "a slot consented at first use must survive a policy reload it has nothing to do with; the policy file has no authority over an artifact-declared slot"
        );
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn a_host_detect_connect_releases_the_held_request_when_the_host_resolves() {
        let _g = crate::test_env::EnvVarGuard::set("SOME_TOKEN", "host-real");
        let (session, notifier, mut rx) =
            unconsented_session_with(vec![("some-provider", CredentialEntry::HostDetect)]);
        session.submit_pending(
            gate(
                "some-provider",
                "POST https://api.some-provider.example/issues",
            ),
            Instant::now(),
        );
        assert!(session.connect_connector_now("some-provider").await);
        assert_eq!(
            decision_frame(&mut rx).decision,
            CredentialDecisionKind::Allow,
            "a host-detect entry that resolves to a real value is armed on the wire, so the held card asks an answered question"
        );
        assert_eq!(
            notifier.dismissed.lock().unwrap().as_slice(),
            &["c1".to_string()]
        );
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn a_host_detect_connect_keeps_the_held_request_when_the_host_is_empty() {
        let _g = crate::test_env::EnvVarGuard::unset("SOME_TOKEN");
        let (session, notifier, mut rx) =
            unconsented_session_with(vec![("some-provider", CredentialEntry::HostDetect)]);
        session.submit_pending(
            gate(
                "some-provider",
                "POST https://api.some-provider.example/issues",
            ),
            Instant::now(),
        );
        assert!(session.connect_connector_now("some-provider").await);
        assert!(
            rx.try_recv().is_err(),
            "the host has no value, so nothing is armed and nothing may release"
        );
        assert!(notifier.dismissed.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_network_offer_connect_without_a_value_keeps_the_held_value_decision() {
        let (session, notifier, mut rx) = unconsented_session_with(Vec::new());
        session.submit_pending(
            gate(
                "some-provider",
                "POST https://api.some-provider.example/issues",
            ),
            Instant::now(),
        );
        assert!(session.connect_connector_now("some-provider").await);
        assert!(
            rx.try_recv().is_err(),
            "no value is armed yet, so nothing may release"
        );
        assert!(
            notifier.dismissed.lock().unwrap().is_empty(),
            "the value decision is still owed; the card stays"
        );
    }

    #[test]
    fn submit_pending_still_prompts_an_armed_credential_on_a_host_it_does_not_inject_into() {
        let (s, n, _rx) = armed_session(
            vec![("some-oauth", armed_oauth("some-token"))],
            vec![some_oauth_provider()],
        );
        s.submit_pending(
            gate("some-oauth", "POST https://evil.example/exfil"),
            Instant::now(),
        );
        assert_eq!(
            n.presented.lock().unwrap().len(),
            1,
            "sending the placeholder to a host with no injection is a real leak attempt and must still prompt"
        );
    }

    #[test]
    fn submit_pending_auto_allows_an_armed_stored_credential_on_its_host() {
        let (s, n, mut rx) = armed_session(
            vec![(
                "some-provider",
                CredentialEntry::Stored {
                    value: "some-real".into(),
                },
            )],
            vec![some_provider()],
        );
        s.submit_pending(
            gate(
                "some-provider",
                "POST https://api.some-provider.example/issues",
            ),
            Instant::now(),
        );
        assert!(n.presented.lock().unwrap().is_empty());
        assert_eq!(
            decision_frame(&mut rx).decision,
            CredentialDecisionKind::Allow
        );
    }

    #[test]
    fn submit_pending_prompts_when_the_armed_oauth_value_is_empty() {
        let (s, n, _rx) = armed_session(
            vec![("some-oauth", armed_oauth(""))],
            vec![some_oauth_provider()],
        );
        s.submit_pending(
            gate("some-oauth", "GET api.some-oauth.example/"),
            Instant::now(),
        );
        assert_eq!(
            n.presented.lock().unwrap().len(),
            1,
            "an empty token is not a usable armed value"
        );
    }

    #[test]
    fn submit_pending_prompts_when_the_action_has_no_parseable_host() {
        let (s, n, _rx) = armed_session(
            vec![("some-oauth", armed_oauth("some-token"))],
            vec![some_oauth_provider()],
        );
        s.submit_pending(gate("some-oauth", "malformed"), Instant::now());
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
        // The proxy emits a credential gate as `METHOD <absolute-url>`, so the scheme must be stripped before the host.
        assert_eq!(
            request_host("POST https://api.some-oauth.example/issues"),
            Some("api.some-oauth.example")
        );
        assert_eq!(
            request_host("GET https://some-oauth.example/graphql"),
            Some("some-oauth.example")
        );
        assert_eq!(
            request_host("GET api.some-oauth.example/x"),
            Some("api.some-oauth.example")
        );
        assert_eq!(
            request_host("CONNECT api.some-oauth.example:443"),
            Some("api.some-oauth.example")
        );
        assert_eq!(
            request_host("POST some-oauth.example/graphql"),
            Some("some-oauth.example")
        );
        assert_eq!(request_host("malformed"), None);
        assert_eq!(request_host("GET /onlypath"), None);
    }

    #[test]
    fn injection_targets_host_matches_header_and_uri_placeholder_domains() {
        let header = CredentialInjection::Header {
            domain: "api.some-oauth.example".into(),
            header: "Authorization".into(),
            value: "token x".into(),
        };
        assert!(injection_targets_host(&header, "api.some-oauth.example"));
        assert!(!injection_targets_host(&header, "evil.example"));
        let uri = CredentialInjection::UriPlaceholder {
            domain: "api.telegram.org".into(),
            value: "v".into(),
        };
        assert!(injection_targets_host(&uri, "api.telegram.org"));
        assert!(!injection_targets_host(&uri, "evil.example"));
    }
}
