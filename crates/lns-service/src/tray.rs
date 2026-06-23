use std::collections::HashMap;
use std::sync::Arc;
use std::thread::JoinHandle;

use anyhow::Context;
use eframe::egui;
use tray_icon::menu::accelerator::{Accelerator, Code, Modifiers};
use tray_icon::menu::{Menu, MenuEvent, MenuItem};
use tray_icon::{Icon, TrayIconBuilder};

use crate::approval_flow::protocol::Decision;
use crate::approval_flow::session::PendingPrompt;
use crate::approval_flow::window::{
    self, CredentialCardPrompt, SignInCard, Snapshot, StackItem, WindowState,
};
use crate::credential_flow::session::CredentialDecisionRequest;
use crate::credential_flow::store::CredentialEntry;
use crate::shutdown::Shutdown;
use crate::ui::{Button, ButtonKind, theme};
use lns_policy::integrations::TokenFallback;

pub const WINDOW_WIDTH: f32 = 460.0;
const WINDOW_HEIGHT: f32 = 300.0;
const SCREEN_EDGE_MARGIN: f32 = 20.0;

pub const MIN_WINDOW_HEIGHT: f32 = 140.0;
const FALLBACK_MAX_HEIGHT: f32 = 720.0;
const INFORM_ITEM_HEIGHT: f32 = 88.0;
const CONNECTING_ITEM_HEIGHT: f32 = 112.0;
const CARD_ITEM_HEIGHT: f32 = 282.0;
const TOKEN_REVEAL_EXTRA: f32 = 150.0;

/// Builds the tray icon + Quit menu and installs the global menu-event handler (Quit signals shutdown); `on_event` lets the caller repaint after any menu event.
fn build_tray_icon(
    shutdown: Arc<Shutdown>,
    on_event: impl Fn() + Send + Sync + 'static,
) -> anyhow::Result<tray_icon::TrayIcon> {
    let menu = Menu::new();
    let quit_item = MenuItem::new(
        "Quit Lens Sandbox",
        true,
        Some(Accelerator::new(Some(Modifiers::META), Code::KeyQ)),
    );
    menu.append(&quit_item)
        .context("failed to append quit menu item")?;
    let quit_id = quit_item.id().clone();

    let icon = load_icon().context("load embedded tray icon")?;
    let builder = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Lens Sandbox")
        .with_icon(icon);
    // Template rendering (monochrome mask adapting to the menu bar) is a macOS concept; on Linux the recolored icon is shown as-is.
    #[cfg(target_os = "macos")]
    let builder = builder.with_icon_as_template(true);
    let tray = builder.build().context("build tray icon")?;

    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        if event.id == quit_id {
            shutdown.signal();
        }
        on_event();
    }));

    Ok(tray)
}

/// Runs the tray on its own GTK main loop (tray-icon needs gtk on Linux), the only place gtk lives; the winit/eframe window owns the main thread. Degrades to no-tray if gtk can't init.
#[cfg(target_os = "linux")]
fn spawn_gtk_tray(shutdown: Arc<Shutdown>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        if let Err(e) = gtk::init() {
            crate::log::warn!(
                "tray unavailable: gtk init failed ({e}); running without a tray icon"
            );
            return;
        }
        let _tray = match build_tray_icon(shutdown.clone(), || {}) {
            Ok(tray) => tray,
            Err(e) => {
                crate::log::warn!("tray unavailable: {e:#}; running without a tray icon");
                return;
            }
        };
        let quit_shutdown = shutdown.clone();
        gtk::glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
            if quit_shutdown.is_set() {
                gtk::main_quit();
                gtk::glib::ControlFlow::Break
            } else {
                gtk::glib::ControlFlow::Continue
            }
        });
        gtk::main();
    })
}

pub fn run_tray(
    shutdown: Arc<Shutdown>,
    ipc_handle: JoinHandle<anyhow::Result<()>>,
    window_state: Arc<WindowState>,
) -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    let gtk_tray = spawn_gtk_tray(shutdown.clone());

    let viewport = egui::ViewportBuilder::default()
        .with_title("Lens Sandbox")
        .with_position([0.0, 0.0])
        .with_visible(false)
        .with_decorations(false)
        // Resizable so the card can grow programmatically when a token field is revealed; with no decorations there are no user-facing resize handles.
        .with_resizable(true)
        .with_always_on_top()
        .with_transparent(true)
        .with_mouse_passthrough(true)
        .with_inner_size([WINDOW_WIDTH, WINDOW_HEIGHT]);

    let mut native_options = eframe::NativeOptions {
        viewport,
        run_and_return: true,
        ..Default::default()
    };
    install_activation_policy(&mut native_options);

    let app_shutdown = shutdown.clone();
    let result = eframe::run_native(
        "lns-service",
        native_options,
        Box::new(move |cc| {
            cc.egui_ctx.set_visuals(window::lds_visuals());
            window::install_system_fonts(&cc.egui_ctx);
            window::install_ctx(cc.egui_ctx.clone());
            let app = TrayApp::new(cc.egui_ctx.clone(), app_shutdown, window_state)
                .map_err(|e| format!("{e:#}"))?;
            Ok(Box::new(app))
        }),
    );

    shutdown.signal();
    let _ = ipc_handle.join();
    #[cfg(target_os = "linux")]
    let _ = gtk_tray.join();

    result.map_err(|e| anyhow::anyhow!("eframe run_native failed: {e}"))
}

/// Whether a desktop session the tray can attach to is present: macOS always has a window server, while Linux needs Wayland or X11 — whose absence (headless servers, CI, plain SSH) means winit cannot create an event loop.
pub fn display_present() -> bool {
    if cfg!(target_os = "macos") {
        return true;
    }
    has_linux_display(|key| std::env::var_os(key).is_some())
}

