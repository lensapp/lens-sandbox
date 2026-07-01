use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
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

pub const WINDOW_WIDTH: f32 = 380.0;
const WINDOW_HEIGHT: f32 = 300.0;
const SCREEN_EDGE_MARGIN: f32 = 20.0;
const SCREEN_RIGHT_MARGIN: f32 = 0.0;

pub const MIN_WINDOW_HEIGHT: f32 = 140.0;
const FALLBACK_MAX_HEIGHT: f32 = 720.0;
const INFORM_ITEM_HEIGHT: f32 = 88.0;
const CONNECTING_ITEM_HEIGHT: f32 = 112.0;
const CARD_ITEM_HEIGHT: f32 = 282.0;
const TOKEN_REVEAL_EXTRA: f32 = 150.0;

const FOLD_SECONDS: f32 = 0.34;
const SLIDE_SECONDS: f32 = 0.22;
const PILE_PEEK: f32 = 9.0;
const PILE_INSET: f32 = 10.0;
const PILE_MAX_LEDGES: usize = 2;
const PILE_HEADER_H: f32 = 19.0;
const PILE_HEADER_BUTTON_CENTER: f32 = 16.0;

/// Builds the tray icon + Quit menu and installs the global menu-event handler (Quit signals shutdown); `on_event` lets the caller repaint after any menu event.
fn build_tray_icon(
    shutdown: Arc<Shutdown>,
    on_event: impl Fn() + Send + Sync + 'static,
) -> anyhow::Result<tray_icon::TrayIcon> {
    let menu = Menu::new();
    let audit_item = MenuItem::new("Audit", true, None);
    menu.append(&audit_item)
        .context("failed to append audit menu item")?;
    let audit_id = audit_item.id().clone();
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
        if event.id == audit_id {
            crate::dashboard::live::request_open();
            if let Some(ctx) = window::ctx() {
                ctx.request_repaint_of(egui::ViewportId::ROOT);
            }
        } else if event.id == quit_id {
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
            window::quiet_debug_overlays(&cc.egui_ctx);
            window::install_system_fonts(&cc.egui_ctx);
            window::install_icon_font(&cc.egui_ctx);
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
    placement: ViewportPlacement,
    credential_inputs: HashMap<String, String>,
    token_drafts: HashMap<String, TokenDraft>,
    remember: HashMap<String, bool>,
    audit: Arc<Mutex<AuditWindow>>,
    audit_open: Arc<AtomicBool>,
}

#[derive(Default)]
struct AuditWindow {
    state: crate::dashboard::DashboardState,
    last_gen: u64,
    focused: bool,
}

/// The transient UI state of one card's token fallback: whether the field is revealed and what's been typed.
#[derive(Default)]
pub struct TokenDraft {
    revealed: bool,
    value: String,
}

impl TokenDraft {
    pub fn is_revealed(&self) -> bool {
        self.revealed
    }
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
            placement: ViewportPlacement::new(),
            credential_inputs: HashMap::new(),
            token_drafts: HashMap::new(),
            remember: HashMap::new(),
            audit: Arc::new(Mutex::new(AuditWindow::default())),
            audit_open: Arc::new(AtomicBool::new(false)),
        })
    }

    fn render_audit_dashboard(&mut self, ctx: &egui::Context) {
        if crate::dashboard::live::take_open_request() {
            self.audit_open.store(true, Ordering::Relaxed);
            if let Ok(mut w) = self.audit.lock() {
                w.state = crate::dashboard::DashboardState::new();
                crate::dashboard::load(&mut w.state);
                w.last_gen = crate::dashboard::live::generation();
                w.focused = false;
            }
            ctx.send_viewport_cmd_to(
                crate::dashboard::live::viewport_id(),
                egui::ViewportCommand::Focus,
            );
        }
        if !self.audit_open.load(Ordering::Relaxed) {
            return;
        }
        let audit = self.audit.clone();
        let audit_open = self.audit_open.clone();
        ctx.show_viewport_deferred(
            crate::dashboard::live::viewport_id(),
            crate::dashboard::viewport_builder(),
            move |ui, _class| audit_frame(ui, &audit, &audit_open),
        );
    }
}

fn audit_frame(ui: &mut egui::Ui, audit: &Mutex<AuditWindow>, audit_open: &AtomicBool) {
    let saved_style = ui.ctx().global_style();
    crate::dashboard::apply_theme(ui.ctx());
    ui.set_style(ui.ctx().global_style());
    let (focused, close_requested) = ui.ctx().input(|i| {
        let vp = i.viewport();
        (vp.focused.unwrap_or(false), vp.close_requested())
    });
    crate::dashboard::live::set_watching(focused);
    if let Ok(mut w) = audit.lock() {
        let generation = crate::dashboard::live::generation();
        if (focused && !w.focused) || generation != w.last_gen {
            crate::dashboard::load(&mut w.state);
            w.last_gen = crate::dashboard::live::generation();
        }
        w.focused = focused;
        if crate::dashboard::render(ui, &mut w.state) == crate::dashboard::DashboardAction::Refresh
        {
            crate::dashboard::load(&mut w.state);
            w.last_gen = crate::dashboard::live::generation();
        }
    }
    if close_requested {
        audit_open.store(false, Ordering::Relaxed);
        crate::dashboard::live::set_watching(false);
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
    }
    ui.ctx().set_global_style(saved_style);
}

/// Drives the approval viewport's show/hide and programmatic resize from the current stack, so the production tray and the `approval_preview` harness exercise the identical window lifecycle.
pub struct ViewportPlacement {
    last_visible: bool,
    current_height: f32,
    pending_hide: bool,
}

impl Default for ViewportPlacement {
    fn default() -> Self {
        Self {
            last_visible: false,
            current_height: WINDOW_HEIGHT,
            pending_hide: false,
        }
    }
}

impl ViewportPlacement {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn sync_visibility(&mut self, ctx: &egui::Context, order: &[StackItem], revealed: usize) {
        let should_show = !order.is_empty();
        if should_show {
            self.pending_hide = false;
        }

