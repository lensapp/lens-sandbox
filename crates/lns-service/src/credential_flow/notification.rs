use std::sync::Arc;

use eframe::egui;
use tokio::sync::mpsc;

use crate::approval_flow::window::{CredentialDecisionDelivery, SignInCard, WindowState};
use crate::credential_flow::providers::DefProvider;
use crate::credential_flow::registry;
use crate::credential_flow::session::{CredentialNotifier, CredentialPendingPrompt, SignInPrompt};

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

    fn present_sign_in(
        &self,
        prompt: &SignInPrompt,
        cancel: tokio::sync::oneshot::Sender<crate::oauth::SignInPivot>,
    ) {
        self.state.insert_sign_in(
            SignInCard {
                credential_id: prompt.credential_id.clone(),
                display_name: prompt.display_name.clone(),
                user_code: prompt.user_code.clone(),
                verification_uri: prompt.verification_uri.clone(),
                token_fallback: prompt.token_fallback.clone(),
                env_var: prompt.env_var.clone(),
                injection_domains: prompt.injection_domains.clone(),
                is_project_defined: prompt.is_project_defined,
                run: prompt.run.clone(),
            },
            cancel,
        );
        self.wake();
    }

    fn dismiss_sign_in(&self, credential_id: &str) {
        self.state.remove_sign_in(credential_id);
        self.wake();
    }

    fn connect_finished(&self, display_name: &str) {
        self.state.clear_connecting(display_name);
        self.wake();
    }

    fn clear_all_connecting(&self) {
        self.state.clear_all_connecting();
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
            bound_value_available: false,
            oauth_display_name: None,
            token_fallback: None,
            env_var: None,
            injection_domains: vec![],
            is_project_defined: false,
            deny_scope: crate::credential_flow::session::DenyScope::Workload,
            run: Some("some-run".into()),
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
        n.present(&prompt("c1", "some-provider"));
        let snap = state.snapshot();
        assert_eq!(snap.pending_credentials.len(), 1);
        assert!(snap.pending_credentials[0].host_value_available);
    }

    #[test]
    fn present_inserts_into_state_with_host_value_flag_cleared() {
        let (n, state, _rx) = fixture(false, false);
        n.present(&prompt("c1", "some-provider"));
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
        n.present(&prompt("c1", "some-provider"));
        n.present(&prompt("c1", "some-provider"));
        assert_eq!(state.snapshot().pending_credentials.len(), 1);
    }

    #[test]
    fn dismiss_removes_from_state() {
        let (n, state, _rx) = fixture(true, false);
        n.present(&prompt("c1", "some-provider"));
        n.dismiss("c1");
        assert_eq!(state.snapshot().pending_credentials.len(), 0);
    }

    #[test]
    fn dismiss_unknown_id_is_a_noop() {
        let (n, state, _rx) = fixture(true, false);
        n.present(&prompt("c1", "some-provider"));
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
        n.present(&prompt("c1", "some-provider"));
        n.dismiss("c1");
        n.inform("hello");
        n.clear_informs();
        assert_eq!(state.snapshot().pending_credentials.len(), 0);
    }

    fn sign_in_prompt() -> SignInPrompt {
        SignInPrompt {
            credential_id: "some-oauth".into(),
            display_name: "GitHub".into(),
            user_code: Some("WXYZ-1234".into()),
            verification_uri: "https://some-oauth.example/login/device".into(),
            token_fallback: Some(lns_policy::connectors::TokenFallback {
                help: Some("https://example.com/pat".into()),
                command: None,
            }),
            env_var: None,
            injection_domains: vec![],
            is_project_defined: false,
            run: Some("some-run".into()),
        }
    }

    #[test]
    fn present_sign_in_inserts_a_sign_in_card() {
        let (n, state, _rx) = fixture(false, false);
        let (cancel_tx, _cancel_rx) = tokio::sync::oneshot::channel();
        n.present_sign_in(&sign_in_prompt(), cancel_tx);
        let snap = state.snapshot();
        assert_eq!(snap.sign_ins.len(), 1);
        assert_eq!(snap.sign_ins[0].display_name, "GitHub");
        assert_eq!(snap.sign_ins[0].user_code.as_deref(), Some("WXYZ-1234"));
        assert_eq!(
            snap.sign_ins[0].token_fallback,
            Some(lns_policy::connectors::TokenFallback {
                help: Some("https://example.com/pat".into()),
                command: None,
            }),
            "the card carries the prompt's token fallback so the UI can offer the pivot"
        );
    }

    #[test]
    fn a_sign_in_card_carries_the_run_the_prompt_names() {
        let (n, state, _rx) = fixture(false, false);
        let (cancel_tx, _cancel_rx) = tokio::sync::oneshot::channel();
        n.present_sign_in(&sign_in_prompt(), cancel_tx);
        assert_eq!(
            state.snapshot().sign_ins[0].run.as_deref(),
            Some("some-run"),
            "the run the service attributed to the prompt must reach the card the developer reads"
        );
    }

    #[test]
    fn a_credential_card_carries_the_run_the_prompt_names() {
        let (n, state, _rx) = fixture(false, false);
        n.present(&prompt("c1", "some-provider"));
        assert_eq!(
            state.snapshot().pending_credentials[0].run.as_deref(),
            Some("some-run"),
            "the run the service attributed to the prompt must reach the card the developer reads"
        );
    }

    #[test]
    fn dismiss_sign_in_removes_the_card() {
        let (n, state, _rx) = fixture(false, false);
        let (cancel_tx, _cancel_rx) = tokio::sync::oneshot::channel();
        n.present_sign_in(&sign_in_prompt(), cancel_tx);
        n.dismiss_sign_in("some-oauth");
        assert!(state.snapshot().sign_ins.is_empty());
    }

    #[test]
    fn connect_finished_clears_the_connecting_placeholder() {
        let (n, state, _rx) = fixture(false, false);
        let mut consent = prompt("c1", "some-oauth");
        consent.oauth_display_name = Some("GitHub".into());
        n.present(&consent);
        state.decide_credential(
            "c1",
            crate::credential_flow::session::CredentialDecisionRequest::Allow(
                crate::credential_flow::store::CredentialEntry::HostDetect,
            ),
        );
        assert_eq!(state.snapshot().connecting, vec!["GitHub".to_string()]);
        n.connect_finished("GitHub");
        assert!(
            state.snapshot().connecting.is_empty(),
            "the resolved connect takes the placeholder down"
        );
    }

    #[test]
    fn clear_all_connecting_drops_every_placeholder() {
        let (n, state, _rx) = fixture(false, false);
        let mut consent = prompt("c1", "some-oauth");
        consent.oauth_display_name = Some("GitHub".into());
        n.present(&consent);
        state.decide_credential(
            "c1",
            crate::credential_flow::session::CredentialDecisionRequest::Allow(
                crate::credential_flow::store::CredentialEntry::HostDetect,
            ),
        );
        assert_eq!(state.snapshot().connecting, vec!["GitHub".to_string()]);
        n.clear_all_connecting();
        assert!(state.snapshot().connecting.is_empty());
    }

    #[test]
    fn noop_notifier_methods_are_safe_to_call() {
        let n = NoopCredentialNotifier;
        n.present(&prompt("c1", "some-provider"));
        n.dismiss("c1");
        n.inform("anything");
        n.clear_informs();
        n.connect_finished("GitHub");
        n.clear_all_connecting();
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