fn has_linux_display(present: impl Fn(&str) -> bool) -> bool {
    present("WAYLAND_DISPLAY") || present("WAYLAND_SOCKET") || present("DISPLAY")
}

/// Run the service without the tray UI — wait for shutdown, then join the IPC thread and surface its result — so a headless host keeps the sandbox running instead of aborting at startup.
pub fn run_headless(
    shutdown: Arc<Shutdown>,
    ipc_handle: JoinHandle<anyhow::Result<()>>,
) -> anyhow::Result<()> {
    crate::log::warn!(
        "no display detected — running headless without the tray; interactive approval prompts can't be shown, so pre-authorize access in lns-policy.yaml"
    );
    shutdown.wait_sync();
    match ipc_handle.join() {
        Ok(result) => result,
        Err(_) => anyhow::bail!("the ipc thread panicked"),
    }
}

struct TrayApp {
    shutdown: Arc<Shutdown>,
    window_state: Arc<WindowState>,
    #[cfg(target_os = "macos")]
    _tray: tray_icon::TrayIcon,
    last_visible: bool,
    /// Kept on `TrayApp` (not [`WindowState`]) so the snapshot passed to [`render_stack`] stays immutable.
    credential_inputs: HashMap<String, String>,
    /// Per-card progressive-disclosure state for the "use a token instead" fallback, keyed by the shown card's id.
    token_drafts: HashMap<String, TokenDraft>,
    /// Per-network-card "remember this decision" toggle, keyed by request id; true persists the choice as an always-rule.
    remember: HashMap<String, bool>,
    /// The viewport inner height last applied; grows to fit a revealed token field and shrinks back when none is open.
    current_height: f32,
}

/// The transient UI state of one card's token fallback: whether the field is revealed and what's been typed.
#[derive(Default)]
pub struct TokenDraft {
    revealed: bool,
    value: String,
}

impl TrayApp {
    fn new(
        ctx: egui::Context,
        shutdown: Arc<Shutdown>,
        window_state: Arc<WindowState>,
    ) -> anyhow::Result<Self> {
        // Linux owns the tray on a dedicated gtk-main thread (spawn_gtk_tray); only macOS builds it in-app.
        #[cfg(target_os = "macos")]
        let _tray = {
            let menu_ctx = ctx.clone();
            build_tray_icon(shutdown.clone(), move || menu_ctx.request_repaint())?
        };

        let watch_shutdown = shutdown.clone();
        let watch_ctx = ctx;
        std::thread::spawn(move || {
            watch_shutdown.wait_sync();
            watch_ctx.request_repaint();
        });

        Ok(Self {
            shutdown,
            window_state,
            #[cfg(target_os = "macos")]
            _tray,
            last_visible: false,
            credential_inputs: HashMap::new(),
            token_drafts: HashMap::new(),
            remember: HashMap::new(),
            current_height: WINDOW_HEIGHT,
        })
    }

    fn sync_viewport_visibility(&mut self, ctx: &egui::Context) {
        let order = self.window_state.snapshot().order;
        let should_show = !order.is_empty();

        match visibility_transition(should_show, self.last_visible) {
            VisibilityTransition::Show => {
                // Place and size the window before revealing it, or macOS flashes it un-positioned mid-screen for a frame.
                let Some(pos) = top_right_position(ctx) else {
                    ctx.request_repaint();
                    return;
                };
                // A seed height keeps the reveal frame (which skips ui()) close to size; ui() then snaps the window to its measured content so no estimate slop shows as bottom padding.
                let revealed = self.token_drafts.values().filter(|d| d.revealed).count();
                let monitor_height = ctx.input(|i| i.viewport().monitor_size).map(|m| m.y);
                let seed = target_height(&order, revealed, monitor_height);
                join_all_spaces();
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos));
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                    WINDOW_WIDTH,
                    seed,
                )));
                ctx.send_viewport_cmd(egui::ViewportCommand::MousePassthrough(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                // The frame that reveals the window saw is_visible=false, so eframe skipped ui(); kick a repaint so the now-visible window paints its cards and fits its height.
                ctx.request_repaint();
                self.current_height = seed;
                self.last_visible = true;
            }
            VisibilityTransition::Hide => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::MousePassthrough(true));
                self.last_visible = false;
            }
            // Resizing while visible is driven by ui()'s content measurement, not an estimate.
            VisibilityTransition::Unchanged => {}
        }
    }

    /// Snaps the viewport to `target` using measured content size, not the window-bounded `min_rect`, so a card taller than the current window grows it instead of being clipped.
    fn fit_height_to_content(&mut self, ctx: &egui::Context, target: f32) {
        if (self.current_height - target).abs() > 0.5 {
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                WINDOW_WIDTH,
                target,
            )));
            self.current_height = target;
        }
    }
}

