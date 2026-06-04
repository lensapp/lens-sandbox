use std::sync::Arc;

use eframe::egui;
use tokio::sync::mpsc;

use crate::approval_flow::window::{CredentialDecisionDelivery, WindowState};
use crate::credential_flow::providers::DefProvider;
use crate::credential_flow::registry;
use crate::credential_flow::session::{CredentialNotifier, CredentialPendingPrompt};

pub struct NoopCredentialNotifier;

impl CredentialNotifier for NoopCredentialNotifier {
    fn present(&self, _: &CredentialPendingPrompt) {}
    fn dismiss(&self, _: &str) {}
    fn inform(&self, _: &str) {}
    fn clear_informs(&self) {}
}

pub struct WindowCredentialNotifier {
    state: Arc<WindowState>,
    decision_tx: mpsc::UnboundedSender<CredentialDecisionDelivery>,
    detect_host: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    ctx: Option<egui::Context>,
}

impl WindowCredentialNotifier {
    pub fn new(
        state: Arc<WindowState>,
        decision_tx: mpsc::UnboundedSender<CredentialDecisionDelivery>,
        ctx: Option<egui::Context>,
        detect_host: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    ) -> Self {
        Self {
            state,
            decision_tx,
            detect_host,
            ctx,
        }
    }

    /// Resolves host-value availability against built-ins ∪ the run's custom providers, so a custom provider's card can offer "use from host" too.
    pub fn with_registry_detection(
        state: Arc<WindowState>,
        decision_tx: mpsc::UnboundedSender<CredentialDecisionDelivery>,
        ctx: Option<egui::Context>,
        custom_providers: Arc<Vec<DefProvider>>,
    ) -> Self {
        Self::new(
            state,
            decision_tx,
            ctx,
            Arc::new(move |id: &str| registry::detect_for_with(id, &custom_providers).is_some()),
        )
    }

    fn wake(&self) {
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint();
        }
    }
}

impl CredentialNotifier for WindowCredentialNotifier {
    fn present(&self, prompt: &CredentialPendingPrompt) {
        let host_value_available = (self.detect_host)(&prompt.credential_id);
        self.state.insert_credential_pending(
            prompt.clone(),
            host_value_available,
            self.decision_tx.clone(),
        );
        self.wake();
    }

