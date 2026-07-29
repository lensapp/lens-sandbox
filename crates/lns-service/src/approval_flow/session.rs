use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::approval_flow::protocol::{
    Credential, Decision, HostFrame, PolicyMessage, RequestDecision, RequestPending, Treatment,
};
use crate::ledger::LedgerRecorder;
use lns_ipc::{ApprovalKind, Decision as LedgerDecision, LedgerEvent};
use lns_policy::connectors::TokenFallback;
use lns_policy::matching::{domain_matches, split_destination, unbracketed};
use lns_policy::{Approval, Policy, PolicyStore, RouteRule, TcpEgressRule};

pub type FrameSink = mpsc::UnboundedSender<HostFrame>;

/// Supplies the registry credentials packed into every emitted `Policy` frame so a network decision is never read upstream as "drop all credentials".
pub type CredentialsProvider = Box<dyn Fn() -> Vec<Credential> + Send + Sync>;

/// Maps a reloaded policy's `connectors:` ids to their catalog routes, so a load that records only the ids gets those routes back live — the boot path and the file watcher derive them the same way.
pub type ConnectorRouteDeriver = Box<dyn Fn(&[String]) -> Vec<RouteRule> + Send + Sync>;

/// Invoked on a policy reload with the reloaded connected-connector ids so the credential subsystem can revoke a disconnected connector's arming.
pub type ArmedReconciler = Box<dyn Fn(&[String]) + Send + Sync>;

/// A connectable connector whose routes aren't allowed yet, so a held request to one of its `patterns` offers to connect it before the plain allow/deny.
pub struct OfferableConnector {
    pub id: String,
    pub display_name: String,
    pub patterns: Vec<String>,
    pub token_fallback: Option<TokenFallback>,
}

/// Connects a connector interactively and reports whether it is now connected; injected so the approval flow can offer a connect without owning the credential machinery. `connect_with_token` arms a pasted token instead of running the interactive sign-in.
pub trait ConnectPort: Send + Sync {
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
    /// Signals that an accepted offer's connect has resolved (either way), so any surface holding the card's slot can release it; default no-op for notifiers without one.
    fn connect_finished(&self, display_name: &str) {
        let _ = display_name;
    }
    /// Drops every in-flight connect placeholder on run teardown so a connect that never resolves can't keep the window pinned; default no-op.
    fn clear_all_connecting(&self) {}
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingPrompt {
    pub id: String,
    pub host: String,
    pub action: String,
    /// Some(display name) when `host` matches a connectable connector, so the card offers to connect it before the plain allow/deny.
    pub offer: Option<String>,
    /// Some when the offered connector declares a token fallback, so the offer card can also reveal "use a token instead".
    pub token_fallback: Option<TokenFallback>,
    /// `Raw` when approving splices the connection through unread, which the card has to say out loud.
    pub treatment: Treatment,
}

impl PendingPrompt {
    /// The destination badges the card shows, with `RAW` leading when nothing between the workload and the destination can read the traffic.
    pub fn badges(&self) -> Vec<String> {
        let mut badges = Vec::new();
        if self.treatment == Treatment::Raw {
            badges.push("RAW".to_string());
        }
        match self.action_port() {
            Some(port) => badges.extend(["TCP".to_string(), port.to_string()]),
            None => badges.push(self.action.clone()),
        }
        badges
    }

    fn action_port(&self) -> Option<&str> {
        let (_, port) = split_destination(&self.action);
        port.filter(|port| port.parse::<u16>().is_ok())
    }

    /// The one line a raw card must carry, because approving it grants traffic lns will never see.
    pub fn caption(&self) -> Option<&'static str> {
        match self.treatment {
            Treatment::Raw => Some(RAW_SPLICE_CAPTION),
            Treatment::Inspected => None,
        }
    }
}

const RAW_SPLICE_CAPTION: &str = "lns cannot inspect this traffic or inject credentials.";

#[derive(Debug)]
struct PendingEntry {
    host: String,
    /// The gate's own name for the destination (`CONNECT db.internal:5432`) — the only place the port survives, since `host` reaches us already stripped of it.
    action: String,
    treatment: Treatment,
    deadline: Instant,
    offer: Option<OfferRef>,
    reason: String,
}

impl PendingEntry {
    /// What the audit chain records as the destination: a raw splice is granted per port, so the host alone would understate the grant.
    fn audit_target(&self) -> &str {
        match self.treatment {
            Treatment::Raw => self.raw_destination().unwrap_or(&self.host),
            Treatment::Inspected => &self.host,
        }
    }

    fn raw_destination(&self) -> Option<&str> {
        raw_destination(&self.action, &self.host)
    }
}

#[derive(Debug, Clone)]
struct OfferRef {
    connector_id: String,
    display_name: String,
}

