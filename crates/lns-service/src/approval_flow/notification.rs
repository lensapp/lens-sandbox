use std::sync::Arc;

use tokio::sync::mpsc;

use crate::approval_flow::inbox::{ApprovalInbox, DecisionDelivery};
use crate::approval_flow::session::{Notifier, PendingPrompt};

pub struct NoopNotifier;

impl Notifier for NoopNotifier {
    fn present(&self, _: &PendingPrompt) {}
    fn dismiss(&self, _: &str) {}
    fn expire(&self, _: &str) {}
    fn inform(&self, _: &str) {}
    fn clear_informs(&self) {}
}

pub struct InboxNotifier {
    state: Arc<ApprovalInbox>,
    decision_tx: mpsc::UnboundedSender<DecisionDelivery>,
}

impl InboxNotifier {
    pub fn new(
        state: Arc<ApprovalInbox>,
        decision_tx: mpsc::UnboundedSender<DecisionDelivery>,
    ) -> Self {
        Self { state, decision_tx }
    }
}

impl Notifier for InboxNotifier {
    fn present(&self, prompt: &PendingPrompt) {
        self.state
            .insert_pending(prompt.clone(), self.decision_tx.clone());
    }

    fn dismiss(&self, id: &str) {
        self.state.remove_pending(id);
    }

    /// The card stays and its buttons still mean what they meant: a grant applies to whatever runs next (§3.2.4).
    fn expire(&self, id: &str) {
        self.state.expire(id);
    }

    fn inform(&self, message: &str) {
        self.state.push_inform(message.to_string());
    }

    fn clear_informs(&self) {
        self.state.clear_informs();
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::approval_flow::protocol::Treatment;

    fn prompt(id: &str, host: &str) -> PendingPrompt {
        PendingPrompt {
            id: id.into(),
            host: host.into(),
            action: format!("CONNECT {host}:443"),
            treatment: Treatment::Inspected,
            run: None,
            offer: None,
        }
    }

    fn fixture() -> (
        InboxNotifier,
        Arc<ApprovalInbox>,
        mpsc::UnboundedReceiver<DecisionDelivery>,
    ) {
        let state = ApprovalInbox::new();
        let (tx, rx) = mpsc::unbounded_channel();
        let n = InboxNotifier::new(state.clone(), tx);
        (n, state, rx)
    }

    #[test]
    fn present_inserts_into_state() {
        let (n, state, _rx) = fixture();
        n.present(&prompt("r1", "api.linear.app"));
        assert_eq!(state.pending_count(), 1);
    }

    #[test]
    fn present_with_duplicate_id_does_not_grow_state() {
        let (n, state, _rx) = fixture();
        n.present(&prompt("r1", "a.test"));
        n.present(&prompt("r1", "a.test"));
        assert_eq!(state.pending_count(), 1);
    }

    #[test]
    fn an_expired_hold_leaves_its_card_where_it_is() {
        // §3.2.4: the workload gave up waiting, but the connect the user is in the middle of still applies to what runs next.
        let (n, state, _rx) = fixture();
        n.present(&prompt("r1", "api.some-provider.example"));
        n.expire("r1");
        assert_eq!(state.pending_count(), 1);
    }

    #[test]
    fn the_silent_notifier_answers_every_call_without_a_window() {
        // lns-service runs headless in tests and on a host with no tray; every notifier method must be safe to call there.
        let n = NoopNotifier;
        n.present(&prompt("r1", "a.test"));
        n.dismiss("r1");
        n.expire("r1");
        n.inform("something happened");
        n.clear_informs();
    }

    #[test]
    fn dismiss_removes_from_state() {
        let (n, state, _rx) = fixture();
        n.present(&prompt("r1", "a.test"));
        n.dismiss("r1");
        assert_eq!(state.pending_count(), 0);
    }

    #[test]
    fn dismiss_unknown_id_is_a_noop() {
        let (n, state, _rx) = fixture();
        n.present(&prompt("r1", "a.test"));
        n.dismiss("never-was");
        assert_eq!(state.pending_count(), 1);
    }

    #[test]
    fn inform_appends_to_state() {
        let (n, state, _rx) = fixture();
        n.inform("rule could not be persisted: disk full");
        let snap = state.snapshot();
        assert_eq!(
            snap.informs,
            vec!["rule could not be persisted: disk full".to_string()]
        );
    }

    #[test]
    fn clear_informs_empties_state_informs() {
        let (n, state, _rx) = fixture();
        n.inform("first");
        n.inform("second");
        n.clear_informs();
        assert!(state.snapshot().informs.is_empty());
    }

    #[test]
    fn present_dismiss_and_inform_state_updates() {
        let (n, state, _rx) = fixture();
        n.present(&prompt("r1", "a.test"));
        n.dismiss("r1");
        n.inform("hello");
        assert_eq!(state.pending_count(), 0);
        assert_eq!(state.snapshot().informs, vec!["hello".to_string()]);
    }

    #[test]
    fn decision_flows_back_on_the_supplied_channel() {
        let (n, state, mut rx) = fixture();
        n.present(&prompt("r1", "a.test"));
        assert!(state.decide("r1", crate::approval_flow::protocol::Decision::AllowOnce));
        let got = rx.try_recv().expect("delivery");
        assert_eq!(got.id, "r1");
        assert_eq!(
            got.action,
            crate::approval_flow::inbox::RequestAction::Decide(
                crate::approval_flow::protocol::Decision::AllowOnce
            )
        );
    }

    #[test]
    fn noop_notifier_methods_are_safe_to_call() {
        let n = NoopNotifier;
        n.present(&prompt("r1", "a.test"));
        n.dismiss("r1");
        n.inform("anything");
        n.clear_informs();
    }
}