    fn dismiss(&self, id: &str) {
        self.state.remove_credential_pending(id);
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
mod tests {
    use super::*;

    fn prompt(id: &str, credential_id: &str) -> CredentialPendingPrompt {
        CredentialPendingPrompt {
            id: id.into(),
            credential_id: credential_id.into(),
            action: format!("use of {credential_id} placeholder"),
        }
    }

    fn fixture(
        host_has: bool,
        with_ctx: bool,
    ) -> (
        WindowCredentialNotifier,
        Arc<WindowState>,
        mpsc::UnboundedReceiver<CredentialDecisionDelivery>,
    ) {
        let state = WindowState::new();
        let (tx, rx) = mpsc::unbounded_channel();
        let ctx = with_ctx.then(egui::Context::default);
        let n = WindowCredentialNotifier::new(state.clone(), tx, ctx, Arc::new(move |_| host_has));
        (n, state, rx)
    }

    #[test]
    fn present_inserts_into_state_with_host_value_flag_set() {
        let (n, state, _rx) = fixture(true, false);
        n.present(&prompt("c1", "github"));
        let snap = state.snapshot();
        assert_eq!(snap.pending_credentials.len(), 1);
        assert!(snap.pending_credentials[0].host_value_available);
    }

    #[test]
    fn present_inserts_into_state_with_host_value_flag_cleared() {
        let (n, state, _rx) = fixture(false, false);
        n.present(&prompt("c1", "github"));
        let snap = state.snapshot();
        assert_eq!(snap.pending_credentials.len(), 1);
        assert!(!snap.pending_credentials[0].host_value_available);
    }

    #[test]
    fn present_passes_credential_id_to_detector_closure() {
        let state = WindowState::new();
        let (tx, _rx) = mpsc::unbounded_channel();
        let observed = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let recorder = {
            let observed = observed.clone();
            Arc::new(move |id: &str| {
                observed.lock().unwrap().push(id.to_string());
                false
            })
        };
        let n = WindowCredentialNotifier::new(state, tx, None, recorder);
        n.present(&prompt("c1", "openai"));
        n.present(&prompt("c2", "linear"));
        assert_eq!(*observed.lock().unwrap(), vec!["openai", "linear"]);
    }

    #[test]
    fn present_with_duplicate_id_does_not_grow_state() {
        let (n, state, _rx) = fixture(true, false);
        n.present(&prompt("c1", "github"));
        n.present(&prompt("c1", "github"));
        assert_eq!(state.snapshot().pending_credentials.len(), 1);
    }

    #[test]
    fn dismiss_removes_from_state() {
        let (n, state, _rx) = fixture(true, false);
        n.present(&prompt("c1", "github"));
        n.dismiss("c1");
        assert_eq!(state.snapshot().pending_credentials.len(), 0);
    }

    #[test]
    fn dismiss_unknown_id_is_a_noop() {
        let (n, state, _rx) = fixture(true, false);
        n.present(&prompt("c1", "github"));
        n.dismiss("never-was");
        assert_eq!(state.snapshot().pending_credentials.len(), 1);
    }

    #[test]
    fn inform_appends_to_state() {
        let (n, state, _rx) = fixture(true, false);
        n.inform("credential rule could not be persisted: disk full");
        let snap = state.snapshot();
        assert_eq!(
            snap.informs,
            vec!["credential rule could not be persisted: disk full".to_string()]
        );
    }

    #[test]
    fn clear_informs_empties_state_informs() {
        let (n, state, _rx) = fixture(true, false);
        n.inform("first");
        n.inform("second");
        n.clear_informs();
        assert!(state.snapshot().informs.is_empty());
    }

    #[test]
    fn present_with_ctx_does_not_panic_and_state_updates() {
        let (n, state, _rx) = fixture(true, true);
        n.present(&prompt("c1", "github"));
        n.dismiss("c1");
        n.inform("hello");
        n.clear_informs();
        assert_eq!(state.snapshot().pending_credentials.len(), 0);
    }

    #[test]
    fn noop_notifier_methods_are_safe_to_call() {
        let n = NoopCredentialNotifier;
        n.present(&prompt("c1", "github"));
        n.dismiss("c1");
        n.inform("anything");
        n.clear_informs();
    }

    #[test]
    #[serial_test::serial(env)]
    fn with_registry_detection_routes_to_registry() {
        use crate::test_env::EnvVarGuard;
        let state = WindowState::new();
        let (tx, _rx) = mpsc::unbounded_channel();
        let n = WindowCredentialNotifier::with_registry_detection(
            state.clone(),
            tx,
            None,
            Arc::new(Vec::new()),
        );

        let _g = EnvVarGuard::set("GITHUB_TOKEN", "ghp_real");
        n.present(&prompt("c1", "github"));
        let snap = state.snapshot();
        assert!(snap.pending_credentials[0].host_value_available);
    }

    #[test]
    #[serial_test::serial(env)]
    fn with_registry_detection_offers_host_value_for_a_custom_provider() {
        use crate::test_env::EnvVarGuard;
        use lns_policy::providers::{InjectionDef, InjectionKind, ProviderDef};
        let state = WindowState::new();
        let (tx, _rx) = mpsc::unbounded_channel();
        let custom = Arc::new(vec![DefProvider::new(ProviderDef {
            id: "acme".into(),
            env_var: "ACME_API_KEY".into(),
            placeholder: "acme_LNSPLACEHOLDER".into(),
            injections: vec![InjectionDef {
                kind: InjectionKind::BearerHeader,
                domain: "api.acme.corp".into(),
                header: None,
            }],
        })]);
        let n = WindowCredentialNotifier::with_registry_detection(state.clone(), tx, None, custom);

        let _g = EnvVarGuard::set("ACME_API_KEY", "acme_real");
        n.present(&prompt("c1", "acme"));
        let snap = state.snapshot();
        assert!(
            snap.pending_credentials[0].host_value_available,
            "a custom provider with its env var set must offer use-from-host"
        );
    }
}