pub struct ApprovalSession {
    policy: Mutex<Policy>,
    pending: Mutex<HashMap<String, PendingEntry>>,
    notifier: Arc<dyn Notifier>,
    store: Arc<dyn PolicyStore>,
    sink: FrameSink,
    timeout: Duration,
    credentials_provider: OnceLock<CredentialsProvider>,
    connector_routes: OnceLock<ConnectorRouteDeriver>,
    armed_reconciler: OnceLock<ArmedReconciler>,
    offerable: Vec<OfferableConnector>,
    connector: OnceLock<Arc<dyn ConnectPort>>,
    connecting: Mutex<HashSet<String>>,
    ledger: OnceLock<Arc<dyn LedgerRecorder>>,
    policy_floor: OnceLock<Policy>,
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
            connector_routes: OnceLock::new(),
            armed_reconciler: OnceLock::new(),
            offerable: Vec::new(),
            connector: OnceLock::new(),
            connecting: Mutex::new(HashSet::new()),
            ledger: OnceLock::new(),
            policy_floor: OnceLock::new(),
        }
    }

    /// Captures the run's connectable connectors so a held request to one of their domains offers to connect before the plain allow/deny.
    pub fn with_offers(mut self, offerable: Vec<OfferableConnector>) -> Self {
        self.offerable = offerable;
        self
    }

    /// Installs the credentials closure once at boot; idempotent, the first provider wins.
    pub fn set_credentials_provider(&self, provider: CredentialsProvider) {
        let _ = self.credentials_provider.set(provider);
    }

    /// Installs the armed-reconciler once at boot so a watcher reload revokes a disconnected connector's arming; idempotent, the first wins.
    pub fn set_armed_reconciler(&self, reconciler: ArmedReconciler) {
        let _ = self.armed_reconciler.set(reconciler);
    }

    /// Installs the connector-route deriver once at boot so a watcher reload re-applies a connected connector's routes instead of dropping them; idempotent, the first wins.
    pub fn set_connector_route_deriver(&self, deriver: ConnectorRouteDeriver) {
        let _ = self.connector_routes.set(deriver);
    }

    /// Installs a sandbox's shipped policy as an always-merged floor, so a watcher reload of the local overlay can never drop the sandbox's rules; idempotent, the first wins.
    pub fn set_policy_floor(&self, floor: Policy) {
        let _ = self.policy_floor.set(floor);
    }

    /// Installs the connect port once the credential subsystem exists; idempotent, the first wins.
    pub fn set_connector(&self, connector: Arc<dyn ConnectPort>) {
        let _ = self.connector.set(connector);
    }

    pub fn set_ledger_recorder(&self, recorder: Arc<dyn LedgerRecorder>) {
        let _ = self.ledger.set(recorder);
    }

    fn offer_for_host(&self, host: &str) -> Option<&OfferableConnector> {
        self.offerable
            .iter()
            .find(|i| i.patterns.iter().any(|p| domain_matches(p, host)))
    }

    /// The (id, display name, token fallback) to offer for `host`, or `None` when nothing matches or the connector is already connected this run.
    fn offer_id_and_name_for(&self, host: &str) -> Option<(String, String, Option<TokenFallback>)> {
        let integ = self.offer_for_host(host)?;
        let already_connected = self
            .policy
            .lock()
            .expect("policy mutex poisoned")
            .connectors
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
        // Connecting a connector arms credential injection, which an opaque splice can never carry — and the offer card has no room for the RAW disclosure.
        let matched = (req.treatment == Treatment::Inspected)
            .then(|| self.offer_id_and_name_for(&req.host))
            .flatten();
        let offer_ref = matched.as_ref().map(|(id, name, _)| OfferRef {
            connector_id: id.clone(),
            display_name: name.clone(),
        });
        // While a connect for this connector is in flight, hold the request silently and let that connect's batch release it — a fresh offer card would only cover the sign-in card.
        let coalesced = offer_ref
            .as_ref()
            .is_some_and(|o| self.is_connecting(&o.connector_id));
        let mut pending = self.pending.lock().expect("pending mutex poisoned");
        if pending.contains_key(&req.id) {
            return;
        }
        pending.insert(
            req.id.clone(),
            PendingEntry {
                host: req.host.clone(),
                action: req.action.clone(),
                treatment: req.treatment,
                deadline: now + self.timeout,
                offer: offer_ref,
                reason: req.reason.clone(),
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
            treatment: req.treatment,
        });
    }

    pub fn record_decision(&self, id: &str, decision: Decision) -> DecisionOutcome {
        let Some(entry) = self.remove_pending(id) else {
            return DecisionOutcome::UnknownId;
        };
        self.notifier.dismiss(id);
        self.send_decision_frame(id, decision);
        self.persist_always_decision(&entry, decision);
        self.record_approval(&entry, decision);
        DecisionOutcome::Resolved
    }

    /// Fails a held request because its card was closed: no rule, no audit line, and `Timeout` on the wire because a dismissal is the absence of a decision rather than a deny the developer picked.
    pub fn dismiss_request(&self, id: &str) -> DecisionOutcome {
        if self.remove_pending(id).is_none() {
            return DecisionOutcome::UnknownId;
        }
        self.notifier.dismiss(id);
        self.send_decision_frame(id, Decision::Timeout);
        DecisionOutcome::Resolved
    }

    /// Writes the rule an "always" decision earns into the table its treatment belongs to; a once-decision earns none.
    fn persist_always_decision(&self, entry: &PendingEntry, decision: Decision) {
        match entry.treatment {
            Treatment::Inspected => {
                if let Some(rule) = rule_for_always_decision(&entry.host, decision) {
                    self.apply_persistent_rule(rule);
                }
            }
            Treatment::Raw => match entry.raw_destination() {
                Some(destination) => {
                    if let Some(rule) = tcp_rule_for_always_decision(destination, decision) {
                        self.apply_persistent_tcp_rule(rule);
                    }
                }
                None if earns_a_rule(decision) => self.report_no_rule_written(&format!(
                    "the gate named the destination {:?}, which this lns cannot read as a rule for {:?}",
                    entry.action, entry.host
                )),
                None => {}
            },
        }
    }

    fn record_approval(&self, entry: &PendingEntry, decision: Decision) {
        let Some(recorder) = self.ledger.get() else {
            return;
        };
        let Some(decision) = Self::ledger_decision(decision) else {
            return;
        };
        recorder.record(LedgerEvent::Approval {
            kind: ApprovalKind::Network,
            target: entry.audit_target().to_string(),
            decision,
            reason: (!entry.reason.is_empty()).then(|| entry.reason.clone()),
            connector: entry.offer.as_ref().map(|o| o.connector_id.clone()),
        });
    }

    fn record_offer_decision(&self, connector_id: &str, connected: bool) {
        let Some(recorder) = self.ledger.get() else {
            return;
        };
        recorder.record(LedgerEvent::Approval {
            kind: ApprovalKind::Connector,
            target: connector_id.to_string(),
            decision: if connected {
                LedgerDecision::AllowOnce
            } else {
                LedgerDecision::DenyOnce
            },
            reason: None,
            connector: Some(connector_id.to_string()),
        });
    }

    fn ledger_decision(decision: Decision) -> Option<LedgerDecision> {
        match decision {
            Decision::AllowAlways => Some(LedgerDecision::AllowAlways),
            Decision::AllowOnce => Some(LedgerDecision::AllowOnce),
            Decision::DenyAlways => Some(LedgerDecision::DenyAlways),
            Decision::DenyOnce => Some(LedgerDecision::DenyOnce),
            Decision::Timeout => None,
        }
    }

    fn remove_pending(&self, id: &str) -> Option<PendingEntry> {
        self.pending
            .lock()
            .expect("pending mutex poisoned")
            .remove(id)
    }

    /// Accepts a held request's connector offer via the interactive connect (oauth sign-in or a straight credential connect). See [`Self::connect_offer_with`].
    pub async fn connect_offer(&self, id: &str) -> DecisionOutcome {
        self.connect_offer_with(id, None).await
    }

    /// Accepts a held request's connector offer by arming a pasted token instead of the interactive connect. See [`Self::connect_offer_with`].
    pub async fn connect_offer_with_token(&self, id: &str, value: String) -> DecisionOutcome {
        self.connect_offer_with(id, Some(value)).await
    }

    /// Drives one connect for the offered connector and releases **every** held request for it — allow-once on success, deny-once closed on failure or a missing connector. A second card for the same connector coalesces onto the in-flight connect instead of starting another. `token` selects the pasted-token connect over the interactive one.
    async fn connect_offer_with(&self, id: &str, token: Option<String>) -> DecisionOutcome {
        let Some(offer) = self.offer_of(id) else {
            return DecisionOutcome::UnknownId;
        };
        if !self.begin_connecting(&offer.connector_id) {
            // Another card already started this connect; its batch will release this request too.
            self.notifier.dismiss(id);
            return DecisionOutcome::Resolved;
        }
        // Hide every offer card for this connector so the sign-in card isn't covered; the requests stay held for the batch release.
        for request_id in self.offer_request_ids(&offer.connector_id) {
            self.notifier.dismiss(&request_id);
        }
        let connected = match self.connector.get() {
            Some(connector) => match token {
                Some(value) => {
                    connector
                        .connect_with_token(&offer.connector_id, value)
                        .await
                }
                None => connector.connect(&offer.connector_id).await,
            },
            None => false,
        };
        self.finish_connecting(&offer.connector_id);
        self.notifier.connect_finished(&offer.display_name);
        let decision = if connected {
            Decision::AllowOnce
        } else {
            Decision::DenyOnce
        };
        self.record_offer_decision(&offer.connector_id, connected);
        for request_id in self.drain_offer_requests(&offer.connector_id) {
            self.send_decision_frame(&request_id, decision);
        }
        DecisionOutcome::Resolved
    }

    /// The offer a held entry carries, if any; does not remove the entry.
    fn offer_of(&self, id: &str) -> Option<OfferRef> {
        self.pending
            .lock()
            .expect("pending mutex poisoned")
            .get(id)?
            .offer
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

    fn offer_request_ids(&self, connector_id: &str) -> Vec<String> {
        self.pending
            .lock()
            .expect("pending mutex poisoned")
            .iter()
            .filter(|(_, e)| offers_connector(e, connector_id))
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Removes and returns every held request offering `connector_id`.
    fn drain_offer_requests(&self, connector_id: &str) -> Vec<String> {
        let mut pending = self.pending.lock().expect("pending mutex poisoned");
        let ids: Vec<String> = pending
            .iter()
            .filter(|(_, e)| offers_connector(e, connector_id))
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
                // A request offering a connector that's mid sign-in must not be swept; its connect releases it.
                .filter(|(_, entry)| {
                    entry
                        .offer
                        .as_ref()
                        .is_none_or(|o| !connecting.contains(&o.connector_id))
                })
                .map(|(id, _)| id.clone())
                .collect()
        };
        expired.iter().filter(|id| self.timeout_one(id)).count()
    }

    fn timeout_one(&self, id: &str) -> bool {
        let Some(entry) = self.remove_pending(id) else {
            return false;
        };
        self.notifier.dismiss(id);
        // A request clicked into a connect right as it expired leaves a placeholder no later connect will resolve; take it down with the request.
        if let Some(offer) = &entry.offer {
            self.notifier.connect_finished(&offer.display_name);
        }
        self.send_decision_frame(id, Decision::Timeout);
        true
    }

    pub fn apply_external_policy(&self, mut new_policy: Policy) {
        if let Some(floor) = self.policy_floor.get() {
            new_policy = crate::artifact::policy::merge_effective(Some(floor), &new_policy);
        }
        if let Some(derive) = self.connector_routes.get() {
            new_policy
                .network
                .egress
                .http
                .extend(derive(&new_policy.connectors));
        }
        if let Some(reconcile) = self.armed_reconciler.get() {
            reconcile(&new_policy.connectors);
        }
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
        self.notifier.clear_all_connecting();
    }

    fn send_decision_frame(&self, id: &str, decision: Decision) {
        let _ = self.sink.send(HostFrame::RequestDecision(RequestDecision {
            id: id.to_string(),
            decision,
        }));
    }

    fn apply_persistent_rule(&self, rule: RouteRule) {
        let (approval, snapshot) = {
            let mut policy = self.policy.lock().expect("policy mutex poisoned");
            (policy.add_approved_rule(rule), policy.clone())
        };
        self.publish_if_it_stands(approval, snapshot);
    }

    /// Says the decision stands for this request but outlived nothing, because a rule the guest cannot parse force-denies the whole policy and so is never written at all.
    fn report_no_rule_written(&self, why: &str) {
        self.notifier.inform(&format!(
            "decision applied to this request only; no policy rule could be written: {why}"
        ));
    }

    fn apply_persistent_tcp_rule(&self, rule: TcpEgressRule) {
        // One rule lens-sandbox-core cannot parse force-denies the whole policy in the guest, so a destination we cannot express is not written at all.
        if let Err(e) = rule.validate() {
            self.report_no_rule_written(&e);
            return;
        }
        let (approval, pre_empted, snapshot) = {
            let mut policy = self.policy.lock().expect("policy mutex poisoned");
            let pre_empted = pre_empted_http_patterns(&policy, &rule);
            (
                policy.add_approved_tcp_rule(rule),
                pre_empted,
                policy.clone(),
            )
        };
        if !self.publish_if_it_stands(approval, snapshot) {
            return;
        }
        // The raw table is consulted first, so this rule is what decides that port now; the http rules it displaces would otherwise go quiet without a word.
        if !pre_empted.is_empty() {
            self.notifier.inform(&format!(
                "that traffic is now spliced raw, so these HTTP rules no longer apply to it: {}",
                pre_empted.join(", ")
            ));
        }
    }

    /// A rule the gate would never reach is not written, so the developer hears that the answer they gave applied to that one request — silence would read as "remembered". Answers whether the decision stands.
    fn publish_if_it_stands(&self, approval: Approval, snapshot: Policy) -> bool {
        let why = match approval {
            Approval::Stands => {
                self.publish_and_persist(snapshot);
                return true;
            }
            Approval::Shadowed(pattern) => format!(
                "the rule for {pattern:?} already decides this destination and the guest stops at the first matching rule"
            ),
            Approval::Unreachable(pattern) => format!(
                "this exact rule is already in the policy file, but behind the rule for {pattern:?} that the guest reaches first — move it ahead of that rule to stop being asked"
            ),
        };
        self.report_no_rule_written(&why);
        false
    }

    /// Hands the updated policy to the running guest first and to disk second, so a file that cannot be written still leaves the decision live for the rest of the run.
    fn publish_and_persist(&self, snapshot: Policy) {
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

    /// Connects a connector live: records the id under `connectors:` and persists only that, while the routes are applied to the in-memory policy and emitted so a held request sees them — boot re-derives the routes from the catalog, so persisting them would leave a residual allow that `disconnect` can't revoke.
    pub fn connect_connector(&self, id: &str, routes: Vec<RouteRule>) {
        let (to_persist, live_network) = {
            let mut policy = self.policy.lock().expect("policy mutex poisoned");
            policy.connect(id);
            let to_persist = policy.clone();
            policy.network.egress.http.extend(routes);
            (to_persist, policy.network.clone())
        };
        let credentials = self.current_credentials();
        let _ = self.sink.send(HostFrame::Policy(PolicyMessage {
            network: Some(live_network),
            credentials,
        }));
        if let Err(e) = self.store.save(&to_persist) {
            self.notifier.inform(&format!(
                "connector connected in-memory but not persisted: {e}"
            ));
        }
        // The routes are live above, so requests held on this connector's offer proceed no matter which surface the consent came from.
        for request_id in self.drain_offer_requests(id) {
            self.notifier.dismiss(&request_id);
            self.send_decision_frame(&request_id, Decision::AllowOnce);
        }
    }
}

fn offers_connector(entry: &PendingEntry, connector_id: &str) -> bool {
    entry
        .offer
        .as_ref()
        .is_some_and(|o| o.connector_id == connector_id)
}

fn rule_for_always_decision(host: &str, decision: Decision) -> Option<RouteRule> {
    match decision {
        Decision::AllowAlways => Some(RouteRule::allow_host(host)),
        Decision::DenyAlways => Some(RouteRule::deny_host(host)),
        Decision::AllowOnce | Decision::DenyOnce | Decision::Timeout => None,
    }
}

fn pre_empted_http_patterns(policy: &Policy, rule: &TcpEgressRule) -> Vec<String> {
    policy
        .network
        .http_rules_pre_empted_by(rule)
        .iter()
        .map(|route| format!("{:?}", route.match_pattern))
        .collect()
}

fn tcp_rule_for_always_decision(destination: &str, decision: Decision) -> Option<TcpEgressRule> {
    match decision {
        Decision::AllowAlways => Some(TcpEgressRule::allow_destination(destination)),
        Decision::DenyAlways => Some(TcpEgressRule::deny_destination(destination)),
        Decision::AllowOnce | Decision::DenyOnce | Decision::Timeout => None,
    }
}

/// Whether the developer asked for a decision that outlives this request, and so is owed an explanation when none can be written.
fn earns_a_rule(decision: Decision) -> bool {
    matches!(decision, Decision::AllowAlways | Decision::DenyAlways)
}

/// The gate's `CONNECT <destination>` taken verbatim, and `None` unless it names the frame's own host — a rule built from a misread action force-denies the whole policy in the guest.
fn raw_destination<'a>(action: &'a str, host: &str) -> Option<&'a str> {
    let destination = action.strip_prefix("CONNECT ")?;
    // Whether a port is present is validation's complaint to make, and a more specific one.
    let (named, _) = split_destination(destination);
    // The gate strips the port from `host` before sending it, so only its brackets come off here.
    (named == unbracketed(host)).then_some(destination)
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
        pub(crate) connects_finished: StdMutex<Vec<String>>,
        pub(crate) all_connecting_cleared: StdMutex<usize>,
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

    #[derive(Default)]
    pub(crate) struct CapturingRecorder {
        pub(crate) events: StdMutex<Vec<LedgerEvent>>,
    }

    impl crate::ledger::LedgerRecorder for CapturingRecorder {
        fn record(&self, event: LedgerEvent) {
            self.events.lock().unwrap().push(event);
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
        fixture_over(Policy::default(), timeout)
    }

    fn fixture_holding(policy: Policy) -> Fixture {
        fixture_over(policy, TEST_TIMEOUT)
    }

    fn fixture_over(policy: Policy, timeout: Duration) -> Fixture {
        let notifier = Arc::new(RecordingNotifier::default());
        let store = Arc::new(CapturingStore::default());
        let (tx, rx) = mpsc::unbounded_channel();
        let session = ApprovalSession::new(policy, notifier.clone(), store.clone(), tx, timeout);
        (session, notifier, store, rx)
    }

    fn pending(id: &str, host: &str) -> RequestPending {
        RequestPending {
            id: id.into(),
            host: host.into(),
            action: format!("CONNECT {host}:443"),
            reason: "policy-ambiguous".into(),
            treatment: Treatment::Inspected,
        }
    }

    fn raw_pending(id: &str, destination: &str) -> RequestPending {
        let host = destination
            .rsplit_once(':')
            .map_or(destination, |(host, _)| host);
        RequestPending {
            id: id.into(),
            host: host.into(),
            action: format!("CONNECT {destination}"),
            reason: "policy-ambiguous".into(),
            treatment: Treatment::Raw,
        }
    }

    fn only_approval(events: &[LedgerEvent]) -> &LedgerEvent {
        assert_eq!(events.len(), 1, "expected exactly one ledger event");
        &events[0]
    }

    #[test]
    fn a_network_decision_is_recorded_to_the_ledger_with_its_reason() {
        let (session, _n, _s, _rx) = fixture();
        let recorder = Arc::new(CapturingRecorder::default());
        session.set_ledger_recorder(recorder.clone());
        session.submit_pending(pending("r1", "api.foo.com"), Instant::now());
        assert_eq!(
            session.record_decision("r1", Decision::AllowAlways),
            DecisionOutcome::Resolved
        );
        let events = recorder.events.lock().unwrap();
        assert_eq!(
            *only_approval(&events),
            LedgerEvent::Approval {
                kind: ApprovalKind::Network,
                target: "api.foo.com".into(),
                decision: LedgerDecision::AllowAlways,
                reason: Some("policy-ambiguous".into()),
                connector: None,
            }
        );
    }

    #[test]
    fn a_decision_without_a_reason_records_none() {
        let (session, _n, _s, _rx) = fixture();
        let recorder = Arc::new(CapturingRecorder::default());
        session.set_ledger_recorder(recorder.clone());
        let mut req = pending("r1", "api.foo.com");
        req.reason = String::new();
        session.submit_pending(req, Instant::now());
        session.record_decision("r1", Decision::DenyOnce);
        let events = recorder.events.lock().unwrap();
        assert_eq!(
            *only_approval(&events),
            LedgerEvent::Approval {
                kind: ApprovalKind::Network,
                target: "api.foo.com".into(),
                decision: LedgerDecision::DenyOnce,
                reason: None,
                connector: None,
            }
        );
    }

    #[test]
    fn a_raw_decision_is_recorded_against_the_port_it_actually_granted() {
        let (session, _n, _s, _rx) = fixture();
        let recorder = Arc::new(CapturingRecorder::default());
        session.set_ledger_recorder(recorder.clone());
        session.submit_pending(raw_pending("r1", "db.internal:5432"), Instant::now());
        session.record_decision("r1", Decision::AllowAlways);
        let events = recorder.events.lock().unwrap();
        assert_eq!(
            *only_approval(&events),
            LedgerEvent::Approval {
                kind: ApprovalKind::Network,
                target: "db.internal:5432".into(),
                decision: LedgerDecision::AllowAlways,
                reason: Some("policy-ambiguous".into()),
                connector: None,
            },
            "a raw grant is per port; recording the bare host would overstate what was audited"
        );
    }

    #[test]
    fn a_raw_and_an_inspected_ask_for_one_destination_each_get_their_own_card() {
        let (session, notifier, _s, _rx) = fixture();
        session.submit_pending(pending("r-inspected", "db.internal"), Instant::now());
        session.submit_pending(raw_pending("r-raw", "db.internal:5432"), Instant::now());
        let presented = notifier.presented.lock().unwrap();
        assert_eq!(
            presented
                .iter()
                .map(|p| (p.id.as_str(), p.treatment))
                .collect::<Vec<_>>(),
            vec![
                ("r-inspected", Treatment::Inspected),
                ("r-raw", Treatment::Raw)
            ],
            "the two grants are not interchangeable, so neither may be answered by the other's card"
        );
    }

    #[test]
    fn a_raw_card_leads_with_raw_and_says_what_lns_cannot_do() {
        let (session, notifier, _s, _rx) = fixture();
        session.submit_pending(raw_pending("r1", "db.internal:5432"), Instant::now());
        let presented = notifier.presented.lock().unwrap();
        let prompt = presented.last().expect("a card");
        assert_eq!(prompt.badges(), vec!["RAW", "TCP", "5432"]);
        assert_eq!(
            prompt.caption(),
            Some("lns cannot inspect this traffic or inject credentials.")
        );
    }

    #[test]
    fn an_inspected_card_carries_no_raw_badge_and_no_caption() {
        let (session, notifier, _s, _rx) = fixture();
        session.submit_pending(pending("r1", "api.example.test"), Instant::now());
        let presented = notifier.presented.lock().unwrap();
        let prompt = presented.last().expect("a card");
        assert_eq!(prompt.badges(), vec!["TCP", "443"]);
        assert_eq!(prompt.caption(), None);
    }

    #[test]
    fn a_card_for_an_action_with_no_port_shows_the_action_itself() {
        let (session, notifier, _s, _rx) = fixture();
        let mut req = pending("r1", "api.example.test");
        req.action = "CONNECT api.example.test".into();
        session.submit_pending(req, Instant::now());
        let mut raw = raw_pending("r2", "db.internal");
        raw.action = "CONNECT db.internal".into();
        session.submit_pending(raw, Instant::now());
        let presented = notifier.presented.lock().unwrap();
        assert_eq!(
            presented[0].badges(),
            vec!["CONNECT api.example.test"],
            "a badge row invented from a port we do not have would be a lie about the grant"
        );
        assert_eq!(
            presented[1].badges(),
            vec!["RAW", "CONNECT db.internal"],
            "the port is what we could not read; that the traffic is opaque is still true"
        );
    }

    #[test]
    fn allow_always_on_a_raw_splice_writes_the_port_scoped_destination() {
        let (session, _n, store, _rx) = fixture();
        session.submit_pending(raw_pending("r1", "db.internal:5432"), Instant::now());
        session.record_decision("r1", Decision::AllowAlways);
        let saved = store.saves.lock().unwrap();
        let policy = saved.last().expect("the decision is persisted");
        assert_eq!(
            policy.network.egress.tcp,
            vec![TcpEgressRule::allow_destination("db.internal:5432")]
        );
        assert!(
            policy.network.egress.http.is_empty(),
            "a raw grant in the inspected table would be silently ignored by the pre-filter"
        );
    }

    #[test]
    fn allow_always_on_a_bracketed_ipv6_splice_keeps_the_destination_verbatim() {
        let (session, _n, store, _rx) = fixture();
        session.submit_pending(raw_pending("r1", "[2001:db8::1]:5432"), Instant::now());
        session.record_decision("r1", Decision::AllowAlways);
        let saved = store.saves.lock().unwrap();
        assert_eq!(
            saved.last().expect("saved").network.egress.tcp,
            vec![TcpEgressRule::allow_destination("[2001:db8::1]:5432")],
            "re-deriving the pattern from host and port is how a bracketed literal gets mangled"
        );
    }

    #[test]
    fn a_raw_destination_that_cannot_be_expressed_is_not_written_and_is_reported() {
        let (session, notifier, store, _rx) = fixture();
        session.submit_pending(raw_pending("r1", "db.internal"), Instant::now());
        session.record_decision("r1", Decision::AllowAlways);
        assert!(
            store.saves.lock().unwrap().is_empty(),
            "one unparseable rule force-denies the whole policy inside the guest"
        );
        let informed = notifier.informed.lock().unwrap();
        assert_eq!(
            *informed,
            vec![
                "decision applied to this request only; no policy rule could be written: egress.tcp rule \"db.internal\" must specify a port, e.g. \"host:443\" or \"10.0.0.0/24:443\"".to_string()
            ]
        );
    }

    #[test]
    fn a_raw_action_the_host_does_not_vouch_for_writes_no_rule_and_is_reported() {
        let (session, notifier, store, _rx) = fixture();
        let mut req = raw_pending("r1", "db.internal:5432");
        req.action = "TCP db.internal:5432".into();
        session.submit_pending(req, Instant::now());
        session.record_decision("r1", Decision::AllowAlways);
        assert!(
            store.saves.lock().unwrap().is_empty(),
            "an action we misread yields a pattern core cannot parse, and one of those force-denies the whole policy"
        );
        assert_eq!(
            notifier.informed.lock().unwrap().len(),
            1,
            "the developer has to hear that the decision outlived nothing"
        );
    }

    #[test]
    fn a_raw_action_naming_a_different_host_than_the_frame_writes_no_rule() {
        let (session, _n, store, _rx) = fixture();
        let mut req = raw_pending("r1", "db.internal:5432");
        req.action = "CONNECT evil.example:5432".into();
        session.submit_pending(req, Instant::now());
        session.record_decision("r1", Decision::AllowAlways);
        assert!(
            store.saves.lock().unwrap().is_empty(),
            "the two fields disagree, so neither is trustworthy enough to splice a destination open forever"
        );
    }

    #[test]
    fn a_raw_action_the_host_does_not_vouch_for_is_audited_against_the_host() {
        let (session, _n, _s, _rx) = fixture();
        let recorder = Arc::new(CapturingRecorder::default());
        session.set_ledger_recorder(recorder.clone());
        let mut req = raw_pending("r1", "db.internal:5432");
        req.action = "TCP db.internal:5432".into();
        session.submit_pending(req, Instant::now());
        session.record_decision("r1", Decision::AllowOnce);
        let events = recorder.events.lock().unwrap();
        assert!(
            matches!(only_approval(&events), LedgerEvent::Approval { target, .. } if target.as_str() == "db.internal"),
            "an unreadable action still has to leave an audit record, and the host is the part the frame vouched for: {events:?}"
        );
    }

    #[test]
    fn a_once_decision_on_an_unreadable_raw_action_says_nothing() {
        let (session, notifier, _s, _rx) = fixture();
        let mut req = raw_pending("r1", "db.internal:5432");
        req.action = "TCP db.internal:5432".into();
        session.submit_pending(req, Instant::now());
        session.record_decision("r1", Decision::AllowOnce);
        assert!(
            notifier.informed.lock().unwrap().is_empty(),
            "a once-decision was never going to write a rule, so there is nothing to apologise for"
        );
    }

    #[test]
    fn a_once_decision_on_a_raw_splice_writes_no_rule() {
        let (session, _n, store, _rx) = fixture();
        session.submit_pending(raw_pending("r1", "db.internal:5432"), Instant::now());
        assert_eq!(
            session.record_decision("r1", Decision::AllowOnce),
            DecisionOutcome::Resolved
        );
        assert!(store.saves.lock().unwrap().is_empty());
    }

    #[test]
    fn deny_always_on_a_raw_splice_writes_a_raw_deny() {
        let (session, _n, store, _rx) = fixture();
        session.submit_pending(raw_pending("r1", "db.internal:5432"), Instant::now());
        session.record_decision("r1", Decision::DenyAlways);
        let saved = store.saves.lock().unwrap();
        assert_eq!(
            saved.last().expect("saved").network.egress.tcp,
            vec![TcpEgressRule::deny_destination("db.internal:5432")]
        );
    }

    #[test]
    fn a_second_always_decision_the_first_ones_rule_pre_empts_writes_nothing_and_says_so() {
        let mut asking = Policy::default();
        asking.add_rule(RouteRule {
            verdict: Verdict::Ask,
            ..RouteRule::allow_host("api.example.test")
        });
        let (session, notifier, store, _rx) = fixture_holding(asking);
        session.submit_pending(pending("r1", "api.example.test"), Instant::now());
        session.submit_pending(pending("r2", "api.example.test"), Instant::now());
        session.record_decision("r1", Decision::AllowAlways);
        session.record_decision("r2", Decision::DenyAlways);
        let saved = store.saves.lock().unwrap();
        assert_eq!(
            saved
                .last()
                .expect("the first decision is persisted")
                .network
                .egress
                .http
                .iter()
                .map(|r| r.verdict)
                .collect::<Vec<_>>(),
            vec![Verdict::Allow, Verdict::Ask],
            "a deny written behind the allow the first card wrote never fires, so it is not written: {saved:?}"
        );
        assert_eq!(
            *notifier.informed.lock().unwrap(),
            vec![
                "decision applied to this request only; no policy rule could be written: the rule for \"api.example.test\" already decides this destination and the guest stops at the first matching rule".to_string()
            ],
            "silence here reads as remembered, and the developer's always-deny was not"
        );
    }

    #[test]
    fn a_raw_always_deny_a_standing_raw_allow_pre_empts_writes_nothing_and_says_so() {
        let mut standing = Policy::default();
        standing
            .network
            .egress
            .tcp
            .push(TcpEgressRule::allow_destination("db.internal:5432"));
        let (session, notifier, store, _rx) = fixture_holding(standing);
        session.submit_pending(raw_pending("r1", "db.internal:5432"), Instant::now());
        session.record_decision("r1", Decision::DenyAlways);
        assert!(
            store.saves.lock().unwrap().is_empty(),
            "the standing raw allow is what the gate reaches, so a deny behind it is a line that never fires"
        );
        assert_eq!(notifier.informed.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_timeout_is_not_recorded_as_a_decision() {
        let (session, _n, _s, _rx) = fixture();
        let recorder = Arc::new(CapturingRecorder::default());
        session.set_ledger_recorder(recorder.clone());
        session.submit_pending(pending("r1", "api.foo.com"), Instant::now());
        session.record_decision("r1", Decision::Timeout);
        assert!(recorder.events.lock().unwrap().is_empty());
    }

    #[test]
    fn ledger_decision_maps_every_user_decision_and_drops_timeout() {
        assert_eq!(
            ApprovalSession::ledger_decision(Decision::AllowOnce),
            Some(LedgerDecision::AllowOnce)
        );
        assert_eq!(
            ApprovalSession::ledger_decision(Decision::AllowAlways),
            Some(LedgerDecision::AllowAlways)
        );
        assert_eq!(
            ApprovalSession::ledger_decision(Decision::DenyOnce),
            Some(LedgerDecision::DenyOnce)
        );
        assert_eq!(
            ApprovalSession::ledger_decision(Decision::DenyAlways),
            Some(LedgerDecision::DenyAlways)
        );
        assert_eq!(ApprovalSession::ledger_decision(Decision::Timeout), None);
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
    fn a_dismissed_card_fails_the_request_closed_and_records_no_approval() {
        let (s, n, store, mut rx) = fixture();
        let recorder = Arc::new(CapturingRecorder::default());
        s.set_ledger_recorder(recorder.clone());
        s.submit_pending(pending("r1", "api.linear.app"), Instant::now());

        let before = s.current_policy();
        assert_eq!(s.dismiss_request("r1"), DecisionOutcome::Resolved);

        assert_eq!(before, s.current_policy());
        assert_eq!(
            decision_frame(&mut rx).decision,
            Decision::Timeout,
            "a closed card reads on the wire as no decision, so it cannot arrive as a deny-once the developer picked"
        );
        assert_eq!(n.dismissed.lock().unwrap().as_slice(), &["r1".to_string()]);
        assert!(store.saves.lock().unwrap().is_empty());
        assert!(
            recorder.events.lock().unwrap().is_empty(),
            "a swatted card must not read as a deny-once the developer picked"
        );
    }

    #[test]
    fn dismissing_an_unknown_request_is_unknownid() {
        let (s, _n, _store, mut rx) = fixture();
        assert_eq!(s.dismiss_request("never-held"), DecisionOutcome::UnknownId);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn allow_always_adds_allow_rule_persists_and_emits_policy_frame() {
        let (s, _n, store, mut rx) = fixture();
        s.submit_pending(pending("r1", "api.linear.app"), Instant::now());

        s.record_decision("r1", Decision::AllowAlways);

        let routes = s.current_policy().network.egress.http;
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].match_pattern, "api.linear.app");
        assert_eq!(routes[0].verdict, Verdict::Allow);

        assert_eq!(decision_frame(&mut rx).decision, Decision::AllowAlways);
        let pushed = policy_frame(&mut rx);
        assert_eq!(
            pushed.network.unwrap().egress.http[0].match_pattern,
            "api.linear.app"
        );

        let saves = store.saves.lock().unwrap();
        assert_eq!(saves.len(), 1);
        assert_eq!(
            saves[0].network.egress.http[0].match_pattern,
            "api.linear.app"
        );
    }

    #[test]
    fn deny_always_adds_deny_rule_persists_and_emits_policy_frame() {
        let (s, _n, store, mut rx) = fixture();
        s.submit_pending(pending("r1", "evil.example"), Instant::now());

        s.record_decision("r1", Decision::DenyAlways);

        let routes = s.current_policy().network.egress.http;
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].verdict, Verdict::Deny);

        assert_eq!(decision_frame(&mut rx).decision, Decision::DenyAlways);
        let pushed = policy_frame(&mut rx);
        assert_eq!(
            pushed.network.unwrap().egress.http[0].verdict,
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

        let routes = s.current_policy().network.egress.http;
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
    fn withdraw_run_clears_connecting_placeholders_so_window_does_not_stay_pinned() {
        let (s, n, _store, _rx) = fixture();
        s.submit_pending(pending("r1", "a"), Instant::now());

        s.withdraw_run();

        assert_eq!(
            *n.all_connecting_cleared.lock().unwrap(),
            1,
            "an in-flight connect placeholder must not survive the run that started it"
        );
    }

    #[test]
    fn timeout_clears_a_connecting_placeholder_for_an_expired_offer() {
        let (s, n, _rx) = offer_session(
            vec![offerable("some-oauth", "GitHub", "api.some-oauth.example")],
            None,
        );
        let t0 = Instant::now();
        s.submit_pending(pending("r1", "api.some-oauth.example"), t0);

        s.tick_timeouts(t0 + TEST_TIMEOUT + Duration::from_secs(1));

        assert_eq!(
            n.connects_finished.lock().unwrap().as_slice(),
            &["GitHub".to_string()],
            "timing out a clicked offer must release the placeholder no later connect will resolve"
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
            pushed.network.unwrap().egress.http[0].match_pattern,
            "api.linear.app"
        );
    }

    #[test]
    fn a_policy_floor_survives_a_reload_keeping_its_deny_deny_dominant() {
        let (s, _n, _store, mut rx) = fixture();
        let mut floor = Policy::default();
        floor.add_rule(RouteRule::deny_host("api.example.test"));
        s.set_policy_floor(floor);

        // A watcher reload of the local overlay that *allows* the host must not drop the floor's deny.
        let mut reloaded = Policy::default();
        reloaded.add_rule(RouteRule::allow_host("api.example.test"));
        s.apply_external_policy(reloaded);

        let routes = s.current_policy().network.egress.http;
        let deny_idx = routes
            .iter()
            .position(|r| r.match_pattern == "api.example.test" && r.verdict == Verdict::Deny);
        let allow_idx = routes
            .iter()
            .position(|r| r.match_pattern == "api.example.test" && r.verdict == Verdict::Allow);
        assert!(
            deny_idx.is_some(),
            "the floor's deny must survive the reload"
        );
        assert!(
            deny_idx < allow_idx,
            "the floor's deny must stay ordered before the reloaded allow: {routes:?}"
        );
        let _ = policy_frame(&mut rx);
    }

    #[test]
    fn apply_external_policy_reconciles_armed_ids_with_the_reloaded_connectors() {
        let (s, _n, _store, _rx) = fixture();
        let seen: Arc<StdMutex<Vec<Vec<String>>>> = Arc::new(StdMutex::new(Vec::new()));
        let seen_clone = seen.clone();
        s.set_armed_reconciler(Box::new(move |connectors| {
            seen_clone.lock().unwrap().push(connectors.to_vec());
        }));
        let mut reloaded = Policy::default();
        reloaded.connect("gitlab");
        s.apply_external_policy(reloaded);
        assert_eq!(
            seen.lock().unwrap().as_slice(),
            &[vec!["gitlab".to_string()]],
            "a policy reload must reconcile the credential subsystem's armed set, or a disconnected connector keeps spending its value"
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
    fn apply_external_policy_re_derives_a_connected_connectors_routes() {
        let (s, _n, _store, mut rx) = fixture();
        s.set_connector_route_deriver(Box::new(|ids| {
            ids.iter()
                .filter(|id| id.as_str() == "some-oauth")
                .map(|_| RouteRule::allow_host("api.some-oauth.example"))
                .collect()
        }));
        let mut reloaded = Policy::default();
        reloaded.connect("some-oauth");

        s.apply_external_policy(reloaded);

        let routes = s.current_policy().network.egress.http;
        assert_eq!(
            routes.len(),
            1,
            "a reloaded id-only policy gets its connector route back live, not dropped"
        );
        assert_eq!(routes[0].match_pattern, "api.some-oauth.example");
        assert_eq!(
            policy_frame(&mut rx).network.unwrap().egress.http[0].match_pattern,
            "api.some-oauth.example",
            "the hot-swap frame carries the re-derived route so the guest sees it"
        );
    }

    #[test]
    fn allow_always_policy_frame_packs_registry_credentials_when_provider_set() {
        let (s, _n, _store, mut rx) = fixture();
        s.set_credentials_provider(Box::new(|| {
            vec![Credential {
                id: "some-provider".into(),
                env_var: Some("SOME_TOKEN".into()),
                placeholder: Some("some-placeholder-0000000000000000000000".into()),
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
        assert_eq!(creds[0].id, "some-provider");
    }

    #[test]
    fn apply_external_policy_packs_registry_credentials_when_provider_set() {
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
    fn connect_connector_persists_only_the_id_but_emits_the_route_live() {
        let (s, _n, store, mut rx) = fixture();
        s.connect_connector("gitlab", vec![RouteRule::allow_host("gitlab.com")]);
        let saves = store.saves.lock().unwrap();
        assert_eq!(saves.len(), 1, "the connection is persisted once");
        assert_eq!(saves[0].connectors, ["gitlab"]);
        assert!(
            !saves[0]
                .network
                .egress
                .http
                .iter()
                .any(|r| r.match_pattern == "gitlab.com"),
            "the route must not be baked into the file — boot re-derives it from the catalog, so persisting it would survive `disconnect`"
        );
        drop(saves);
        let v = serde_json::to_value(rx.try_recv().expect("policy frame")).unwrap();
        assert_eq!(v["type"], "policy");
        assert_eq!(
            v["network"]["egress"]["http"][0]["match"], "gitlab.com",
            "the live frame still carries the route so a held request can proceed"
        );
    }

    #[test]
    fn connect_connector_informs_when_persist_fails() {
        let (s, n, store, _rx) = fixture();
        store.fail_next(io::ErrorKind::PermissionDenied, "disk full");
        s.connect_connector("gitlab", vec![RouteRule::allow_host("gitlab.com")]);
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

    impl ConnectPort for FakeConnector {
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

    fn offerable(id: &str, name: &str, pattern: &str) -> OfferableConnector {
        OfferableConnector {
            id: id.into(),
            display_name: name.into(),
            patterns: vec![pattern.into()],
            token_fallback: None,
        }
    }

    fn offerable_with_fallback(id: &str, name: &str, pattern: &str) -> OfferableConnector {
        OfferableConnector {
            token_fallback: Some(TokenFallback {
                help: Some("https://example.com/pat".into()),
                command: None,
            }),
            ..offerable(id, name, pattern)
        }
    }

    fn offer_session(
        offers: Vec<OfferableConnector>,
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
    fn submit_pending_with_a_matching_offer_presents_the_connector_display_name() {
        let (s, n, _rx) = offer_session(
            vec![offerable("some-oauth", "GitHub", "api.some-oauth.example")],
            None,
        );
        s.submit_pending(pending("r1", "api.some-oauth.example"), Instant::now());
        assert_eq!(
            n.presented.lock().unwrap()[0].offer.as_deref(),
            Some("GitHub"),
            "a held request to a connector domain offers to connect it"
        );
    }

    #[test]
    fn a_raw_splice_to_a_connector_domain_is_never_offered_as_a_connect() {
        let (s, n, _rx) = offer_session(
            vec![offerable("some-oauth", "GitHub", "api.some-oauth.example")],
            None,
        );
        s.submit_pending(
            raw_pending("r1", "api.some-oauth.example:5432"),
            Instant::now(),
        );
        let presented = n.presented.lock().unwrap();
        let prompt = &presented[0];
        assert_eq!(
            prompt.offer, None,
            "connecting arms credential injection, which an opaque splice can never carry; the offer card would also hide the RAW disclosure"
        );
        assert_eq!(prompt.badges(), vec!["RAW", "TCP", "5432"]);
        assert_eq!(prompt.caption(), Some(RAW_SPLICE_CAPTION));
    }

    #[test]
    fn connecting_a_connector_releases_its_held_offer_requests() {
        let (s, n, mut rx) = offer_session(
            vec![offerable(
                "some-provider",
                "SomeProvider",
                "api.some-provider.example",
            )],
            None,
        );
        s.submit_pending(pending("r1", "api.some-provider.example"), Instant::now());
        assert_eq!(n.presented.lock().unwrap().len(), 1);
        s.connect_connector(
            "some-provider",
            vec![RouteRule::allow_host("api.some-provider.example")],
        );
        let _live_routes = policy_frame(&mut rx);
        let d = decision_frame(&mut rx);
        assert_eq!(d.id, "r1");
        assert_eq!(
            d.decision,
            Decision::AllowOnce,
            "consent from the credential card connected the connector, so its held offer request must release instead of waiting out the timeout"
        );
        assert_eq!(
            n.dismissed.lock().unwrap().as_slice(),
            &["r1".to_string()],
            "the offer card asks a question the connect already answered"
        );
    }

    #[test]
    fn submit_pending_without_a_matching_offer_presents_no_offer() {
        let (s, n, _rx) = offer_session(
            vec![offerable("some-oauth", "GitHub", "api.some-oauth.example")],
            None,
        );
        s.submit_pending(pending("r1", "example.com"), Instant::now());
        assert_eq!(n.presented.lock().unwrap()[0].offer, None);
    }

    #[test]
    fn submit_pending_surfaces_the_offered_connectors_token_fallback() {
        let (s, n, _rx) = offer_session(
            vec![offerable_with_fallback(
                "some-oauth",
                "GitHub",
                "api.some-oauth.example",
            )],
            None,
        );
        s.submit_pending(pending("r1", "api.some-oauth.example"), Instant::now());
        let presented = n.presented.lock().unwrap();
        assert_eq!(presented[0].offer.as_deref(), Some("GitHub"));
        assert_eq!(
            presented[0].token_fallback,
            Some(TokenFallback {
                help: Some("https://example.com/pat".into()),
                command: None,
            }),
            "an offer for a connector that declares a token fallback carries it to the card"
        );
    }

    #[test]
    fn submit_pending_carries_no_token_fallback_for_an_offer_that_declares_none() {
        let (s, n, _rx) = offer_session(
            vec![offerable("some-oauth", "GitHub", "api.some-oauth.example")],
            None,
        );
        s.submit_pending(pending("r1", "api.some-oauth.example"), Instant::now());
        assert_eq!(n.presented.lock().unwrap()[0].token_fallback, None);
    }

    #[tokio::test]
    async fn connect_offer_with_token_arms_via_the_token_and_releases_the_held_request_allow_once()
    {
        let connector = Arc::new(FakeConnector::new(true));
        let (s, n, mut rx) = offer_session(
            vec![offerable_with_fallback(
                "some-oauth",
                "GitHub",
                "api.some-oauth.example",
            )],
            Some(connector.clone()),
        );
        s.submit_pending(pending("r1", "api.some-oauth.example"), Instant::now());

        let outcome = s
            .connect_offer_with_token("r1", "some-pasted-token".into())
            .await;

        assert_eq!(outcome, DecisionOutcome::Resolved);
        assert_eq!(
            connector.connected_with_token.lock().unwrap().as_slice(),
            &[("some-oauth".to_string(), "some-pasted-token".to_string())],
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
            s.connect_offer_with_token("r1", "some-pasted-token".into())
                .await,
            DecisionOutcome::UnknownId
        );
        assert!(connector.connected_with_token.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn connect_offer_success_releases_the_held_request_with_allow_once() {
        let connector = Arc::new(FakeConnector::new(true));
        let (s, n, mut rx) = offer_session(
            vec![offerable("some-oauth", "GitHub", "api.some-oauth.example")],
            Some(connector.clone()),
        );
        s.submit_pending(pending("r1", "api.some-oauth.example"), Instant::now());

        let outcome = s.connect_offer("r1").await;

        assert_eq!(outcome, DecisionOutcome::Resolved);
        assert_eq!(
            connector.connected.lock().unwrap().as_slice(),
            &["some-oauth".to_string()],
            "accepting the offer drives a connect of the matched connector"
        );
        assert_eq!(decision_frame(&mut rx).decision, Decision::AllowOnce);
        assert_eq!(n.dismissed.lock().unwrap().as_slice(), &["r1".to_string()]);
    }

    #[tokio::test]
    async fn connect_offer_failure_releases_the_held_request_deny_once_closed() {
        let connector = Arc::new(FakeConnector::new(false));
        let (s, _n, mut rx) = offer_session(
            vec![offerable("some-oauth", "GitHub", "api.some-oauth.example")],
            Some(connector),
        );
        s.submit_pending(pending("r1", "api.some-oauth.example"), Instant::now());

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
            vec![offerable("some-oauth", "GitHub", "api.some-oauth.example")],
            None,
        );
        s.submit_pending(pending("r1", "api.some-oauth.example"), Instant::now());

        s.connect_offer("r1").await;

        assert_eq!(decision_frame(&mut rx).decision, Decision::DenyOnce);
    }

    #[tokio::test]
    async fn an_accepted_connector_offer_records_an_connector_allow() {
        let connector = Arc::new(FakeConnector::new(true));
        let (s, _n, _rx) = offer_session(
            vec![offerable("some-oauth", "GitHub", "api.some-oauth.example")],
            Some(connector),
        );
        let recorder = Arc::new(CapturingRecorder::default());
        s.set_ledger_recorder(recorder.clone());
        s.submit_pending(pending("r1", "api.some-oauth.example"), Instant::now());

        s.connect_offer("r1").await;

        let events = recorder.events.lock().unwrap();
        assert_eq!(
            *only_approval(&events),
            LedgerEvent::Approval {
                kind: ApprovalKind::Connector,
                target: "some-oauth".into(),
                decision: LedgerDecision::AllowOnce,
                reason: None,
                connector: Some("some-oauth".into()),
            }
        );
    }

    #[tokio::test]
    async fn a_failed_connector_connect_records_an_connector_deny() {
        let connector = Arc::new(FakeConnector::new(false));
        let (s, _n, _rx) = offer_session(
            vec![offerable("some-oauth", "GitHub", "api.some-oauth.example")],
            Some(connector),
        );
        let recorder = Arc::new(CapturingRecorder::default());
        s.set_ledger_recorder(recorder.clone());
        s.submit_pending(pending("r1", "api.some-oauth.example"), Instant::now());

        s.connect_offer("r1").await;

        let events = recorder.events.lock().unwrap();
        assert_eq!(
            *only_approval(&events),
            LedgerEvent::Approval {
                kind: ApprovalKind::Connector,
                target: "some-oauth".into(),
                decision: LedgerDecision::DenyOnce,
                reason: None,
                connector: Some("some-oauth".into()),
            }
        );
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
            vec![offerable("some-oauth", "GitHub", "api.some-oauth.example")],
            None,
        );
        assert_eq!(s.connect_offer("never").await, DecisionOutcome::UnknownId);
    }

    #[tokio::test]
    async fn connect_offer_releases_every_held_request_for_the_connector() {
        let connector = Arc::new(FakeConnector::new(true));
        let (s, n, mut rx) = offer_session(
            vec![offerable("some-oauth", "GitHub", "api.some-oauth.example")],
            Some(connector.clone()),
        );
        s.submit_pending(pending("r1", "api.some-oauth.example"), Instant::now());
        s.submit_pending(pending("r2", "api.some-oauth.example"), Instant::now());

        s.connect_offer("r1").await;

        assert_eq!(
            connector.connected.lock().unwrap().as_slice(),
            &["some-oauth".to_string()],
            "one sign-in serves every held request for the connector"
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

    #[tokio::test]
    async fn connect_offer_signals_connect_finished_with_the_offers_display_name() {
        let connector = Arc::new(FakeConnector::new(true));
        let (s, n, _rx) = offer_session(
            vec![offerable("some-oauth", "GitHub", "api.some-oauth.example")],
            Some(connector),
        );
        s.submit_pending(pending("r1", "api.some-oauth.example"), Instant::now());

        s.connect_offer("r1").await;

        assert_eq!(
            n.connects_finished.lock().unwrap().as_slice(),
            &["GitHub".to_string()],
            "the resolved connect releases the slot its placeholder holds in the window"
        );
    }

    #[tokio::test]
    async fn connect_offer_failure_still_signals_connect_finished() {
        let connector = Arc::new(FakeConnector::new(false));
        let (s, n, _rx) = offer_session(
            vec![offerable("some-oauth", "GitHub", "api.some-oauth.example")],
            Some(connector),
        );
        s.submit_pending(pending("r1", "api.some-oauth.example"), Instant::now());

        s.connect_offer("r1").await;

        assert_eq!(
            n.connects_finished.lock().unwrap().as_slice(),
            &["GitHub".to_string()],
            "a failed connect must not leave a connecting placeholder behind"
        );
    }

    #[tokio::test]
    async fn connect_offer_without_a_connector_signals_connect_finished() {
        let (s, n, _rx) = offer_session(
            vec![offerable("some-oauth", "GitHub", "api.some-oauth.example")],
            None,
        );
        s.submit_pending(pending("r1", "api.some-oauth.example"), Instant::now());

        s.connect_offer("r1").await;

        assert_eq!(
            n.connects_finished.lock().unwrap().as_slice(),
            &["GitHub".to_string()]
        );
    }

    #[tokio::test]
    async fn connect_offer_with_token_signals_connect_finished() {
        let connector = Arc::new(FakeConnector::new(true));
        let (s, n, _rx) = offer_session(
            vec![offerable_with_fallback(
                "some-oauth",
                "GitHub",
                "api.some-oauth.example",
            )],
            Some(connector),
        );
        s.submit_pending(pending("r1", "api.some-oauth.example"), Instant::now());

        s.connect_offer_with_token("r1", "some-pasted-token".into())
            .await;

        assert_eq!(
            n.connects_finished.lock().unwrap().as_slice(),
            &["GitHub".to_string()]
        );
    }

    #[test]
    fn submit_pending_coalesces_a_request_while_its_connector_is_connecting() {
        let (s, n, _rx) = offer_session(
            vec![offerable("some-oauth", "GitHub", "api.some-oauth.example")],
            None,
        );
        s.begin_connecting("some-oauth");
        s.submit_pending(pending("r1", "api.some-oauth.example"), Instant::now());
        assert!(
            n.presented.lock().unwrap().is_empty(),
            "a request arriving mid sign-in raises no new card"
        );
        assert_eq!(
            s.offer_request_ids("some-oauth"),
            vec!["r1".to_string()],
            "but it is still held, to be released by the in-flight connect"
        );
    }

    #[tokio::test]
    async fn connect_offer_for_a_sibling_while_connecting_does_not_start_a_second_connect() {
        let connector = Arc::new(FakeConnector::new(true));
        let (s, n, mut rx) = offer_session(
            vec![offerable("some-oauth", "GitHub", "api.some-oauth.example")],
            Some(connector.clone()),
        );
        s.begin_connecting("some-oauth");
        s.submit_pending(pending("r2", "api.some-oauth.example"), Instant::now());

        let outcome = s.connect_offer("r2").await;

        assert_eq!(outcome, DecisionOutcome::Resolved);
        assert!(
            connector.connected.lock().unwrap().is_empty(),
            "a second sign-in must not run while one is in flight"
        );
        assert!(
            n.connects_finished.lock().unwrap().is_empty(),
            "the duplicate click must not release the in-flight connect's placeholder"
        );
        assert!(
            rx.try_recv().is_err(),
            "the in-flight connect releases r2, not this duplicate click"
        );
        assert_eq!(
            s.offer_request_ids("some-oauth"),
            vec!["r2".to_string()],
            "r2 stays held for the in-flight connect's batch"
        );
    }

    #[test]
    fn submit_pending_does_not_offer_an_already_connected_connector() {
        let mut policy = Policy::default();
        policy.connect("some-oauth");
        let notifier = Arc::new(RecordingNotifier::default());
        let store = Arc::new(CapturingStore::default());
        let (tx, _rx) = mpsc::unbounded_channel();
        let s =
            ApprovalSession::new(policy, notifier.clone(), store, tx, TEST_TIMEOUT).with_offers(
                vec![offerable("some-oauth", "GitHub", "api.some-oauth.example")],
            );
        s.submit_pending(pending("r1", "api.some-oauth.example"), Instant::now());
        assert_eq!(
            notifier.presented.lock().unwrap()[0].offer,
            None,
            "a connected connector is not re-offered"
        );
    }

    #[test]
    fn an_offer_held_during_a_connect_is_not_swept_by_the_timeout_ticker() {
        let (s, _n, mut rx) = offer_session(
            vec![offerable("some-oauth", "GitHub", "api.some-oauth.example")],
            None,
        );
        let t0 = Instant::now();
        s.submit_pending(pending("r1", "api.some-oauth.example"), t0);
        s.begin_connecting("some-oauth");
        assert_eq!(
            s.tick_timeouts(t0 + TEST_TIMEOUT * 2),
            0,
            "a connecting offer must not time out under the sign-in"
        );
        s.finish_connecting("some-oauth");
        assert_eq!(
            s.tick_timeouts(t0 + TEST_TIMEOUT * 2),
            1,
            "once the connect ends the request can time out"
        );
        assert_eq!(decision_frame(&mut rx).decision, Decision::Timeout);
    }
}
