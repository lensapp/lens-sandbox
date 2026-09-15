use crate::approval_flow::protocol::Decision;
use crate::approval_flow::session::{ConnectionChoice, PendingPrompt};
use lns_ipc::{
    ApprovalConnection, LiveApproval, LiveApprovalAction, LiveApprovalSnapshot, Response,
};
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::{mpsc, watch};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionDelivery {
    pub id: String,
    pub action: RequestAction,
}

/// What the user chose on a card: a wire decision, an answer about the connector that serves the destination, or a closed card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestAction {
    OpenConnectBrowser,
    BeginConnect {
        method: String,
        label: String,
    },
    AnswerConnect {
        values: lns_ipc::SecretValues,
    },
    Decide(Decision),
    /// Connect this run to the offered connector by the named method (§3.2.4).
    Grant {
        method: String,
        connection: ConnectionChoice,
    },
    /// A standing no for this run; the ordinary card then asks what the hold stood in for.
    Decline,
    /// A closed card: fail the held request, but record nothing — the developer made no decision.
    Dismiss,
}

pub struct ApprovalInbox {
    inner: Mutex<InboxInner>,
    updates: watch::Sender<LiveApprovalSnapshot>,
}

#[derive(Default)]
struct InboxInner {
    pending: Vec<PendingEntry>,
    informs: Vec<InformEntry>,
    next_seq: u64,
}

impl InboxInner {
    fn alloc_seq(&mut self) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        seq
    }

    fn order(&self) -> Vec<StackItem> {
        let mut keyed: Vec<(u64, StackItem)> = Vec::new();
        keyed.extend(seq_keyed(
            self.informs.iter().map(|e| e.seq),
            StackItem::Inform,
        ));
        keyed.extend(seq_keyed(
            self.pending.iter().map(|e| e.seq),
            StackItem::Network,
        ));
        keyed.sort_by_key(|(seq, _)| *seq);
        keyed.into_iter().map(|(_, item)| item).collect()
    }
}

fn seq_keyed(
    seqs: impl Iterator<Item = u64>,
    item: fn(usize) -> StackItem,
) -> impl Iterator<Item = (u64, StackItem)> {
    seqs.enumerate().map(move |(i, seq)| (seq, item(i)))
}

struct PendingEntry {
    presentation: String,
    token: String,
    waiting: bool,
    submitting: bool,
    prompt: PendingPrompt,
    decision_tx: mpsc::UnboundedSender<DecisionDelivery>,
    seq: u64,
}

struct InformEntry {
    msg: String,
    seq: u64,
}

impl ApprovalInbox {
    pub fn dismiss_notices(&self, notices: &[String]) -> Response {
        self.lock()
            .informs
            .retain(|entry| !notices.contains(&entry.msg));
        Response::Acknowledged
    }
    pub fn watch(&self) -> watch::Receiver<LiveApprovalSnapshot> {
        self.updates.subscribe()
    }

    pub fn respond(&self, token: &str, action: LiveApprovalAction) -> Response {
        let mut inner = self.lock();
        let Some(index) = inner
            .pending
            .iter()
            .position(|entry| entry.token == token && !entry.submitting)
        else {
            return Response::LiveApprovalStale;
        };
        let action = match requested_action(&inner.pending[index].prompt, action) {
            Ok(action) => action,
            Err(message) => return Response::Error { message },
        };
        let keep = !matches!(action, RequestAction::Dismiss | RequestAction::Decide(_));
        let entry = &inner.pending[index];
        if entry
            .decision_tx
            .send(DecisionDelivery {
                id: entry.prompt.id.clone(),
                action,
            })
            .is_err()
        {
            inner.pending.remove(index);
            return Response::Error {
                message: "the run is no longer receiving decisions".into(),
            };
        }
        if keep {
            inner.pending[index].submitting = true;
        } else {
            inner.pending.remove(index);
        }
        Response::LiveApprovalSubmitted
    }

