use std::collections::HashMap;
use std::time::Duration;

use eframe::egui;
use lns_policy::connectors::TokenFallback;
use lns_service::approval_flow::session::PendingPrompt;
use lns_service::approval_flow::window::{
    CredentialCardPrompt, SignInCard, Snapshot, StackItem, install_icon_font, install_system_fonts,
    lds_visuals, quiet_debug_overlays,
};
use lns_service::credential_flow::session::DenyScope;
use lns_service::tray::{
    CardAction, MIN_WINDOW_HEIGHT, TokenDraft, ViewportPlacement, WINDOW_WIDTH, content_cap,
    install_activation_policy, refresh_window_shadows, render_stack,
};

const SEED_HEIGHT: f32 = 300.0;

/// Once every card is dismissed the window hides (exactly like production); reseed the stack after this many seconds so the close-then-hide path can be reproduced repeatedly.
const RESHOW_DELAY: f64 = 1.5;

struct Preview {
    snapshot: Snapshot,
    credential_inputs: HashMap<String, String>,
    token_drafts: HashMap<String, TokenDraft>,
    remember: HashMap<String, bool>,
    placement: ViewportPlacement,
    reshow_at: Option<f64>,
}

impl Preview {
    fn maybe_reseed(&mut self, ctx: &egui::Context) {
        if !self.snapshot.order.is_empty() {
            self.reshow_at = None;
            return;
        }
        let now = ctx.input(|i| i.time);
        match self.reshow_at {
            None => {
                eprintln!("[approval-preview] stack empty → hiding window");
                self.reshow_at = Some(now + RESHOW_DELAY);
                ctx.request_repaint_after(Duration::from_secs_f64(RESHOW_DELAY));
            }
            Some(at) if now >= at => {
                eprintln!("[approval-preview] reseeding → re-showing window");
                self.snapshot = seed();
                self.reshow_at = None;
                ctx.request_repaint();
            }
            Some(_) => {}
        }
    }
}

impl eframe::App for Preview {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.maybe_reseed(ctx);
        let revealed = self
            .token_drafts
            .values()
            .filter(|d| d.is_revealed())
            .count();
        self.placement
            .sync_visibility(ctx, &self.snapshot.order, revealed);
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
        self.placement.fit_height(ui.ctx(), target);
        if let Some(action) = action {
            eprintln!("[approval-preview] {action:?}");
            dismiss_card(&mut self.snapshot, &action);
            ui.ctx().request_repaint();
        }
        refresh_window_shadows();
    }
}

fn dismiss_card(snapshot: &mut Snapshot, action: &CardAction) {
    if let CardAction::CloseAll = action {
        snapshot.order.clear();
        return;
    }
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
        CardAction::DismissNetwork { id } => snapshot
            .pending
            .iter()
            .position(|p| p.id == *id)
            .map(StackItem::Network),
        CardAction::DismissCredential { id } => snapshot
            .pending_credentials
            .iter()
            .position(|p| p.id == *id)
            .map(StackItem::Credential),
        CardAction::OpenBrowser { .. } | CardAction::CloseAll => None,
    };
    if let Some(item) = item {
        snapshot.order.retain(|entry| *entry != item);
    }
}

fn seed() -> Snapshot {
    if std::env::var_os("APPROVAL_PREVIEW_ONE").is_some() {
        seed_one()
    } else {
        seed_all()
    }
}

fn seed_one() -> Snapshot {
    Snapshot {
        pending: vec![PendingPrompt {
            id: "net-allow".into(),
            host: "pypi.org".into(),
            action: "CONNECT pypi.org:443".into(),
            offer: None,
            token_fallback: None,
        }],
        pending_credentials: vec![],
        sign_ins: vec![],
        informs: vec![],
        connecting: vec![],
        order: vec![StackItem::Network(0)],
    }
}

