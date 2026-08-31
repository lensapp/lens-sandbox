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
use crate::approval_flow::session::{ConnectionChoice, PendingPrompt};
use crate::approval_flow::window::{self, Snapshot, StackItem, WindowState};
use crate::shutdown::Shutdown;
use crate::ui::{Button, ButtonKind, theme};

pub const WINDOW_WIDTH: f32 = 380.0;
const WINDOW_HEIGHT: f32 = 300.0;
const SCREEN_EDGE_MARGIN: f32 = 20.0;
const SCREEN_RIGHT_MARGIN: f32 = 0.0;

pub const MIN_WINDOW_HEIGHT: f32 = 140.0;
const FALLBACK_MAX_HEIGHT: f32 = 720.0;
const INFORM_ITEM_HEIGHT: f32 = 88.0;
const CARD_ITEM_HEIGHT: f32 = 282.0;

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

fn approval_viewport() -> egui::ViewportBuilder {
    egui::ViewportBuilder::default()
        .with_title("Lens Sandbox")
        .with_position([0.0, 0.0])
        .with_visible(false)
        .with_decorations(false)
        // Resizable so the card can grow programmatically when a token field is revealed; with no decorations there are no user-facing resize handles.
        .with_resizable(true)
        .with_always_on_top()
        .with_transparent(true)
        .with_mouse_passthrough(true)
        // The card must never become the active window on its own; the developer keeps typing where they were.
        .with_active(false)
        .with_inner_size([WINDOW_WIDTH, WINDOW_HEIGHT])
}

pub fn run_tray(
    shutdown: Arc<Shutdown>,
    ipc_handle: JoinHandle<anyhow::Result<()>>,
    window_state: Arc<WindowState>,
) -> anyhow::Result<()> {
    #[cfg(target_os = "linux")]
    let gtk_tray = spawn_gtk_tray(shutdown.clone());

    let mut native_options = eframe::NativeOptions {
        viewport: approval_viewport(),
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
            crate::ui::texture_delta_guard::install(&cc.egui_ctx);
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
    display_present_with(|key| std::env::var_os(key))
}

/// `LNS_HEADLESS` (any value but empty or "0") forces the tray off — for servers and CI hosts where the approval window can never appear.
fn display_present_with(env: impl Fn(&str) -> Option<std::ffi::OsString>) -> bool {
    if env("LNS_HEADLESS").is_some_and(|v| !v.is_empty() && v != "0") {
        return false;
    }
    if cfg!(target_os = "macos") {
        return true;
    }
    has_linux_display(|key| env(key).is_some())
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
        "no display detected — running headless without the tray; interactive approval prompts can't be shown, so pre-authorize access in lns-local-mixin.yaml"
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
    cards: CardState,
    audit: Arc<Mutex<AuditWindow>>,
    audit_open: Arc<AtomicBool>,
}

#[derive(Default)]
struct AuditWindow {
    state: crate::dashboard::DashboardState,
    last_gen: u64,
    focused: bool,
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
            cards: CardState::default(),
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

    pub fn sync_visibility(&mut self, ctx: &egui::Context, order: &[StackItem]) {
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
                let seed = target_height(order, monitor_height);
                join_all_spaces();
                set_window_shadows(true);
                ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos));
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                    WINDOW_WIDTH,
                    seed,
                )));
                ctx.send_viewport_cmd(egui::ViewportCommand::MousePassthrough(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
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
        self.placement.sync_visibility(ctx, &order);
        self.render_audit_dashboard(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let snapshot = self.window_state.snapshot();
        self.cards.prune(&snapshot);

        let monitor_height = ui.ctx().input(|i| i.viewport().monitor_size).map(|m| m.y);
        let cap = content_cap(monitor_height);
        let (action, content_height) = render_stack(ui, &snapshot, &mut self.cards, cap);
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
            Some(CardAction::DismissInform { index }) => {
                apply_dismissal(&self.window_state, &Dismissal::Inform { index });
                ui.ctx().request_repaint();
            }
            Some(CardAction::OpenBrowser { url }) => {
                crate::browser::open(&url);
                ui.ctx().request_repaint();
            }
            Some(CardAction::DismissNetwork { id }) => {
                apply_dismissal(&self.window_state, &Dismissal::Network { id });
                ui.ctx().request_repaint();
            }
            Some(CardAction::Grant {
                id,
                method,
                connection,
            }) => {
                self.window_state.grant(&id, &method, connection);
                ui.ctx().request_repaint();
            }
            Some(CardAction::Decline { id }) => {
                self.window_state.decline(&id);
                ui.ctx().request_repaint();
            }
            None => {}
        }

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
    /// Closing a card carries no verdict, so a dismissal names only the card — there is nowhere to put a decision the developer did not make.
    DismissNetwork {
        id: String,
    },
    DismissInform {
        index: usize,
    },
    /// Connect this project to the offered connector, with the connection the card chose.
    Grant {
        id: String,
        method: String,
        connection: ConnectionChoice,
    },
    /// A standing no for this project (§3.2.4).
    Decline {
        id: String,
    },
    OpenBrowser {
        url: String,
    },
}

/// Every way a card can leave the stack without a verdict; closed as a type so adding a card kind fails to compile in [`apply_dismissal`] rather than silently hanging its held request.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Dismissal {
    Network { id: String },
    Inform { index: usize },
}

impl From<Dismissal> for CardAction {
    fn from(dismissal: Dismissal) -> Self {
        match dismissal {
            Dismissal::Network { id } => Self::DismissNetwork { id },
            Dismissal::Inform { index } => Self::DismissInform { index },
        }
    }
}