impl eframe::App for TrayApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.shutdown.is_set() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        self.sync_viewport_visibility(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let snapshot = self.window_state.snapshot();
        prune_credential_inputs(&mut self.credential_inputs, &snapshot);
        prune_token_drafts(&mut self.token_drafts, &snapshot);
        prune_remember(&mut self.remember, &snapshot);

        let monitor_height = ui.ctx().input(|i| i.viewport().monitor_size).map(|m| m.y);
        let cap = content_cap(monitor_height);
        let (action, content_height) = render_stack(
            ui,
            &snapshot,
            &mut self.credential_inputs,
            &mut self.token_drafts,
            &mut self.remember,
            cap,
        );
        let target = content_height.clamp(MIN_WINDOW_HEIGHT, cap);
        self.fit_height_to_content(ui.ctx(), target);

        match action {
            Some(CardAction::Decide { id, decision }) => {
                self.window_state.decide(&id, decision);
                ui.ctx().request_repaint();
            }
            Some(CardAction::DecideCredential { id, request }) => {
                self.credential_inputs.remove(&id);
                self.window_state.decide_credential(&id, request);
                ui.ctx().request_repaint();
            }
            Some(CardAction::DismissInform { index }) => {
                self.window_state.dismiss_inform(index);
                ui.ctx().request_repaint();
            }
            Some(CardAction::OpenBrowser { url }) => {
                crate::browser::open(&url);
                ui.ctx().request_repaint();
            }
            Some(CardAction::CancelSignIn { credential_id }) => {
                self.window_state.cancel_sign_in(&credential_id);
                ui.ctx().request_repaint();
            }
            Some(CardAction::ConnectOffer { id }) => {
                self.window_state.connect_offer(&id);
                ui.ctx().request_repaint();
            }
            Some(CardAction::DeclineOffer { id }) => {
                self.window_state.decline_offer(&id);
                ui.ctx().request_repaint();
            }
            Some(CardAction::UseOfferToken { id, value }) => {
                self.window_state.use_offer_token(&id, value);
                ui.ctx().request_repaint();
            }
            Some(CardAction::UseTokenSignIn {
                credential_id,
                value,
            }) => {
                self.window_state.pivot_sign_in(&credential_id, value);
                ui.ctx().request_repaint();
            }
            None => {}
        }

        refresh_window_shadows();
    }
}

const BTN_WIDTH: f32 = 188.0;
const BTN_GAP: f32 = 12.0;

#[derive(Debug, PartialEq, Eq)]
pub enum CardAction {
    Decide {
        id: String,
        decision: Decision,
    },
    DecideCredential {
        id: String,
        request: CredentialDecisionRequest,
    },
    DismissInform {
        index: usize,
    },
    OpenBrowser {
        url: String,
    },
    CancelSignIn {
        credential_id: String,
    },
    ConnectOffer {
        id: String,
    },
    DeclineOffer {
        id: String,
    },
    UseOfferToken {
        id: String,
        value: String,
    },
    UseTokenSignIn {
        credential_id: String,
        value: String,
    },
}

/// What the user did in the token-fallback affordance shared by every connect card.
enum TokenFallbackEvent {
    Save(String),
    OpenHelp(String),
}

fn item_height(item: &StackItem) -> f32 {
    match item {
        StackItem::Inform(_) => INFORM_ITEM_HEIGHT,
        StackItem::Connecting(_) => CONNECTING_ITEM_HEIGHT,
        _ => CARD_ITEM_HEIGHT,
    }
}

/// The tallest the window may grow before the stack scrolls rather than running off-screen — the usable monitor height, or a fixed fallback when the monitor size isn't known yet.
pub fn content_cap(monitor_height: Option<f32>) -> f32 {
    monitor_height
        .map(|h| (h - 2.0 * SCREEN_EDGE_MARGIN).max(MIN_WINDOW_HEIGHT))
        .unwrap_or(FALLBACK_MAX_HEIGHT)
}

/// A pre-measurement height seed for the reveal frame alone (which skips ui()), after which ui() snaps the window to its measured content.
fn target_height(items: &[StackItem], revealed: usize, monitor_height: Option<f32>) -> f32 {
    if items.is_empty() {
        return MIN_WINDOW_HEIGHT;
    }
    let content = items.iter().map(item_height).sum::<f32>()
        + theme::CARD_GAP * (items.len() - 1) as f32
        + revealed as f32 * TOKEN_REVEAL_EXTRA
        + 2.0 * theme::STACK_MARGIN as f32;
    content.clamp(MIN_WINDOW_HEIGHT, content_cap(monitor_height))
}

/// Renders the stack and returns the fired action plus the content's natural height (the scroll area's `content_size`, not the window-bounded laid-out size), which the caller sizes the window from so a card taller than the window grows it instead of being clipped.
pub fn render_stack(
    ui: &mut egui::Ui,
    snapshot: &Snapshot,
    credential_inputs: &mut HashMap<String, String>,
    token_drafts: &mut HashMap<String, TokenDraft>,
    remember: &mut HashMap<String, bool>,
    scroll_max: f32,
) -> (Option<CardAction>, f32) {
    use egui::{Frame, Margin, Sense};

    if snapshot.order.is_empty() {
        return (None, 0.0);
    }

    ui.style_mut().spacing.scroll.floating = true;
    ui.style_mut().spacing.scroll.fade.strength = 0.0;
    let out = egui::ScrollArea::vertical()
        .max_height(scroll_max)
        .auto_shrink([false, true])
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .show(ui, |ui| {
            Frame::new()
                .inner_margin(Margin::same(theme::STACK_MARGIN))
                .show(ui, |ui| {
                    let mut action = None;
                    for (idx, item) in snapshot.order.iter().enumerate() {
                        if idx > 0 {
                            ui.add_space(theme::CARD_GAP);
                        }
                        let wrapped = crate::ui::card(ui, |ui| {
                            render_item(
                                ui,
                                item,
                                snapshot,
                                credential_inputs,
                                token_drafts,
                                remember,
                            )
                        });
                        let mut fired = match *item {
                            StackItem::Inform(i)
                                if wrapped.response.interact(Sense::click()).clicked() =>
                            {
                                Some(CardAction::DismissInform { index: i })
                            }
                            _ => wrapped.inner,
                        };
                        let close_rect = egui::Rect::from_center_size(
                            wrapped.response.rect.left_top() + egui::vec2(3.0, 3.0),
                            egui::Vec2::splat(22.0),
                        );
                        if fired.is_none()
                            && ui.rect_contains_pointer(wrapped.response.rect.union(close_rect))
                            && let Some(close) = close_action(item, snapshot)
                            && close_button(ui, egui::Id::new(("card-close", idx)), close_rect)
                                .clicked()
                        {
                            fired = Some(close);
                        }
                        action = action.or(fired);
                    }
                    action
                })
                .inner
        });
    (out.inner, out.content_size.y)
}