    pub fn complete_delivery(&self, id: &str) {
        if let Some(entry) = self
            .lock()
            .pending
            .iter_mut()
            .find(|entry| entry.prompt.id == id && entry.submitting)
        {
            entry.submitting = false;
            entry.token = uuid::Uuid::new_v4().to_string();
        }
    }

    pub fn expire(&self, id: &str) {
        if let Some(entry) = self
            .lock()
            .pending
            .iter_mut()
            .find(|entry| entry.prompt.id == id)
        {
            entry.waiting = false;
            entry.token = uuid::Uuid::new_v4().to_string();
        }
    }

    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(InboxInner::default()),
            updates: watch::channel(LiveApprovalSnapshot::default()).0,
        })
    }

    pub fn insert_pending(
        &self,
        prompt: PendingPrompt,
        decision_tx: mpsc::UnboundedSender<DecisionDelivery>,
    ) {
        let mut g = self.lock();
        // Presenting an id already on screen updates it in place: a declined offer re-presents the same request as the ordinary question, and the card must show that rather than the answer already given. The seq stays so the card keeps its place in the pile.
        if let Some(entry) = g.pending.iter_mut().find(|e| e.prompt.id == prompt.id) {
            entry.prompt = prompt;
            entry.token = uuid::Uuid::new_v4().to_string();
            entry.submitting = false;
            return;
        }
        let seq = g.alloc_seq();
        g.pending.push(PendingEntry {
            presentation: uuid::Uuid::new_v4().to_string(),
            token: uuid::Uuid::new_v4().to_string(),
            waiting: true,
            submitting: false,
            prompt,
            decision_tx,
            seq,
        });
    }

    pub fn remove_pending(&self, id: &str) {
        self.lock().pending.retain(|e| e.prompt.id != id);
    }

    pub fn push_inform(&self, msg: String) {
        let mut g = self.lock();
        let seq = g.alloc_seq();
        g.informs.push(InformEntry { msg, seq });
    }

    pub fn clear_informs(&self) {
        self.lock().informs.clear();
    }

    pub fn dismiss_inform(&self, index: usize) {
        let mut g = self.lock();
        if index < g.informs.len() {
            g.informs.remove(index);
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        let g = self.lock();
        Snapshot {
            pending: g.pending.iter().map(|e| e.prompt.clone()).collect(),
            informs: g.informs.iter().map(|e| e.msg.clone()).collect(),
            order: g.order(),
        }
    }

    pub fn pending_count(&self) -> usize {
        self.lock().pending.len()
    }

    pub fn decide(&self, id: &str, decision: Decision) -> bool {
        self.deliver(id, RequestAction::Decide(decision))
    }

    pub fn grant(&self, id: &str, method: &str, connection: ConnectionChoice) -> bool {
        self.deliver(
            id,
            RequestAction::Grant {
                method: method.to_string(),
                connection,
            },
        )
    }

    /// Keeps the card: a decline is answered by the ordinary question the hold stood in for, on the same request.
    pub fn decline(&self, id: &str) -> bool {
        self.keep_and_deliver(id, RequestAction::Decline)
    }

    /// Keeps the card: a sign-in runs over several rounds, and every one of them is drawn on the card that began it.
    pub fn begin_connect(&self, id: &str, method: String, label: String) -> bool {
        self.keep_and_deliver(id, RequestAction::BeginConnect { method, label })
    }

    pub fn open_connect_browser(&self, id: &str) -> bool {
        self.keep_and_deliver(id, RequestAction::OpenConnectBrowser)
    }

    pub fn answer_connect(&self, id: &str, values: lns_ipc::SecretValues) -> bool {
        self.keep_and_deliver(id, RequestAction::AnswerConnect { values })
    }

    fn keep_and_deliver(&self, id: &str, action: RequestAction) -> bool {
        let g = self.lock();
        let Some(entry) = g
            .pending
            .iter()
            .find(|e| e.prompt.id == id && !e.submitting)
        else {
            return false;
        };
        let _ = entry.decision_tx.send(DecisionDelivery {
            id: id.to_string(),
            action,
        });
        true
    }

    /// Drops the card and fails its held request without recording a decision. See [`RequestAction::Dismiss`].
    pub fn dismiss(&self, id: &str) -> bool {
        self.deliver(id, RequestAction::Dismiss)
    }

    fn deliver(&self, id: &str, action: RequestAction) -> bool {
        let mut g = self.lock();
        let Some(idx) = g
            .pending
            .iter()
            .position(|e| e.prompt.id == id && !e.submitting)
        else {
            return false;
        };
        let entry = g.pending.remove(idx);
        let _ = entry.decision_tx.send(DecisionDelivery {
            id: id.to_string(),
            action,
        });
        true
    }

    fn lock(&self) -> InboxGuard<'_> {
        InboxGuard {
            inner: self.inner.lock().expect("approval inbox mutex poisoned"),
            updates: &self.updates,
            changed: false,
        }
    }
}