fn item_height(item: &StackItem) -> f32 {
    match item {
        StackItem::Inform(_) => INFORM_ITEM_HEIGHT,
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
fn target_height(items: &[StackItem], monitor_height: Option<f32>) -> f32 {
    if items.is_empty() {
        return MIN_WINDOW_HEIGHT;
    }
    let content = items.iter().map(item_height).sum::<f32>()
        + theme::CARD_GAP * (items.len() - 1) as f32
        + 2.0 * theme::STACK_MARGIN as f32;
    content.clamp(MIN_WINDOW_HEIGHT, content_cap(monitor_height))
}

/// Renders the stack and returns the fired action plus the content's natural height (the scroll area's `content_size`, not the window-bounded laid-out size), which the caller sizes the window from so a card taller than the window grows it instead of being clipped.
pub fn render_stack(
    ui: &mut egui::Ui,
    snapshot: &Snapshot,
    cards: &mut CardState,
    scroll_max: f32,
) -> (Option<CardAction>, f32) {
    if snapshot.order.is_empty() {
        return (None, 0.0);
    }
    if snapshot.order.len() == 1 {
        ui.ctx()
            .data_mut(|d| d.insert_temp(egui::Id::new("approval-pile-expanded"), false));
        return render_single(ui, snapshot, cards, scroll_max);
    }
    render_pile(ui, snapshot, cards, scroll_max)
}

fn render_single(
    ui: &mut egui::Ui,
    snapshot: &Snapshot,
    cards: &mut CardState,
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
                        let (item_action, response) =
                            render_item(ui, item, snapshot, cards, card_width);
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
                            fired = Some(close.into());
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

/// Dismisses every card the pile is showing; `pub` so a behavioural test drives the same fan-out the header ✕ does rather than deciding each card itself.
pub fn close_all(state: &WindowState, snapshot: &Snapshot) {
    let mut had_inform = false;
    for item in &snapshot.order {
        match close_action(item, snapshot) {
            // Informs are cleared in one shot below, since dismissing them by index in a loop shifts the indices still to come.
            Some(Dismissal::Inform { .. }) => had_inform = true,
            Some(dismissal) => apply_dismissal(state, &dismissal),
            None => {}
        }
    }
    if had_inform {
        state.clear_informs();
    }
}

/// The single place a closed card becomes a non-decision, shared by the per-card ✕ and the pile's close-all.
fn apply_dismissal(state: &WindowState, dismissal: &Dismissal) {
    match dismissal {
        Dismissal::Network { id } => {
            state.dismiss(id);
        }
        Dismissal::Inform { index } => {
            state.dismiss_inform(*index);
        }
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
            Stroke::new(1.0_f32, window::BORDER.gamma_multiply(a)),
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
    cards: &mut CardState,
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
                render_item(ui, &order[i], snapshot, cards, w)
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
        return Some(close.into());
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
    label: &str,
) -> (egui::Response, egui::Rect) {
    use egui::{Color32, CornerRadius, Sense, Stroke, StrokeKind};

    let resp = ui.interact(rect, id, Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
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
        Stroke::new(1.0_f32, Color32::from_white_alpha(stroke_a)),
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

    let (show_less, content) = pile_pill(
        ui,
        ctx,
        pill_rect,
        Id::new("approval-pile-showless"),
        t,
        label,
    );
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

    let (close_all, content) = pile_pill(
        ui,
        ctx,
        close_rect,
        Id::new("approval-pile-closeall"),
        t,
        CLOSE_ALL_LABEL,
    );
    let arm = content.width() * 0.18;
    let c = content.center();
    let x = Stroke::new(1.6_f32, fg);
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
    cards: &mut CardState,
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
        cards,
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
    cards: &mut CardState,
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
            match &prompt.offer {
                Some(offer) => {
                    let draft = cards.drafts.entry(prompt.id.clone()).or_default();
                    render_connector_card(ui, prompt, offer, draft, width)
                }
                None => {
                    let flag = cards.remember.entry(prompt.id.clone()).or_default();
                    render_network_card(ui, prompt, flag, width)
                }
            }
        }
    }
}

fn render_inform_content(ui: &mut egui::Ui, msg: &str) {
    use egui::RichText;

    ui.horizontal(|ui| {
        ui.colored_label(window::STATUS_WARNING, "⚠");
        ui.colored_label(window::TEXT_PRIMARY, RichText::new(msg).size(12.0));
    });
}

fn close_action(item: &StackItem, snapshot: &Snapshot) -> Option<Dismissal> {
    match *item {
        StackItem::Inform(i) => Some(Dismissal::Inform { index: i }),
        StackItem::Network(i) => Some(Dismissal::Network {
            id: snapshot.pending[i].id.clone(),
        }),
    }
}

const CLOSE_LABEL: &str = "Dismiss";
const CLOSE_ALL_LABEL: &str = "Dismiss all";

fn close_button(ui: &mut egui::Ui, id: egui::Id, rect: egui::Rect) -> egui::Response {
    use egui::{Color32, Sense, Stroke, vec2};

    let resp = ui.interact(rect, id, Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, CLOSE_LABEL));
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
    let x = Stroke::new(1.6_f32, Color32::from_gray(235));
    painter.line_segment([center - vec2(arm, arm), center + vec2(arm, arm)], x);
    painter.line_segment([center + vec2(arm, -arm), center - vec2(arm, -arm)], x);
    resp
}

fn run_identity_line(ui: &mut egui::Ui, run: Option<&String>) {
    use egui::RichText;

    let Some(name) = run else { return };
    ui.add_space(5.0);
    ui.label(
        RichText::new(name)
            .size(theme::FONT_CAPTION)
            .strong()
            .color(window::TEXT_PRIMARY),
    );
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
            run_identity_line(ui, prompt.run.as_ref());
            ui.add_space(6.0);
            ui.label(
                RichText::new(format!("Connect to {}?", prompt.host))
                    .size(theme::FONT_TITLE)
                    .strong()
                    .color(window::TEXT_ACCENT),
            );
            ui.add_space(8.0);
            crate::ui::badges(ui, prompt.badges());
            if let Some(caption) = prompt.caption() {
                ui.add_space(6.0);
                ui.label(
                    RichText::new(caption)
                        .size(theme::FONT_CAPTION)
                        .color(window::TEXT_MUTED),
                );
            }
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

/// The card §3.2.4 requires: what applying the method will do, which connection it uses, and the two answers a project can give.
fn render_connector_card(
    ui: &mut egui::Ui,
    prompt: &PendingPrompt,
    offer: &lns_ipc::ConnectorView,
    draft: &mut OfferDraft,
    width: f32,
) -> (Option<CardAction>, egui::Response) {
    use egui::RichText;

    let id = prompt.id.clone();
    let method = chosen_method(offer, draft);
    let method_name = method.map(|method| method.name.clone()).unwrap_or_default();
    // Shared because both closures run inside one call: the body decides readiness from what the user just did, the footer draws the button from it.
    let ready = std::cell::Cell::new(false);
    let out = crate::ui::card_sectioned(
        ui,
        width,
        |ui| {
            crate::ui::eyebrow(ui, egui_material_icons::icons::ICON_PUBLIC, "CONNECTOR");
            run_identity_line(ui, prompt.run.as_ref());
            ui.add_space(6.0);
            ui.label(
                RichText::new(format!("Connect {} with {}?", prompt.host, offer.name))
                    .size(theme::FONT_TITLE)
                    .strong()
                    .color(window::TEXT_ACCENT),
            );
            ui.add_space(8.0);
            crate::ui::badges(ui, prompt.badges());
            match method {
                Some(method) => {
                    render_connection_choice(ui, offer, method, draft);
                    render_disclosure(ui, method);
                    ready.set(ready_to_grant(method, draft));
                }
                // §3.2.2: the card names what needed a newer lns, and keeps `Never here` — the hold outranks an ordinary allow, so declining is the only answer that ends it.
                None => render_needs_a_newer_lns(ui, offer),
            }
        },
        |ui| {
            ui.add_space(BTN_GAP);
            let mut chosen = None;
            let ready = ready.get();
            ui.columns(2, |cols| {
                if enabled_primary_button(&mut cols[0], "Connect", ready).clicked() && ready {
                    chosen = Some(ConnectorChoice::Grant);
                }
                if deny_button(&mut cols[1], "Never here").clicked() {
                    chosen = Some(ConnectorChoice::Decline);
                }
            });
            chosen
        },
    );
    let action = out.inner.map(|choice| match choice {
        ConnectorChoice::Grant => CardAction::Grant {
            id: id.clone(),
            method: method_name,
            connection: connection_choice(draft),
        },
        ConnectorChoice::Decline => CardAction::Decline { id },
    });
    (action, out.response)
}

enum ConnectorChoice {
    Grant,
    Decline,
}

/// The method this card is offering: the one the draft names, else the first this version can offer.
fn chosen_method<'a>(
    offer: &'a lns_ipc::ConnectorView,
    draft: &OfferDraft,
) -> Option<&'a lns_ipc::ConnectorMethodView> {
    let offerable = || offer.methods.iter().find(|method| method.offerable);
    match &draft.method {
        Some(named) => offer.methods.iter().find(|method| &method.name == named),
        None => offerable(),
    }
}

/// Which connection the grant is made with, every one this connector holds, because §3.2.4 makes the choice and its authority part of the disclosure.
fn render_connection_choice(
    ui: &mut egui::Ui,
    offer: &lns_ipc::ConnectorView,
    method: &lns_ipc::ConnectorMethodView,
    draft: &mut OfferDraft,
) {
    if method.auth_label.is_none() {
        return;
    }
    let held: Vec<&lns_ipc::ConnectorConnectionView> = offer
        .connections
        .iter()
        .filter(|connection| connection.method == method.name)
        .collect();
    // Nothing to choose between, so the card asks for the one thing it needs instead of offering a choice of one.
    if held.is_empty() {
        // Once, not every frame: refilling would type the suggestion back under the cursor of a user clearing the name.
        if !draft.connecting {
            begin_connecting(method, draft, offer);
        }
        render_new_connection(ui, method, draft);
        return;
    }
    if draft.connection.is_none() && !draft.connecting {
        draft.connection = Some(held[0].label.clone());
    }
    ui.add_space(14.0);
    section_label(ui, "CONNECTION");
    ui.add_space(6.0);
    let picked = crate::ui::chips(
        ui,
        held.iter()
            .map(|connection| {
                (
                    connection.label.as_str(),
                    !draft.connecting
                        && draft.connection.as_deref() == Some(connection.label.as_str()),
                )
            })
            .chain(std::iter::once((NEW_CONNECTION, draft.connecting))),
    );
    match picked.as_deref() {
        Some(NEW_CONNECTION) => begin_connecting(method, draft, offer),
        Some(label) => {
            draft.connection = Some(label.to_string());
            draft.connecting = false;
        }
        None => {}
    }
    render_chosen_authority(ui, &held, draft);
    render_new_connection(ui, method, draft);
}

/// Introduces a group so its controls are not read as more of the badges above them.
fn section_label(ui: &mut egui::Ui, text: &str) {
    ui.label(
        egui::RichText::new(text)
            .size(theme::FONT_EYEBROW)
            .color(window::TEXT_MUTED),
    );
}

/// The chip that starts a new connection instead of picking one already held. "connection" throughout, because that is the word `lns connector` and the spec both use.
const NEW_CONNECTION: &str = "+ new";

/// What the chosen connection can reach, which §3.2.4 makes part of the disclosure — under the chips, because only the chosen one is being granted.
fn render_chosen_authority(
    ui: &mut egui::Ui,
    held: &[&lns_ipc::ConnectorConnectionView],
    draft: &OfferDraft,
) {
    if draft.connecting {
        return;
    }
    let Some(chosen) = held
        .iter()
        .find(|connection| draft.connection.as_deref() == Some(connection.label.as_str()))
        .filter(|connection| !connection.authority.is_empty())
    else {
        return;
    };
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new(chosen.authority.join(", "))
            .size(theme::FONT_CAPTION)
            .color(window::TEXT_MUTED),
    );
}