fn render_item(
    ui: &mut egui::Ui,
    item: &StackItem,
    snapshot: &Snapshot,
    credential_inputs: &mut HashMap<String, String>,
    token_drafts: &mut HashMap<String, TokenDraft>,
    remember: &mut HashMap<String, bool>,
) -> Option<CardAction> {
    match *item {
        StackItem::Inform(i) => {
            render_inform_content(ui, &snapshot.informs[i]);
            None
        }
        StackItem::Network(i) => {
            let prompt = &snapshot.pending[i];
            if let Some(display_name) = &prompt.offer {
                let draft = token_drafts.entry(prompt.id.clone()).or_default();
                render_offer_card(ui, prompt, display_name, draft)
            } else {
                let flag = remember.entry(prompt.id.clone()).or_default();
                render_network_card(ui, prompt, flag)
            }
        }
        StackItem::SignIn(i) => {
            let card = &snapshot.sign_ins[i];
            let draft = token_drafts.entry(card.credential_id.clone()).or_default();
            render_sign_in_card(ui, card, draft)
        }
        StackItem::Credential(i) => {
            let prompt = &snapshot.pending_credentials[i];
            let input = credential_inputs.entry(prompt.id.clone()).or_default();
            let draft = token_drafts.entry(prompt.id.clone()).or_default();
            render_credential_card(ui, prompt, input, draft)
        }
        StackItem::Connecting(i) => {
            render_connecting_card(ui, &snapshot.connecting[i]);
            None
        }
    }
}

fn render_connecting_card(ui: &mut egui::Ui, display_name: &str) {
    use egui::RichText;

    render_card_header(ui, "CONNECT");

    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add(egui::Spinner::new().size(18.0).color(window::ACCENT_GREEN));
        ui.label(
            RichText::new(format!("Connecting to {display_name}…"))
                .size(16.0)
                .strong()
                .color(window::TEXT_ACCENT),
        );
    });
}

fn render_inform_content(ui: &mut egui::Ui, msg: &str) {
    use egui::{Align, Layout, RichText};

    ui.horizontal(|ui| {
        ui.colored_label(window::STATUS_WARNING, "⚠");
        ui.colored_label(window::TEXT_PRIMARY, RichText::new(msg).size(12.0));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.colored_label(window::TEXT_MUTED, RichText::new("✕  dismiss").size(10.5));
        });
    });
}

/// Closing must resolve a pending request rather than just hide it, or the workload stays blocked — so the corner ✕ maps to each card's safe negative.
fn close_action(item: &StackItem, snapshot: &Snapshot) -> Option<CardAction> {
    match *item {
        StackItem::Inform(i) => Some(CardAction::DismissInform { index: i }),
        StackItem::Network(i) => Some(CardAction::Decide {
            id: snapshot.pending[i].id.clone(),
            decision: Decision::DenyOnce,
        }),
        StackItem::SignIn(i) => Some(CardAction::CancelSignIn {
            credential_id: snapshot.sign_ins[i].credential_id.clone(),
        }),
        StackItem::Credential(i) => Some(CardAction::DecideCredential {
            id: snapshot.pending_credentials[i].id.clone(),
            request: CredentialDecisionRequest::Deny,
        }),
        StackItem::Connecting(_) => None,
    }
}

fn close_button(ui: &mut egui::Ui, id: egui::Id, rect: egui::Rect) -> egui::Response {
    use egui::{Color32, Sense, Stroke, vec2};

    let resp = ui.interact(rect, id, Sense::click());
    let center = rect.center();
    let radius = rect.width() * 0.5;
    let painter = ui.painter();

    painter.circle_filled(
        center + vec2(0.0, 1.5),
        radius + 1.5,
        Color32::from_black_alpha(45),
    );
    painter.circle_filled(
        center + vec2(0.0, 0.8),
        radius + 0.5,
        Color32::from_black_alpha(75),
    );
    let fill = if resp.hovered() {
        Color32::from_gray(96)
    } else {
        Color32::from_gray(72)
    };
    painter.circle_filled(center, radius, fill);

    let arm = radius * 0.34;
    let x = Stroke::new(1.6, Color32::from_gray(235));
    painter.line_segment([center - vec2(arm, arm), center + vec2(arm, arm)], x);
    painter.line_segment([center + vec2(arm, -arm), center - vec2(arm, -arm)], x);
    resp
}

fn render_network_card(
    ui: &mut egui::Ui,
    prompt: &PendingPrompt,
    remember: &mut bool,
) -> Option<CardAction> {
    use egui::RichText;

    let id = prompt.id.clone();

    render_card_header(ui, "APPROVAL NEEDED");

    ui.add_space(6.0);
    ui.label(
        RichText::new(&prompt.host)
            .size(22.0)
            .strong()
            .color(window::TEXT_ACCENT),
    );
    ui.add_space(2.0);
    ui.label(
        RichText::new(&prompt.action)
            .size(12.0)
            .monospace()
            .color(window::TEXT_MUTED),
    );

    ui.add_space(16.0);
    remember_toggle(ui, remember);
    ui.add_space(BTN_GAP);

    let mut chosen: Option<Decision> = None;
    ui.horizontal(|ui| {
        if primary_button(ui, "Allow").clicked() {
            chosen = Some(allow_decision(*remember));
        }
        ui.add_space(BTN_GAP);
        if deny_button(ui, "Deny").clicked() {
            chosen = Some(deny_decision(*remember));
        }
    });

    chosen.map(|decision| CardAction::Decide { id, decision })
}

fn allow_decision(remember: bool) -> Decision {
    if remember {
        Decision::AllowAlways
    } else {
        Decision::AllowOnce
    }
}

fn deny_decision(remember: bool) -> Decision {
    if remember {
        Decision::DenyAlways
    } else {
        Decision::DenyOnce
    }
}

