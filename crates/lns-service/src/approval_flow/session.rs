use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::approval_flow::protocol::{
    Decision, GrantedPayload, HostFrame, PolicyMessage, RequestDecision, RequestPending, Treatment,
};
use crate::connector::offer::Offer;
use crate::ledger::LedgerRecorder;
use lns_ipc::{ApprovalKind, ConnectorView, Decision as LedgerDecision, LedgerEvent};
use lns_policy::matching::{split_destination, unbracketed};
use lns_policy::{Approval, Policy, PolicyStore, RouteRule, TcpEgressRule};

pub type FrameSink = mpsc::UnboundedSender<HostFrame>;

pub trait Notifier: Send + Sync {
    fn present(&self, pending: &PendingPrompt);
    fn dismiss(&self, id: &str);
    /// The request timed out but its card stays: an expired hold does not cancel a connect the user is in the middle of (§3.2.4).
    fn expire(&self, id: &str);
    fn inform(&self, message: &str);
    fn clear_informs(&self);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingPrompt {
    pub id: String,
    pub host: String,
    pub action: String,
    /// `Raw` when approving splices the connection through unread, which the card has to say out loud.
    pub treatment: Treatment,
    /// The requesting run's name, attributed by the service from the session channel — never from workload-supplied data.
    pub run: Option<String>,
    /// The connector that serves this destination, when exactly one does: the card then asks whether to connect it rather than whether to allow the traffic (§3.2.1).
    pub offer: Option<ConnectorView>,
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

const RAW_SPLICE_CAPTION: &str = "lns cannot inspect this traffic.";

#[derive(Debug)]
struct PendingEntry {
    host: String,
    /// The gate's own name for the destination (`CONNECT db.internal:5432`), the only place the port survives.
    action: String,
    treatment: Treatment,
    deadline: Instant,
    reason: String,
    offer: Option<ConnectorView>,
    /// True once the workload gave up waiting: the request is answered, the offer is not.
    expired: bool,
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

/// Which account a grant is made with: none for a method that does not authenticate, one this machine already holds, or one the card is creating.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileChoice {
    None,
    Held(String),
    New {
        label: String,
        values: lns_ipc::SecretValues,
    },
}

/// The connector store's side of a card decision. Every method writes to disk, so the session takes it as a port (§3.2.4).
pub trait ConnectorPort: Send + Sync {
    /// Stores what an authentication returned, answering with the project directories whose grant its authority no longer matches.
    fn connect(
        &self,
        name: &str,
        method: &str,
        label: &str,
        values: lns_ipc::SecretValues,
    ) -> Result<Vec<String>, String>;

    /// Records this project's grant against the bytes the card disclosed, answering with the egress the method opens.
    fn grant(
        &self,
        name: &str,
        digest: &str,
        method: &str,
        profile: Option<&str>,
    ) -> Result<GrantedPayload, String>;

    fn decline(&self, name: &str) -> Result<(), String>;
}

pub struct ApprovalSession {
    policy: Mutex<Policy>,
    /// What the developer's own file holds — never the artifact baseline the running `policy` also carries.
    persisted: Mutex<Policy>,
    pending: Mutex<HashMap<String, PendingEntry>>,
    notifier: Arc<dyn Notifier>,
    store: Arc<dyn PolicyStore>,
    sink: FrameSink,
    timeout: Duration,
    ledger: OnceLock<Arc<dyn LedgerRecorder>>,
    shipped: OnceLock<Policy>,
    /// What each granted connector supplies, kept apart from both files so a reload of either cannot retract it, and per connector because a run may grant more than one (§3.3.2 source 4).
    granted: Mutex<BTreeMap<String, GrantedPayload>>,
    /// What has already been said once, so a misconfiguration every request trips does not fill the window.
    said: Mutex<std::collections::HashSet<String>>,
    connectors: OnceLock<Arc<dyn ConnectorPort>>,
    /// The connectors this project has not decided. Mutable, because a grant lifts its own hold and every later frame must be published without it (§3.2.1).
    offers: Mutex<Vec<ConnectorView>>,
    run: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionOutcome {
    Resolved,
    UnknownId,
}

impl ApprovalSession {
    pub fn new(
        policy: Policy,
        persisted: Policy,
        notifier: Arc<dyn Notifier>,
        store: Arc<dyn PolicyStore>,
        sink: FrameSink,
        timeout: Duration,
    ) -> Self {
        Self {
            policy: Mutex::new(policy),
            persisted: Mutex::new(persisted),
            pending: Mutex::new(HashMap::new()),
            notifier,
            store,
            sink,
            timeout,
            ledger: OnceLock::new(),
            shipped: OnceLock::new(),
            granted: Mutex::new(BTreeMap::new()),
            said: Mutex::new(std::collections::HashSet::new()),
            connectors: OnceLock::new(),
            offers: Mutex::new(Vec::new()),
            run: None,
        }
    }

    /// Names the run every card this session raises speaks for; the service attributes it from the channel the request arrived on, never from the workload.
    pub fn for_run(mut self, run: String) -> Self {
        self.run = Some(run);
        self
    }

    /// Installs the baseline every reload folds the decisions file over — what every source *but* this directory's own decided, since a copy of that file frozen here would outlive a rule the developer deletes; idempotent, the first wins.
    pub fn set_shipped_policy(&self, shipped: Policy) {
        let _ = self.shipped.set(shipped);
    }

    pub fn set_connector_port(&self, port: Arc<dyn ConnectorPort>) {
        let _ = self.connectors.set(port);
    }

    /// Connects an account if the card made one, records the grant, then publishes and releases — in that order, because each step is what makes the next one correct (§3.2.4).
    pub fn grant_offer(&self, id: &str, method: &str, profile: ProfileChoice) -> DecisionOutcome {
        let Some(offer) = self.offer_of(id) else {
            return DecisionOutcome::UnknownId;
        };
        let Some(port) = self.connectors.get() else {
            self.notifier
                .inform("no connector store is wired to this run, so nothing was granted");
            return DecisionOutcome::Resolved;
        };
        let label = match self.resolve_profile(port.as_ref(), &offer.name, method, profile) {
            Ok(label) => label,
            Err(why) => {
                self.notifier.inform(&why);
                return DecisionOutcome::Resolved;
            }
        };
        // Every request waiting on this connector, named before the offer is dropped from them.
        let held = self.held_for(&offer.name);
        match port.grant(&offer.name, &offer.digest, method, label.as_deref()) {
            Ok(opened) => {
                self.forget_offer(&offer.name);
                self.apply_granted_egress(&offer.name, opened);
                self.release(&held);
                DecisionOutcome::Resolved
            }
            Err(why) => {
                self.notifier.inform(&why);
                DecisionOutcome::Resolved
            }
        }
    }

    /// Records a standing no for this project, then lets the ordinary card ask what the hold was standing in for (§3.2.4).
    pub fn decline_offer(&self, id: &str) -> DecisionOutcome {
        let Some(offer) = self.offer_of(id) else {
            return DecisionOutcome::UnknownId;
        };
        if let Some(port) = self.connectors.get()
            && let Err(why) = port.decline(&offer.name)
        {
            self.notifier.inform(&why);
            return DecisionOutcome::Resolved;
        }
        let held = self.held_for(&offer.name);
        self.forget_offer(&offer.name);
        self.represent(&held);
        DecisionOutcome::Resolved
    }

    /// The label the grant names, creating the account first when the card made one — the grant reads the store, so it must already be there.
    fn resolve_profile(
        &self,
        port: &dyn ConnectorPort,
        name: &str,
        method: &str,
        profile: ProfileChoice,
    ) -> Result<Option<String>, String> {
        match profile {
            ProfileChoice::None => Ok(None),
            ProfileChoice::Held(label) => Ok(Some(label)),
            ProfileChoice::New { label, values } => {
                let invalidated = port.connect(name, method, &label, values)?;
                if !invalidated.is_empty() {
                    self.notifier.inform(&format!(
                        "this connection no longer covers what {} granted, so {} must decide again",
                        invalidated.join(", "),
                        if invalidated.len() == 1 { "it" } else { "they" }
                    ));
                }
                Ok(Some(label))
            }
        }
    }

    fn offer_of(&self, id: &str) -> Option<ConnectorView> {
        self.pending
            .lock()
            .expect("pending mutex poisoned")
            .get(id)
            .and_then(|entry| entry.offer.clone())
    }

    fn forget_offer(&self, name: &str) {
        self.offers
            .lock()
            .expect("offers mutex poisoned")
            .retain(|offer| offer.name != name);
        let mut pending = self.pending.lock().expect("pending mutex poisoned");
        for entry in pending.values_mut() {
            if entry.offer.as_ref().is_some_and(|offer| offer.name == name) {
                entry.offer = None;
            }
        }
    }

    /// Lets go of every request that was waiting on this connector, because one answer decided it for all of them.
    fn release(&self, ids: &[String]) {
        for id in ids {
            self.notifier.dismiss(id);
            if let Some(entry) = self.remove_pending(id) {
                self.answer(id, &entry, Decision::AllowOnce);
            }
        }
    }

    /// Asks again, now without the offer: the hold already made this request a question, and only the user can answer it.
    fn represent(&self, ids: &[String]) {
        for id in ids {
            if let Some(prompt) = self.prompt_of(id) {
                self.notifier.present(&prompt);
            }
        }
    }

    fn held_for(&self, name: &str) -> Vec<String> {
        self.pending
            .lock()
            .expect("pending mutex poisoned")
            .iter()
            .filter(|(_, entry)| entry.offer.as_ref().is_some_and(|offer| offer.name == name))
            .map(|(id, _)| id.clone())
            .collect()
    }

    fn prompt_of(&self, id: &str) -> Option<PendingPrompt> {
        let pending = self.pending.lock().expect("pending mutex poisoned");
        let entry = pending.get(id)?;
        Some(PendingPrompt {
            id: id.to_string(),
            host: entry.host.clone(),
            action: entry.action.clone(),
            treatment: entry.treatment,
            run: self.run.clone(),
            offer: entry.offer.clone(),
        })
    }

    pub fn set_ledger_recorder(&self, recorder: Arc<dyn LedgerRecorder>) {
        let _ = self.ledger.set(recorder);
    }

    pub fn current_policy(&self) -> Policy {
        self.policy.lock().expect("policy mutex poisoned").clone()
    }

    /// The connectors this project has not decided; every destination they serve is held so the run asks rather than proceeds (§3.2.1).
    pub fn hold_for_offers(&self, offers: Vec<ConnectorView>) {
        *self.offers.lock().expect("offers mutex poisoned") = offers;
    }

    /// The policy as the guest receives it, holds and supplies and all — the one shape both the boot frame and every reload go through.
    pub fn policy_message(&self) -> PolicyMessage {
        let policy = self.current_policy();
        let held = self.held_patterns();
        match self.granted_layer() {
            Some(granted) => PolicyMessage::granting(policy, &held, &granted),
            None => PolicyMessage::seeded_holding(policy, &held),
        }
    }

    /// Derived rather than stored, so dropping a connector from `offers` lifts its holds on the very next frame.
    fn held_patterns(&self) -> Vec<String> {
        self.offers
            .lock()
            .expect("offers mutex poisoned")
            .iter()
            .flat_map(|offer| offer.serves.clone())
            .collect()
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
                action: req.action.clone(),
                treatment: req.treatment,
                deadline: now + self.timeout,
                reason: req.reason.clone(),
                offer: None,
                expired: false,
            },
        );
        drop(pending);
        let offer = self.offer_for(&req);
        if let Some(offer) = &offer
            && let Some(entry) = self
                .pending
                .lock()
                .expect("pending mutex poisoned")
                .get_mut(&req.id)
        {
            entry.offer = Some(offer.clone());
        }
        self.notifier.present(&PendingPrompt {
            id: req.id,
            host: req.host,
            action: req.action,
            treatment: req.treatment,
            run: self.run.clone(),
            offer,
        });
    }

    /// The connector offered for this request, or none — including when two serve it, which no card may resolve by choosing (§3.2.4).
    fn offer_for(&self, req: &RequestPending) -> Option<ConnectorView> {
        let destination = offered_destination(req);
        let offers = self.offers.lock().expect("offers mutex poisoned");
        match crate::connector::offer::offers_for(destination, &offers) {
            Offer::One(connector) => Some(connector.clone()),
            Offer::None => None,
            Offer::Ambiguous(names) => {
                let names = names.join(" and ");
                drop(offers);
                self.inform_once(&format!(
                    "{names} both serve {destination}, so neither is offered; uninstall one"
                ));
                None
            }
        }
    }

    /// One message per misconfiguration, however many requests reach it.
    fn inform_once(&self, message: &str) {
        let mut said = self.said.lock().expect("said mutex poisoned");
        if said.insert(message.to_string()) {
            self.notifier.inform(message);
        }
    }

    /// Publishes the rule before the decision that wakes the held connection, and writes the file only once both frames have left, so a slow disk cannot hold the request.
    pub fn record_decision(&self, id: &str, decision: Decision) -> DecisionOutcome {
        let Some(entry) = self.remove_pending(id) else {
            return DecisionOutcome::UnknownId;
        };
        self.notifier.dismiss(id);
        let rule_stands = self.apply_always_decision(&entry, decision);
        // The rule and the audit line are the developer's standing answer either way; only the wire frame is about a request the guest still has.
        self.answer(id, &entry, decision);
        if rule_stands {
            self.save_persisted();
        }
        self.record_approval(&entry, decision);
        DecisionOutcome::Resolved
    }

    /// Sends the wire decision unless the request already timed out, because the guest forgot that id when it did.
    fn answer(&self, id: &str, entry: &PendingEntry, decision: Decision) {
        if !entry.expired {
            self.send_decision_frame(id, decision);
        }
    }

    /// Fails a held request because its card was closed: no rule, no audit line, and `Timeout` on the wire because a dismissal is the absence of a decision rather than a deny the developer picked.
    pub fn dismiss_request(&self, id: &str) -> DecisionOutcome {
        let Some(entry) = self.remove_pending(id) else {
            return DecisionOutcome::UnknownId;
        };
        self.notifier.dismiss(id);
        self.answer(id, &entry, Decision::Timeout);
        DecisionOutcome::Resolved
    }

    /// Writes the rule an "always" decision earns into the table its treatment belongs to and answers whether it stands; a once-decision earns none.
    fn apply_always_decision(&self, entry: &PendingEntry, decision: Decision) -> bool {
        match entry.treatment {
            Treatment::Inspected => match rule_for_always_decision(&entry.host, decision) {
                Some(rule) => self.apply_persistent_rule(rule),
                None => false,
            },
            Treatment::Raw => match entry.raw_destination() {
                Some(destination) => match tcp_rule_for_always_decision(destination, decision) {
                    Some(rule) => self.apply_persistent_tcp_rule(rule),
                    None => false,
                },
                None => {
                    if earns_a_rule(decision) {
                        self.report_no_rule_written(&format!(
                            "the gate named the destination {:?}, which this lns cannot read as a rule for {:?}",
                            entry.action, entry.host
                        ));
                    }
                    false
                }
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

    pub fn tick_timeouts(&self, now: Instant) -> usize {
        let expired: Vec<String> = {
            let pending = self.pending.lock().expect("pending mutex poisoned");
            pending
                .iter()
                .filter(|(_, entry)| !entry.expired && entry.deadline <= now)
                .map(|(id, _)| id.clone())
                .collect()
        };
        expired.iter().filter(|id| self.timeout_one(id)).count()
    }

    fn timeout_one(&self, id: &str) -> bool {
        let Some(entry) = self.remove_pending(id) else {
            return false;
        };
        // The request is gone, but the offer it raised is still the user's to answer, and the profile applies to what runs next.
        if entry.offer.is_some() {
            self.keep_offer(id, entry);
            self.notifier.expire(id);
        } else {
            self.notifier.dismiss(id);
        }
        self.send_decision_frame(id, Decision::Timeout);
        true
    }

    /// Puts the entry back with its deadline lifted, so the card it belongs to still has something to answer for.
    fn keep_offer(&self, id: &str, mut entry: PendingEntry) {
        entry.expired = true;
        self.pending
            .lock()
            .expect("pending mutex poisoned")
            .insert(id.to_string(), entry);
    }

    pub fn apply_external_policy(&self, new_policy: Policy) {
        *self.persisted.lock().expect("persisted mutex poisoned") = new_policy.clone();
        *self.policy.lock().expect("policy mutex poisoned") = self.effective_over(&new_policy);
        self.send_policy_frame();
    }

    /// Installs what one granted connector supplies as part of the grant layer: behind this directory's decisions, ahead of the artifact (§3.3.2).
    pub fn apply_granted_egress(&self, connector: &str, granted: GrantedPayload) {
        self.granted
            .lock()
            .expect("granted mutex poisoned")
            .insert(connector.to_string(), granted);
        let own = self
            .persisted
            .lock()
            .expect("persisted mutex poisoned")
            .clone();
        *self.policy.lock().expect("policy mutex poisoned") = self.effective_over(&own);
        self.send_policy_frame();
    }

    fn effective_over(&self, own: &Policy) -> Policy {
        let granted = self.granted_layer();
        let opened = granted.as_ref().map(|granted| &granted.egress);
        match (self.shipped.get(), opened) {
            (None, None) => own.clone(),
            (shipped, opened) => crate::artifact::policy::merge_effective(shipped, opened, own),
        }
    }

    fn granted_layer(&self) -> Option<GrantedPayload> {
        GrantedPayload::combined(&self.granted.lock().expect("granted mutex poisoned"))
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

    fn send_policy_frame(&self) {
        let _ = self.sink.send(HostFrame::Policy(self.policy_message()));
    }

    fn apply_persistent_rule(&self, rule: RouteRule) -> bool {
        let approval = self
            .policy
            .lock()
            .expect("policy mutex poisoned")
            .add_approved_rule(rule.clone());
        if approval == Approval::Stands {
            self.persisted
                .lock()
                .expect("persisted mutex poisoned")
                .add_approved_rule(rule);
        }
        self.publish_if_it_stands(approval)
    }

    /// Says the decision stands for this request but outlived nothing.
    fn report_no_rule_written(&self, why: &str) {
        self.notifier.inform(&format!(
            "decision applied to this request only; no policy rule could be written: {why}"
        ));
    }

    fn apply_persistent_tcp_rule(&self, rule: TcpEgressRule) -> bool {
        // One rule lens-sandbox-core cannot parse force-denies the whole policy in the guest, so a destination we cannot express is not written at all.
        if let Err(e) = rule.validate() {
            self.report_no_rule_written(&e);
            return false;
        }
        let (approval, pre_empted) = {
            let mut policy = self.policy.lock().expect("policy mutex poisoned");
            let pre_empted = pre_empted_http_patterns(&policy, &rule);
            (policy.add_approved_tcp_rule(rule.clone()), pre_empted)
        };
        if approval == Approval::Stands {
            self.persisted
                .lock()
                .expect("persisted mutex poisoned")
                .add_approved_tcp_rule(rule);
        }
        if !self.publish_if_it_stands(approval) {
            return false;
        }
        // The http rules this raw rule displaces would otherwise go quiet without a word.
        if !pre_empted.is_empty() {
            self.notifier.inform(&format!(
                "that traffic is now spliced raw, so these HTTP rules no longer apply to it: {}",
                pre_empted.join(", ")
            ));
        }
        true
    }

    /// Answers whether the decision stands, telling the developer when it applied to one request only — silence there would read as "remembered".
    fn publish_if_it_stands(&self, approval: Approval) -> bool {
        let why = match approval {
            Approval::Stands => {
                self.send_policy_frame();
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

    /// Runs after both frames have left, and a file that cannot be written still leaves the decision live for the rest of the run.
    fn save_persisted(&self) {
        let to_persist = self
            .persisted
            .lock()
            .expect("persisted mutex poisoned")
            .clone();
        if let Err(e) = self.store.save(&to_persist) {
            self.notifier.inform(&format!(
                "policy rule applied in-memory but not persisted: {e}"
            ));
        }
    }
}

fn rule_for_always_decision(host: &str, decision: Decision) -> Option<RouteRule> {
    match decision {
        Decision::AllowAlways => Some(RouteRule::allow_host(host).approved()),
        Decision::DenyAlways => Some(RouteRule::deny_host(host).approved()),
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
        Decision::AllowAlways => Some(TcpEgressRule::allow_destination(destination).approved()),
        Decision::DenyAlways => Some(TcpEgressRule::deny_destination(destination).approved()),
        Decision::AllowOnce | Decision::DenyOnce | Decision::Timeout => None,
    }
}

/// Whether the developer asked for a decision that outlives this request, and so is owed an explanation when none can be written.
fn earns_a_rule(decision: Decision) -> bool {
    matches!(decision, Decision::AllowAlways | Decision::DenyAlways)
}

/// The gate's `CONNECT <destination>` taken verbatim, and `None` unless it names the frame's own host.
/// What a `serves` pattern is matched against: a raw splice is granted per port, so the host alone would offer for a service the connector does not serve.
fn offered_destination(req: &RequestPending) -> &str {
    match req.treatment {
        Treatment::Raw => raw_destination(&req.action, &req.host).unwrap_or(&req.host),
        Treatment::Inspected => &req.host,
    }
}

fn raw_destination<'a>(action: &'a str, host: &str) -> Option<&'a str> {
    let destination = action.strip_prefix("CONNECT ")?;
    let (named, _) = split_destination(destination);
    // The gate strips the port from `host` before sending it, so only brackets come off here.
    (named == unbracketed(host)).then_some(destination)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::approval_flow::protocol::{WireDefaultVerdict, WireTcpEgressRule, WireVerdict};
    use lns_policy::{RouteRule, Transport, Verdict};
    use std::io;
    use std::sync::Mutex as StdMutex;

    #[derive(Default)]
    pub(crate) struct RecordingNotifier {
        pub(crate) presented: StdMutex<Vec<PendingPrompt>>,
        pub(crate) dismissed: StdMutex<Vec<String>>,
        pub(crate) expired: StdMutex<Vec<String>>,
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
        fn expire(&self, id: &str) {
            self.expired.lock().unwrap().push(id.to_string());
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
        let session = ApprovalSession::new(
            policy.clone(),
            policy,
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
    fn a_network_card_names_the_run_that_raised_it() {
        let (session, notifier, _s, _rx) = fixture();
        let session = session.for_run("some-run".into());
        session.submit_pending(pending("r1", "api.example.test"), Instant::now());
        assert_eq!(
            notifier.presented.lock().unwrap()[0].run.as_deref(),
            Some("some-run"),
            "two sandboxes asking for the same host raise identical cards unless each names its run"
        );
    }

    #[test]
    fn a_network_card_from_a_session_with_no_run_names_nothing() {
        let (session, notifier, _s, _rx) = fixture();
        session.submit_pending(pending("r1", "api.example.test"), Instant::now());
        assert_eq!(
            notifier.presented.lock().unwrap()[0].run,
            None,
            "a card with no originating run must stay silent rather than name one"
        );
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
        assert_eq!(prompt.caption(), Some("lns cannot inspect this traffic."));
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

    /// A run whose effective policy is the developer's file plus an artifact baseline — the shape `supervisor::adapter::start` builds.
    fn fixture_over_a_merged_run() -> Fixture {
        let mut own = Policy::default();
        own.add_rule(RouteRule::allow_host("mine.example.test"));
        let mut effective = own.clone();
        effective.add_rule(RouteRule::allow_host("from-the-artifact.example.test"));
        let notifier = Arc::new(RecordingNotifier::default());
        let store = Arc::new(CapturingStore::default());
        let (tx, rx) = mpsc::unbounded_channel();
        let session = ApprovalSession::new(
            effective,
            own,
            notifier.clone(),
            store.clone(),
            tx,
            TEST_TIMEOUT,
        );
        (session, notifier, store, rx)
    }

    #[test]
    fn an_approval_writes_only_the_developers_own_policy_back() {
        let (session, _n, store, _rx) = fixture_over_a_merged_run();
        session.submit_pending(pending("r1", "just-approved.example.test"), Instant::now());
        session.record_decision("r1", Decision::AllowAlways);

        let saved = store.saves.lock().unwrap();
        let written: Vec<&str> = saved
            .last()
            .expect("the decision is persisted")
            .network
            .egress
            .http
            .iter()
            .map(|rule| rule.match_pattern.as_str())
            .collect();
        assert_eq!(
            written,
            vec!["mine.example.test", "just-approved.example.test"],
            "a pulled artifact's rule and a connector's derived route are not the developer's, so an approval must not write them into their file"
        );
    }

    #[test]
    fn an_approval_still_reaches_the_guest_with_the_whole_effective_policy() {
        let (session, _n, _store, mut rx) = fixture_over_a_merged_run();
        session.submit_pending(pending("r1", "just-approved.example.test"), Instant::now());
        session.record_decision("r1", Decision::AllowAlways);

        let mut published = Vec::new();
        while let Ok(frame) = rx.try_recv() {
            if let HostFrame::Policy(message) = frame
                && let Some(network) = message.network
            {
                published = network
                    .egress
                    .http
                    .iter()
                    .map(|rule| rule.match_pattern.clone())
                    .collect();
            }
        }
        for expected in [
            "just-approved.example.test",
            "mine.example.test",
            "from-the-artifact.example.test",
        ] {
            assert!(
                published.iter().any(|p| p == expected),
                "the guest still enforces the whole effective policy; {expected} missing from {published:?}"
            );
        }
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
            vec![TcpEgressRule::allow_destination("db.internal:5432").approved()]
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
            vec![TcpEgressRule::allow_destination("[2001:db8::1]:5432").approved()],
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
            vec![TcpEgressRule::deny_destination("db.internal:5432").approved()]
        );
    }

    #[test]
    fn a_second_always_decision_the_first_ones_rule_pre_empts_writes_nothing_and_says_so() {
        let (session, notifier, store, _rx) = fixture_holding(Policy::default());
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
            vec![Verdict::Allow],
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

    fn tcp_verdicts(message: &PolicyMessage) -> Vec<(String, WireVerdict)> {
        message
            .network
            .as_ref()
            .expect("a network section")
            .egress
            .tcp
            .iter()
            .map(|rule| (rule.match_pattern.clone(), rule.verdict))
            .collect()
    }

    /// An offer carrying only what a hold reads: what it serves.
    fn serving(destination: &str) -> ConnectorView {
        named_serving("some-provider", destination)
    }

    fn named_serving(name: &str, destination: &str) -> ConnectorView {
        ConnectorView {
            name: name.to_string(),
            digest: "sha256:abc".to_string(),
            serves: vec![destination.to_string()],
            methods: Vec::new(),
            profiles: Vec::new(),
        }
    }

    fn allowing_one_raw_destination(destination: &str) -> Policy {
        let mut policy = Policy::default();
        policy
            .network
            .egress
            .tcp
            .push(TcpEgressRule::allow_destination(destination));
        policy
    }

    #[test]
    fn a_served_destination_is_held_in_every_policy_this_session_publishes() {
        // §3.2.1: the run allows the destination, so nothing would ask about it; the hold is what raises the offer.
        let (s, _n, _store, _rx) = fixture_holding(allowing_one_raw_destination(
            "db.some-provider.example:5432",
        ));
        s.hold_for_offers(vec![serving("db.some-provider.example")]);

        assert_eq!(
            tcp_verdicts(&s.policy_message()),
            [
                (
                    "db.some-provider.example:5432".to_string(),
                    WireVerdict::Ask
                ),
                (
                    "db.some-provider.example:5432".to_string(),
                    WireVerdict::Allow
                ),
            ],
            "the hold must lead the allow it narrows, because the guest gate is first-match-wins"
        );
    }

    #[test]
    fn a_served_destination_raises_a_connector_card_rather_than_a_network_card() {
        // §3.2.1: the run reached something a connector serves, so the question is "connect it?", not "allow it?".
        let (s, n, _store, _rx) = fixture();
        s.hold_for_offers(vec![serving("api.some-provider.example")]);

        s.submit_pending(pending("r1", "api.some-provider.example"), Instant::now());

        let presented = n.presented.lock().unwrap();
        assert_eq!(
            presented[0].offer.as_ref().map(|offer| offer.name.as_str()),
            Some("some-provider")
        );
    }

    #[test]
    fn a_destination_no_connector_serves_raises_the_ordinary_card() {
        let (s, n, _store, _rx) = fixture();
        s.hold_for_offers(vec![serving("api.some-provider.example")]);

        s.submit_pending(pending("r1", "api.unrelated.example"), Instant::now());

        assert!(n.presented.lock().unwrap()[0].offer.is_none());
    }

    #[test]
    fn a_raw_destination_is_offered_on_its_own_host_and_port() {
        // §3.2.1 exists so a raw-stream connector is offered; matching the bare host would miss a `serves` entry that names a port.
        let (s, n, _store, _rx) = fixture();
        s.hold_for_offers(vec![serving("db.some-provider.example:5432")]);

        s.submit_pending(
            RequestPending {
                id: "r1".into(),
                host: "db.some-provider.example".into(),
                action: "CONNECT db.some-provider.example:5432".into(),
                reason: "policy-ambiguous".into(),
                treatment: Treatment::Raw,
            },
            Instant::now(),
        );

        assert_eq!(
            n.presented.lock().unwrap()[0]
                .offer
                .as_ref()
                .map(|offer| offer.name.as_str()),
            Some("some-provider")
        );
    }

    #[test]
    fn two_connectors_serving_one_destination_raise_the_ordinary_card_and_say_why() {
        // Install cannot detect two mid-segment wildcards, so this is reachable; picking one would grant an authority the user never chose between.
        let (s, n, _store, _rx) = fixture();
        s.hold_for_offers(vec![
            named_serving("some-provider", "api.*.example"),
            named_serving("other-provider", "*.eu.example"),
        ]);

        s.submit_pending(pending("r1", "api.eu.example"), Instant::now());

        assert!(
            n.presented.lock().unwrap()[0].offer.is_none(),
            "no connector card, because there is no one connector to name"
        );
        let informs = n.informed.lock().unwrap();
        assert!(
            informs
                .iter()
                .any(|line| line.contains("some-provider") && line.contains("other-provider")),
            "the user is told which two to choose between: {informs:?}"
        );
    }

    #[test]
    fn the_ambiguity_is_said_once_however_many_requests_reach_it() {
        let (s, n, _store, _rx) = fixture();
        s.hold_for_offers(vec![
            named_serving("some-provider", "api.*.example"),
            named_serving("other-provider", "*.eu.example"),
        ]);

        s.submit_pending(pending("r1", "api.eu.example"), Instant::now());
        s.submit_pending(pending("r2", "api.eu.example"), Instant::now());

        assert_eq!(
            n.informed.lock().unwrap().len(),
            1,
            "one misconfiguration is one message, however busy the workload"
        );
    }

    /// One recorded grant: which method, under what account, and against which bytes.
    #[derive(Debug, PartialEq, Eq)]
    struct Granted {
        name: String,
        digest: String,
        method: String,
        profile: Option<String>,
    }

    /// One recorded connect: which method, under what name, and which values it was given.
    #[derive(Debug, PartialEq, Eq)]
    struct Connected {
        name: String,
        method: String,
        label: String,
        keys: Vec<String>,
    }

    #[derive(Default)]
    struct FakeConnectorPort {
        connected: StdMutex<Vec<Connected>>,
        granted: StdMutex<Vec<Granted>>,
        opens: Option<GrantedPayload>,
        invalidated: Vec<String>,
        refuse: Option<String>,
    }

    impl ConnectorPort for FakeConnectorPort {
        fn connect(
            &self,
            name: &str,
            method: &str,
            label: &str,
            values: lns_ipc::SecretValues,
        ) -> Result<Vec<String>, String> {
            if let Some(why) = &self.refuse {
                return Err(why.clone());
            }
            self.connected.lock().unwrap().push(Connected {
                name: name.to_string(),
                method: method.to_string(),
                label: label.to_string(),
                keys: values.0.keys().cloned().collect(),
            });
            Ok(self.invalidated.clone())
        }

        fn grant(
            &self,
            name: &str,
            digest: &str,
            method: &str,
            profile: Option<&str>,
        ) -> Result<GrantedPayload, String> {
            if let Some(why) = &self.refuse {
                return Err(why.clone());
            }
            self.granted.lock().unwrap().push(Granted {
                name: name.to_string(),
                digest: digest.to_string(),
                method: method.to_string(),
                profile: profile.map(str::to_string),
            });
            Ok(self.opens.clone().unwrap_or_default())
        }

        fn decline(&self, _name: &str) -> Result<(), String> {
            match &self.refuse {
                Some(why) => Err(why.clone()),
                None => Ok(()),
            }
        }
    }

    fn offering(method: &str, profiles: &[&str]) -> ConnectorView {
        ConnectorView {
            name: "some-provider".to_string(),
            digest: "sha256:abc".to_string(),
            serves: vec!["api.some-provider.example".to_string()],
            methods: vec![lns_ipc::ConnectorMethodView {
                name: method.to_string(),
                label: method.to_string(),
                needs_connect: !profiles.is_empty(),
                offerable: true,
                opens: vec!["api.some-provider.example".to_string()],
                writes: Vec::new(),
                env: Vec::new(),
                credentials: Vec::new(),
                help: None,
            }],
            profiles: profiles
                .iter()
                .map(|label| lns_ipc::ConnectorProfileView {
                    label: label.to_string(),
                    method: method.to_string(),
                    authority: Vec::new(),
                })
                .collect(),
        }
    }

    fn offered(port: Arc<FakeConnectorPort>) -> Fixture {
        let f = fixture();
        f.0.set_connector_port(port);
        f.0.hold_for_offers(vec![offering("token", &["work", "personal"])]);
        f.0.submit_pending(pending("r1", "api.some-provider.example"), Instant::now());
        f
    }

    #[test]
    fn granting_records_the_profile_the_card_chose() {
        let port = Arc::new(FakeConnectorPort::default());
        let (s, _n, _store, _rx) = offered(port.clone());

        s.grant_offer("r1", "token", ProfileChoice::Held("personal".into()));

        assert_eq!(
            port.granted.lock().unwrap().as_slice(),
            [Granted {
                name: "some-provider".to_string(),
                digest: "sha256:abc".to_string(),
                method: "token".to_string(),
                profile: Some("personal".to_string()),
            }],
            "the card is where the user names the account, and the bytes it disclosed are the bytes consented to"
        );
    }

    #[test]
    fn granting_publishes_the_opened_egress_before_it_releases_the_request() {
        // The guest re-evaluates the released request against the table it holds, so a decision that arrives first is decided by the old one.
        let port = Arc::new(FakeConnectorPort {
            opens: Some(GrantedPayload {
                egress: allowing("api.some-provider.example"),
                ..GrantedPayload::default()
            }),
            ..FakeConnectorPort::default()
        });
        let (s, _n, _store, mut rx) = offered(port);

        s.grant_offer("r1", "token", ProfileChoice::Held("work".into()));

        let published = policy_frame(&mut rx);
        assert!(
            published
                .network
                .expect("a network section")
                .egress
                .http
                .iter()
                .any(|rule| rule.match_pattern == "api.some-provider.example"
                    && rule.verdict == WireVerdict::Allow),
            "the grant's own rules must be in the table before the request is let go"
        );
        assert_eq!(decision_frame(&mut rx).decision, Decision::AllowOnce);
    }

    #[test]
    fn granting_lifts_the_hold_so_the_next_frame_stops_asking() {
        let port = Arc::new(FakeConnectorPort::default());
        let (s, _n, _store, mut rx) = offered(port);

        s.grant_offer("r1", "token", ProfileChoice::Held("work".into()));

        let published = policy_frame(&mut rx);
        assert!(
            !published
                .network
                .expect("a network section")
                .egress
                .tcp
                .iter()
                .any(|rule| rule.verdict == WireVerdict::Ask),
            "the connector is decided, so nothing it serves is held any more"
        );
    }

    #[test]
    fn granting_releases_every_request_held_for_the_same_connector() {
        // One answer decides the connector, so a sibling request waiting on it must not sit there until it times out.
        let port = Arc::new(FakeConnectorPort::default());
        let (s, n, _store, mut rx) = offered(port);
        s.submit_pending(pending("r2", "api.some-provider.example"), Instant::now());

        s.grant_offer("r1", "token", ProfileChoice::Held("work".into()));

        let _ = policy_frame(&mut rx);
        let mut released: Vec<String> = Vec::new();
        while let Ok(frame) = rx.try_recv() {
            if let HostFrame::RequestDecision(decision) = frame {
                assert_eq!(decision.decision, Decision::AllowOnce);
                released.push(decision.id);
            }
        }
        released.sort();
        assert_eq!(released, ["r1", "r2"]);
        assert_eq!(n.dismissed.lock().unwrap().len(), 2);
    }

    #[test]
    fn a_new_connection_is_made_before_the_grant_that_names_it() {
        // The grant looks its profile up in the store, so a grant written first would refuse the label the user just typed.
        let port = Arc::new(FakeConnectorPort::default());
        let (s, _n, _store, _rx) = offered(port.clone());

        s.grant_offer(
            "r1",
            "token",
            ProfileChoice::New {
                label: "token-2".into(),
                values: lns_ipc::SecretValues(
                    [("SOME_TOKEN".to_string(), "sk-live".to_string())]
                        .into_iter()
                        .collect(),
                ),
            },
        );

        assert_eq!(
            port.connected.lock().unwrap()[0].label,
            "token-2",
            "the connect must have happened, under the name the card carried"
        );
        assert_eq!(
            port.granted.lock().unwrap()[0].profile,
            Some("token-2".to_string()),
            "and the grant must name the profile it just created"
        );
    }

    #[test]
    fn a_connect_that_fails_grants_nothing_and_leaves_the_offer_standing() {
        // §3.2.4: authentication that fails grants nothing. A card that vanished would leave the request held with no way to answer it.
        let port = Arc::new(FakeConnectorPort {
            refuse: Some("the token was rejected".into()),
            ..FakeConnectorPort::default()
        });
        let (s, n, _store, mut rx) = offered(port.clone());

        s.grant_offer(
            "r1",
            "token",
            ProfileChoice::New {
                label: "token-2".into(),
                values: lns_ipc::SecretValues::default(),
            },
        );

        assert!(port.granted.lock().unwrap().is_empty());
        assert!(
            rx.try_recv().is_err(),
            "no frame at all: the request is still held and the table did not change"
        );
        assert!(n.dismissed.lock().unwrap().is_empty(), "the card stays");
        assert!(
            n.informed
                .lock()
                .unwrap()
                .iter()
                .any(|line| line.contains("the token was rejected")),
            "the user is told why"
        );
    }

    #[test]
    fn a_connection_that_invalidates_another_projects_grant_says_so() {
        // §3.2.4: those projects must decide again, and nothing in this run can do it for them.
        let port = Arc::new(FakeConnectorPort {
            invalidated: vec!["/other/project".to_string()],
            ..FakeConnectorPort::default()
        });
        let (s, n, _store, _rx) = offered(port);

        s.grant_offer(
            "r1",
            "token",
            ProfileChoice::New {
                label: "token-2".into(),
                values: lns_ipc::SecretValues::default(),
            },
        );

        assert!(
            n.informed
                .lock()
                .unwrap()
                .iter()
                .any(|line| line.contains("/other/project")),
            "a silent invalidation is a project that stops working with no explanation"
        );
    }

    #[test]
    fn granting_a_method_that_needs_no_account_names_no_profile() {
        let port = Arc::new(FakeConnectorPort::default());
        let (s, _n, _store, _rx) = fixture();
        s.set_connector_port(port.clone());
        s.hold_for_offers(vec![offering("open", &[])]);
        s.submit_pending(pending("r1", "api.some-provider.example"), Instant::now());

        s.grant_offer("r1", "open", ProfileChoice::None);

        assert_eq!(port.granted.lock().unwrap()[0].profile, None);
    }

    #[test]
    fn a_grant_the_store_refuses_leaves_the_request_held() {
        // The bytes changed under the card, so the store refuses; the card must not vanish and the table must not move.
        let port = Arc::new(FakeConnectorPort {
            refuse: Some("some-provider was replaced since this card was raised".into()),
            ..FakeConnectorPort::default()
        });
        let (s, n, _store, mut rx) = offered(port);

        s.grant_offer("r1", "token", ProfileChoice::Held("work".into()));

        assert!(rx.try_recv().is_err(), "nothing was published or released");
        assert!(n.dismissed.lock().unwrap().is_empty());
        assert!(
            n.informed
                .lock()
                .unwrap()
                .iter()
                .any(|line| line.contains("replaced")),
            "the user is told why"
        );
    }

    #[test]
    fn answering_a_card_whose_request_is_gone_is_not_an_answer() {
        let port = Arc::new(FakeConnectorPort::default());
        let (s, _n, _store, _rx) = fixture();
        s.set_connector_port(port);
        assert_eq!(
            s.grant_offer("gone", "token", ProfileChoice::None),
            DecisionOutcome::UnknownId
        );
        assert_eq!(s.decline_offer("gone"), DecisionOutcome::UnknownId);
    }

    #[test]
    fn a_run_with_no_connector_store_wired_grants_nothing_and_says_so() {
        // Rather than panicking on a missing port, or silently dropping the answer the user gave.
        let (s, n, _store, mut rx) = fixture();
        s.hold_for_offers(vec![offering("open", &[])]);
        s.submit_pending(pending("r1", "api.some-provider.example"), Instant::now());

        s.grant_offer("r1", "open", ProfileChoice::None);

        assert!(rx.try_recv().is_err());
        assert!(
            n.informed
                .lock()
                .unwrap()
                .iter()
                .any(|line| line.contains("nothing was granted"))
        );
    }

    #[test]
    fn a_decline_the_store_refuses_is_not_recorded_as_one() {
        let port = Arc::new(FakeConnectorPort {
            refuse: Some("the grants file is read-only".into()),
            ..FakeConnectorPort::default()
        });
        let (s, n, _store, _rx) = offered(port);

        s.decline_offer("r1");

        assert!(
            n.informed
                .lock()
                .unwrap()
                .iter()
                .any(|line| line.contains("read-only"))
        );
        s.submit_pending(pending("r2", "api.some-provider.example"), Instant::now());
        assert!(
            n.presented.lock().unwrap().last().unwrap().offer.is_some(),
            "nothing was recorded, so the next request is still offered"
        );
    }

    #[test]
    fn declining_keeps_the_request_and_lets_the_ordinary_card_ask() {
        // The hold already replaced this run's allow with an ask for this request, and the host has no first-match evaluator to answer it with.
        let port = Arc::new(FakeConnectorPort::default());
        let (s, n, _store, mut rx) = offered(port);

        s.decline_offer("r1");

        assert!(
            rx.try_recv().is_err(),
            "no decision frame: the request is still the user's to answer"
        );
        let presented = n.presented.lock().unwrap();
        assert!(
            presented
                .last()
                .expect("a re-presented card")
                .offer
                .is_none(),
            "the connector is decided, so what is left is the ordinary question"
        );
    }

    #[test]
    fn declining_stops_the_next_request_being_offered() {
        let port = Arc::new(FakeConnectorPort::default());
        let (s, n, _store, _rx) = offered(port);

        s.decline_offer("r1");
        s.submit_pending(pending("r2", "api.some-provider.example"), Instant::now());

        assert!(
            n.presented.lock().unwrap().last().unwrap().offer.is_none(),
            "a decline is a standing no for this project (§3.2.4)"
        );
    }

    #[test]
    fn a_hold_that_expires_keeps_the_card_so_the_connect_can_finish() {
        // §3.2.4: "An expired hold does not cancel the connect: the user finishes, the profile applies, and the next request succeeds." Dismissing here would take the card away mid-token.
        let port = Arc::new(FakeConnectorPort::default());
        let (s, n, _store, mut rx) = offered(port.clone());

        s.tick_timeouts(Instant::now() + TEST_TIMEOUT + Duration::from_millis(1));

        assert_eq!(
            decision_frame(&mut rx).decision,
            Decision::Timeout,
            "the workload's request fails as any refused request does"
        );
        assert!(
            n.dismissed.lock().unwrap().is_empty(),
            "but the card is still there to answer"
        );
        assert_eq!(
            s.grant_offer("r1", "token", ProfileChoice::Held("work".into())),
            DecisionOutcome::Resolved,
            "and the grant still applies, for whatever runs next"
        );
        assert_eq!(port.granted.lock().unwrap().len(), 1);
    }

    #[test]
    fn an_expired_request_is_not_decided_twice() {
        // The guesthas forgotten this id when it timed out; a second frame would decide a request that no longer exists.
        let port = Arc::new(FakeConnectorPort::default());
        let (s, _n, _store, mut rx) = offered(port);
        s.tick_timeouts(Instant::now() + TEST_TIMEOUT + Duration::from_millis(1));
        assert_eq!(decision_frame(&mut rx).decision, Decision::Timeout);

        s.grant_offer("r1", "token", ProfileChoice::Held("work".into()));

        let _ = policy_frame(&mut rx);
        assert!(
            rx.try_recv().is_err(),
            "the policy frame is the only thing left to send"
        );
    }

    #[test]
    fn closing_an_expired_card_does_not_answer_its_request_again() {
        // The guest forgot this id when it timed out; a second Timeout would decide a request that no longer exists.
        let port = Arc::new(FakeConnectorPort::default());
        let (s, _n, _store, mut rx) = offered(port);
        s.tick_timeouts(Instant::now() + TEST_TIMEOUT + Duration::from_millis(1));
        assert_eq!(decision_frame(&mut rx).decision, Decision::Timeout);

        assert_eq!(s.dismiss_request("r1"), DecisionOutcome::Resolved);

        assert!(rx.try_recv().is_err(), "no second frame for the same id");
    }

    #[test]
    fn answering_an_expired_card_writes_the_rule_without_answering_the_wire() {
        // The rule is the developer's standing answer and outlives the request; the frame is about a request the guest no longer has.
        let port = Arc::new(FakeConnectorPort::default());
        let (s, _n, store, mut rx) = offered(port);
        s.tick_timeouts(Instant::now() + TEST_TIMEOUT + Duration::from_millis(1));
        assert_eq!(decision_frame(&mut rx).decision, Decision::Timeout);

        s.decline_offer("r1");
        assert_eq!(
            s.record_decision("r1", Decision::AllowAlways),
            DecisionOutcome::Resolved
        );

        assert!(
            s.current_policy()
                .network
                .egress
                .http
                .iter()
                .any(|rule| rule.match_pattern == "api.some-provider.example"),
            "the developer's always-allow still stands for what runs next"
        );
        assert!(!store.saves.lock().unwrap().is_empty(), "and is written");
        let frames: Vec<HostFrame> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert!(
            frames
                .iter()
                .all(|frame| matches!(frame, HostFrame::Policy(_))),
            "but no decision frame for a request that timed out: {frames:?}"
        );
    }

    #[test]
    fn an_expired_hold_is_not_swept_a_second_time() {
        let port = Arc::new(FakeConnectorPort::default());
        let (s, _n, _store, _rx) = offered(port);
        let past = Instant::now() + TEST_TIMEOUT + Duration::from_millis(1);

        assert_eq!(s.tick_timeouts(past), 1);
        assert_eq!(
            s.tick_timeouts(past),
            0,
            "the entry stays only to carry the offer, so it must not look like a fresh timeout every tick"
        );
    }

    #[test]
    fn an_ordinary_card_still_goes_away_when_its_request_times_out() {
        let (s, n, _store, mut rx) = fixture();
        s.submit_pending(pending("r1", "api.example.test"), Instant::now());

        s.tick_timeouts(Instant::now() + TEST_TIMEOUT + Duration::from_millis(1));

        assert_eq!(decision_frame(&mut rx).decision, Decision::Timeout);
        assert_eq!(n.dismissed.lock().unwrap().as_slice(), &["r1".to_string()]);
    }

    #[test]
    fn a_session_holding_nothing_publishes_the_policy_unchanged() {
        let (s, _n, _store, _rx) = fixture_holding(allowing_one_raw_destination(
            "db.some-provider.example:5432",
        ));
        assert_eq!(
            tcp_verdicts(&s.policy_message()),
            [(
                "db.some-provider.example:5432".to_string(),
                WireVerdict::Allow
            )],
            "a machine with no undecided connector installed sends what it always sent"
        );
    }

    #[test]
    fn a_reload_republishes_the_hold_rather_than_dropping_it() {
        // The developer edits the decisions file mid-run; the connector is still undecided, so its destinations must stay held.
        let (s, _n, _store, mut rx) = fixture_holding(allowing_one_raw_destination(
            "db.some-provider.example:5432",
        ));
        s.hold_for_offers(vec![serving("db.some-provider.example")]);

        s.apply_external_policy(allowing_one_raw_destination(
            "db.some-provider.example:5432",
        ));

        let published = policy_frame(&mut rx);
        assert_eq!(
            tcp_verdicts(&published)
                .iter()
                .filter(|(_, verdict)| *verdict == WireVerdict::Ask)
                .count(),
            1,
            "the reloaded frame still holds the served destination: {published:?}"
        );
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

    fn drain(rx: &mut mpsc::UnboundedReceiver<HostFrame>) -> Vec<HostFrame> {
        let mut frames = Vec::new();
        while let Ok(frame) = rx.try_recv() {
            frames.push(frame);
        }
        frames
    }

    fn frame_kinds(frames: &[HostFrame]) -> Vec<&'static str> {
        frames
            .iter()
            .map(|frame| match frame {
                HostFrame::Policy(_) => "policy",
                HostFrame::RequestDecision(_) => "decision",
            })
            .collect()
    }

    struct StoreWatchingFrames {
        frames_before_save: StdMutex<Vec<HostFrame>>,
        frames: StdMutex<mpsc::UnboundedReceiver<HostFrame>>,
    }

    impl PolicyStore for StoreWatchingFrames {
        fn save(&self, _policy: &Policy) -> io::Result<()> {
            let mut frames = self.frames.lock().unwrap();
            self.frames_before_save
                .lock()
                .unwrap()
                .extend(drain(&mut frames));
            Ok(())
        }
    }

    #[test]
    fn the_policy_an_always_decision_writes_reaches_the_guest_before_the_decision() {
        let (s, _n, _store, mut rx) = fixture();
        s.submit_pending(pending("r1", "api.linear.app"), Instant::now());

        s.record_decision("r1", Decision::AllowAlways);

        assert_eq!(
            frame_kinds(&drain(&mut rx)),
            vec!["policy", "decision"],
            "the decision wakes the held connection, so a policy behind it can be read too late"
        );
    }

    #[test]
    fn the_decisions_file_is_written_only_once_both_frames_have_left() {
        let (tx, rx) = mpsc::unbounded_channel();
        let store = Arc::new(StoreWatchingFrames {
            frames_before_save: StdMutex::new(Vec::new()),
            frames: StdMutex::new(rx),
        });
        let session = ApprovalSession::new(
            Policy::default(),
            Policy::default(),
            Arc::new(RecordingNotifier::default()),
            store.clone(),
            tx,
            TEST_TIMEOUT,
        );
        session.submit_pending(pending("r1", "api.linear.app"), Instant::now());

        session.record_decision("r1", Decision::AllowAlways);

        assert_eq!(
            frame_kinds(&store.frames_before_save.lock().unwrap()),
            vec!["policy", "decision"],
            "a slow or failing disk write must not delay releasing the held request"
        );
    }

    #[test]
    fn a_decision_publishes_the_standing_tables_and_not_just_the_rule_it_wrote() {
        let mut standing = Policy::default();
        standing
            .network
            .egress
            .tcp
            .push(TcpEgressRule::allow_destination("db.internal:5432"));
        let (s, _n, _store, mut rx) = fixture_holding(standing);
        s.submit_pending(pending("r1", "api.linear.app"), Instant::now());

        s.record_decision("r1", Decision::AllowAlways);

        let network = policy_frame(&mut rx)
            .network
            .expect("a frame without a network section retracts every rule the guest holds");
        assert_eq!(
            network.egress.tcp,
            vec![WireTcpEgressRule::from(&TcpEgressRule::allow_destination(
                "db.internal:5432"
            ))],
            "the guest replaces every map on each apply, so a table left out of this frame is a table withdrawn"
        );
        assert_eq!(
            network
                .egress
                .http
                .iter()
                .map(|route| route.match_pattern.as_str())
                .collect::<Vec<_>>(),
            vec!["api.linear.app"]
        );
    }

    #[test]
    fn a_published_policy_carries_the_defaults_the_guest_cannot_derive_from_the_tables() {
        let (s, _n, _store, mut rx) = fixture();
        s.submit_pending(raw_pending("r1", "db.internal:5432"), Instant::now());

        s.record_decision("r1", Decision::AllowAlways);

        let network = policy_frame(&mut rx).network.expect("a network section");
        assert_eq!(
            network.egress.tcp,
            vec![WireTcpEgressRule::from(
                &TcpEgressRule::allow_destination("db.internal:5432").approved()
            )]
        );
        assert_eq!(
            network.default_verdict,
            WireDefaultVerdict::Ask,
            "a policy that decides nothing else must keep asking rather than fail every destination closed"
        );
        assert_eq!(
            network.default_transport,
            Transport::Direct,
            "core fail-closes a non-deny verdict when the transport default is anything else"
        );
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

        let pushed = policy_frame(&mut rx);
        assert_eq!(
            pushed.network.unwrap().egress.http[0].match_pattern,
            "api.linear.app"
        );
        assert_eq!(decision_frame(&mut rx).decision, Decision::AllowAlways);

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

        let pushed = policy_frame(&mut rx);
        assert_eq!(
            pushed.network.unwrap().egress.http[0].verdict,
            WireVerdict::Deny
        );
        assert_eq!(decision_frame(&mut rx).decision, Decision::DenyAlways);

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

        assert!(policy_frame(&mut rx).network.is_some());
        assert_eq!(decision_frame(&mut rx).decision, Decision::AllowAlways);

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
            pushed.network.unwrap().egress.http[0].match_pattern,
            "api.linear.app"
        );
    }

    #[test]
    fn a_reloaded_decision_decides_over_the_document_it_layers_onto() {
        let (s, _n, _store, mut rx) = fixture();
        let mut document = Policy::default();
        document.add_rule(RouteRule::deny_host("api.example.test"));
        s.set_shipped_policy(document);

        let mut reloaded = Policy::default();
        reloaded.add_rule(RouteRule::allow_host("api.example.test"));
        s.apply_external_policy(reloaded);

        let routes = s.current_policy().network.egress.http;
        let allow_idx = routes
            .iter()
            .position(|r| r.match_pattern == "api.example.test" && r.verdict == Verdict::Allow)
            .expect("the approval the developer just made must survive the reload");
        let deny_idx = routes
            .iter()
            .position(|r| r.match_pattern == "api.example.test" && r.verdict == Verdict::Deny)
            .expect("what the document said is still in the table, behind the decision that overruled it");
        assert!(
            allow_idx < deny_idx,
            "the developer's decisions file is the last source, so an approval they just made must decide rather than be dropped by the document under it: {routes:?}"
        );
        let _ = policy_frame(&mut rx);
    }

    #[test]
    fn a_reload_keeps_what_a_grant_opened() {
        // The developer edits the decisions file mid-run; the grant is not in that file, so re-reading it must not retract what a connector opened.
        let (s, _n, _store, mut rx) = fixture();
        let mut document = Policy::default();
        document.add_rule(RouteRule::deny_host("*"));
        s.set_shipped_policy(document);
        s.apply_granted_egress(
            "some-provider",
            GrantedPayload {
                egress: allowing("api.some-provider.example"),
                ..GrantedPayload::default()
            },
        );
        let _ = policy_frame(&mut rx);

        s.apply_external_policy(allowing("unrelated.example"));

        let routes = s.current_policy().network.egress.http;
        assert!(
            routes
                .iter()
                .any(|rule| rule.match_pattern == "api.some-provider.example"),
            "the grant is a source of its own: {routes:?}"
        );
        let _ = policy_frame(&mut rx);
    }

    #[test]
    fn what_a_grant_supplies_rides_on_every_later_frame() {
        // The guest replaces each section a frame carries, so a reload that omitted them would retract the credential mid-run.
        let (s, _n, _store, mut rx) = fixture();
        s.apply_granted_egress(
            "some-provider",
            GrantedPayload {
                egress: allowing("api.some-provider.example"),
                credentials: vec![crate::approval_flow::protocol::WireCredential {
                    id: "SOME_TOKEN".into(),
                    env_var: Some("SOME_TOKEN".into()),
                    placeholder: Some("some-provider-LNSPLACEHOLDER00".into()),
                    injections: Vec::new(),
                }],
                env: [("SOME_REGION".to_string(), "eu".to_string())]
                    .into_iter()
                    .collect(),
                files: vec![crate::approval_flow::protocol::WireFile::text(
                    "~/.some-provider/config.json",
                    "{}",
                    None,
                )],
            },
        );
        let _ = policy_frame(&mut rx);

        s.apply_external_policy(allowing("unrelated.example"));

        let republished = policy_frame(&mut rx);
        assert_eq!(
            republished.credentials.map(|c| c.len()),
            Some(1),
            "the credential must still be armed after an unrelated reload"
        );
        assert_eq!(
            republished
                .env
                .and_then(|env| env.get("SOME_REGION").cloned()),
            Some("eu".to_string())
        );
        assert_eq!(
            republished
                .files
                .as_deref()
                .map(|files| files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>()),
            Some(vec!["~/.some-provider/config.json"]),
            "a frame that omits files deletes the ones the last frame wrote, so an unrelated reload would take the granted file back"
        );
    }

    #[test]
    fn a_second_grant_does_not_retract_the_first() {
        // The guest replaces each section it receives, so a frame carrying only the newest grant takes the older one's egress and credential away mid-run.
        let (s, _n, _store, mut rx) = fixture();
        s.apply_granted_egress(
            "alpha",
            GrantedPayload {
                egress: allowing("api.alpha.example"),
                credentials: vec![credential("ALPHA_TOKEN")],
                env: [("ALPHA_REGION".to_string(), "eu".to_string())]
                    .into_iter()
                    .collect(),
                ..GrantedPayload::default()
            },
        );
        let _ = policy_frame(&mut rx);

        s.apply_granted_egress(
            "beta",
            GrantedPayload {
                egress: allowing("api.beta.example"),
                credentials: vec![credential("BETA_TOKEN")],
                env: [("BETA_REGION".to_string(), "us".to_string())]
                    .into_iter()
                    .collect(),
                ..GrantedPayload::default()
            },
        );

        let published = policy_frame(&mut rx);
        let allowed: Vec<String> = published
            .network
            .expect("a network section")
            .egress
            .http
            .iter()
            .map(|rule| rule.match_pattern.clone())
            .collect();
        assert!(
            allowed.contains(&"api.alpha.example".to_string())
                && allowed.contains(&"api.beta.example".to_string()),
            "both grants are the project's; neither answer undid the other: {allowed:?}"
        );
        let armed: Vec<String> = published
            .credentials
            .expect("credentials")
            .into_iter()
            .map(|credential| credential.id)
            .collect();
        assert_eq!(armed, ["ALPHA_TOKEN", "BETA_TOKEN"]);
        let env = published.env.expect("env");
        assert_eq!(env.len(), 2, "{env:?}");
    }

    fn credential(id: &str) -> crate::approval_flow::protocol::WireCredential {
        crate::approval_flow::protocol::WireCredential {
            id: id.to_string(),
            env_var: Some(id.to_string()),
            placeholder: Some(format!("{id}-LNSPLACEHOLDER00")),
            injections: Vec::new(),
        }
    }

    #[test]
    fn a_run_that_granted_nothing_sends_no_supply_sections_at_all() {
        // An empty list is not the same as no section: the guest would read one as "replace what you have with nothing".
        let (s, _n, _store, mut rx) = fixture();
        s.apply_external_policy(allowing("api.example.test"));

        let published = policy_frame(&mut rx);
        assert_eq!((published.credentials, published.env), (None, None));
    }

    #[test]
    fn a_live_grant_leaves_the_policy_a_recorded_grant_produces() {
        // The design rests on this: a grant taken mid-run must leave the run in the state the recorded grant produces. That the boot path installs that same payload is pinned in supervisor::adapter, against the store rather than this fake.
        let opened = allowing("api.some-provider.example");
        let mut shipped = Policy::default();
        shipped.add_rule(RouteRule::deny_host("*"));
        let mut own = Policy::default();
        own.add_rule(RouteRule::allow_host("docs.some-vendor.example"));

        let live = {
            let port = Arc::new(FakeConnectorPort {
                opens: Some(GrantedPayload {
                    egress: opened.clone(),
                    ..GrantedPayload::default()
                }),
                ..FakeConnectorPort::default()
            });
            let (s, _n, _store, _rx) = fixture();
            s.set_connector_port(port);
            s.set_shipped_policy(shipped.clone());
            s.apply_external_policy(own.clone());
            s.hold_for_offers(vec![offering("token", &["work"])]);
            s.submit_pending(pending("r1", "api.some-provider.example"), Instant::now());
            s.grant_offer("r1", "token", ProfileChoice::Held("work".into()));
            s.policy_message()
        };

        let relaunched = {
            // A fresh run of the same project: the grant is already recorded, so nothing is held and the opened egress is in place from the first frame.
            let (s, _n, _store, _rx) = fixture();
            s.set_shipped_policy(shipped);
            s.apply_external_policy(own);
            s.apply_granted_egress(
                "some-provider",
                GrantedPayload {
                    egress: opened,
                    ..GrantedPayload::default()
                },
            );
            s.policy_message()
        };

        assert_eq!(
            live, relaunched,
            "a grant applied live must leave exactly the table a relaunch would boot with"
        );
    }

    #[test]
    fn a_grant_is_decided_by_this_directorys_own_deny() {
        // §3.3.2: the connector is source 4 and the decisions file source 5, so a `deny` the developer typed still wins.
        let (s, _n, _store, mut rx) = fixture();
        s.set_shipped_policy(Policy::default());
        let mut typed = Policy::default();
        typed.add_rule(RouteRule::deny_host("api.some-provider.example"));
        s.apply_external_policy(typed);
        let _ = policy_frame(&mut rx);

        s.apply_granted_egress(
            "some-provider",
            GrantedPayload {
                egress: allowing("api.some-provider.example"),
                ..GrantedPayload::default()
            },
        );

        let routes = s.current_policy().network.egress.http;
        assert_eq!(
            routes.first().map(|rule| rule.verdict),
            Some(Verdict::Deny),
            "a first-match gate must reach the developer's own deny first: {routes:?}"
        );
        let _ = policy_frame(&mut rx);
    }

    fn allowing(host: &str) -> Policy {
        let mut policy = Policy::default();
        policy.add_rule(RouteRule::allow_host(host));
        policy
    }

    #[test]
    fn a_destination_the_developer_deleted_mid_run_stops_being_decided() {
        let (s, _n, _store, mut rx) = fixture();
        let mut authored = Policy::default();
        authored.add_rule(RouteRule::deny_host("*"));
        s.set_shipped_policy(authored);
        let mut decided = Policy::default();
        decided.add_rule(RouteRule::allow_host("docs.some-vendor.example"));
        s.apply_external_policy(decided);
        let _ = policy_frame(&mut rx);

        s.apply_external_policy(Policy::default());

        assert_eq!(
            s.current_policy()
                .network
                .egress
                .http
                .iter()
                .map(|rule| (rule.match_pattern.clone(), rule.verdict))
                .collect::<Vec<_>>(),
            [("*".to_string(), Verdict::Deny)],
            "the baseline is what every source but this directory's own decided, so deleting the rule is what retracts it — a copy frozen at boot would keep the host open for the rest of the run"
        );
        let _ = policy_frame(&mut rx);
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