struct InboxGuard<'a> {
    inner: std::sync::MutexGuard<'a, InboxInner>,
    updates: &'a watch::Sender<LiveApprovalSnapshot>,
    changed: bool,
}

impl std::ops::Deref for InboxGuard<'_> {
    type Target = InboxInner;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl std::ops::DerefMut for InboxGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.changed = true;
        &mut self.inner
    }
}

impl Drop for InboxGuard<'_> {
    fn drop(&mut self) {
        if self.changed {
            let snapshot =
                LiveApprovalSnapshot {
                    approvals: self
                        .inner
                        .pending
                        .iter()
                        .map(|entry| LiveApproval {
                            id: entry.presentation.clone(),
                            token: entry.token.clone(),
                            host: entry.prompt.host.clone(),
                            action: entry.prompt.action.clone(),
                            run: entry.prompt.run.clone(),
                            raw: entry.prompt.treatment
                                == crate::approval_flow::protocol::Treatment::Raw,
                            waiting: entry.waiting,
                            submitting: entry.submitting,
                            offer: entry.prompt.offer.clone(),
                            connect_seq: entry.prompt.connect_seq,
                            connect: entry.prompt.connect.as_ref().map(|ask| {
                                lns_ipc::LiveConnectAsk {
                                    connector: ask.connector.clone(),
                                    method: ask.method.clone(),
                                    message: ask.message.clone(),
                                    fields: ask.fields.clone(),
                                    from_code: ask.from_code,
                                    oauth: ask.oauth.clone(),
                                }
                            }),
                        })
                        .collect(),
                    notices: self
                        .inner
                        .informs
                        .iter()
                        .map(|entry| entry.msg.clone())
                        .collect(),
                };
            self.updates.send_if_modified(|current| {
                if *current == snapshot {
                    return false;
                }
                *current = snapshot;
                true
            });
        }
    }
}

fn requested_action(
    prompt: &PendingPrompt,
    action: LiveApprovalAction,
) -> Result<RequestAction, String> {
    match action {
        LiveApprovalAction::Dismiss => Ok(RequestAction::Dismiss),
        LiveApprovalAction::Decline if prompt.offer.is_some() => Ok(RequestAction::Decline),
        LiveApprovalAction::BeginConnect { method, label } if prompt.connect.is_none() => {
            let offer = prompt
                .offer
                .as_ref()
                .ok_or("this approval offers no connector")?;
            let selected = offer.methods.iter().any(|candidate| {
                candidate.name == method && candidate.offerable && candidate.auth_label.is_some()
            });
            if !selected
                || label.trim().is_empty()
                || offer.connections.iter().any(|held| held.label == label)
            {
                return Err("choose an available method and a new connection name".into());
            }
            Ok(RequestAction::BeginConnect { method, label })
        }
        LiveApprovalAction::AnswerConnect { values } if prompt.connect.is_some() => {
            Ok(RequestAction::AnswerConnect { values })
        }
        LiveApprovalAction::OpenConnectBrowser
            if prompt.connect.as_ref().is_some_and(|ask| {
                matches!(
                    ask.oauth,
                    Some(
                        lns_ipc::OAuthProgress::DeviceAuthorization { .. }
                            | lns_ipc::OAuthProgress::WaitingForBrowser { .. }
                    )
                )
            }) =>
        {
            Ok(RequestAction::OpenConnectBrowser)
        }
        LiveApprovalAction::Grant { method, connection } if prompt.connect.is_none() => {
            grant_action(prompt, method, connection)
        }
        action if prompt.offer.is_none() => network_action(action),
        _ => Err("this approval requires a connector decision".into()),
    }
}