fn remember_toggle(ui: &mut egui::Ui, remember: &mut bool) {
    use egui::{RichText, Sense};

    let (glyph, color) = if *remember {
        ("◉", window::ACCENT_GREEN)
    } else {
        ("○", window::TEXT_MUTED)
    };
    let resp = ui.add(
        egui::Label::new(
            RichText::new(format!("{glyph}  Remember this decision"))
                .size(12.0)
                .color(color),
        )
        .sense(Sense::click()),
    );
    if resp.clicked() {
        *remember = !*remember;
    }
}

fn render_offer_card(
    ui: &mut egui::Ui,
    prompt: &PendingPrompt,
    display_name: &str,
    draft: &mut TokenDraft,
) -> Option<CardAction> {
    use egui::RichText;

    let id = prompt.id.clone();

    render_card_header(ui, "CONNECT");

    ui.add_space(6.0);
    ui.label(
        RichText::new(format!("Connect to {display_name}?"))
            .size(22.0)
            .strong()
            .color(window::TEXT_ACCENT),
    );
    ui.add_space(2.0);
    ui.label(
        RichText::new(format!("A workload wants to reach {}.", prompt.host))
            .size(12.0)
            .color(window::TEXT_MUTED),
    );

    ui.add_space(18.0);
    ui.add(egui::Separator::default().spacing(0.0));
    ui.add_space(16.0);

    let mut action: Option<CardAction> = None;
    ui.horizontal(|ui| {
        if primary_button(ui, &format!("Connect {display_name}")).clicked() {
            action = Some(CardAction::ConnectOffer { id: id.clone() });
        }
        ui.add_space(BTN_GAP);
        if secondary_button(ui, "Not now").clicked() {
            action = Some(CardAction::DeclineOffer { id });
        }
    });

    if action.is_none()
        && let Some(fallback) = &prompt.token_fallback
    {
        action = match render_token_fallback(ui, fallback, draft) {
            Some(TokenFallbackEvent::Save(value)) => Some(CardAction::UseOfferToken {
                id: prompt.id.clone(),
                value,
            }),
            Some(TokenFallbackEvent::OpenHelp(url)) => Some(CardAction::OpenBrowser { url }),
            None => None,
        };
    }

    action
}

fn render_credential_card(
    ui: &mut egui::Ui,
    prompt: &CredentialCardPrompt,
    input: &mut String,
    draft: &mut TokenDraft,
) -> Option<CardAction> {
    use egui::RichText;

    let id = prompt.id.clone();

    if let Some(display_name) = &prompt.oauth_display_name {
        return render_oauth_consent_card(ui, prompt, display_name, draft);
    }

    render_card_header(ui, "CREDENTIAL NEEDED");

    ui.add_space(6.0);
    ui.label(
        RichText::new(&prompt.credential_id)
            .size(22.0)
            .strong()
            .color(window::TEXT_ACCENT),
    );
    ui.add_space(2.0);
    ui.label(
        RichText::new(&prompt.action)
            .size(12.0)
            .monospace()
            .color(window::TEXT_MUTED),
    );

    if !prompt.host_value_available {
        ui.add_space(8.0);
        ui.colored_label(
            window::TEXT_MUTED,
            RichText::new("No credential detected on host.")
                .size(11.5)
                .italics(),
        );
    }

    ui.add_space(18.0);
    ui.add(egui::Separator::default().spacing(0.0));
    ui.add_space(16.0);

    let mut chosen: Option<CredentialDecisionRequest> = None;

    if prompt.host_value_available {
        ui.horizontal(|ui| {
            if primary_button(ui, "Use from host").clicked() {
                chosen = Some(CredentialDecisionRequest::Allow(
                    CredentialEntry::HostDetect,
                ));
            }
        });
        ui.add_space(BTN_GAP);
    }

    secret_input(ui, input, "Enter a value");
    ui.add_space(BTN_GAP);
    ui.horizontal(|ui| {
        let submit_enabled = !input.trim().is_empty();
        let submit_resp = Button::new("Submit value", ButtonKind::Primary)
            .enabled(submit_enabled)
            .min_size(egui::vec2(BTN_WIDTH, 0.0))
            .show(ui);
        if submit_resp.clicked() && submit_enabled {
            chosen = Some(CredentialDecisionRequest::Allow(CredentialEntry::Stored {
                value: input.trim().to_string(),
            }));
        }
        ui.add_space(BTN_GAP);
        if deny_button(ui, "Deny").clicked() {
            chosen = Some(CredentialDecisionRequest::Deny);
        }
    });

    chosen.map(|request| CardAction::DecideCredential { id, request })
}

fn render_oauth_consent_card(
    ui: &mut egui::Ui,
    prompt: &CredentialCardPrompt,
    display_name: &str,
    draft: &mut TokenDraft,
) -> Option<CardAction> {
    use egui::RichText;

    render_card_header(ui, "CONNECT");

    ui.add_space(6.0);
    ui.label(
        RichText::new(format!("Connect to {display_name}?"))
            .size(22.0)
            .strong()
            .color(window::TEXT_ACCENT),
    );
    ui.add_space(2.0);
    ui.label(
        RichText::new(format!(
            "A workload wants to use your {display_name} access."
        ))
        .size(12.0)
        .color(window::TEXT_MUTED),
    );

    ui.add_space(18.0);
    ui.add(egui::Separator::default().spacing(0.0));
    ui.add_space(16.0);

    let mut chosen: Option<CredentialDecisionRequest> = None;
    ui.horizontal(|ui| {
        if primary_button(ui, "Connect").clicked() {
            chosen = Some(CredentialDecisionRequest::Allow(
                CredentialEntry::HostDetect,
            ));
        }
        ui.add_space(BTN_GAP);
        if deny_button(ui, "Deny").clicked() {
            chosen = Some(CredentialDecisionRequest::Deny);
        }
    });

    if chosen.is_none()
        && let Some(fallback) = &prompt.token_fallback
    {
        match render_token_fallback(ui, fallback, draft) {
            Some(TokenFallbackEvent::Save(value)) => {
                chosen = Some(CredentialDecisionRequest::Allow(CredentialEntry::Stored {
                    value,
                }));
            }
            Some(TokenFallbackEvent::OpenHelp(url)) => {
                return Some(CardAction::OpenBrowser { url });
            }
            None => {}
        }
    }

    chosen.map(|request| CardAction::DecideCredential {
        id: prompt.id.clone(),
        request,
    })
}