/// The connect entry, expanded in place: one field per credential the method declares, plus the name this connection is kept under.
fn render_new_connection(
    ui: &mut egui::Ui,
    method: &lns_ipc::ConnectorMethodView,
    draft: &mut OfferDraft,
) {
    use egui::RichText;

    if !draft.connecting {
        return;
    }
    if let Some(help) = &method.help {
        ui.add_space(6.0);
        ui.label(
            RichText::new(help)
                .size(theme::FONT_CAPTION)
                .color(window::TEXT_MUTED),
        );
    }
    ui.add_space(6.0);
    ui.add(
        egui::TextEdit::singleline(&mut draft.label)
            .hint_text("Name this connection")
            .margin(egui::Margin::symmetric(10, 9))
            .desired_width(f32::INFINITY),
    );
    let asked_for = method.auth_label.as_deref().unwrap_or_default();
    for ask in &method.asks {
        ui.add_space(6.0);
        let value = draft.values.entry(ask.clone()).or_default();
        secret_input(ui, value, asked_for);
    }
}

/// Suggests a name nothing already holds, because reusing one silently replaces the connection under it — counting them is not enough, since disconnecting one leaves its successor's name taken.
fn begin_connecting(
    method: &lns_ipc::ConnectorMethodView,
    draft: &mut OfferDraft,
    offer: &lns_ipc::ConnectorView,
) {
    draft.connecting = true;
    draft.connection = None;
    if draft.label.is_empty() {
        draft.label = offer.free_connection_name(&method.name);
    }
}