fn network_action(action: LiveApprovalAction) -> Result<RequestAction, String> {
    let decision = match action {
        LiveApprovalAction::AllowOnce => Decision::AllowOnce,
        LiveApprovalAction::AllowAlways => Decision::AllowAlways,
        LiveApprovalAction::DenyOnce => Decision::DenyOnce,
        LiveApprovalAction::DenyAlways => Decision::DenyAlways,
        _ => return Err("this approval does not offer that action".into()),
    };
    Ok(RequestAction::Decide(decision))
}

fn grant_action(
    prompt: &PendingPrompt,
    method: String,
    connection: ApprovalConnection,
) -> Result<RequestAction, String> {
    let offer = prompt
        .offer
        .as_ref()
        .ok_or("this approval offers no connector")?;
    let connection = approval_connection(offer, &method, connection)?;
    Ok(RequestAction::Grant { method, connection })
}

pub(crate) fn approval_connection(
    offer: &lns_ipc::ConnectorView,
    method: &str,
    connection: ApprovalConnection,
) -> Result<ConnectionChoice, String> {
    let selected = offer
        .methods
        .iter()
        .find(|candidate| candidate.name == method && candidate.offerable)
        .ok_or("this connector method is unavailable")?;
    let choice = match connection {
        ApprovalConnection::None if selected.auth_label.is_none() => ConnectionChoice::None,
        ApprovalConnection::Held { label }
            if offer
                .connections
                .iter()
                .any(|held| held.label == label && held.method == method) =>
        {
            ConnectionChoice::Held(label)
        }
        ApprovalConnection::New { label, values }
            if valid_new_connection(offer, selected, &label, &values) =>
        {
            ConnectionChoice::New { label, values }
        }
        _ => return Err("choose a connection for the offered method".into()),
    };
    Ok(choice)
}