fn render_sign_in_card(
    ui: &mut egui::Ui,
    card: &SignInCard,
    draft: &mut TokenDraft,
) -> Option<CardAction> {
    use egui::RichText;

    render_card_header(ui, "CONNECT");

    ui.add_space(6.0);
    ui.label(
        RichText::new(format!("Connect to {}", card.display_name))
            .size(22.0)
            .strong()
            .color(window::TEXT_ACCENT),
    );
    ui.add_space(8.0);
    match &card.user_code {
        Some(user_code) => {
            ui.label(
                RichText::new("Enter this code on the page that opens:")
                    .size(12.0)
                    .color(window::TEXT_MUTED),
            );
            ui.add_space(6.0);
            ui.label(
                RichText::new(user_code)
                    .size(28.0)
                    .strong()
                    .monospace()
                    .color(window::TEXT_ACCENT),
            );
        }
        None => {
            ui.label(
                RichText::new("Your browser is opening to finish signing in…")
                    .size(12.0)
                    .color(window::TEXT_MUTED),
            );
        }
    }

    ui.add_space(18.0);
    ui.add(egui::Separator::default().spacing(0.0));
    ui.add_space(16.0);

    let mut action: Option<CardAction> = None;
    ui.horizontal(|ui| {
        if primary_button(ui, &format!("Open {}", card.display_name)).clicked() {
            action = Some(CardAction::OpenBrowser {
                url: card.verification_uri.clone(),
            });
        }
        ui.add_space(BTN_GAP);
        if secondary_button(ui, "Cancel").clicked() {
            action = Some(CardAction::CancelSignIn {
                credential_id: card.credential_id.clone(),
            });
        }
    });

    if action.is_none()
        && let Some(fallback) = &card.token_fallback
    {
        action = match render_token_fallback(ui, fallback, draft) {
            Some(TokenFallbackEvent::Save(value)) => Some(CardAction::UseTokenSignIn {
                credential_id: card.credential_id.clone(),
                value,
            }),
            Some(TokenFallbackEvent::OpenHelp(url)) => Some(CardAction::OpenBrowser { url }),
            None => None,
        };
    }

    action
}

/// Progressive disclosure shared by every connect card: a muted "Use a token instead" that, once clicked, reveals a password field + "Save token" and (when declared) a help link. Returns the user's action without performing it.
fn render_token_fallback(
    ui: &mut egui::Ui,
    fallback: &TokenFallback,
    draft: &mut TokenDraft,
) -> Option<TokenFallbackEvent> {
    use egui::{RichText, Sense};

    ui.add_space(BTN_GAP);
    if !draft.revealed {
        let link = ui.add(
            egui::Label::new(
                RichText::new("Use a token instead")
                    .size(11.5)
                    .underline()
                    .color(window::TEXT_MUTED),
            )
            .sense(Sense::click()),
        );
        if link.clicked() {
            draft.revealed = true;
        }
        return None;
    }

    ui.add_space(6.0);
    secret_input(ui, &mut draft.value, "Paste a token");

    let mut event = None;
    if let Some(help) = &fallback.help {
        ui.add_space(6.0);
        let link = ui.add(
            egui::Label::new(
                RichText::new("How do I create a token?")
                    .size(10.5)
                    .underline()
                    .color(window::TEXT_MUTED),
            )
            .sense(Sense::click()),
        );
        if link.clicked() {
            event = Some(TokenFallbackEvent::OpenHelp(help.clone()));
        }
    }

    ui.add_space(BTN_GAP);
    let enabled = !draft.value.trim().is_empty();
    if wide_primary_button(ui, "Save token", enabled).clicked() && enabled {
        event = Some(TokenFallbackEvent::Save(draft.value.trim().to_string()));
    }
    event
}

fn render_card_header(ui: &mut egui::Ui, label: &str) {
    use egui::RichText;

    ui.label(
        RichText::new(label)
            .size(10.5)
            .strong()
            .color(window::ACCENT_GREEN),
    );
}

fn secret_input(ui: &mut egui::Ui, value: &mut String, hint: &str) -> egui::Response {
    ui.add(
        egui::TextEdit::singleline(value)
            .password(true)
            .hint_text(hint)
            .margin(egui::Margin::symmetric(10, 9))
            .desired_width(f32::INFINITY),
    )
}

fn primary_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    Button::new(label, ButtonKind::Primary)
        .min_size(egui::vec2(BTN_WIDTH, 0.0))
        .show(ui)
}

fn wide_primary_button(ui: &mut egui::Ui, label: &str, enabled: bool) -> egui::Response {
    Button::new(label, ButtonKind::Primary)
        .enabled(enabled)
        .min_size(egui::vec2(ui.available_width(), 0.0))
        .show(ui)
}

fn secondary_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    Button::new(label, ButtonKind::Secondary)
        .min_size(egui::vec2(BTN_WIDTH, 0.0))
        .show(ui)
}

fn deny_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    Button::new(label, ButtonKind::Danger)
        .min_size(egui::vec2(BTN_WIDTH, 0.0))
        .show(ui)
}