        match visibility_transition(should_show, self.last_visible) {
            VisibilityTransition::Show => {
                // Place and size the window before revealing it, or macOS flashes it un-positioned mid-screen for a frame.
                let Some(pos) = top_right_position(ctx) else {
                    ctx.request_repaint();
                    return;
                };
                // A seed height keeps the reveal frame (which skips ui()) close to size; ui() then snaps the window to its measured content so no estimate slop shows as bottom padding.
                let monitor_height = ctx.input(|i| i.viewport().monitor_size).map(|m| m.y);
                let seed = target_height(order, revealed, monitor_height);
                join_all_spaces();
                set_window_shadows(true);
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
            VisibilityTransition::Hide if !self.pending_hide => {
                // The window must stay up one more frame to composite its now-empty content, or macOS strands the last card-shaped shadow as a ghost after the orderOut.
                self.pending_hide = true;
                ctx.request_repaint();
            }
            VisibilityTransition::Hide => {
                set_window_shadows(false);
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::MousePassthrough(true));
                self.last_visible = false;
                self.pending_hide = false;
            }
            // Resizing while visible is driven by ui()'s content measurement, not an estimate.
            VisibilityTransition::Unchanged => {}
        }
    }

    /// Snaps the viewport to `target` using measured content size, not the window-bounded `min_rect`, so a card taller than the current window grows it instead of being clipped.
    pub fn fit_height(&mut self, ctx: &egui::Context, target: f32) {
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
        let order = self.window_state.snapshot().order;
        let revealed = self
            .token_drafts
            .values()
            .filter(|d| d.is_revealed())
            .count();
        self.placement.sync_visibility(ctx, &order, revealed);
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
        self.placement.fit_height(ui.ctx(), target);

        match action {
            Some(CardAction::CloseAll) => {
                close_all(&self.window_state, &snapshot);
                ui.ctx().request_repaint();
            }
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

        self.render_audit_dashboard(ui.ctx());
        refresh_window_shadows();
    }
}

const BTN_GAP: f32 = 12.0;

#[derive(Debug, PartialEq, Eq)]
pub enum CardAction {
    /// Dismiss every card at once (the pile's close-all header button), denying or cancelling each held request.
    CloseAll,
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
    if snapshot.order.is_empty() {
        return (None, 0.0);
    }
    if snapshot.order.len() == 1 {
        ui.ctx()
            .data_mut(|d| d.insert_temp(egui::Id::new("approval-pile-expanded"), false));
        return render_single(
            ui,
            snapshot,
            credential_inputs,
            token_drafts,
            remember,
            scroll_max,
        );
    }
    render_pile(
        ui,
        snapshot,
        credential_inputs,
        token_drafts,
        remember,
        scroll_max,
    )
}

fn render_single(
    ui: &mut egui::Ui,
    snapshot: &Snapshot,
    credential_inputs: &mut HashMap<String, String>,
    token_drafts: &mut HashMap<String, TokenDraft>,
    remember: &mut HashMap<String, bool>,
    scroll_max: f32,
) -> (Option<CardAction>, f32) {
    use egui::{Frame, Margin};

    ui.style_mut().spacing.scroll.floating = true;
    ui.style_mut().spacing.scroll.fade.strength = 0.0;
    let out = egui::ScrollArea::vertical()
        .max_height(scroll_max)
        .auto_shrink([false, true])
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .show(ui, |ui| {
            let card_width = WINDOW_WIDTH - 2.0 * theme::STACK_MARGIN as f32;
            Frame::new()
                .inner_margin(Margin::same(theme::STACK_MARGIN))
                .show(ui, |ui| {
                    let mut action = None;
                    for (idx, item) in snapshot.order.iter().enumerate() {
                        if idx > 0 {
                            ui.add_space(theme::CARD_GAP);
                        }
                        let (item_action, response) = render_item(
                            ui,
                            item,
                            snapshot,
                            credential_inputs,
                            token_drafts,
                            remember,
                            card_width,
                        );
                        let mut fired = item_action;
                        let close_rect = egui::Rect::from_center_size(
                            response.rect.left_top() + egui::vec2(3.0, 3.0),
                            egui::Vec2::splat(22.0),
                        );
                        if fired.is_none()
                            && ui.rect_contains_pointer(response.rect.union(close_rect))
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

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}

fn ledge_alpha(depth: usize) -> f32 {
    match depth {
        1 => 0.60,
        _ => 0.32,
    }
}

struct PileGeom {
    left: f32,
    top: f32,
    width: f32,
}

/// The two layouts a notification pile animates between, derived purely from measured card heights so the rendering and the window sizing agree: each card's expanded slot offset, the folded/expanded content heights, and how many ledges peek when collapsed.
struct PileLayout {
    expanded_y: Vec<f32>,
    expanded_h: f32,
    folded_h: f32,
    visible: usize,
}

impl PileLayout {
    fn new(heights: &[f32]) -> Self {
        let n = heights.len();
        let mut expanded_y = vec![0.0_f32; n];
        expanded_y[0] = PILE_HEADER_H;
        for i in 1..n {
            expanded_y[i] = expanded_y[i - 1] + heights[i - 1] + theme::CARD_GAP;
        }
        let expanded_h = expanded_y[n - 1] + heights[n - 1];
        let visible = (n - 1).min(PILE_MAX_LEDGES);
        let folded_h = heights[0] + visible as f32 * PILE_PEEK;
        Self {
            expanded_y,
            expanded_h,
            folded_h,
            visible,
        }
    }

    fn depth(&self, i: usize) -> usize {
        i.min(self.visible).max(1)
    }

    /// Card `i`'s top offset at fold fraction `t`, lerping from its folded peek to its fanned-out slot.
    fn card_y(&self, i: usize, t: f32) -> f32 {
        let folded = if i == 0 {
            0.0
        } else {
            self.depth(i) as f32 * PILE_PEEK
        };
        lerp(folded, self.expanded_y[i], t)
    }

    /// Card `i`'s per-side horizontal inset at fold fraction `t`: narrowed as a ledge when folded, flush when fanned out.
    fn card_inset(&self, i: usize, t: f32) -> f32 {
        if i == 0 {
            return 0.0;
        }
        lerp(self.depth(i) as f32 * PILE_INSET, 0.0, t)
    }
}

struct PileMemory {
    expanded: bool,
    heights: Vec<f32>,
}

/// Keyed by underlying id, not list position, so a mid-list removal can't reuse a neighbour's scope id (an egui id-clash flash) or cached height.
fn card_key(item: &StackItem, snapshot: &Snapshot) -> egui::Id {
    match *item {
        StackItem::Network(i) => egui::Id::new(("pile-net", &snapshot.pending[i].id)),
        StackItem::Credential(i) => {
            egui::Id::new(("pile-cred", &snapshot.pending_credentials[i].id))
        }
        StackItem::SignIn(i) => egui::Id::new(("pile-signin", &snapshot.sign_ins[i].credential_id)),
        StackItem::Connecting(i) => egui::Id::new(("pile-conn", &snapshot.connecting[i])),
        StackItem::Inform(i) => egui::Id::new(("pile-inform", i)),
    }
}

fn read_pile_memory(ctx: &egui::Context, snapshot: &Snapshot) -> PileMemory {
    let expanded = ctx
        .data(|d| d.get_temp::<bool>(egui::Id::new("approval-pile-expanded")))
        .unwrap_or(false);
    let cache = ctx
        .data(|d| d.get_temp::<HashMap<egui::Id, f32>>(egui::Id::new("approval-pile-heights")))
        .unwrap_or_default();
    let heights = snapshot
        .order
        .iter()
        .map(|item| {
            cache
                .get(&card_key(item, snapshot))
                .copied()
                .unwrap_or_else(|| item_height(item))
        })
        .collect();
    PileMemory { expanded, heights }
}

/// Reads and updates the pile's scroll offset; only the fully-expanded pile scrolls, and only when its fanned-out content overflows the window.
fn pile_scroll_offset(
    ctx: &egui::Context,
    ui: &egui::Ui,
    geom: &PileGeom,
    expanded: bool,
    max_scroll: f32,
) -> f32 {
    let id = egui::Id::new("approval-pile-scroll");
    let mut scroll = ctx.data(|d| d.get_temp::<f32>(id)).unwrap_or(0.0);
    if !expanded || max_scroll <= 0.0 {
        scroll = 0.0;
    } else {
        let region = egui::Rect::from_min_size(
            egui::pos2(geom.left, geom.top),
            egui::vec2(geom.width, ui.max_rect().height()),
        );
        if ui.rect_contains_pointer(region) {
            let dy = ui.input(|i| i.smooth_scroll_delta.y);
            if dy != 0.0 {
                ctx.request_repaint();
            }
            scroll -= dy;
        }
        scroll = scroll.clamp(0.0, max_scroll);
    }
    ctx.data_mut(|d| d.insert_temp(id, scroll));
    scroll
}

fn close_all(state: &WindowState, snapshot: &Snapshot) {
    let mut had_inform = false;
    for item in &snapshot.order {
        match *item {
            StackItem::Network(i) => {
                state.decide(&snapshot.pending[i].id, Decision::DenyOnce);
            }
            StackItem::Credential(i) => {
                state.decide_credential(
                    &snapshot.pending_credentials[i].id,
                    CredentialDecisionRequest::Deny,
                );
            }
            StackItem::SignIn(i) => {
                state.cancel_sign_in(&snapshot.sign_ins[i].credential_id);
            }
            StackItem::Inform(_) => had_inform = true,
            StackItem::Connecting(_) => {}
        }
    }
    if had_inform {
        state.clear_informs();
    }
}

fn paint_pile_ledges(ui: &egui::Ui, geom: &PileGeom, layout: &PileLayout, top_h: f32, t: f32) {
    use egui::{Color32, CornerRadius, Stroke, StrokeKind, pos2, vec2};

    if t >= 0.999 {
        return;
    }
    let radius = CornerRadius::same(theme::CARD_CORNER_RADIUS);
    for depth in (1..=layout.visible).rev() {
        let inset = depth as f32 * PILE_INSET;
        let rect = egui::Rect::from_min_size(
            pos2(geom.left + inset, geom.top + depth as f32 * PILE_PEEK),
            vec2(geom.width - 2.0 * inset, top_h),
        );
        let a = ledge_alpha(depth) * (1.0 - t);
        let fill = Color32::from_rgba_unmultiplied(
            window::BG_SECONDARY.r(),
            window::BG_SECONDARY.g(),
            window::BG_SECONDARY.b(),
            (theme::CARD_FILL_ALPHA as f32 * a) as u8,
        );
        ui.painter().rect(
            rect,
            radius,
            fill,
            Stroke::new(1.0, window::BORDER.gamma_multiply(a)),
            StrokeKind::Inside,
        );
    }
}

/// Renders the real cards at their interpolated positions (deepest first so the top card wins input), clipped under the header so scrolled cards vanish behind it, and returns the fired action plus each card's freshly-measured height.
#[allow(clippy::too_many_arguments)]
fn render_pile_cards(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    snapshot: &Snapshot,
    credential_inputs: &mut HashMap<String, String>,
    token_drafts: &mut HashMap<String, TokenDraft>,
    remember: &mut HashMap<String, bool>,
    geom: &PileGeom,
    layout: &PileLayout,
    heights: &[f32],
    t: f32,
    scroll: f32,
) -> (Option<CardAction>, Vec<f32>) {
    use egui::{Layout, Rect, UiBuilder, pos2, vec2};

    let order = &snapshot.order;
    let n = order.len();
    let mut measured = heights.to_vec();
    let mut action: Option<CardAction> = None;
    let allow_close = !(0.001..=0.999).contains(&t);
    let clip = Rect::from_min_max(
        pos2(geom.left - 8.0, geom.top + PILE_HEADER_H * t),
        pos2(geom.left + geom.width + 8.0, ui.max_rect().bottom()),
    );

    let mut indices: Vec<usize> = (1..n).rev().collect();
    indices.push(0);
    for i in indices {
        if i != 0 && t <= 0.001 {
            continue;
        }
        let key = card_key(&order[i], snapshot);
        let inset = layout.card_inset(i, t);
        let w = geom.width - 2.0 * inset;
        // Animate the layout slot (not the scroll offset, which must track the wheel 1:1) so a removal slides neighbours up macOS-style; during the fold the slot is already driven by `t`, so track it instantly.
        let slot_y = geom.top + layout.card_y(i, t);
        let settle = if t >= 0.999 { SLIDE_SECONDS } else { 0.0 };
        let y = ctx.animate_value_with_time(key.with("slide"), slot_y, settle) - scroll;
        let min = pos2(geom.left + inset, y);
        let alpha = if i == 0 { 1.0 } else { t };
        let res = ui.scope_builder(
            UiBuilder::new()
                .id_salt(key)
                .max_rect(Rect::from_min_size(min, vec2(w, 10_000.0)))
                .layout(Layout::top_down(egui::Align::Min)),
            |ui| {
                ui.set_clip_rect(clip);
                ui.set_opacity(alpha);
                ui.set_width(w);
                render_item(
                    ui,
                    &order[i],
                    snapshot,
                    credential_inputs,
                    token_drafts,
                    remember,
                    w,
                )
            },
        );
        let (item_action, card_resp) = res.inner;
        measured[i] = card_resp.rect.height();
        if action.is_none() {
            action = item_action;
        }
        if action.is_none()
            && allow_close
            && let Some(close) = pile_close(ui, &order[i], snapshot, card_resp.rect, key)
        {
            action = Some(close);
        }
    }
    (action, measured)
}

/// The hover ✕ on a settled card, mirroring [`render_single`]'s close affordance.
fn pile_close(
    ui: &mut egui::Ui,
    item: &StackItem,
    snapshot: &Snapshot,
    card_rect: egui::Rect,
    key: egui::Id,
) -> Option<CardAction> {
    let close_rect = egui::Rect::from_center_size(
        card_rect.left_top() + egui::vec2(3.0, 3.0),
        egui::Vec2::splat(22.0),
    );
    if ui.rect_contains_pointer(card_rect.union(close_rect))
        && let Some(close) = close_action(item, snapshot)
        && close_button(ui, key.with("close"), close_rect).clicked()
    {
        return Some(close);
    }
    None
}

enum PileHeaderEvent {
    ShowLess,
    CloseAll,
}

/// A macOS-style translucent capsule that reads on any wallpaper, brightening on hover and depressing on press; returns its response and the (press-scaled) content rect to draw a label or glyph into.
fn pile_pill(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    rect: egui::Rect,
    id: egui::Id,
    alpha: f32,
) -> (egui::Response, egui::Rect) {
    use egui::{Color32, CornerRadius, Sense, Stroke, StrokeKind};

    let resp = ui.interact(rect, id, Sense::click());
    let hover = ctx.animate_bool_with_time(id.with("hover"), resp.hovered(), 0.10);
    let press =
        ctx.animate_bool_with_time(id.with("press"), resp.is_pointer_button_down_on(), 0.06);
    let r = egui::Rect::from_center_size(rect.center(), rect.size() * (1.0 - 0.06 * press));
    let radius = CornerRadius::same((r.height() * 0.5) as u8);
    let fill_a = ((170.0 + 55.0 * hover) * alpha).clamp(0.0, 255.0) as u8;
    let stroke_a = ((55.0 + 45.0 * hover) * alpha).clamp(0.0, 255.0) as u8;
    ui.painter().rect(
        r,
        radius,
        Color32::from_rgba_unmultiplied(96, 97, 104, fill_a),
        Stroke::new(1.0, Color32::from_white_alpha(stroke_a)),
        StrokeKind::Inside,
    );
    if resp.hovered() {
        ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    (resp, r)
}

/// The fanned-out header: a "Show Less" pill and a circular close-all button, right-aligned and fading in with the pile; returns whichever was clicked.
fn pile_header(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    geom: &PileGeom,
    t: f32,
) -> Option<PileHeaderEvent> {
    use egui::{Align2, Color32, FontId, Id, Rect, Stroke, Vec2, pos2, vec2};

    if t <= 0.01 {
        return None;
    }
    let h = 22.0;
    let cy = geom.top - theme::STACK_MARGIN as f32 + PILE_HEADER_BUTTON_CENTER;
    let right = geom.left + geom.width;
    let label = "Show Less";
    let text_w = ui
        .painter()
        .layout_no_wrap(label.to_owned(), FontId::proportional(12.0), Color32::WHITE)
        .rect
        .width();

    let close_rect = Rect::from_center_size(pos2(right - h / 2.0, cy), Vec2::splat(h));
    let pill_rect = Rect::from_center_size(
        pos2(close_rect.left() - 8.0 - (text_w + 24.0) / 2.0, cy),
        vec2(text_w + 24.0, h),
    );
    let fg = Color32::from_rgba_unmultiplied(236, 237, 242, (255.0 * t) as u8);
    let mut event = None;

    let (show_less, content) = pile_pill(ui, ctx, pill_rect, Id::new("approval-pile-showless"), t);
    ui.painter().text(
        content.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(12.0),
        fg,
    );
    if show_less.clicked() {
        event = Some(PileHeaderEvent::ShowLess);
    }

    let (close_all, content) = pile_pill(ui, ctx, close_rect, Id::new("approval-pile-closeall"), t);
    let arm = content.width() * 0.18;
    let c = content.center();
    let x = Stroke::new(1.6, fg);
    ui.painter()
        .line_segment([c - vec2(arm, arm), c + vec2(arm, arm)], x);
    ui.painter()
        .line_segment([c + vec2(arm, -arm), c - vec2(arm, -arm)], x);
    if close_all.clicked() {
        event = Some(PileHeaderEvent::CloseAll);
    }

    event
}

/// The click target over a collapsed pile — the whole top card and its peeking ledges — registered under the cards so the top card's own buttons still win; returns whether it was clicked to fan the pile out.
fn pile_expand_hit(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    geom: &PileGeom,
    layout: &PileLayout,
    t: f32,
    expanded: bool,
) -> bool {
    use egui::{Id, Rect, Sense, pos2};

    if expanded || t >= 0.5 {
        return false;
    }
    let hit = Rect::from_min_max(
        pos2(geom.left, geom.top),
        pos2(geom.left + geom.width, geom.top + layout.folded_h + 6.0),
    );
    let resp = ui.interact(hit, Id::new("approval-pile-expand"), Sense::click());
    if resp.contains_pointer() {
        ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.clicked()
}

/// Renders the multi-card stack as a macOS-style notification pile: collapsed it shows the top card with the rest peeking as stacked ledges; a click fans them out, a "Show Less" header folds them back, and the fully-fanned list scrolls when it overflows — every position, width, and opacity animating between the two layouts.
fn render_pile(
    ui: &mut egui::Ui,
    snapshot: &Snapshot,
    credential_inputs: &mut HashMap<String, String>,
    token_drafts: &mut HashMap<String, TokenDraft>,
    remember: &mut HashMap<String, bool>,
    scroll_max: f32,
) -> (Option<CardAction>, f32) {
    use egui::{Id, vec2};

    let ctx = ui.ctx().clone();
    let mut mem = read_pile_memory(&ctx, snapshot);
    let t = ease_out_cubic(ctx.animate_bool_with_time(
        Id::new("approval-pile-t"),
        mem.expanded,
        FOLD_SECONDS,
    ));
    let layout = PileLayout::new(&mem.heights);
    let margin = theme::STACK_MARGIN as f32;
    let geom = PileGeom {
        left: ui.max_rect().min.x + margin,
        top: ui.max_rect().min.y + margin,
        width: WINDOW_WIDTH - 2.0 * margin,
    };
    let top_h = mem.heights[0];

    let viewport_h = (scroll_max - 2.0 * margin).max(0.0);
    let max_scroll = (layout.expanded_h - viewport_h).max(0.0);
    let scroll = pile_scroll_offset(&ctx, ui, &geom, mem.expanded, max_scroll);

    paint_pile_ledges(ui, &geom, &layout, top_h, t);
    let expand_clicked = pile_expand_hit(ui, &ctx, &geom, &layout, t, mem.expanded);
    let (mut action, measured) = render_pile_cards(
        ui,
        &ctx,
        snapshot,
        credential_inputs,
        token_drafts,
        remember,
        &geom,
        &layout,
        &mem.heights,
        t,
        scroll,
    );
    match pile_header(ui, &ctx, &geom, t) {
        Some(PileHeaderEvent::ShowLess) => {
            mem.expanded = false;
            ctx.request_repaint();
        }
        Some(PileHeaderEvent::CloseAll) => {
            action.get_or_insert(CardAction::CloseAll);
            ctx.request_repaint();
        }
        None => {}
    }
    if expand_clicked {
        mem.expanded = true;
        ctx.request_repaint();
    }

    let cache: HashMap<egui::Id, f32> = snapshot
        .order
        .iter()
        .zip(&measured)
        .map(|(item, h)| (card_key(item, snapshot), *h))
        .collect();
    ctx.data_mut(|d| {
        d.insert_temp(Id::new("approval-pile-expanded"), mem.expanded);
        d.insert_temp(Id::new("approval-pile-heights"), cache);
    });

    let window_h = if mem.expanded || t > 0.001 {
        layout.expanded_h
    } else {
        layout.folded_h
    };
    let target = (window_h + 2.0 * margin).min(scroll_max);
    // Settle the window height only when fanned out, so a removal shrinks it smoothly; during the fold it snaps so the fan-out never clips against a lagging window.
    let settle = if mem.expanded && t >= 0.999 {
        SLIDE_SECONDS
    } else {
        0.0
    };
    let total = ctx.animate_value_with_time(Id::new("approval-pile-window-h"), target, settle);
    ui.allocate_space(vec2(WINDOW_WIDTH, total));
    (action, total)
}

fn render_item(
    ui: &mut egui::Ui,
    item: &StackItem,
    snapshot: &Snapshot,
    credential_inputs: &mut HashMap<String, String>,
    token_drafts: &mut HashMap<String, TokenDraft>,
    remember: &mut HashMap<String, bool>,
    width: f32,
) -> (Option<CardAction>, egui::Response) {
    match *item {
        StackItem::Inform(i) => {
            let r = crate::ui::card(ui, width, |ui| {
                render_inform_content(ui, &snapshot.informs[i])
            });
            (None, r.response)
        }
        StackItem::Network(i) => {
            let prompt = &snapshot.pending[i];
            if let Some(display_name) = &prompt.offer {
                let draft = token_drafts.entry(prompt.id.clone()).or_default();
                render_offer_card(ui, prompt, display_name, draft, width)
            } else {
                let flag = remember.entry(prompt.id.clone()).or_default();
                render_network_card(ui, prompt, flag, width)
            }
        }
        StackItem::SignIn(i) => {
            let card = &snapshot.sign_ins[i];
            let draft = token_drafts.entry(card.credential_id.clone()).or_default();
            render_sign_in_card(ui, card, draft, width)
        }
        StackItem::Credential(i) => {
            let prompt = &snapshot.pending_credentials[i];
            let input = credential_inputs.entry(prompt.id.clone()).or_default();
            let draft = token_drafts.entry(prompt.id.clone()).or_default();
            render_credential_card(ui, prompt, input, draft, width)
        }
        StackItem::Connecting(i) => {
            let r = crate::ui::card(ui, width, |ui| {
                render_connecting_card(ui, &snapshot.connecting[i])
            });
            (None, r.response)
        }
    }
}

fn render_connecting_card(ui: &mut egui::Ui, display_name: &str) {
    use egui::RichText;

    crate::ui::eyebrow(ui, egui_material_icons::icons::ICON_LINK, "CONNECT");

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
    use egui::RichText;

    ui.horizontal(|ui| {
        ui.colored_label(window::STATUS_WARNING, "⚠");
        ui.colored_label(window::TEXT_PRIMARY, RichText::new(msg).size(12.0));
    });
}

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
    width: f32,
) -> (Option<CardAction>, egui::Response) {
    use egui::RichText;

    let id = prompt.id.clone();
    let out = crate::ui::card_sectioned(
        ui,
        width,
        |ui| {
            crate::ui::eyebrow(ui, egui_material_icons::icons::ICON_PUBLIC, "NETWORK");
            ui.add_space(6.0);
            ui.label(
                RichText::new(format!("Connect to {}?", prompt.host))
                    .size(theme::FONT_TITLE)
                    .strong()
                    .color(window::TEXT_ACCENT),
            );
            ui.add_space(8.0);
            crate::ui::badges(ui, connection_badges(&prompt.action));
        },
        |ui| {
            remember_toggle(ui, remember);
            ui.add_space(BTN_GAP);
            let mut chosen: Option<Decision> = None;
            ui.columns(2, |cols| {
                if primary_button(&mut cols[0], "Allow").clicked() {
                    chosen = Some(allow_decision(*remember));
                }
                if deny_button(&mut cols[1], "Deny").clicked() {
                    chosen = Some(deny_decision(*remember));
                }
            });
            chosen
        },
    );
    (
        out.inner
            .map(|decision| CardAction::Decide { id, decision }),
        out.response,
    )
}

fn connection_badges(action: &str) -> Vec<String> {
    match action.rsplit_once(':') {
        Some((_, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => {
            vec!["TCP".to_string(), port.to_string()]
        }
        _ => vec![action.to_string()],
    }
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
    use egui::{Color32, CornerRadius, RichText, Sense, Shape, Stroke, StrokeKind, vec2};

    let resp = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let (rect, _) = ui.allocate_exact_size(vec2(15.0, 15.0), Sense::hover());
            let painter = ui.painter().clone();
            let (fill, border) = if *remember {
                (Color32::WHITE, Color32::WHITE)
            } else {
                (Color32::TRANSPARENT, window::TEXT_MUTED)
            };
            painter.rect(
                rect,
                CornerRadius::same(4),
                fill,
                Stroke::new(1.5, border),
                StrokeKind::Inside,
            );
            if *remember {
                let c = rect.center();
                let r = rect.width() * 0.5;
                painter.add(Shape::line(
                    vec![
                        c + vec2(-0.4 * r, 0.0),
                        c + vec2(-0.1 * r, 0.34 * r),
                        c + vec2(0.42 * r, -0.34 * r),
                    ],
                    Stroke::new(1.7, window::BG_PRIMARY),
                ));
            }
            ui.label(
                RichText::new("Don't ask again for this host")
                    .size(theme::FONT_CAPTION)
                    .color(window::TEXT_MUTED),
            );
        })
        .response;
    if resp.interact(Sense::click()).clicked() {
        *remember = !*remember;
    }
}

fn render_offer_card(
    ui: &mut egui::Ui,
    prompt: &PendingPrompt,
    display_name: &str,
    draft: &mut TokenDraft,
    width: f32,
) -> (Option<CardAction>, egui::Response) {
    use egui::RichText;

    let id = prompt.id.clone();
    let out = crate::ui::card_sectioned(
        ui,
        width,
        |ui| {
            crate::ui::eyebrow(ui, egui_material_icons::icons::ICON_LINK, "CONNECT");
            ui.add_space(6.0);
            ui.label(
                RichText::new(format!("Connect to {display_name}?"))
                    .size(theme::FONT_TITLE)
                    .strong()
                    .color(window::TEXT_ACCENT),
            );
            ui.add_space(2.0);
            ui.label(
                RichText::new(format!("A workload wants to reach {}.", prompt.host))
                    .size(theme::FONT_BODY)
                    .color(window::TEXT_MUTED),
            );
        },
        |ui| {
            let mut action: Option<CardAction> = None;
            ui.columns(2, |cols| {
                if primary_button(&mut cols[0], "Connect").clicked() {
                    action = Some(CardAction::ConnectOffer { id: id.clone() });
                }
                if secondary_button(&mut cols[1], "Not now").clicked() {
                    action = Some(CardAction::DeclineOffer { id: id.clone() });
                }
            });
            if action.is_none()
                && let Some(fallback) = &prompt.token_fallback
            {
                action = match render_token_fallback(ui, fallback, draft) {
                    Some(TokenFallbackEvent::Save(value)) => Some(CardAction::UseOfferToken {
                        id: id.clone(),
                        value,
                    }),
                    Some(TokenFallbackEvent::OpenHelp(url)) => {
                        Some(CardAction::OpenBrowser { url })
                    }
                    None => None,
                };
            }
            action
        },
    );
    (out.inner, out.response)
}

fn render_credential_card(
    ui: &mut egui::Ui,
    prompt: &CredentialCardPrompt,
    input: &mut String,
    draft: &mut TokenDraft,
    width: f32,
) -> (Option<CardAction>, egui::Response) {
    use egui::RichText;

    if let Some(display_name) = &prompt.oauth_display_name {
        return render_oauth_consent_card(ui, prompt, display_name, draft, width);
    }

    let id = prompt.id.clone();
    let out = crate::ui::card_sectioned(
        ui,
        width,
        |ui| {
            crate::ui::eyebrow(ui, egui_material_icons::icons::ICON_KEY, "CREDENTIAL");
            ui.add_space(6.0);
            ui.label(
                RichText::new(&prompt.credential_id)
                    .size(theme::FONT_TITLE)
                    .strong()
                    .color(window::TEXT_ACCENT),
            );
            ui.add_space(2.0);
            ui.label(
                RichText::new(&prompt.action)
                    .size(theme::FONT_CAPTION)
                    .monospace()
                    .color(window::TEXT_MUTED),
            );
            if let Some(env) = &prompt.env_var {
                ui.add_space(4.0);
                ui.label(
                    RichText::new(format!("Reads host env: ${env}"))
                        .size(theme::FONT_CAPTION)
                        .color(window::TEXT_MUTED),
                );
            }
            if !prompt.injection_domains.is_empty() {
                ui.add_space(2.0);
                let domains = prompt.injection_domains.join(", ");
                ui.label(
                    RichText::new(format!("Sends to: {domains}"))
                        .size(theme::FONT_CAPTION)
                        .color(window::TEXT_MUTED),
                );
            }
            if prompt.is_project_defined {
                ui.add_space(4.0);
                ui.label(
                    RichText::new("Project-defined provider (not built-in)")
                        .size(theme::FONT_CAPTION)
                        .strong()
                        .color(window::TEXT_WARN),
                );
            }
            if !prompt.host_value_available {
                ui.add_space(8.0);
                ui.colored_label(
                    window::TEXT_MUTED,
                    RichText::new("No credential detected on host.")
                        .size(theme::FONT_CAPTION)
                        .italics(),
                );
            }
        },
        |ui| {
            let mut chosen: Option<CredentialDecisionRequest> = None;
            if prompt.host_value_available {
                if primary_button(ui, "Use detected value").clicked() {
                    chosen = Some(CredentialDecisionRequest::Allow(
                        CredentialEntry::HostDetect,
                    ));
                }
                ui.add_space(BTN_GAP);
            }
            secret_input(ui, input, "Enter a value");
            ui.add_space(BTN_GAP);
            let submit_enabled = !input.trim().is_empty();
            ui.columns(2, |cols| {
                if enabled_primary_button(&mut cols[0], "Submit", submit_enabled).clicked()
                    && submit_enabled
                {
                    chosen = Some(CredentialDecisionRequest::Allow(CredentialEntry::Stored {
                        value: input.trim().to_string(),
                    }));
                }
                if deny_button(&mut cols[1], "Deny").clicked() {
                    chosen = Some(CredentialDecisionRequest::Deny);
                }
            });
            chosen.map(|request| CardAction::DecideCredential {
                id: id.clone(),
                request,
            })
        },
    );
    (out.inner, out.response)
}

fn render_oauth_consent_card(
    ui: &mut egui::Ui,
    prompt: &CredentialCardPrompt,
    display_name: &str,
    draft: &mut TokenDraft,
    width: f32,
) -> (Option<CardAction>, egui::Response) {
    use egui::RichText;

    let id = prompt.id.clone();
    let out = crate::ui::card_sectioned(
        ui,
        width,
        |ui| {
            crate::ui::eyebrow(ui, egui_material_icons::icons::ICON_LINK, "CONNECT");
            ui.add_space(6.0);
            ui.label(
                RichText::new(format!("Connect to {display_name}?"))
                    .size(theme::FONT_TITLE)
                    .strong()
                    .color(window::TEXT_ACCENT),
            );
            ui.add_space(2.0);
            ui.label(
                RichText::new(format!(
                    "A workload wants to use your {display_name} access."
                ))
                .size(theme::FONT_BODY)
                .color(window::TEXT_MUTED),
            );
        },
        |ui| {
            let mut chosen: Option<CredentialDecisionRequest> = None;
            ui.columns(2, |cols| {
                if primary_button(&mut cols[0], "Connect").clicked() {
                    chosen = Some(CredentialDecisionRequest::Allow(
                        CredentialEntry::HostDetect,
                    ));
                }
                if deny_button(&mut cols[1], "Deny").clicked() {
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
                id: id.clone(),
                request,
            })
        },
    );
    (out.inner, out.response)
}

fn render_sign_in_card(
    ui: &mut egui::Ui,
    card: &SignInCard,
    draft: &mut TokenDraft,
    width: f32,
) -> (Option<CardAction>, egui::Response) {
    use egui::RichText;

    let out = crate::ui::card_sectioned(
        ui,
        width,
        |ui| {
            crate::ui::eyebrow(ui, egui_material_icons::icons::ICON_LINK, "CONNECT");
            ui.add_space(6.0);
            ui.label(
                RichText::new(format!("Connect to {}", card.display_name))
                    .size(theme::FONT_TITLE)
                    .strong()
                    .color(window::TEXT_ACCENT),
            );
            ui.add_space(8.0);
            match &card.user_code {
                Some(user_code) => {
                    ui.label(
                        RichText::new("Enter this code on the page that opens:")
                            .size(theme::FONT_BODY)
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
                            .size(theme::FONT_BODY)
                            .color(window::TEXT_MUTED),
                    );
                }
            }
        },
        |ui| {
            let mut action: Option<CardAction> = None;
            ui.columns(2, |cols| {
                if primary_button(&mut cols[0], "Open").clicked() {
                    action = Some(CardAction::OpenBrowser {
                        url: card.verification_uri.clone(),
                    });
                }
                if secondary_button(&mut cols[1], "Cancel").clicked() {
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
                    Some(TokenFallbackEvent::OpenHelp(url)) => {
                        Some(CardAction::OpenBrowser { url })
                    }
                    None => None,
                };
            }
            action
        },
    );
    (out.inner, out.response)
}

/// Progressive disclosure shared by every connect card: a muted "Use a token instead" that, once clicked, reveals a password field + "Save" and (when declared) a help link. Returns the user's action without performing it.
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
    if enabled_primary_button(ui, "Save", enabled).clicked() && enabled {
        event = Some(TokenFallbackEvent::Save(draft.value.trim().to_string()));
    }
    event
}

fn secret_input(ui: &mut egui::Ui, value: &mut String, hint: &str) -> egui::Response {
    ui.scope(|ui| {
        ui.style_mut().visuals.widgets.inactive.bg_stroke = egui::Stroke::new(1.0, window::BORDER);
        ui.style_mut().visuals.widgets.hovered.bg_stroke =
            egui::Stroke::new(1.0, egui::Color32::from_gray(96));
        ui.add(
            egui::TextEdit::singleline(value)
                .password(true)
                .hint_text(hint)
                .margin(egui::Margin::symmetric(10, 9))
                .desired_width(f32::INFINITY),
        )
    })
    .inner
}

fn primary_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    enabled_primary_button(ui, label, true)
}

fn enabled_primary_button(ui: &mut egui::Ui, label: &str, enabled: bool) -> egui::Response {
    Button::new(label, ButtonKind::Primary)
        .enabled(enabled)
        .min_size(egui::vec2(ui.available_width(), 0.0))
        .show(ui)
}

fn secondary_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    Button::new(label, ButtonKind::Secondary)
        .min_size(egui::vec2(ui.available_width(), 0.0))
        .show(ui)
}

fn deny_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    Button::new(label, ButtonKind::Danger)
        .min_size(egui::vec2(ui.available_width(), 0.0))
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
        monitor.x - WINDOW_WIDTH - SCREEN_RIGHT_MARGIN,
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
pub fn install_activation_policy(opts: &mut eframe::NativeOptions) {
    use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
    opts.event_loop_builder = Some(Box::new(|builder| {
        builder.with_activation_policy(ActivationPolicy::Accessory);
    }));
}

#[cfg(not(target_os = "macos"))]
pub fn install_activation_policy(_opts: &mut eframe::NativeOptions) {}

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

/// A transparent window's shadow recomputes only on resize, never on a same-size repaint, so a scrolled card needs explicit per-frame invalidation or its shadow freezes at the old position.
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

/// Dropped before the hide so the card-shaped shadow can't outlive the window; re-enabled on the next show.
#[cfg(target_os = "macos")]
fn set_window_shadows(enabled: bool) {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;

    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    for window in NSApplication::sharedApplication(mtm).windows().iter() {
        window.setHasShadow(enabled);
    }
}

#[cfg(not(target_os = "macos"))]
fn set_window_shadows(_enabled: bool) {}

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
            env_var: None,
            injection_domains: vec![],
            is_project_defined: false,
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

    fn shape_has_stroke_color(shape: &egui::Shape, c: egui::Color32) -> bool {
        match shape {
            egui::Shape::Rect(r) => r.stroke.color == c,
            egui::Shape::LineSegment { stroke, .. } => stroke.color == c,
            egui::Shape::Vec(v) => v.iter().any(|s| shape_has_stroke_color(s, c)),
            _ => false,
        }
    }

    fn pile_seed() -> Snapshot {
        let net = |id: &str, host: &str, offer: Option<&str>| PendingPrompt {
            id: id.into(),
            host: host.into(),
            action: format!("CONNECT {host}:443"),
            offer: offer.map(str::to_string),
            token_fallback: None,
        };
        Snapshot {
            pending: vec![net("n0", "a.test", None), net("n1", "b.test", Some("Svc"))],
            pending_credentials: vec![credential_prompt("c0"), credential_prompt("c1")],
            sign_ins: Vec::new(),
            informs: vec!["warn".into()],
            connecting: vec!["Svc".into()],
            order: vec![
                StackItem::Network(0),
                StackItem::Network(1),
                StackItem::Credential(0),
                StackItem::Credential(1),
                StackItem::Connecting(0),
                StackItem::Inform(0),
            ],
        }
    }

    #[test]
    fn clash_detector_catches_a_deliberate_id_clash() {
        let ctx = egui::Context::default();
        let error = ctx.global_style().visuals.error_fg_color;
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(400.0, 400.0),
            )),
            ..Default::default()
        };
        let output = ctx.run_ui(input, |ui| {
            let id = egui::Id::new("dup");
            ui.interact(
                egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(50.0, 20.0)),
                id,
                egui::Sense::click(),
            );
            ui.interact(
                egui::Rect::from_min_size(egui::pos2(200.0, 200.0), egui::vec2(50.0, 20.0)),
                id,
                egui::Sense::click(),
            );
        });
        assert!(
            output
                .shapes
                .iter()
                .any(|cs| shape_has_stroke_color(&cs.shape, error)),
            "the detector must catch a deliberate id clash, else the negative test is meaningless"
        );
    }

    #[test]
    fn pile_unfold_animation_has_no_id_clash() {
        let ctx = egui::Context::default();
        ctx.set_pixels_per_point(2.0);
        window::install_icon_font(&ctx);
        let snapshot = pile_seed();
        let error = ctx.global_style().visuals.error_fg_color;
        let (mut ci, mut td, mut rem) = (HashMap::new(), HashMap::new(), HashMap::new());
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("approval-pile-expanded"), true));

        let mut clash = false;
        let mut time = 0.0_f64;
        for frame in 0..16 {
            time += 0.03;
            let mut events = Vec::new();
            if frame == 0 {
                events.push(egui::Event::PointerMoved(egui::pos2(70.0, 40.0)));
            }
            let input = egui::RawInput {
                time: Some(time),
                events,
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(WINDOW_WIDTH, 6000.0),
                )),
                ..Default::default()
            };
            let output = ctx.run_ui(input, |ui| {
                render_stack(ui, &snapshot, &mut ci, &mut td, &mut rem, 6000.0);
            });
            let reds = [error, egui::Color32::RED, egui::Color32::ORANGE];
            if output
                .shapes
                .iter()
                .any(|cs| reds.iter().any(|c| shape_has_stroke_color(&cs.shape, *c)))
            {
                clash = true;
            }
        }
        assert!(
            !clash,
            "egui painted a red debug overlay during the unfold animation"
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
        assert_eq!(pos.x, 1920.0 - WINDOW_WIDTH - SCREEN_RIGHT_MARGIN);
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