fn valid_new_connection(
    offer: &lns_ipc::ConnectorView,
    method: &lns_ipc::ConnectorMethodView,
    label: &str,
    values: &lns_ipc::SecretValues,
) -> bool {
    method.auth_label.is_some()
        && !label.trim().is_empty()
        && !offer.connections.iter().any(|held| held.label == label)
        && method
            .asks
            .iter()
            .all(|key| values.0.get(key).is_some_and(|value| !value.is_empty()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub pending: Vec<PendingPrompt>,
    pub informs: Vec<String>,
    /// Every entry above in arrival order, so a card keeps its place in the stack as others come and go.
    pub order: Vec<StackItem>,
}

/// One renderable entry of the approval window's stack, indexing into its [`Snapshot`] list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackItem {
    Inform(usize),
    Network(usize),
}

static GLOBAL: OnceLock<Arc<ApprovalInbox>> = OnceLock::new();

pub fn install(state: Arc<ApprovalInbox>) {
    let _ = GLOBAL.set(state);
}

pub fn get() -> Option<Arc<ApprovalInbox>> {
    GLOBAL.get().cloned()
}

#[cfg(test)]
mod tests {
    #[test]
    fn clearing_observed_notices_preserves_notices_that_arrived_later() {
        let inbox = super::ApprovalInbox::new();
        inbox.push_inform("old warning".into());
        let observed = inbox.watch().borrow().notices.clone();
        inbox.push_inform("new warning".into());
        assert_eq!(
            inbox.dismiss_notices(&observed),
            lns_ipc::Response::Acknowledged
        );
        assert_eq!(
            inbox.watch().borrow().notices,
            ["new warning"],
            "a client dismisses only the notices it saw"
        );
    }
    use super::*;

    #[test]
    fn native_sign_in_begins_without_collecting_outputs_and_keeps_the_card() {
        let inbox = ApprovalInbox::new();
        let (tx, mut rx) = mpsc::unbounded_channel();
        inbox.insert_pending(connector_prompt(), tx);
        let token = inbox.watch().borrow().approvals[0].token.clone();
        let action = serde_json::from_value(serde_json::json!({
            "kind": "begin_connect", "method": "token", "label": "personal"
        }))
        .expect("native cards must support service-driven sign-in");
        assert_eq!(
            inbox.respond(&token, action),
            Response::LiveApprovalSubmitted
        );
        assert_eq!(
            rx.try_recv().unwrap().action,
            RequestAction::BeginConnect {
                method: "token".into(),
                label: "personal".into()
            }
        );
        assert!(inbox.watch().borrow().approvals[0].submitting);
    }

    #[test]
    fn native_snapshot_preserves_the_current_sign_in_round() {
        let inbox = ApprovalInbox::new();
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut prompt = connector_prompt();
        prompt.connect_seq = 2;
        prompt.connect = Some(crate::approval_flow::session::ConnectAsk {
            connector: "provider".into(),
            method: "token".into(),
            message: "Next code".into(),
            fields: vec![lns_ipc::ConnectorFieldView {
                name: "otp".into(),
                label: "One-time code".into(),
                secret: true,
            }],
            from_code: true,
            oauth: None,
        });
        inbox.insert_pending(prompt, tx);
        let snapshot = serde_json::to_value(&*inbox.watch().borrow()).unwrap();
        assert_eq!(
            snapshot["approvals"][0]["connect"]["fields"][0]["name"],
            "otp"
        );
        assert_eq!(snapshot["approvals"][0]["connect_seq"], 2);
    }

    #[test]
    fn native_sign_in_rejects_stale_rounds_and_actions_the_card_did_not_offer() {
        let inbox = ApprovalInbox::new();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut prompt = connector_prompt();
        inbox.insert_pending(prompt.clone(), tx.clone());
        let token = inbox.watch().borrow().approvals[0].token.clone();
        for action in [
            LiveApprovalAction::BeginConnect {
                method: "missing".into(),
                label: "personal".into(),
            },
            LiveApprovalAction::BeginConnect {
                method: "token".into(),
                label: "work".into(),
            },
            LiveApprovalAction::BeginConnect {
                method: "token".into(),
                label: " ".into(),
            },
            LiveApprovalAction::AnswerConnect {
                values: Default::default(),
            },
            LiveApprovalAction::OpenConnectBrowser,
        ] {
            assert!(matches!(
                inbox.respond(&token, action),
                Response::Error { .. }
            ));
        }
        assert!(rx.try_recv().is_err());
        prompt.connect = Some(crate::approval_flow::session::ConnectAsk {
            connector: "provider".into(),
            method: "token".into(),
            message: String::new(),
            fields: vec![],
            from_code: false,
            oauth: Some(lns_ipc::OAuthProgress::DeviceAuthorization {
                verification_uri: "https://provider.test/device".into(),
                user_code: "ABCD".into(),
            }),
        });
        prompt.connect_seq = 1;
        inbox.insert_pending(prompt.clone(), tx.clone());
        assert_eq!(
            inbox.respond(&token, LiveApprovalAction::Dismiss),
            Response::LiveApprovalStale
        );
        let token = inbox.watch().borrow().approvals[0].token.clone();
        assert_eq!(
            inbox.respond(&token, LiveApprovalAction::OpenConnectBrowser),
            Response::LiveApprovalSubmitted
        );
        assert_eq!(
            rx.try_recv().unwrap().action,
            RequestAction::OpenConnectBrowser
        );
        assert!(inbox.watch().borrow().approvals[0].submitting);
        inbox.complete_delivery("request");
        let token = inbox.watch().borrow().approvals[0].token.clone();
        let values = lns_ipc::SecretValues([("scopeOption".into(), "read".into())].into());
        assert_eq!(
            inbox.respond(
                &token,
                LiveApprovalAction::AnswerConnect {
                    values: values.clone()
                }
            ),
            Response::LiveApprovalSubmitted
        );
        assert_eq!(
            rx.try_recv().unwrap().action,
            RequestAction::AnswerConnect { values }
        );
        assert_eq!(
            inbox.respond(&token, LiveApprovalAction::Dismiss),
            Response::LiveApprovalStale
        );
        inbox.complete_delivery("request");
        let token = inbox.watch().borrow().approvals[0].token.clone();
        assert!(matches!(
            inbox.respond(
                &token,
                LiveApprovalAction::BeginConnect {
                    method: "token".into(),
                    label: "personal".into()
                }
            ),
            Response::Error { .. }
        ));
        assert!(matches!(
            inbox.respond(
                &token,
                LiveApprovalAction::Grant {
                    method: "token".into(),
                    connection: ApprovalConnection::Held {
                        label: "work".into()
                    }
                }
            ),
            Response::Error { .. }
        ));
        prompt.connect = None;
        prompt.offer = None;
        inbox.insert_pending(prompt, tx);
        let token = inbox.watch().borrow().approvals[0].token.clone();
        assert!(matches!(
            inbox.respond(
                &token,
                LiveApprovalAction::BeginConnect {
                    method: "token".into(),
                    label: "personal".into()
                }
            ),
            Response::Error { .. }
        ));
    }

    #[test]
    fn native_browser_opening_requires_a_browser_ready_oauth_round() {
        for oauth in [
            None,
            Some(lns_ipc::OAuthProgress::SelectingScopes { options: vec![] }),
            Some(lns_ipc::OAuthProgress::Starting {
                destinations: vec![],
                scopes: vec![],
            }),
            Some(lns_ipc::OAuthProgress::Canceled),
            Some(lns_ipc::OAuthProgress::Expired),
            Some(lns_ipc::OAuthProgress::WaitingForBrowser {
                authorization_endpoint: "https://provider.test/authorize".into(),
                redirect_uri: "http://127.0.0.1:4567/callback".into(),
            }),
        ] {
            let allowed = matches!(
                oauth,
                Some(lns_ipc::OAuthProgress::WaitingForBrowser { .. })
            );
            let inbox = ApprovalInbox::new();
            let (tx, mut rx) = mpsc::unbounded_channel();
            let mut prompt = connector_prompt();
            prompt.connect = Some(crate::approval_flow::session::ConnectAsk {
                connector: "provider".into(),
                method: "token".into(),
                message: String::new(),
                fields: vec![],
                from_code: false,
                oauth,
            });
            inbox.insert_pending(prompt, tx);
            let token = inbox.watch().borrow().approvals[0].token.clone();
            let response = inbox.respond(&token, LiveApprovalAction::OpenConnectBrowser);
            if allowed {
                assert_eq!(response, Response::LiveApprovalSubmitted);
                assert_eq!(
                    rx.try_recv().unwrap().action,
                    RequestAction::OpenConnectBrowser
                );
            } else {
                assert!(matches!(response, Response::Error { .. }));
                assert!(
                    rx.try_recv().is_err(),
                    "no browser action may be delivered before the user selects permissions"
                );
            }
        }
    }

    fn connector_prompt() -> PendingPrompt {
        PendingPrompt {
            connect: None,
            connect_seq: 0,
            id: "request".into(),
            host: "api.example.com".into(),
            action: "CONNECT api.example.com:443".into(),
            treatment: crate::approval_flow::protocol::Treatment::Raw,
            run: Some("run".into()),
            offer: Some(lns_ipc::ConnectorView {
                description: None,
                name: "provider".into(),
                digest: "sha256:fixture".into(),
                serves: vec!["api.example.com".into()],
                methods: vec![lns_ipc::ConnectorMethodView {
                    oauth: None,
                    hosts: vec![],
                    runs_programs: false,
                    carries_code: false,
                    name: "token".into(),
                    label: "Token".into(),
                    auth_label: Some("Token".into()),
                    offerable: true,
                    opens: vec![],
                    writes: vec![],
                    env: vec![],
                    credentials: vec![],
                    asks: vec!["TOKEN".into()],
                    help: None,
                    overrides: Some(vec![]),
                }],
                connections: vec![lns_ipc::ConnectorConnectionView {
                    label: "work".into(),
                    method: "token".into(),
                    authority: vec!["read".into()],
                }],
            }),
        }
    }

    #[test]
    fn connector_hold_expiry_keeps_form_identity_but_invalidates_the_old_answer() {
        let inbox = ApprovalInbox::new();
        let (tx, _rx) = mpsc::unbounded_channel();
        inbox.insert_pending(connector_prompt(), tx);
        let before = inbox.watch().borrow().approvals[0].clone();
        inbox.expire("request");
        let after = inbox.watch().borrow().approvals[0].clone();
        assert_eq!(after.id, before.id);
        assert_ne!(after.token, before.token);
        assert!(!after.waiting);
        assert!(after.raw);
        assert_eq!(after.offer, before.offer);
        assert_eq!(
            inbox.respond(&before.token, LiveApprovalAction::Decline),
            Response::LiveApprovalStale
        );
    }

    #[test]
    fn a_connector_command_is_not_resubmitted_while_the_run_applies_it() {
        let inbox = ApprovalInbox::new();
        let (tx, mut rx) = mpsc::unbounded_channel();
        inbox.insert_pending(connector_prompt(), tx.clone());
        let token = inbox.watch().borrow().approvals[0].token.clone();
        assert_eq!(
            inbox.respond(&token, LiveApprovalAction::Decline),
            Response::LiveApprovalSubmitted
        );
        assert!(inbox.watch().borrow().approvals[0].submitting);
        assert_eq!(
            inbox.respond(&token, LiveApprovalAction::Decline),
            Response::LiveApprovalStale
        );
        assert_eq!(rx.try_recv().unwrap().action, RequestAction::Decline);
        assert!(rx.try_recv().is_err());
        inbox.complete_delivery("request");
        let after = inbox.watch().borrow().approvals[0].clone();
        assert!(!after.submitting);
        assert_ne!(after.token, token);
        inbox.insert_pending(connector_prompt(), tx);
        assert_ne!(inbox.watch().borrow().approvals[0].token, after.token);
        inbox.complete_delivery("absent");
        inbox.expire("absent");
    }

    #[test]
    fn clients_cannot_bypass_a_connector_offer_with_a_network_answer() {
        let inbox = ApprovalInbox::new();
        let (tx, mut rx) = mpsc::unbounded_channel();
        inbox.insert_pending(connector_prompt(), tx);
        let token = inbox.watch().borrow().approvals[0].token.clone();
        assert!(matches!(
            inbox.respond(&token, LiveApprovalAction::AllowAlways),
            Response::Error { .. }
        ));
        assert!(rx.try_recv().is_err());
        assert_eq!(inbox.watch().borrow().approvals.len(), 1);
    }

    #[test]
    fn connector_actions_require_an_offered_method_and_a_matching_connection() {
        let prompt = connector_prompt();
        let held = ApprovalConnection::Held {
            label: "work".into(),
        };
        assert_eq!(
            grant_action(&prompt, "token".into(), held).unwrap(),
            RequestAction::Grant {
                method: "token".into(),
                connection: ConnectionChoice::Held("work".into()),
            }
        );
        let values = lns_ipc::SecretValues([("TOKEN".into(), "secret".into())].into());
        assert!(
            grant_action(
                &prompt,
                "token".into(),
                ApprovalConnection::New {
                    label: "personal".into(),
                    values
                }
            )
            .is_ok()
        );
        for (method, connection) in [
            (
                "token",
                ApprovalConnection::New {
                    label: "work".into(),
                    values: lns_ipc::SecretValues([("TOKEN".into(), "replacement".into())].into()),
                },
            ),
            (
                "token",
                ApprovalConnection::New {
                    label: "personal".into(),
                    values: lns_ipc::SecretValues::default(),
                },
            ),
            ("missing", ApprovalConnection::None),
            ("token", ApprovalConnection::None),
            (
                "token",
                ApprovalConnection::Held {
                    label: "missing".into(),
                },
            ),
            (
                "token",
                ApprovalConnection::New {
                    label: " ".into(),
                    values: lns_ipc::SecretValues::default(),
                },
            ),
        ] {
            assert!(grant_action(&prompt, method.into(), connection).is_err());
        }
        let mut no_auth = prompt.clone();
        no_auth.offer.as_mut().unwrap().methods[0].auth_label = None;
        assert!(grant_action(&no_auth, "token".into(), ApprovalConnection::None).is_ok());
        let mut no_offer = prompt;
        no_offer.offer = None;
        assert!(grant_action(&no_offer, "token".into(), ApprovalConnection::None).is_err());
        assert!(requested_action(&no_offer, LiveApprovalAction::Decline).is_err());
    }

    #[test]
    fn the_existing_card_cannot_answer_while_another_client_is_submitting() {
        let inbox = ApprovalInbox::new();
        let (tx, _rx) = mpsc::unbounded_channel();
        inbox.insert_pending(connector_prompt(), tx);
        let token = inbox.watch().borrow().approvals[0].token.clone();
        inbox.respond(&token, LiveApprovalAction::Decline);
        assert!(!inbox.decline("request"));
        assert!(!inbox.decide("request", Decision::AllowOnce));
    }

    #[test]
    fn every_network_action_preserves_its_scope_and_dismiss_is_not_a_verdict() {
        let mut prompt = connector_prompt();
        prompt.offer = None;
        for (action, decision) in [
            (LiveApprovalAction::AllowOnce, Decision::AllowOnce),
            (LiveApprovalAction::AllowAlways, Decision::AllowAlways),
            (LiveApprovalAction::DenyOnce, Decision::DenyOnce),
            (LiveApprovalAction::DenyAlways, Decision::DenyAlways),
        ] {
            assert_eq!(
                requested_action(&prompt, action).unwrap(),
                RequestAction::Decide(decision)
            );
        }
        assert_eq!(
            requested_action(&prompt, LiveApprovalAction::Dismiss).unwrap(),
            RequestAction::Dismiss
        );
        assert!(
            requested_action(
                &prompt,
                LiveApprovalAction::Grant {
                    method: "token".into(),
                    connection: ApprovalConnection::None
                }
            )
            .is_err()
        );
    }

    #[test]
    fn redundant_updates_do_not_wake_idle_clients_and_notices_do() {
        let inbox = ApprovalInbox::new();
        let mut watching = inbox.watch();
        inbox.remove_pending("absent");
        assert!(!watching.has_changed().unwrap());
        inbox.push_inform("could not save".into());
        assert!(watching.has_changed().unwrap());
        assert_eq!(watching.borrow_and_update().notices, ["could not save"]);
        inbox.clear_informs();
        assert!(watching.borrow().notices.is_empty());
    }
}