fn prune_credential_inputs(inputs: &mut HashMap<String, String>, snapshot: &Snapshot) {
    let still_pending: std::collections::HashSet<&str> = snapshot
        .pending_credentials
        .iter()
        .map(|p| p.id.as_str())
        .collect();
    inputs.retain(|id, _| still_pending.contains(id.as_str()));
}

fn prune_remember(remember: &mut HashMap<String, bool>, snapshot: &Snapshot) {
    let still_pending: std::collections::HashSet<&str> =
        snapshot.pending.iter().map(|p| p.id.as_str()).collect();
    remember.retain(|id, _| still_pending.contains(id.as_str()));
}

/// Token drafts are keyed by the shown card's id — a network request id, a credential prompt id, or a sign-in credential id — so a draft survives only while its card is still on screen.
fn prune_token_drafts(drafts: &mut HashMap<String, TokenDraft>, snapshot: &Snapshot) {
    let mut live: std::collections::HashSet<&str> = std::collections::HashSet::new();
    live.extend(snapshot.pending.iter().map(|p| p.id.as_str()));
    live.extend(snapshot.pending_credentials.iter().map(|p| p.id.as_str()));
    live.extend(snapshot.sign_ins.iter().map(|c| c.credential_id.as_str()));
    drafts.retain(|key, _| live.contains(key.as_str()));
}

pub fn position_top_right(monitor: egui::Vec2) -> egui::Pos2 {
    egui::Pos2::new(
        monitor.x - WINDOW_WIDTH - SCREEN_EDGE_MARGIN,
        SCREEN_EDGE_MARGIN,
    )
}

fn top_right_position(ctx: &egui::Context) -> Option<egui::Pos2> {
    let monitor = ctx.input(|i| i.viewport().monitor_size)?;
    Some(position_top_right(monitor))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VisibilityTransition {
    Show,
    Hide,
    Unchanged,
}

fn visibility_transition(should_show: bool, last_visible: bool) -> VisibilityTransition {
    match (should_show, last_visible) {
        (true, false) => VisibilityTransition::Show,
        (false, true) => VisibilityTransition::Hide,
        _ => VisibilityTransition::Unchanged,
    }
}

#[cfg(target_os = "macos")]
fn install_activation_policy(opts: &mut eframe::NativeOptions) {
    use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
    opts.event_loop_builder = Some(Box::new(|builder| {
        builder.with_activation_policy(ActivationPolicy::Accessory);
    }));
}

#[cfg(not(target_os = "macos"))]
fn install_activation_policy(_opts: &mut eframe::NativeOptions) {}

/// Lets the always-on-top approval window appear on whichever macOS Space is active — including a full-screen app's Space — instead of staying pinned to the desktop it was created on.
#[cfg(target_os = "macos")]
fn join_all_spaces() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSWindowCollectionBehavior};

    let Some(mtm) = MainThreadMarker::new() else {
        crate::log::warn!("skipped tray-window Space behavior: not on the main thread");
        return;
    };
    let extra = NSWindowCollectionBehavior::CanJoinAllSpaces
        | NSWindowCollectionBehavior::FullScreenAuxiliary;
    for window in NSApplication::sharedApplication(mtm).windows().iter() {
        window.setCollectionBehavior(window.collectionBehavior() | extra);
    }
}

#[cfg(not(target_os = "macos"))]
fn join_all_spaces() {}

/// macOS recomputes a transparent window's shadow only on resize, so without per-frame invalidation a scrolled card's shadow freezes at its old position.
#[cfg(target_os = "macos")]
pub fn refresh_window_shadows() {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;

    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    for window in NSApplication::sharedApplication(mtm).windows().iter() {
        window.invalidateShadow();
    }
}

#[cfg(not(target_os = "macos"))]
pub fn refresh_window_shadows() {}

fn load_icon() -> anyhow::Result<Icon> {
    const ICON_BYTES: &[u8] = include_bytes!("../assets/lensTemplate@2x.png");
    let decoder = png::Decoder::new(ICON_BYTES);
    let mut reader = decoder.read_info().context("read embedded icon PNG info")?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut buf)
        .context("decode embedded icon PNG frame")?;
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf[..info.buffer_size()].to_vec(),
        png::ColorType::Rgb => buf[..info.buffer_size()]
            .chunks_exact(3)
            .flat_map(|px| [px[0], px[1], px[2], 0xFF])
            .collect(),
        other => anyhow::bail!("unsupported PNG color type for tray icon: {other:?}"),
    };
    // The shipped icon is a macOS black template (shape carried by alpha); recolor it white on Linux so it shows on the dark panel — appindicator has no template concept.
    #[cfg(target_os = "linux")]
    let rgba = whiten_template(rgba);
    Icon::from_rgba(rgba, info.width, info.height).context("build Icon from RGBA")
}

