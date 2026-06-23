use std::collections::HashMap;

use eframe::egui;
use lns_policy::integrations::TokenFallback;
use lns_service::approval_flow::session::PendingPrompt;
use lns_service::approval_flow::window::{
    CredentialCardPrompt, SignInCard, Snapshot, StackItem, install_system_fonts, lds_visuals,
};
use lns_service::tray::{
    CardAction, MIN_WINDOW_HEIGHT, TokenDraft, WINDOW_WIDTH, content_cap, position_top_right,
    refresh_window_shadows, render_stack,
};

const SEED_HEIGHT: f32 = 300.0;

struct Preview {
    snapshot: Snapshot,
    credential_inputs: HashMap<String, String>,
    token_drafts: HashMap<String, TokenDraft>,
    remember: HashMap<String, bool>,
    current_height: f32,
    placed: bool,
}

impl eframe::App for Preview {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.placed {
            return;
        }
        let Some(monitor) = ctx.input(|i| i.viewport().monitor_size) else {
            ctx.request_repaint();
            return;
        };
        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(position_top_right(
            monitor,
        )));
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.request_repaint();
        self.placed = true;
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let monitor_height = ui.ctx().input(|i| i.viewport().monitor_size).map(|m| m.y);
        let cap = content_cap(monitor_height);
        let (action, content_height) = render_stack(
            ui,
            &self.snapshot,
            &mut self.credential_inputs,
            &mut self.token_drafts,
            &mut self.remember,
            cap,
        );
        let target = content_height.clamp(MIN_WINDOW_HEIGHT, cap);
        if (self.current_height - target).abs() > 0.5 {
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                    WINDOW_WIDTH,
                    target,
                )));
            self.current_height = target;
        }
        if let Some(action) = action {
            eprintln!("[approval-preview] {action:?}");
            dismiss_card(&mut self.snapshot, &action);
            ui.ctx().request_repaint();
        }
        refresh_window_shadows();
    }
}

fn dismiss_card(snapshot: &mut Snapshot, action: &CardAction) {
    let item = match action {
        CardAction::Decide { id, .. }
        | CardAction::ConnectOffer { id }
        | CardAction::DeclineOffer { id }
        | CardAction::UseOfferToken { id, .. } => snapshot
            .pending
            .iter()
            .position(|p| p.id == *id)
            .map(StackItem::Network),
        CardAction::DecideCredential { id, .. } => snapshot
            .pending_credentials
            .iter()
            .position(|p| p.id == *id)
            .map(StackItem::Credential),
        CardAction::CancelSignIn { credential_id }
        | CardAction::UseTokenSignIn { credential_id, .. } => snapshot
            .sign_ins
            .iter()
            .position(|c| c.credential_id == *credential_id)
            .map(StackItem::SignIn),
        CardAction::DismissInform { index } => Some(StackItem::Inform(*index)),
        CardAction::OpenBrowser { .. } => None,
    };
    if let Some(item) = item {
        snapshot.order.retain(|entry| *entry != item);
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
        .with_decorations(false)
        .with_transparent(true)
        .with_always_on_top()
        .with_visible(false)
        .with_inner_size([WINDOW_WIDTH, SEED_HEIGHT]);
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "approval preview",
        options,
        Box::new(|cc| {
            cc.egui_ctx.set_visuals(lds_visuals());
            install_system_fonts(&cc.egui_ctx);
            Ok(Box::new(Preview {
                snapshot: seed(),
                credential_inputs: HashMap::new(),
                token_drafts: HashMap::new(),
                remember: HashMap::new(),
                current_height: SEED_HEIGHT,
                placed: false,
            }))
        }),
    )
}
