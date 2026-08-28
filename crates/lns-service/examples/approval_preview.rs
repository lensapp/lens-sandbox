use std::time::Duration;

use eframe::egui;
use lns_service::approval_flow::protocol::Treatment;
use lns_service::approval_flow::session::PendingPrompt;
use lns_service::approval_flow::window::{
    Snapshot, StackItem, install_icon_font, install_system_fonts, lds_visuals, quiet_debug_overlays,
};
use lns_service::tray::{
    CardAction, MIN_WINDOW_HEIGHT, ViewportPlacement, WINDOW_WIDTH, content_cap,
    install_activation_policy, refresh_window_shadows, render_stack,
};

const SEED_HEIGHT: f32 = 300.0;

/// Once every card is dismissed the window hides (exactly like production); reseed the stack after this many seconds so the close-then-hide path can be reproduced repeatedly.
const RESHOW_DELAY: f64 = 1.5;

struct Preview {
    snapshot: Snapshot,
    cards: lns_service::tray::CardState,
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
        self.placement.sync_visibility(ctx, &self.snapshot.order);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let monitor_height = ui.ctx().input(|i| i.viewport().monitor_size).map(|m| m.y);
        let cap = content_cap(monitor_height);
        let (action, content_height) = render_stack(ui, &self.snapshot, &mut self.cards, cap);
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
        | CardAction::DismissNetwork { id }
        | CardAction::Grant { id, .. } => snapshot
            .pending
            .iter()
            .position(|p| p.id == *id)
            .map(StackItem::Network),
        CardAction::DismissInform { index } => Some(StackItem::Inform(*index)),
        // A decline keeps the card: the ordinary question the hold stood in for is still unanswered.
        CardAction::Decline { .. } | CardAction::OpenBrowser { .. } | CardAction::CloseAll => None,
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
            treatment: Treatment::Inspected,
            run: Some("brave-otter".into()),
            offer: None,
        }],
        informs: vec![],
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
                treatment: Treatment::Inspected,
                run: Some("brave-otter".into()),
                offer: None,
            },
            PendingPrompt {
                id: "net-raw".into(),
                host: "db.internal".into(),
                action: "CONNECT db.internal:5432".into(),
                treatment: Treatment::Raw,
                run: Some("brave-otter".into()),
                offer: None,
            },
        ],
        informs: vec!["decision applied to this request only".into()],
        order: vec![
            StackItem::Network(0),
            StackItem::Network(1),
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
                cards: Default::default(),
                placement: ViewportPlacement::new(),
                reshow_at: None,
            }))
        }),
    )
}