#[cfg(target_os = "linux")]
fn whiten_template(mut rgba: Vec<u8>) -> Vec<u8> {
    for px in rgba.chunks_exact_mut(4) {
        px[0] = 0xFF;
        px[1] = 0xFF;
        px[2] = 0xFF;
    }
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_icon_decodes_successfully() {
        let icon = load_icon().expect("embedded template PNG should decode");
        drop(icon);
    }

    #[test]
    fn has_linux_display_true_when_any_display_var_is_present() {
        assert!(has_linux_display(|k| k == "DISPLAY"));
        assert!(has_linux_display(|k| k == "WAYLAND_DISPLAY"));
        assert!(has_linux_display(|k| k == "WAYLAND_SOCKET"));
    }

    #[test]
    fn has_linux_display_false_when_no_display_var_is_present() {
        assert!(!has_linux_display(|_| false));
    }

    #[test]
    fn run_headless_joins_the_ipc_thread_and_surfaces_success() {
        let shutdown = Arc::new(Shutdown::new());
        shutdown.signal();
        let ipc = std::thread::spawn(|| Ok(()));
        assert!(run_headless(shutdown, ipc).is_ok());
    }

    #[test]
    fn run_headless_surfaces_an_ipc_thread_error() {
        let shutdown = Arc::new(Shutdown::new());
        shutdown.signal();
        let ipc = std::thread::spawn(|| anyhow::bail!("ipc boom"));
        let err = run_headless(shutdown, ipc).expect_err("ipc error must surface");
        assert!(err.to_string().contains("ipc boom"), "got: {err}");
    }

    #[test]
    fn quit_accelerator_constructs() {
        let accel = Accelerator::new(Some(Modifiers::META), Code::KeyQ);
        let repr = format!("{accel:?}");
        assert!(repr.contains("KeyQ"), "expected KeyQ in {repr}");
    }

    #[test]
    fn remember_toggle_maps_the_allow_and_deny_decisions() {
        assert_eq!(allow_decision(false), Decision::AllowOnce);
        assert_eq!(allow_decision(true), Decision::AllowAlways);
        assert_eq!(deny_decision(false), Decision::DenyOnce);
        assert_eq!(deny_decision(true), Decision::DenyAlways);
    }

    fn credential_prompt(id: &str) -> CredentialCardPrompt {
        CredentialCardPrompt {
            id: id.to_string(),
            credential_id: "cred".to_string(),
            action: "read".to_string(),
            host_value_available: false,
            oauth_display_name: None,
            token_fallback: None,
        }
    }

    #[test]
    fn prune_credential_inputs_drops_buffers_for_no_longer_pending_prompts() {
        let snapshot = Snapshot {
            pending: Vec::new(),
            pending_credentials: vec![credential_prompt("keep")],
            sign_ins: Vec::new(),
            informs: Vec::new(),
            connecting: Vec::new(),
            order: vec![StackItem::Credential(0)],
        };
        let mut inputs = HashMap::new();
        inputs.insert("keep".to_string(), "typed so far".to_string());
        inputs.insert("stale".to_string(), "abandoned".to_string());

        prune_credential_inputs(&mut inputs, &snapshot);

        assert_eq!(inputs.get("keep").map(String::as_str), Some("typed so far"));
        assert!(
            !inputs.contains_key("stale"),
            "stale buffer must be dropped"
        );
    }

    #[test]
    fn target_height_is_the_floor_when_the_stack_is_empty() {
        assert_eq!(target_height(&[], 0, Some(1080.0)), MIN_WINDOW_HEIGHT);
    }

    #[test]
    fn content_cap_insets_a_known_monitor_and_falls_back_without_one() {
        assert_eq!(content_cap(Some(900.0)), 900.0 - 2.0 * SCREEN_EDGE_MARGIN);
        assert_eq!(content_cap(None), FALLBACK_MAX_HEIGHT);
        assert_eq!(
            content_cap(Some(10.0)),
            MIN_WINDOW_HEIGHT,
            "a tiny monitor still leaves at least the floor so the scroll area never goes negative"
        );
    }

    #[test]
    fn target_height_grows_with_each_additional_card() {
        let one = target_height(&[StackItem::Network(0)], 0, Some(2000.0));
        let two = target_height(
            &[StackItem::Network(0), StackItem::Credential(0)],
            0,
            Some(2000.0),
        );
        assert!(
            two > one,
            "a second concurrent card makes the window taller rather than hiding the first"
        );
    }

    #[test]
    fn a_connecting_placeholder_takes_less_room_than_a_full_card() {
        assert!(item_height(&StackItem::Connecting(0)) < item_height(&StackItem::Network(0)));
        assert_eq!(
            item_height(&StackItem::Connecting(0)),
            CONNECTING_ITEM_HEIGHT
        );
    }

    #[test]
    fn target_height_adds_room_for_a_revealed_token_field() {
        let collapsed = target_height(&[StackItem::Network(0)], 0, Some(2000.0));
        let revealed = target_height(&[StackItem::Network(0)], 1, Some(2000.0));
        assert!(revealed > collapsed);
    }

    #[test]
    fn target_height_caps_a_long_stack_at_the_usable_monitor_height() {
        let many: Vec<StackItem> = (0..40).map(StackItem::Network).collect();
        assert_eq!(
            target_height(&many, 0, Some(900.0)),
            900.0 - 2.0 * SCREEN_EDGE_MARGIN,
            "a long stack scrolls inside a screen-bounded window instead of running off-screen"
        );
    }

    #[test]
    fn target_height_falls_back_to_a_fixed_cap_without_a_known_monitor() {
        let many: Vec<StackItem> = (0..40).map(StackItem::Network).collect();
        assert_eq!(target_height(&many, 0, None), FALLBACK_MAX_HEIGHT);
    }

    #[test]
    fn position_top_right_insets_the_window_from_the_monitor_corner() {
        let pos = position_top_right(egui::Vec2::new(1920.0, 1080.0));
        assert_eq!(pos.x, 1920.0 - WINDOW_WIDTH - SCREEN_EDGE_MARGIN);
        assert_eq!(pos.y, SCREEN_EDGE_MARGIN);
    }

    #[test]
    fn visibility_transition_show_when_not_yet_visible() {
        assert_eq!(
            visibility_transition(true, false),
            VisibilityTransition::Show
        );
    }

    #[test]
    fn visibility_transition_hide_when_currently_visible() {
        assert_eq!(
            visibility_transition(false, true),
            VisibilityTransition::Hide
        );
    }

    #[test]
    fn visibility_transition_unchanged_when_already_showing() {
        assert_eq!(
            visibility_transition(true, true),
            VisibilityTransition::Unchanged
        );
    }

    #[test]
    fn visibility_transition_unchanged_when_already_hidden() {
        assert_eq!(
            visibility_transition(false, false),
            VisibilityTransition::Unchanged
        );
    }
}
