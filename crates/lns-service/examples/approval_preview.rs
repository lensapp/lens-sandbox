use std::collections::HashMap;

use eframe::egui;
use lns_policy::integrations::TokenFallback;
use lns_service::approval_flow::session::PendingPrompt;
use lns_service::approval_flow::window::{
    CredentialCardPrompt, SignInCard, Snapshot, StackItem, lds_visuals,
};
use lns_service::tray::{TokenDraft, WINDOW_WIDTH, render_stack};

struct Preview {
    snapshot: Snapshot,
    credential_inputs: HashMap<String, String>,
    token_drafts: HashMap<String, TokenDraft>,
}

impl eframe::App for Preview {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.04, 0.04, 0.05, 1.0]
    }

    fn logic(&mut self, _ctx: &egui::Context, _frame: &mut eframe::Frame) {}

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let scroll_max = ui.available_height();
        let (action, _content_height) = render_stack(
            ui,
            &self.snapshot,
            &mut self.credential_inputs,
            &mut self.token_drafts,
            scroll_max,
        );
        if let Some(action) = action {
            eprintln!("[approval-preview] {action:?}");
        }
    }
}

fn seed() -> Snapshot {
    Snapshot {
        pending: vec![
            PendingPrompt {
                id: "net-allow".into(),
                host: "api.example.test".into(),
                action: "connect".into(),
                offer: None,
                token_fallback: None,
            },
            PendingPrompt {
                id: "net-offer".into(),
                host: "api.some-provider.example".into(),
                action: "connect".into(),
                offer: Some("Some Provider".into()),
                token_fallback: Some(TokenFallback {
                    help: Some("https://help.example/token".into()),
                }),
            },
        ],
        pending_credentials: vec![
            CredentialCardPrompt {
                id: "cred-value".into(),
                credential_id: "some-provider".into(),
                action: "read".into(),
                host_value_available: true,
                oauth_display_name: None,
                token_fallback: None,
            },
            CredentialCardPrompt {
                id: "cred-oauth".into(),
                credential_id: "some-oauth".into(),
                action: "connect".into(),
                host_value_available: false,
                oauth_display_name: Some("Some OAuth".into()),
                token_fallback: Some(TokenFallback {
                    help: Some("https://help.example/oauth".into()),
                }),
            },
        ],
        sign_ins: vec![SignInCard {
            credential_id: "some-oauth".into(),
            display_name: "Some OAuth".into(),
            user_code: Some("ABCD-1234".into()),
            verification_uri: "https://example.test/device".into(),
            token_fallback: None,
        }],
        informs: vec!["Workload finished — the sandbox is idle.".into()],
        connecting: vec!["Some Provider".into()],
        order: vec![
            StackItem::Network(0),
            StackItem::Network(1),
            StackItem::Credential(0),
            StackItem::Credential(1),
            StackItem::SignIn(0),
            StackItem::Connecting(0),
            StackItem::Inform(0),
        ],
    }
}

fn main() -> eframe::Result {
    let viewport = egui::ViewportBuilder::default()
        .with_title("approval preview")
        .with_inner_size([WINDOW_WIDTH, 820.0]);
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "approval preview",
        options,
        Box::new(|cc| {
            cc.egui_ctx.set_visuals(lds_visuals());
            Ok(Box::new(Preview {
                snapshot: seed(),
                credential_inputs: HashMap::new(),
                token_drafts: HashMap::new(),
            }))
        }),
    )
}
