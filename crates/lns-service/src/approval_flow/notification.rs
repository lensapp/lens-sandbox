use std::sync::Arc;

use eframe::egui;
use tokio::sync::mpsc;

use crate::approval_flow::session::{Notifier, PendingPrompt};
use crate::approval_flow::window::{DecisionDelivery, WindowState};

pub struct NoopNotifier;

impl Notifier for NoopNotifier {
    fn present(&self, _: &PendingPrompt) {}
    fn dismiss(&self, _: &str) {}
    fn inform(&self, _: &str) {}
    fn clear_informs(&self) {}
}

pub struct WindowNotifier {
    state: Arc<WindowState>,
    decision_tx: mpsc::UnboundedSender<DecisionDelivery>,
    ctx: Option<egui::Context>,
}

impl WindowNotifier {
    pub fn new(
        state: Arc<WindowState>,
        decision_tx: mpsc::UnboundedSender<DecisionDelivery>,
        ctx: Option<egui::Context>,
    ) -> Self {
        Self {
            state,
            decision_tx,
            ctx,
        }
    }

    fn wake(&self) {
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint();
        }
    }
}

impl Notifier for WindowNotifier {
    fn present(&self, prompt: &PendingPrompt) {
        self.state
            .insert_pending(prompt.clone(), self.decision_tx.clone());
        self.wake();
    }

    fn dismiss(&self, id: &str) {
        self.state.remove_pending(id);
        self.wake();
    }

    fn inform(&self, message: &str) {
        self.state.push_inform(message.to_string());
        self.wake();
    }

    fn clear_informs(&self) {
        self.state.clear_informs();
        self.wake();
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn prompt(id: &str, host: &str) -> PendingPrompt {
        PendingPrompt {
            id: id.into(),
            host: host.into(),
            action: format!("CONNECT {host}:443"),
            offer: None,
        }
    }

    fn fixture(
        with_ctx: bool,
    ) -> (
        WindowNotifier,
        Arc<WindowState>,
        mpsc::UnboundedReceiver<DecisionDelivery>,
    ) {
        let state = WindowState::new();
        let (tx, rx) = mpsc::unbounded_channel();
        let ctx = with_ctx.then(egui::Context::default);
        let n = WindowNotifier::new(state.clone(), tx, ctx);
        (n, state, rx)
    }

    #[test]
    fn present_inserts_into_state() {
        let (n, state, _rx) = fixture(false);
        n.present(&prompt("r1", "api.linear.app"));
        assert_eq!(state.pending_count(), 1);
    }

    #[test]
    fn present_with_duplicate_id_does_not_grow_state() {
        let (n, state, _rx) = fixture(false);
        n.present(&prompt("r1", "a.test"));
        n.present(&prompt("r1", "a.test"));
        assert_eq!(state.pending_count(), 1);
    }

    #[test]
    fn dismiss_removes_from_state() {
        let (n, state, _rx) = fixture(false);
        n.present(&prompt("r1", "a.test"));
        n.dismiss("r1");
        assert_eq!(state.pending_count(), 0);
    }

    #[test]
    fn dismiss_unknown_id_is_a_noop() {
        let (n, state, _rx) = fixture(false);
        n.present(&prompt("r1", "a.test"));
        n.dismiss("never-was");
        assert_eq!(state.pending_count(), 1);
    }

    #[test]
    fn inform_appends_to_state() {
        let (n, state, _rx) = fixture(false);
        n.inform("rule could not be persisted: disk full");
        let snap = state.snapshot();
        assert_eq!(
            snap.informs,
            vec!["rule could not be persisted: disk full".to_string()]
        );
    }

    #[test]
    fn clear_informs_empties_state_informs() {
        let (n, state, _rx) = fixture(false);
        n.inform("first");
        n.inform("second");
        n.clear_informs();
        assert!(state.snapshot().informs.is_empty());
    }

    #[test]
    fn present_with_ctx_does_not_panic_and_state_updates() {
        let (n, state, _rx) = fixture(true);
        n.present(&prompt("r1", "a.test"));
        n.dismiss("r1");
        n.inform("hello");
        assert_eq!(state.pending_count(), 0);
        assert_eq!(state.snapshot().informs, vec!["hello".to_string()]);
    }

    #[test]
    fn decision_flows_back_on_the_supplied_channel() {
        let (n, state, mut rx) = fixture(false);
        n.present(&prompt("r1", "a.test"));
        assert!(state.decide("r1", crate::approval_flow::protocol::Decision::AllowOnce));
        let got = rx.try_recv().expect("delivery");
        assert_eq!(got.id, "r1");
        assert_eq!(
            got.action,
            crate::approval_flow::window::RequestAction::Decide(
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