fn secret_input(ui: &mut egui::Ui, value: &mut String, hint: &str) -> egui::Response {
    ui.scope(|ui| {
        ui.style_mut().visuals.widgets.inactive.bg_stroke =
            egui::Stroke::new(1.0_f32, window::BORDER);
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

/// What applying this method will do, each line named by §3.2.4 and omitted when there is nothing to say.
fn render_disclosure(ui: &mut egui::Ui, method: &lns_ipc::ConnectorMethodView) {
    for (label, items) in [
        ("Opens", &method.opens),
        ("Sets", &method.env),
        ("Writes", &method.writes),
    ] {
        if items.is_empty() {
            continue;
        }
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(format!("{label}: {}", items.join(", ")))
                .size(theme::FONT_CAPTION)
                .color(window::TEXT_MUTED),
        );
    }
}

/// Names the methods this version cannot deliver, so the card says why `Connect` is dead rather than looking broken (§3.2.2).
fn render_needs_a_newer_lns(ui: &mut egui::Ui, offer: &lns_ipc::ConnectorView) {
    let named: Vec<&str> = offer
        .methods
        .iter()
        .filter(|method| !method.offerable)
        .map(|method| method.name.as_str())
        .collect();
    // A method that writes files is unsupported rather than out of date, so "update lns" would send the user somewhere that does not help.
    let why = if offer.methods.iter().any(|method| !method.writes.is_empty()) {
        "writes files, which this lns cannot deliver yet"
    } else {
        "needs a newer lns"
    };
    ui.add_space(10.0);
    ui.label(
        egui::RichText::new(format!(
            "{} {why}. Nothing here can be connected — choose Never here.",
            named.join(", ")
        ))
        .size(theme::FONT_CAPTION)
        .color(window::TEXT_MUTED),
    );
}

/// A method that authenticates cannot be granted until there is a connection behind it.
fn ready_to_grant(method: &lns_ipc::ConnectorMethodView, draft: &OfferDraft) -> bool {
    if method.auth_label.is_none() {
        return true;
    }
    if !draft.connecting {
        return draft.connection.is_some();
    }
    !draft.label.trim().is_empty()
        && method
            .asks
            .iter()
            .all(|ask| draft.values.get(ask).is_some_and(|v| !v.trim().is_empty()))
}

fn connection_choice(draft: &OfferDraft) -> ConnectionChoice {
    if draft.connecting {
        return ConnectionChoice::New {
            label: draft.label.trim().to_string(),
            values: lns_ipc::SecretValues(draft.values.clone()),
        };
    }
    match &draft.connection {
        Some(label) => ConnectionChoice::Held(label.clone()),
        None => ConnectionChoice::None,
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
                Stroke::new(1.5_f32, border),
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
                    Stroke::new(1.7_f32, window::BG_PRIMARY),
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

fn primary_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    enabled_primary_button(ui, label, true)
}

fn enabled_primary_button(ui: &mut egui::Ui, label: &str, enabled: bool) -> egui::Response {
    Button::new(label, ButtonKind::Primary)
        .enabled(enabled)
        .min_size(egui::vec2(ui.available_width(), 0.0))
        .show(ui)
}

fn deny_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    Button::new(label, ButtonKind::Danger)
        .min_size(egui::vec2(ui.available_width(), 0.0))
        .show(ui)
}

/// What the render thread keeps per card between frames: a toggle, and any half-typed connection. The draft lives here and never in shared state, so a value being typed exists only in this thread.
#[derive(Default)]
pub struct CardState {
    remember: HashMap<String, bool>,
    drafts: HashMap<String, OfferDraft>,
}

impl CardState {
    /// Drops what belongs to cards that have left, so a draft cannot outlive the request it answers.
    fn prune(&mut self, snapshot: &Snapshot) {
        let live: std::collections::HashSet<&str> =
            snapshot.pending.iter().map(|p| p.id.as_str()).collect();
        self.remember.retain(|id, _| live.contains(id.as_str()));
        self.drafts.retain(|id, _| live.contains(id.as_str()));
    }
}

/// A connector card being filled in: which method, which connection, and any connection the card is creating.
#[derive(Default)]
pub struct OfferDraft {
    method: Option<String>,
    connection: Option<String>,
    connecting: bool,
    label: String,
    values: std::collections::BTreeMap<String, String>,
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
    use crate::approval_flow::protocol::Treatment;

    fn reveal_commands() -> Vec<egui::ViewportCommand> {
        let ctx = egui::Context::default();
        let mut placement = ViewportPlacement::new();
        let mut input = egui::RawInput::default();
        input
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .monitor_size = Some(egui::vec2(1920.0, 1080.0));
        let output = ctx.run_ui(input, |ctx| {
            placement.sync_visibility(ctx, &[StackItem::Network(0)]);
        });
        output.viewport_output[&egui::ViewportId::ROOT]
            .commands
            .clone()
    }

    #[test]
    fn revealing_the_approval_window_shows_it() {
        assert!(
            reveal_commands().contains(&egui::ViewportCommand::Visible(true)),
            "a raised card must reveal the window"
        );
    }

    #[test]
    fn revealing_the_approval_window_does_not_take_keyboard_focus() {
        let commands = reveal_commands();
        assert!(
            !commands.contains(&egui::ViewportCommand::Focus),
            "the window must appear beside what the developer types in, not steal the keyboard: {commands:?}"
        );
    }

    #[test]
    fn the_approval_window_never_asks_to_be_the_active_window() {
        assert_eq!(
            approval_viewport().active,
            Some(false),
            "an approval window that activates on creation steals focus from the foreground app"
        );
    }

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
    fn lns_headless_forces_the_display_off_on_any_platform() {
        assert!(!display_present_with(
            |key| (key == "LNS_HEADLESS").then(|| std::ffi::OsString::from("1"))
        ));
        assert!(!display_present_with(|key| match key {
            "LNS_HEADLESS" => Some(std::ffi::OsString::from("true")),
            _ => Some(std::ffi::OsString::from(":0")),
        }));
    }

    #[test]
    fn an_empty_or_zero_lns_headless_does_not_force_headless() {
        let with = |value: &'static str| {
            move |key: &str| match key {
                "LNS_HEADLESS" => Some(std::ffi::OsString::from(value)),
                "DISPLAY" => Some(std::ffi::OsString::from(":0")),
                _ => None,
            }
        };
        assert!(display_present_with(with("")));
        assert!(display_present_with(with("0")));
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

    /// Clicks the control labelled `label` in a headless `render_stack` and returns what it fired, hovering first so hover-only affordances exist and fanning the pile out when `expanded`.
    fn click_labelled_control(
        snapshot: Snapshot,
        label: &str,
        expanded: bool,
    ) -> Option<CardAction> {
        use egui_kittest::kittest::Queryable;

        let fired: Arc<Mutex<Option<CardAction>>> = Arc::new(Mutex::new(None));
        let sink = fired.clone();
        let mut cards = CardState::default();

        // egui applies added fonts at the next pass, so the icon font goes in on a pass that draws nothing — otherwise a card's eyebrow icon panics on an unbound family.
        let mut prepared = false;
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(520.0, 1200.0))
            .build_ui(move |ui| {
                if !prepared {
                    crate::approval_flow::window::install_icon_font(ui.ctx());
                    ui.ctx().data_mut(|d| {
                        d.insert_temp(egui::Id::new("approval-pile-expanded"), expanded);
                    });
                    prepared = true;
                    return;
                }
                let (action, _) = render_stack(ui, &snapshot, &mut cards, 1000.0);
                if let Some(action) = action {
                    *sink.lock().expect("action sink poisoned") = Some(action);
                }
            });

        harness.run();
        harness
            .input_mut()
            .events
            .push(egui::Event::PointerMoved(egui::pos2(120.0, 60.0)));
        harness.run();
        harness.get_by_label(label).click();
        harness.run();

        fired.lock().expect("action sink poisoned").take()
    }

    fn offered_prompt(
        method: &str,
        connections: &[(&str, &[&str])],
        credentials: &[&str],
    ) -> Snapshot {
        Snapshot {
            pending: vec![PendingPrompt {
                id: "r1".into(),
                host: "api.some-provider.example".into(),
                action: "CONNECT api.some-provider.example:443".into(),
                treatment: Treatment::Inspected,
                run: Some("my-agent".into()),
                offer: Some(lns_ipc::ConnectorView {
                    name: "some-provider".into(),
                    digest: "sha256:abc".into(),
                    serves: vec!["api.some-provider.example".into()],
                    methods: vec![lns_ipc::ConnectorMethodView {
                        name: method.into(),
                        label: method.into(),
                        auth_label: (!credentials.is_empty()).then(|| "token".to_string()),
                        offerable: true,
                        opens: vec!["api.some-provider.example".into()],
                        writes: Vec::new(),
                        env: Vec::new(),
                        credentials: credentials.iter().map(|c| c.to_string()).collect(),
                        asks: credentials.iter().map(|c| c.to_string()).collect(),
                        help: None,
                    }],
                    connections: connections
                        .iter()
                        .map(|(label, authority)| lns_ipc::ConnectorConnectionView {
                            label: label.to_string(),
                            method: method.into(),
                            authority: authority.iter().map(|a| a.to_string()).collect(),
                        })
                        .collect(),
                }),
            }],
            informs: Vec::new(),
            order: vec![StackItem::Network(0)],
        }
    }

    #[test]
    fn a_served_destination_offers_to_connect_rather_than_to_allow() {
        // The whole point of the hold: the question is which connector to use, not whether to let the bytes through.
        let fired = click_labelled_control(offered_prompt("open", &[], &[]), "Connect", false);
        assert_eq!(
            fired,
            Some(CardAction::Grant {
                id: "r1".into(),
                method: "open".into(),
                connection: ConnectionChoice::None,
            }),
            "a method that does not authenticate is granted with no connection behind it"
        );
    }

    #[test]
    fn the_card_grants_with_the_connection_it_defaulted_to() {
        let fired = click_labelled_control(
            offered_prompt(
                "token",
                &[("work", &["repo"]), ("personal", &[])],
                &["SOME_TOKEN"],
            ),
            "Connect",
            false,
        );
        assert_eq!(
            fired,
            Some(CardAction::Grant {
                id: "r1".into(),
                method: "token".into(),
                connection: ConnectionChoice::Held("work".into()),
            }),
            "with several connections held the card picks the first and says which, rather than granting nameless"
        );
    }

    #[test]
    fn the_connection_the_card_chose_is_the_one_the_grant_names() {
        // The user's whole reason for the radio list: two connections held, and this run needs the second.
        let chosen = OfferDraft {
            connection: Some("personal".into()),
            ..OfferDraft::default()
        };
        assert_eq!(
            connection_choice(&chosen),
            ConnectionChoice::Held("personal".into())
        );
    }

    #[test]
    fn a_connection_being_made_carries_its_values_rather_than_a_name_that_is_not_stored_yet() {
        // The label names a connection that does not exist until the connect runs, so sending it as a held one would refuse.
        let typing = OfferDraft {
            connecting: true,
            label: "  token-2  ".into(),
            values: [("SOME_TOKEN".to_string(), "sk-live".to_string())]
                .into_iter()
                .collect(),
            connection: Some("work".into()),
            ..OfferDraft::default()
        };
        assert_eq!(
            connection_choice(&typing),
            ConnectionChoice::New {
                label: "token-2".into(),
                values: lns_ipc::SecretValues(
                    [("SOME_TOKEN".to_string(), "sk-live".to_string())]
                        .into_iter()
                        .collect()
                ),
            },
            "a connection in progress wins over whatever was selected before it, and its name is trimmed"
        );
    }

    /// True when a control with this label is on screen, driven through the same headless harness as `click_labelled_control`.
    fn control_exists(snapshot: Snapshot, label: &str) -> bool {
        use egui_kittest::kittest::Queryable;
        let mut cards = CardState::default();
        let mut prepared = false;
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(520.0, 1200.0))
            .build_ui(move |ui| {
                if !prepared {
                    crate::approval_flow::window::install_icon_font(ui.ctx());
                    prepared = true;
                    return;
                }
                render_stack(ui, &snapshot, &mut cards, 1000.0);
            });
        harness.run();
        harness.run();
        harness.query_by_label(label).is_some()
    }

    #[test]
    fn a_method_with_no_connection_of_its_own_goes_straight_to_the_connect_form() {
        // The connector holds a connection, but not for this method: offering "Connect a new one" would be a link to swap a connection the user does not have.
        let mut snapshot = offered_prompt("session", &[], &["SESSION_TOKEN"]);
        let offer = snapshot.pending[0].offer.as_mut().expect("an offer");
        // Held under the *other* method, and named after this one: the store keys a connection by connector and label alone, so the suggestion must step past it.
        offer.connections.push(lns_ipc::ConnectorConnectionView {
            label: "session".into(),
            method: "token".into(),
            authority: Vec::new(),
        });

        assert_eq!(
            form_state(snapshot, "session-2"),
            (false, true),
            "no chip to swap a connection the user does not have, and the form open with a free name already in it"
        );
    }

    /// Whether the card offers another connection to switch to, and whether the connect form is open with the suggested name in its field.
    fn form_state(snapshot: Snapshot, suggested: &str) -> (bool, bool) {
        use egui_kittest::kittest::Queryable;
        let mut cards = CardState::default();
        let mut prepared = false;
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(520.0, 1200.0))
            .build_ui(move |ui| {
                if !prepared {
                    crate::approval_flow::window::install_icon_font(ui.ctx());
                    prepared = true;
                    return;
                }
                render_stack(ui, &snapshot, &mut cards, 1000.0);
            });
        harness.run();
        harness.run();
        (
            harness.query_all_by_label(NEW_CONNECTION).next().is_some(),
            // by_all rather than by_one: a text input exposes its value on the field and again on its inner text node.
            harness.query_all_by_value(suggested).next().is_some(),
        )
    }

    #[test]
    fn a_method_that_already_holds_a_connection_offers_the_choice_first() {
        // Every connection is a chip, so the one being granted is visible without opening anything.
        let snapshot = offered_prompt(
            "token",
            &[("work", &[]), ("personal", &[])],
            &["SOME_TOKEN"],
        );
        for chip in ["work", "personal", NEW_CONNECTION] {
            assert!(control_exists(snapshot.clone(), chip), "{chip}");
        }
    }

    /// Clicks `chip`, then `Connect`, in one card — so the second click sees what the first chose.
    fn pick_then_connect(snapshot: Snapshot, chip: &str) -> Option<CardAction> {
        use egui_kittest::kittest::Queryable;
        let fired: Arc<Mutex<Option<CardAction>>> = Arc::new(Mutex::new(None));
        let sink = fired.clone();
        let mut cards = CardState::default();
        let mut prepared = false;
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(520.0, 1200.0))
            .build_ui(move |ui| {
                if !prepared {
                    crate::approval_flow::window::install_icon_font(ui.ctx());
                    prepared = true;
                    return;
                }
                if let (action, _) = render_stack(ui, &snapshot, &mut cards, 1000.0)
                    && let Some(action) = action
                {
                    *sink.lock().expect("sink poisoned") = Some(action);
                }
            });
        harness.run();
        harness.run();
        harness.get_by_label(chip).click();
        harness.run();
        harness.get_by_label("Connect").click();
        harness.run();
        fired.lock().expect("sink poisoned").take()
    }

    #[test]
    fn granting_carries_the_connection_whose_chip_was_clicked() {
        // The whole reason for a selector: two connections held, and this run needs the second.
        let snapshot = offered_prompt(
            "token",
            &[("work", &["repo"]), ("personal", &[])],
            &["SOME_TOKEN"],
        );

        assert_eq!(
            pick_then_connect(snapshot, "personal"),
            Some(CardAction::Grant {
                id: "r1".into(),
                method: "token".into(),
                connection: ConnectionChoice::Held("personal".into()),
            })
        );
    }

    #[test]
    fn the_chosen_connections_authority_is_disclosed_under_its_chip() {
        // §3.2.4 makes the connection's authority part of what the card discloses, and the chosen one is the connection the grant applies — so it must follow the choice, not sit on whichever connection happens to be first.
        let held: &[(&str, &[&str])] = &[("work", &["repo", "issues"]), ("personal", &[])];
        assert!(control_exists(
            offered_prompt("token", held, &["SOME_TOKEN"]),
            "repo, issues"
        ));

        let after_choosing_the_other =
            scopes_after_picking(offered_prompt("token", held, &["SOME_TOKEN"]), "personal");

        assert!(
            !after_choosing_the_other,
            "the other connection grants nothing named, so nothing is disclosed"
        );
    }

    /// Whether `repo, issues` is still on the card once `chip` is the chosen connection.
    fn scopes_after_picking(snapshot: Snapshot, chip: &str) -> bool {
        use egui_kittest::kittest::Queryable;
        let mut cards = CardState::default();
        let mut prepared = false;
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(520.0, 1200.0))
            .build_ui(move |ui| {
                if !prepared {
                    crate::approval_flow::window::install_icon_font(ui.ctx());
                    prepared = true;
                    return;
                }
                render_stack(ui, &snapshot, &mut cards, 1000.0);
            });
        harness.run();
        harness.run();
        harness.get_by_label(chip).click();
        harness.run();
        harness.query_all_by_label("repo, issues").next().is_some()
    }

    #[test]
    fn a_name_the_user_clears_stays_cleared() {
        // The suggestion is offered once. Refilling it every frame types it back under the cursor of a user who is replacing it.
        let snapshot = offered_prompt("token", &[], &["SOME_TOKEN"]);
        let mut cards = CardState::default();
        let mut prepared = false;
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(520.0, 1200.0))
            .build_ui(move |ui| {
                if !prepared {
                    crate::approval_flow::window::install_icon_font(ui.ctx());
                    prepared = true;
                    return;
                }
                render_stack(ui, &snapshot, &mut cards, 1000.0);
            });
        harness.run();
        harness.run();

        use egui_kittest::kittest::Queryable;
        harness
            .query_all_by_value("token")
            .next()
            .expect("the suggested name")
            .focus();
        for _ in 0.."token".len() {
            harness.key_press(egui::Key::Backspace);
            harness.run();
        }

        assert!(
            harness.query_all_by_value("token").next().is_none(),
            "the field the user emptied is still empty"
        );
    }

    #[test]
    fn a_connector_this_version_cannot_offer_still_has_an_answer() {
        // The hold outranks an ordinary allow, so falling back to the network card would leave a destination that asks on every request forever. `Never here` is the only answer that clears it.
        let mut snapshot = offered_prompt("oauth", &[], &[]);
        let offer = snapshot.pending[0].offer.as_mut().expect("an offer");
        offer.methods[0].offerable = false;

        assert_eq!(
            click_labelled_control(snapshot, "Never here", false),
            Some(CardAction::Decline { id: "r1".into() })
        );
    }

    #[test]
    fn a_connector_this_version_cannot_offer_cannot_be_connected() {
        let mut snapshot = offered_prompt("oauth", &[], &[]);
        let offer = snapshot.pending[0].offer.as_mut().expect("an offer");
        offer.methods[0].offerable = false;

        assert_eq!(
            click_labelled_control(snapshot, "Connect", false),
            None,
            "there is nothing this version knows how to apply"
        );
    }

    #[test]
    fn never_here_answers_for_the_project_rather_than_the_request() {
        assert_eq!(
            click_labelled_control(offered_prompt("open", &[], &[]), "Never here", false),
            Some(CardAction::Decline { id: "r1".into() }),
            "§3.2.4: declining is a standing no for this project, not a deny of this one request"
        );
    }

    #[test]
    fn a_method_needing_a_connection_cannot_be_granted_until_there_is_one() {
        // Granting first would refuse at the store, after the user already consented.
        let waiting = OfferDraft {
            connecting: true,
            label: "token".into(),
            ..OfferDraft::default()
        };
        let method = lns_ipc::ConnectorMethodView {
            name: "token".into(),
            label: "token".into(),
            auth_label: Some("token".into()),
            offerable: true,
            opens: Vec::new(),
            writes: Vec::new(),
            env: Vec::new(),
            credentials: vec!["SOME_TOKEN".into()],
            asks: vec!["SOME_TOKEN".into()],
            help: None,
        };
        assert!(
            !ready_to_grant(&method, &waiting),
            "no value typed yet, so there is nothing to connect with"
        );

        let filled = OfferDraft {
            values: [("SOME_TOKEN".to_string(), "sk-live".to_string())]
                .into_iter()
                .collect(),
            ..waiting
        };
        assert!(ready_to_grant(&method, &filled));
    }

    #[test]
    fn a_new_connection_is_named_something_not_already_taken() {
        // §7.1: a colliding label silently replaces the connection already under it, which is the one outcome a second connection must not produce.
        let method = lns_ipc::ConnectorMethodView {
            name: "token".into(),
            label: "token".into(),
            auth_label: Some("token".into()),
            offerable: true,
            opens: Vec::new(),
            writes: Vec::new(),
            env: Vec::new(),
            credentials: Vec::new(),
            asks: Vec::new(),
            help: None,
        };
        let mut first = OfferDraft::default();
        begin_connecting(&method, &mut first, &holding(&[]));
        assert_eq!(first.label, "token");

        let mut second = OfferDraft::default();
        begin_connecting(&method, &mut second, &holding(&["token"]));
        assert_eq!(
            second.label, "token-2",
            "the rule itself is pinned in lns-ipc; this is the card asking for it"
        );

        let mut typed = OfferDraft {
            label: "personal".into(),
            ..OfferDraft::default()
        };
        begin_connecting(&method, &mut typed, &holding(&["token"]));
        assert_eq!(
            typed.label, "personal",
            "a name the user already typed is not overwritten by the suggestion"
        );
    }

    fn holding(labels: &[&str]) -> lns_ipc::ConnectorView {
        lns_ipc::ConnectorView {
            name: "some-provider".into(),
            digest: "sha256:abc".into(),
            serves: Vec::new(),
            methods: Vec::new(),
            connections: labels
                .iter()
                .map(|label| lns_ipc::ConnectorConnectionView {
                    label: label.to_string(),
                    method: "token".into(),
                    authority: Vec::new(),
                })
                .collect(),
        }
    }

    #[test]
    fn a_draft_does_not_outlive_the_card_it_was_typed_into() {
        // A half-typed value must not sit in memory once its request is gone.
        let mut cards = CardState::default();
        cards.drafts.insert(
            "r1".into(),
            OfferDraft {
                values: [("SOME_TOKEN".to_string(), "sk-live".to_string())]
                    .into_iter()
                    .collect(),
                ..OfferDraft::default()
            },
        );
        cards.remember.insert("r1".into(), true);

        cards.prune(&Snapshot {
            pending: Vec::new(),
            informs: Vec::new(),
            order: Vec::new(),
        });

        assert!(cards.drafts.is_empty() && cards.remember.is_empty());
    }

    #[test]
    fn the_close_all_pill_fires_a_close_all_rather_than_a_per_card_decision() {
        assert_eq!(
            click_labelled_control(pile_seed(), CLOSE_ALL_LABEL, true),
            Some(CardAction::CloseAll),
            "the pile header's ✕ must route through close_all, not decide each card where no test can see it"
        );
    }

    #[test]
    fn closing_a_network_card_decides_nothing() {
        let snapshot = Snapshot {
            pending: vec![PendingPrompt {
                id: "r1".into(),
                host: "api.example.test".into(),
                action: "CONNECT api.example.test:443".into(),
                treatment: Treatment::Inspected,
                run: Some("some-run".into()),
                offer: None,
            }],
            informs: Vec::new(),
            order: vec![StackItem::Network(0)],
        };

        assert_eq!(
            close_action(&StackItem::Network(0), &snapshot),
            Some(Dismissal::Network { id: "r1".into() }),
            "a dismissed card must not be recorded in the audit chain as a deny-once the developer picked"
        );
    }

    #[test]
    fn closing_every_card_at_once_decides_nothing() {
        let state = WindowState::new();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        state.insert_pending(
            PendingPrompt {
                id: "r1".into(),
                host: "api.example.test".into(),
                action: "CONNECT api.example.test:443".into(),
                treatment: Treatment::Inspected,
                run: Some("some-run".into()),
                offer: None,
            },
            tx,
        );

        close_all(&state, &state.snapshot());

        let delivery = rx
            .try_recv()
            .expect("close-all must resolve every held request");
        assert_eq!(
            delivery.action,
            crate::approval_flow::window::RequestAction::Dismiss,
            "one click on close-all must not permanently deny every held request in the stack"
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
        let net = |id: &str, host: &str| PendingPrompt {
            id: id.into(),
            host: host.into(),
            action: format!("CONNECT {host}:443"),
            treatment: Treatment::Inspected,
            run: Some("some-run".into()),
            offer: None,
        };
        Snapshot {
            pending: vec![net("n0", "a.test"), net("n1", "b.test")],
            informs: vec!["warn".into()],
            order: vec![
                StackItem::Network(0),
                StackItem::Network(1),
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
        let mut rem = CardState::default();
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
                render_stack(ui, &snapshot, &mut rem, 6000.0);
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
        assert_eq!(target_height(&[], Some(1080.0)), MIN_WINDOW_HEIGHT);
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
        let one = target_height(&[StackItem::Network(0)], Some(2000.0));
        let two = target_height(
            &[StackItem::Network(0), StackItem::Network(1)],
            Some(2000.0),
        );
        assert!(
            two > one,
            "a second concurrent card makes the window taller rather than hiding the first"
        );
    }

    #[test]
    fn target_height_caps_a_long_stack_at_the_usable_monitor_height() {
        let many: Vec<StackItem> = (0..40).map(StackItem::Network).collect();
        assert_eq!(
            target_height(&many, Some(900.0)),
            900.0 - 2.0 * SCREEN_EDGE_MARGIN,
            "a long stack scrolls inside a screen-bounded window instead of running off-screen"
        );
    }

    #[test]
    fn target_height_falls_back_to_a_fixed_cap_without_a_known_monitor() {
        let many: Vec<StackItem> = (0..40).map(StackItem::Network).collect();
        assert_eq!(target_height(&many, None), FALLBACK_MAX_HEIGHT);
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