fn seed_all() -> Snapshot {
    Snapshot {
        pending: vec![
            PendingPrompt {
                id: "net-allow".into(),
                host: "pypi.org".into(),
                action: "CONNECT pypi.org:443".into(),
                offer: None,
                token_fallback: None,
            },
            PendingPrompt {
                id: "net-offer".into(),
                host: "openrouter.ai".into(),
                action: "CONNECT openrouter.ai:443".into(),
                offer: Some("OpenRouter".into()),
                token_fallback: None,
            },
        ],
        pending_credentials: vec![
            CredentialCardPrompt {
                id: "cred-value".into(),
                credential_id: "openai".into(),
                action: "use of openai placeholder".into(),
                host_value_available: true,
                bound_value_available: false,
                oauth_display_name: None,
                token_fallback: None,
                env_var: Some("OPENAI_API_KEY".into()),
                injection_domains: vec!["api.openai.com".into()],
                is_project_defined: false,
                deny_scope: DenyScope::Workload,
                origin: None,
            },
            CredentialCardPrompt {
                id: "cred-novalue".into(),
                credential_id: "some-provider".into(),
                action: "GET api.some-provider.example/settings".into(),
                host_value_available: false,
                bound_value_available: false,
                oauth_display_name: None,
                token_fallback: Some(TokenFallback {
                    help: Some("https://api.some-provider.example/tokens".into()),
                    command: Some("some-tool setup-token".into()),
                }),
                env_var: Some("SOME_TOKEN".into()),
                injection_domains: vec!["api.some-provider.example".into()],
                is_project_defined: false,
                deny_scope: DenyScope::Workload,
                origin: None,
            },
            CredentialCardPrompt {
                id: "cred-oauth".into(),
                credential_id: "github".into(),
                action: "use of github placeholder".into(),
                host_value_available: false,
                bound_value_available: true,
                oauth_display_name: Some("GitHub".into()),
                token_fallback: Some(TokenFallback {
                    help: Some("https://github.com/settings/personal-access-tokens/new".into()),
                    command: None,
                }),
                env_var: None,
                injection_domains: vec!["api.github.com".into(), "github.com".into()],
                is_project_defined: false,
                deny_scope: DenyScope::Workload,
                origin: None,
            },
        ],
        sign_ins: vec![SignInCard {
            credential_id: "github".into(),
            display_name: "GitHub".into(),
            user_code: Some("ABCD-1234".into()),
            verification_uri: "https://github.com/login/device".into(),
            token_fallback: Some(TokenFallback {
                help: Some("https://github.com/settings/personal-access-tokens/new".into()),
                command: None,
            }),
            env_var: None,
            injection_domains: vec!["api.github.com".into(), "github.com".into()],
            is_project_defined: false,
            origin: None,
        }],
        informs: vec!["sign-in to GitHub failed: device code expired".into()],
        connecting: vec!["OpenRouter".into()],
        order: vec![
            StackItem::Network(0),
            StackItem::Network(1),
            StackItem::Credential(0),
            StackItem::Credential(1),
            StackItem::Credential(2),
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
        .with_resizable(true)
        .with_transparent(true)
        .with_always_on_top()
        .with_mouse_passthrough(true)
        .with_visible(false)
        .with_inner_size([WINDOW_WIDTH, SEED_HEIGHT]);
    let mut options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    install_activation_policy(&mut options);
    eframe::run_native(
        "approval preview",
        options,
        Box::new(|cc| {
            cc.egui_ctx.set_visuals(lds_visuals());
            quiet_debug_overlays(&cc.egui_ctx);
            lns_service::ui::texture_delta_guard::install(&cc.egui_ctx);
            install_system_fonts(&cc.egui_ctx);
            install_icon_font(&cc.egui_ctx);
            Ok(Box::new(Preview {
                snapshot: seed(),
                credential_inputs: HashMap::new(),
                token_drafts: HashMap::new(),
                remember: HashMap::new(),
                placement: ViewportPlacement::new(),
                reshow_at: None,
            }))
        }),
    )
}
