use std::collections::HashMap;
use std::sync::Arc;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::approval_flow::protocol::{
    Decision, HostFrame, PolicyMessage, RequestDecision, RequestPending, Treatment, WireNetwork,
};
use crate::ledger::LedgerRecorder;
use lns_ipc::{ApprovalKind, Decision as LedgerDecision, LedgerEvent};
use lns_policy::matching::{split_destination, unbracketed};
use lns_policy::{Approval, Policy, PolicyStore, RouteRule, TcpEgressRule};

pub type FrameSink = mpsc::UnboundedSender<HostFrame>;

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
    /// `Raw` when approving splices the connection through unread, which the card has to say out loud.
    pub treatment: Treatment,
    /// The requesting run's name, attributed by the service from the session channel — never from workload-supplied data.
    pub run: Option<String>,
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

    pub fn set_ledger_recorder(&self, recorder: Arc<dyn LedgerRecorder>) {
        let _ = self.ledger.set(recorder);
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
                action: req.action.clone(),
                treatment: req.treatment,
                deadline: now + self.timeout,
                reason: req.reason.clone(),
            },
        );
        drop(pending);
        self.notifier.present(&PendingPrompt {
            id: req.id,
            host: req.host,
            action: req.action,
            treatment: req.treatment,
            run: self.run.clone(),
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

    pub fn apply_external_policy(&self, mut new_policy: Policy) {
        *self.persisted.lock().expect("persisted mutex poisoned") = new_policy.clone();
        if let Some(shipped) = self.shipped.get() {
            new_policy = crate::artifact::policy::merge_effective(Some(shipped), &new_policy);
        }
        *self.policy.lock().expect("policy mutex poisoned") = new_policy.clone();
        let _ = self.sink.send(HostFrame::Policy(PolicyMessage {
            network: Some(WireNetwork::seeded(new_policy.network)),
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
        let (approval, effective) = {
            let mut policy = self.policy.lock().expect("policy mutex poisoned");
            (policy.add_approved_rule(rule.clone()), policy.clone())
        };
        if approval == Approval::Stands {
            self.persisted
                .lock()
                .expect("persisted mutex poisoned")
                .add_approved_rule(rule);
        }
        self.publish_if_it_stands(approval, effective);
    }

    /// Says the decision stands for this request but outlived nothing.
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
        let (approval, pre_empted, effective) = {
            let mut policy = self.policy.lock().expect("policy mutex poisoned");
            let pre_empted = pre_empted_http_patterns(&policy, &rule);
            (
                policy.add_approved_tcp_rule(rule.clone()),
                pre_empted,
                policy.clone(),
            )
        };
        if approval == Approval::Stands {
            self.persisted
                .lock()
                .expect("persisted mutex poisoned")
                .add_approved_tcp_rule(rule);
        }
        if !self.publish_if_it_stands(approval, effective) {
            return;
        }
        // The http rules this raw rule displaces would otherwise go quiet without a word.
        if !pre_empted.is_empty() {
            self.notifier.inform(&format!(
                "that traffic is now spliced raw, so these HTTP rules no longer apply to it: {}",
                pre_empted.join(", ")
            ));
        }
    }

    /// Answers whether the decision stands, telling the developer when it applied to one request only — silence there would read as "remembered".
    fn publish_if_it_stands(&self, approval: Approval, effective: Policy) -> bool {
        let why = match approval {
            Approval::Stands => {
                self.publish_and_persist(effective);
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
    fn publish_and_persist(&self, effective: Policy) {
        let _ = self.sink.send(HostFrame::Policy(PolicyMessage {
            network: Some(WireNetwork::seeded(effective.network)),
        }));
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
fn raw_destination<'a>(action: &'a str, host: &str) -> Option<&'a str> {
    let destination = action.strip_prefix("CONNECT ")?;
    let (named, _) = split_destination(destination);
    // The gate strips the port from `host` before sending it, so only brackets come off here.
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
