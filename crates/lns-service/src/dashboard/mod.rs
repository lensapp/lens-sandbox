pub mod approvals;
mod filter;
mod format;
pub mod live;
mod sandboxes;

pub use filter::{Filters, KINDS, visible_indices};

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, CursorIcon, FontId, Frame, Layout, Margin,
    RichText, Sense, Stroke, Vec2, vec2,
};
use egui_material_icons::{MaterialIcon, icons};
use lns_audit::TimelineRow;

use crate::approval_flow::entries::Entry;
use crate::approval_flow::window::{
    ACCENT_GREEN, BG_PRIMARY, BG_SECONDARY, BG_TERTIARY, BORDER, CATEGORY, STATUS_CRITICAL,
    STATUS_WARNING, TEXT_MUTED, TEXT_PRIMARY,
};
use crate::ui::theme;

// Two lists under one panel share a scroll id, so each list names its own.
const TIMELINE_LIST: &str = "timeline-list";
const APPROVALS_LIST: &str = "approvals-list";
const SIDEBAR_LIST: &str = "sidebar-sandboxes";

const TRAFFIC_LIGHT_INSET: f32 = 80.0;
const SIDEBAR_WIDTH: f32 = 216.0;
const ROW_HEIGHT: f32 = 26.0;
const ICON_COL: f32 = 22.0;
const W_TIME: f32 = 92.0;
const DETAIL_WIDTH: f32 = 344.0;

const CHROME_FILL: Color32 = BG_SECONDARY;
const CONTENT_FILL: Color32 = BG_PRIMARY;
const SELECT_FILL: Color32 = BG_TERTIARY;
const INPUT_FILL: Color32 = BG_TERTIARY;
const HOVER_FILL: Color32 = Color32::from_rgb(0x26, 0x28, 0x2a);

const FS_BODY: f32 = 15.0;
const FS_SECONDARY: f32 = 14.0;
const FS_LABEL: f32 = 13.0;

const MODAL_FILL: Color32 = Color32::from_rgb(0x24, 0x27, 0x2a);
const WEAK_BORDER: Color32 = Color32::from_rgba_premultiplied(20, 20, 20, 20);
const HOVER_LINE: Color32 = Color32::from_rgba_premultiplied(34, 34, 34, 34);
const DRAG_LINE: Color32 = Color32::from_rgba_premultiplied(64, 64, 64, 64);

#[derive(Debug, Clone, Default)]
pub struct Sandbox {
    pub id: String,
    pub name: String,
    pub image: String,
    pub status: String,
}

/// Which of the window's two lists is on screen: what a run did, or what it was asked.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum View {
    #[default]
    Timeline,
    Approvals,
}

#[derive(Debug, Default)]
pub struct DashboardState {
    pub view: View,
    pub approvals: Vec<Entry>,
    pub approval_notice: Option<String>,
    pub approval_answers: std::collections::BTreeSet<String>,
    /// The row the developer expanded, if any.
    pub open: Option<OpenRow>,
    pub answer_open: bool,
    pub sandbox_open: bool,
    pub archive_open: Option<bool>,
    pub rows: Vec<TimelineRow>,
    pub warnings: Vec<String>,
    pub sandboxes: Vec<Sandbox>,
    pub selected_sandbox: Option<String>,
    pub kinds: std::collections::BTreeSet<String>,
    pub kind_open: bool,
    pub kind_query: String,
    pub selected: Option<usize>,
    pub detail_row: Option<TimelineRow>,
    pub sidebar_open: bool,
    pub search_open: bool,
    pub search_query: String,
    pub last_error: Option<String>,
    pub copied: Option<(egui::Id, f64)>,
}

impl DashboardState {
    pub fn new() -> Self {
        Self {
            sidebar_open: true,
            ..Self::default()
        }
    }

    pub fn seeded(rows: Vec<TimelineRow>, warnings: Vec<String>, sandboxes: Vec<Sandbox>) -> Self {
        Self {
            rows,
            warnings,
            sandboxes,
            ..Self::new()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DashboardAction {
    None,
    Refresh,
}

pub fn background() -> Color32 {
    CONTENT_FILL
}

pub fn style(ctx: &egui::Context) {
    use egui::FontFamily::{Monospace, Proportional};
    use egui::TextStyle;
    ctx.global_style_mut(|s| {
        s.text_styles
            .insert(TextStyle::Body, FontId::new(FS_BODY, Proportional));
        s.text_styles
            .insert(TextStyle::Button, FontId::new(FS_BODY, Proportional));
        s.text_styles
            .insert(TextStyle::Small, FontId::new(FS_LABEL, Proportional));
        s.text_styles
            .insert(TextStyle::Monospace, FontId::new(FS_SECONDARY, Monospace));
        s.text_styles
            .insert(TextStyle::Heading, FontId::new(20.0, Proportional));
        s.spacing.item_spacing = vec2(10.0, 8.0);
        s.spacing.button_padding = vec2(12.0, 6.0);
    });
}

fn dashboard_visuals() -> egui::Visuals {
    let mut v = egui::Visuals::dark();
    v.override_text_color = Some(TEXT_PRIMARY);
    v.panel_fill = CONTENT_FILL;
    v.window_fill = CHROME_FILL;
    v.window_stroke = Stroke::new(1.0_f32, BORDER);
    v.window_corner_radius = CornerRadius::same(10);
    v.extreme_bg_color = INPUT_FILL;
    v.faint_bg_color = CHROME_FILL;
    v.selection.bg_fill = CATEGORY;
    v.selection.stroke = Stroke::new(0.0_f32, Color32::WHITE);
    let radius = CornerRadius::same(4);
    for w in [
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = radius;
        w.bg_stroke = Stroke::NONE;
        w.fg_stroke = Stroke::new(1.0_f32, TEXT_PRIMARY);
        w.weak_bg_fill = INPUT_FILL;
        w.bg_fill = INPUT_FILL;
    }
    v.widgets.hovered.weak_bg_fill = HOVER_FILL;
    v.widgets.hovered.bg_fill = HOVER_FILL;
    v.widgets.hovered.fg_stroke = Stroke::new(1.0_f32, HOVER_LINE);
    v.widgets.active.fg_stroke = Stroke::new(1.0_f32, DRAG_LINE);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, WEAK_BORDER);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0_f32, TEXT_PRIMARY);
    v
}

pub fn load(state: &mut DashboardState) {
    load_timeline(state);
    load_approvals(state);
}

fn load_approvals(state: &mut DashboardState) {
    match crate::cache::root() {
        Ok(root) => {
            state.approvals =
                crate::approval_flow::answering::entries(&root, &crate::run_registry::known_ids());
        }
        Err(e) => set_error(state, e),
    }
    if !approvals::still_listed(
        state.open.as_ref().map(|open| open.id.as_str()),
        &state.approvals,
    ) {
        state.open = None;
    }
}

fn load_timeline(state: &mut DashboardState) {
    let runs = match lns_ipc::audit_runs_root() {
        Ok(path) => path,
        Err(e) => return set_error(state, e),
    };
    let ledger = match lns_ipc::connection_ledger() {
        Ok(path) => path,
        Err(e) => return set_error(state, e),
    };
    match lns_audit::collect_timeline(&runs, &ledger, None) {
        Ok(timeline) => {
            state.sandboxes = sandboxes::merge_sandboxes(&active_sandboxes(), &timeline.rows);
            state.rows = timeline.rows;
            state.warnings = timeline.warnings;
            state.last_error = None;
            if state.selected.is_some_and(|i| i >= state.rows.len()) {
                state.selected = None;
            }
        }
        Err(e) => state.last_error = Some(format!("{e:#}")),
    }
}

fn active_sandboxes() -> Vec<Sandbox> {
    crate::run_registry::snapshot()
        .into_iter()
        .map(|s| Sandbox {
            id: s.id,
            name: s.name,
            image: s.image,
            status: status_word(&s.status),
        })
        .collect()
}

fn status_word(status: &lns_ipc::RunStatus) -> String {
    match status {
        lns_ipc::RunStatus::Running => "running".to_string(),
        lns_ipc::RunStatus::Exited { .. } => "exited".to_string(),
    }
}

pub fn apply_theme(ctx: &egui::Context) {
    style(ctx);
    ctx.set_visuals(dashboard_visuals());
}

pub fn viewport_builder() -> egui::ViewportBuilder {
    egui::ViewportBuilder::default()
        .with_title("LNS")
        .with_fullsize_content_view(true)
        .with_titlebar_shown(false)
        .with_title_shown(false)
        .with_titlebar_buttons_shown(true)
        .with_resizable(true)
        .with_inner_size([960.0, 640.0])
        .with_min_inner_size([640.0, 400.0])
}

fn set_error(state: &mut DashboardState, e: impl std::fmt::Display) {
    state.last_error = Some(e.to_string());
}

pub fn render(ui: &mut egui::Ui, state: &mut DashboardState) -> DashboardAction {
    ui.ctx().set_visuals(dashboard_visuals());
    sidebar_toggle(ui, state);
    let detail_reveal = ui.ctx().animate_bool_with_time(
        egui::Id::new("dashboard-detail-anim"),
        state.selected.is_some(),
        0.16,
    );
    let detail_open = state.selected.is_some() || detail_reveal > 0.002;
    let action = if detail_open {
        DashboardAction::None
    } else {
        refresh_button(ui)
    };
    if state.sidebar_open {
        sidebar(ui, state);
    }
    if detail_open {
        detail_panel(ui, state, detail_reveal);
    }
    match state.view {
        View::Timeline => central(ui, state),
        View::Approvals => approvals_panel(ui, state),
    }
    let reveal = ui.ctx().animate_bool_with_time(
        egui::Id::new("dashboard-search-anim"),
        state.search_open,
        0.12,
    );
    if state.search_open || reveal > 0.002 {
        search_modal(ui, state, reveal);
    }
    action
}

fn refresh_button(ui: &mut egui::Ui) -> DashboardAction {
    let clicked = egui::Area::new(egui::Id::new("dashboard-refresh"))
        .anchor(Align2::RIGHT_TOP, vec2(-6.0, 5.0))
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            icon_button(ui, icons::ICON_REFRESH)
                .on_hover_text("Refresh")
                .clicked()
        })
        .inner;
    if clicked {
        DashboardAction::Refresh
    } else {
        DashboardAction::None
    }
}

fn sidebar_toggle(ui: &mut egui::Ui, state: &mut DashboardState) {
    let icon = if state.sidebar_open {
        icons::ICON_LEFT_PANEL_CLOSE
    } else {
        icons::ICON_LEFT_PANEL_OPEN
    };
    let clicked = egui::Area::new(egui::Id::new("dashboard-sidebar-toggle"))
        .fixed_pos(egui::pos2(TRAFFIC_LIGHT_INSET, 5.0))
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            icon_button(ui, icon)
                .on_hover_text("Toggle sidebar")
                .clicked()
        })
        .inner;
    if clicked {
        state.sidebar_open = !state.sidebar_open;
    }
}

fn sidebar(ui: &mut egui::Ui, state: &mut DashboardState) {
    let sandboxes = state.sandboxes.clone();
    egui::Panel::left("dashboard-sidebar")
        .resizable(true)
        .default_size(SIDEBAR_WIDTH)
        .min_size(170.0)
        .max_size(360.0)
        .show_separator_line(true)
        .frame(Frame::new().fill(CHROME_FILL).inner_margin(Margin::same(8)))
        .show_inside(ui, |ui| {
            ui.add_space(26.0);
            if menu_item(
                ui,
                icons::ICON_RECEIPT_LONG,
                "Audit",
                state.view == View::Timeline,
                None,
            )
            .clicked()
            {
                state.view = View::Timeline;
            }
            if menu_item(
                ui,
                icons::ICON_GAVEL,
                "Approvals",
                state.view == View::Approvals,
                Some(approvals::waiting(
                    &state.approvals,
                    state.selected_sandbox.as_deref(),
                    &sandboxes,
                )),
            )
            .clicked()
            {
                state.view = View::Approvals;
                state.approval_notice = None;
                // The detail panel is not view-gated, so an audit row left open would sit over the approvals list and narrow it.
                state.selected = None;
            }
            if menu_item(
                ui,
                icons::ICON_DNS,
                "All sandboxes",
                state.selected_sandbox.is_none(),
                None,
            )
            .clicked()
            {
                state.selected_sandbox = None;
                state.selected = None;
            }
            if !sandboxes.is_empty() {
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    ui.label(RichText::new("SANDBOXES").size(FS_LABEL).color(TEXT_MUTED));
                });
                ui.add_space(2.0);
            }
            // A machine holds as many runs as it likes, and the panel is one window tall.
            egui::ScrollArea::vertical()
                .id_salt(SIDEBAR_LIST)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for sb in &sandboxes {
                        sidebar_item(
                            ui,
                            state,
                            Some(&sb.id),
                            &sb.name,
                            &sb.image,
                            &sb.id,
                            &sb.status,
                        );
                    }
                });
        });
}

fn menu_item(
    ui: &mut egui::Ui,
    icon: MaterialIcon,
    label: &str,
    selected: bool,
    count: Option<usize>,
) -> egui::Response {
    let fill = if selected {
        SELECT_FILL
    } else {
        Color32::TRANSPARENT
    };
    let response = Frame::new()
        .fill(fill)
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(8, 7))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                glyph(ui, icon, TEXT_MUTED, 18.0);
                ui.add_space(8.0);
                ui.label(RichText::new(label).size(FS_BODY).color(TEXT_PRIMARY));
                if let Some(waiting) = count.filter(|n| *n > 0) {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(waiting.to_string())
                                .size(FS_LABEL)
                                .color(STATUS_WARNING),
                        );
                    });
                }
            });
        })
        .response
        .interact(Sense::click());
    row_click(&response);
    response
}

fn sidebar_item(
    ui: &mut egui::Ui,
    state: &mut DashboardState,
    id: Option<&str>,
    name: &str,
    image: &str,
    run: &str,
    status: &str,
) {
    let selected = state.selected_sandbox.as_deref() == id;
    let fill = if selected {
        SELECT_FILL
    } else {
        Color32::TRANSPARENT
    };
    let short = if run.is_empty() {
        String::new()
    } else {
        lns_ipc::short_run_id(run).to_string()
    };
    let short = if short == name { String::new() } else { short };
    let response = Frame::new()
        .fill(fill)
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                if !status.is_empty() {
                    status_dot(ui, status);
                }
                ui.vertical(|ui| {
                    ui.label(RichText::new(name).size(FS_BODY).color(TEXT_PRIMARY));
                    if !image.is_empty() {
                        ui.label(RichText::new(image).size(FS_LABEL).color(TEXT_MUTED));
                    }
                    if !short.is_empty() {
                        ui.label(
                            RichText::new(short)
                                .size(FS_LABEL)
                                .color(TEXT_MUTED)
                                .monospace(),
                        );
                    }
                });
            });
        })
        .response
        .interact(Sense::click());
    row_click(&response);
    if response.clicked() {
        state.selected_sandbox = id.map(str::to_string);
        state.selected = None;
    }
}

fn central(ui: &mut egui::Ui, state: &mut DashboardState) {
    egui::CentralPanel::default()
        .frame(
            Frame::new()
                .fill(CONTENT_FILL)
                .inner_margin(Margin::same(theme::STACK_MARGIN)),
        )
        .show_inside(ui, |ui| {
            ui.add_space(16.0);
            ui.horizontal(|ui| {
                kind_chooser(ui, state);
                if search_button(ui).clicked() {
                    state.search_open = true;
                }
            });
            ui.add_space(12.0);
            let filters = Filters {
                kinds: state.kinds.iter().cloned().collect(),
                sandbox: state.selected_sandbox.clone().unwrap_or_default(),
                search: String::new(),
            };
            let visible = visible_indices(&state.rows, &filters);
            egui::ScrollArea::vertical()
                .id_salt(TIMELINE_LIST)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if visible.is_empty() {
                        ui.add_space(8.0);
                        ui.colored_label(TEXT_MUTED, "No audit events.");
                        return;
                    }
                    for &i in &visible {
                        event_row(ui, state, i);
                    }
                });
        });
}

fn approvals_panel(ui: &mut egui::Ui, state: &mut DashboardState) {
    let mut chosen: Option<(String, RowAction)> = None;
    let notice = state.approval_notice.clone();
    egui::CentralPanel::default()
        .frame(
            Frame::new()
                .fill(CONTENT_FILL)
                .inner_margin(Margin::same(theme::STACK_MARGIN)),
        )
        .show_inside(ui, |ui| {
            ui.add_space(16.0);
            approval_choosers(ui, state);
            ui.add_space(12.0);
            if let Some(said) = notice {
                ui.label(
                    RichText::new(said)
                        .size(FS_SECONDARY)
                        .color(STATUS_CRITICAL),
                );
                ui.add_space(10.0);
            }
            let listed = approvals::listing(
                &state.approvals,
                state.selected_sandbox.as_deref(),
                &state.sandboxes,
                &state.approval_answers,
            );
            if listed.waiting.is_empty() && listed.archived.is_empty() {
                ui.colored_label(TEXT_MUTED, "Nothing has been asked.");
                return;
            }
            // Outside the scroll area, so the heads stay above the rows they name.
            // The scrollbar's lane is never the columns' to claim: it appears only once the list scrolls, and nothing recomputes the widths when it does.
            let bar = ui.spacing().scroll.bar_width + ui.spacing().scroll.bar_inner_margin;
            let table = ui.available_width() - DISCLOSURE_COL - 2.0 * f32::from(ROW_INSET) - bar;
            approval_header(ui, table);
            let waiting = approvals::groups(&state.approvals, &listed.waiting);
            let shown = approvals::archive_shown(state.archive_open, &listed);
            let archived = if shown {
                approvals::groups(&state.approvals, &listed.archived)
            } else {
                Vec::new()
            };
            let mut chose = None;
            egui::ScrollArea::vertical()
                .id_salt(APPROVALS_LIST)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if let Some(act) = approval_groups(ui, state, waiting, table) {
                        chosen = Some(act);
                    }
                    if !listed.archived.is_empty() {
                        chose = archive_heading(ui, listed.archived.len(), shown);
                    }
                    if let Some(act) = approval_groups(ui, state, archived, table) {
                        chosen = Some(act);
                    }
                });
            if chose.is_some() {
                state.archive_open = chose;
            }
        });
    match chosen {
        Some((id, RowAction::Answer(answer))) => answer_entry(state, &id, answer),
        Some((id, RowAction::Remove)) => remove_entry(state, &id),
        Some((id, RowAction::Toggle)) => toggle(state, &id),
        Some((id, RowAction::Grant(method, connection))) => {
            grant_connector(state, &id, &method, connection);
        }
        None => {}
    }
}

/// The expanded row: what it asks about, the grant being composed on it where it is a connector, and the offer that grant discloses — read once when the row opens, because it is the run's own held offer and the frame must not re-read every file to draw it.
#[derive(Debug)]
pub struct OpenRow {
    id: String,
    draft: crate::tray::OfferDraft,
    offer: Option<lns_ipc::ConnectorView>,
}

/// What a click on a row asked for. A notice is the one row that can be cleared, so it is the one that offers this alongside no answers at all.
#[derive(Debug, Clone)]
enum RowAction {
    Answer(lns_ipc::ApprovalAnswer),
    Remove,
    /// Expand this row, or collapse it.
    Toggle,
    /// Grant it, with the method and connection the row composed.
    Grant(String, crate::approval_flow::session::ConnectionChoice),
}

const DISCLOSURE_COL: f32 = 18.0;
/// The inner margin every row frame carries, which the header must clear to sit above its own cells.
const ROW_INSET: i8 = 6;

/// One list of rows, gathered under the runs they were asked of, and whichever of them the developer clicked.
fn approval_groups(
    ui: &mut egui::Ui,
    state: &mut DashboardState,
    groups: Vec<approvals::Group>,
    table: f32,
) -> Option<(String, RowAction)> {
    let mut chosen = None;
    for group in groups {
        sandbox_heading(ui, &group);
        for i in group.rows {
            let entry = &state.approvals[i];
            let open = state
                .open
                .as_mut()
                .filter(|open| open.id == entry.id)
                .map(|open| (&mut open.draft, open.offer.as_ref()));
            if let Some(act) = approval_row(ui, entry, table, open) {
                chosen = Some((entry.id.clone(), act));
            }
        }
        ui.add_space(6.0);
    }
    chosen
}

/// The archive of what nothing is waiting on, behind one click, and what that click chose if it came.
fn archive_heading(ui: &mut egui::Ui, held: usize, shown: bool) -> Option<bool> {
    ui.add_space(2.0);
    let mark = if shown {
        icons::ICON_EXPAND_MORE
    } else {
        icons::ICON_CHEVRON_RIGHT
    };
    let row = Frame::new()
        .inner_margin(Margin::symmetric(ROW_INSET, 5))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                glyph(ui, mark, TEXT_MUTED, 16.0);
                ui.label(
                    RichText::new(format!("Archive ({held})"))
                        .size(FS_LABEL)
                        .color(TEXT_MUTED),
                );
            });
        })
        .response
        .interact(Sense::click());
    row_click(&row);
    row.clicked().then_some(!shown)
}

/// The run every row beneath it was asked of, which the rows themselves no longer carry.
fn sandbox_heading(ui: &mut egui::Ui, group: &approvals::Group) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(f32::from(ROW_INSET));
        glyph(ui, icons::ICON_DNS, TEXT_MUTED, 14.0);
        ui.label(
            RichText::new(&group.sandbox)
                .monospace()
                .size(FS_SECONDARY)
                .color(TEXT_PRIMARY),
        );
        if group.waiting > 0 {
            ui.label(
                RichText::new(format!("{} waiting", group.waiting))
                    .size(FS_LABEL)
                    .color(STATUS_WARNING),
            );
        }
    });
    ui.add_space(2.0);
}

fn approval_header(ui: &mut egui::Ui, available: f32) {
    let gutter = ui.spacing().item_spacing.x;
    ui.horizontal(|ui| {
        ui.add_space(DISCLOSURE_COL + f32::from(ROW_INSET));
        for (column, width) in approvals::columns()
            .into_iter()
            .zip(approvals::widths(available, gutter))
        {
            cell(
                ui,
                width,
                RichText::new(column.head())
                    .size(FS_LABEL)
                    .color(TEXT_MUTED),
            );
        }
    });
    ui.add_space(2.0);
}

fn approval_row(
    ui: &mut egui::Ui,
    entry: &Entry,
    available: f32,
    open: Option<(
        &mut crate::tray::OfferDraft,
        Option<&lns_ipc::ConnectorView>,
    )>,
) -> Option<RowAction> {
    let mut chosen = None;
    let expanded = open.is_some();
    let fill = if expanded {
        SELECT_FILL
    } else {
        Color32::TRANSPARENT
    };
    let gutter = ui.spacing().item_spacing.x;
    // One block, not two: the open row and what it opened must not read as separate cards with a gap between them.
    let joined = ui.spacing().item_spacing.y;
    ui.spacing_mut().item_spacing.y = 0.0;
    let row = Frame::new()
        .fill(fill)
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(ROW_INSET, 5))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(
                    vec2(DISCLOSURE_COL, ROW_HEIGHT),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        ui.set_min_size(vec2(DISCLOSURE_COL, ROW_HEIGHT));
                        let mark = if expanded {
                            icons::ICON_EXPAND_MORE
                        } else {
                            icons::ICON_CHEVRON_RIGHT
                        };
                        glyph(ui, mark, TEXT_MUTED, 16.0);
                    },
                );
                for (column, width) in approvals::columns()
                    .into_iter()
                    .zip(approvals::widths(available, gutter))
                {
                    approval_cell(ui, width, column.cell(entry));
                }
            });
        })
        .response
        .interact(Sense::click());
    row_click(&row);
    if row.clicked() {
        chosen = Some(RowAction::Toggle);
    }
    if let Some((draft, offer)) = open
        && let Some(act) = approval_expansion(ui, entry, draft, offer)
    {
        chosen = Some(act);
    }
    ui.spacing_mut().item_spacing.y = joined;
    ui.add_space(2.0);
    chosen
}

/// One cell of a collapsed row, drawn by what it holds rather than by where it sits.
fn approval_cell(ui: &mut egui::Ui, width: f32, held: approvals::Cell) {
    ui.allocate_ui_with_layout(
        vec2(width, ROW_HEIGHT),
        Layout::left_to_right(Align::Center),
        |ui| {
            // A cell shrinks to its own text unless it is told not to, and a table of shrunken cells leaves the heads over nothing and the last column short of the window.
            ui.set_min_size(vec2(width, ROW_HEIGHT));
            match held {
                approvals::Cell::Question(asked) => {
                    glyph(ui, mark(asked), CATEGORY, 16.0).on_hover_text(asked.hint());
                }
                approvals::Cell::Subject { text, raw } => {
                    if raw {
                        ui.label(RichText::new("RAW").size(FS_LABEL).color(STATUS_WARNING));
                    }
                    ui.add(
                        egui::Label::new(
                            RichText::new(text)
                                .monospace()
                                .size(FS_SECONDARY)
                                .color(TEXT_PRIMARY),
                        )
                        .truncate(),
                    );
                }
                approvals::Cell::Answer { text, tone } => {
                    ui.add(
                        egui::Label::new(
                            RichText::new(text).size(FS_LABEL).color(answer_ink(tone)),
                        )
                        .truncate(),
                    );
                }
            }
        },
    );
}

/// The mark that stands for a question, in place of the word the column used to spend its width on.
fn mark(asked: approvals::Asked) -> MaterialIcon {
    match asked {
        approvals::Asked::Destination => icons::ICON_SWAP_HORIZ,
        approvals::Asked::Connector => icons::ICON_LINK,
        approvals::Asked::Notice => icons::ICON_INFO,
    }
}

fn answer_ink(tone: approvals::Tone) -> Color32 {
    match tone {
        approvals::Tone::Waiting => STATUS_WARNING,
        approvals::Tone::Allowed => ACCENT_GREEN,
        approvals::Tone::Denied => STATUS_CRITICAL,
        approvals::Tone::Quiet => TEXT_MUTED,
    }
}

/// What the row shows once it is open: the action the card showed, then everything that can answer it.
fn approval_expansion(
    ui: &mut egui::Ui,
    entry: &Entry,
    draft: &mut crate::tray::OfferDraft,
    offer: Option<&lns_ipc::ConnectorView>,
) -> Option<RowAction> {
    let mut chosen = None;
    Frame::new()
        .fill(SELECT_FILL)
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(ROW_INSET, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.add_space(DISCLOSURE_COL + f32::from(ROW_INSET));
                ui.vertical(|ui| {
                    if let Some(action) = approvals::action(entry) {
                        ui.label(RichText::new(action).size(FS_LABEL).color(TEXT_MUTED));
                        ui.add_space(8.0);
                    }
                    if approvals::is_grantable(entry)
                        && let Some(act) = grant_form(ui, offer, draft)
                    {
                        chosen = Some(act);
                    }
                    ui.horizontal(|ui| {
                        for answer in approvals::offers(entry) {
                            if ui.button(approvals::label(answer)).clicked() {
                                chosen = Some(RowAction::Answer(answer));
                            }
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if icon_button(ui, icons::ICON_DELETE)
                                .on_hover_text(
                                    "Remove from the list. What this entry decided stays decided.",
                                )
                                .clicked()
                            {
                                chosen = Some(RowAction::Remove);
                            }
                        });
                    });
                });
            });
        });
    chosen
}

/// The card's own grant, unfolded under the row: the method, the connection it authenticates with, and the whole payload before the button (cli-spec §7.1).
fn grant_form(
    ui: &mut egui::Ui,
    offer: Option<&lns_ipc::ConnectorView>,
    draft: &mut crate::tray::OfferDraft,
) -> Option<RowAction> {
    let Some(offer) = offer else {
        ui.add_space(8.0);
        ui.label(
            RichText::new(crate::approval_flow::answering::NOT_OFFERED)
                .size(FS_LABEL)
                .color(STATUS_WARNING),
        );
        return None;
    };
    let Some(method) = crate::tray::chosen_method(offer, draft) else {
        ui.add_space(8.0);
        ui.label(
            RichText::new("this connector's methods need a newer lns")
                .size(FS_LABEL)
                .color(STATUS_WARNING),
        );
        return None;
    };
    let method = method.clone();
    ui.add_space(8.0);
    crate::tray::render_connection_choice(ui, offer, &method, draft);
    crate::tray::render_disclosure(ui, &method);
    ui.add_space(10.0);
    let ready = crate::tray::ready_to_grant(&method, draft);
    ui.add_enabled(ready, egui::Button::new("Grant to this sandbox"))
        .clicked()
        .then(|| RowAction::Grant(method.name.clone(), crate::tray::connection_choice(draft)))
}

fn toggle(state: &mut DashboardState, id: &str) {
    state.approval_notice = None;
    if state.open.as_ref().is_some_and(|open| open.id == id) {
        state.open = None;
        return;
    }
    state.open = Some(OpenRow {
        id: id.to_string(),
        draft: crate::tray::OfferDraft::default(),
        offer: offer_behind(id),
    });
}

/// The offer the run still holds for this row, which is what the form discloses.
fn offer_behind(id: &str) -> Option<lns_ipc::ConnectorView> {
    let root = crate::cache::root().ok()?;
    crate::approval_flow::answering::offered(
        &root,
        &crate::run_registry::known_ids(),
        crate::run_registry::approvals,
        id,
    )
}

fn grant_connector(
    state: &mut DashboardState,
    id: &str,
    method: &str,
    connection: crate::approval_flow::session::ConnectionChoice,
) {
    let root = match crate::cache::root() {
        Ok(root) => root,
        Err(e) => return set_error(state, e),
    };
    let granted = crate::approval_flow::answering::grant(
        &root,
        &crate::run_registry::known_ids(),
        crate::run_registry::approvals,
        id,
        method,
        connection,
    );
    state.approval_notice = approvals::granting_reported(&granted);
    state.open = None;
    load_approvals(state);
}

/// The two filters over the list: which sandbox asked, and which answer a row carries.
fn approval_choosers(ui: &mut egui::Ui, state: &mut DashboardState) {
    ui.horizontal(|ui| {
        sandbox_chooser(ui, state);
        answer_chooser(ui, state);
    });
}

fn sandbox_chooser(ui: &mut egui::Ui, state: &mut DashboardState) {
    let label = match &state.selected_sandbox {
        None => "all sandboxes".to_string(),
        Some(id) => state
            .sandboxes
            .iter()
            .find(|sandbox| &sandbox.id == id)
            .map_or_else(|| id.clone(), |sandbox| sandbox.name.clone()),
    };
    let control = control_button(ui, &label, state.sandbox_open);
    if control.clicked() {
        state.sandbox_open = !state.sandbox_open;
    }
    if !state.sandbox_open {
        return;
    }
    let sandboxes = state.sandboxes.clone();
    let popup = egui::Area::new(egui::Id::new("approvals-sandbox-popup"))
        .order(egui::Order::Foreground)
        .fixed_pos(control.rect.left_bottom() + vec2(0.0, 4.0))
        .constrain(true)
        .show(ui.ctx(), |ui| {
            popup_body(ui, |ui| {
                if dropdown_pick(ui, "all sandboxes", state.selected_sandbox.is_none()) {
                    choose_sandbox(state, None);
                }
                for sandbox in &sandboxes {
                    let picked = state.selected_sandbox.as_deref() == Some(sandbox.id.as_str());
                    if dropdown_pick(ui, &sandbox.name, picked) {
                        choose_sandbox(state, Some(sandbox.id.clone()));
                    }
                }
            });
        });
    if dismissed(ui, &popup.response, &control) {
        state.sandbox_open = false;
    }
}

fn answer_chooser(ui: &mut egui::Ui, state: &mut DashboardState) {
    let label = match state.approval_answers.len() {
        0 => "any answer".to_string(),
        1 => state
            .approval_answers
            .iter()
            .next()
            .cloned()
            .unwrap_or_default(),
        n => format!("{n} answers"),
    };
    let control = control_button(ui, &label, state.answer_open);
    if control.clicked() {
        state.answer_open = !state.answer_open;
    }
    if !state.answer_open {
        return;
    }
    let popup = egui::Area::new(egui::Id::new("approvals-answer-popup"))
        .order(egui::Order::Foreground)
        .fixed_pos(control.rect.left_bottom() + vec2(0.0, 4.0))
        .constrain(true)
        .show(ui.ctx(), |ui| {
            popup_body(ui, |ui| {
                if dropdown_item(ui, "any answer", state.approval_answers.is_empty()) {
                    state.approval_answers.clear();
                    keep_open_if_shown(state);
                }
                for answer in approvals::answers() {
                    let picked = state.approval_answers.contains(answer);
                    if dropdown_item(ui, answer, picked) {
                        if !state.approval_answers.remove(answer) {
                            state.approval_answers.insert(answer.to_string());
                        }
                        keep_open_if_shown(state);
                    }
                }
            });
        });
    if dismissed(ui, &popup.response, &control) {
        state.answer_open = false;
    }
}

/// Narrows every list to one sandbox, dropping the open audit detail with it — the detail panel is not view-gated, so a row of the sandbox you just left would stay on screen.
fn choose_sandbox(state: &mut DashboardState, id: Option<String>) {
    state.selected_sandbox = id;
    state.selected = None;
    state.sandbox_open = false;
    keep_open_if_shown(state);
}

/// A row the filter no longer shows is not a row that is open — and one it still shows keeps the grant half composed on it.
fn keep_open_if_shown(state: &mut DashboardState) {
    let rows = approvals::listing(
        &state.approvals,
        state.selected_sandbox.as_deref(),
        &state.sandboxes,
        &state.approval_answers,
    )
    .rows();
    if !approvals::still_shown(
        state.open.as_ref().map(|open| open.id.as_str()),
        &state.approvals,
        &rows,
    ) {
        state.open = None;
    }
}

fn popup_body(ui: &mut egui::Ui, body: impl FnOnce(&mut egui::Ui)) {
    Frame::new()
        .fill(MODAL_FILL)
        .stroke(Stroke::new(1.0_f32, BORDER))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(6))
        .show(ui, |ui| {
            ui.set_width(SELECT_WIDTH + 8.0);
            body(ui);
        });
}

fn dismissed(ui: &egui::Ui, popup: &egui::Response, control: &egui::Response) -> bool {
    let clicked_out = ui.input(|i| i.pointer.any_pressed())
        && !popup.contains_pointer()
        && !control.contains_pointer();
    clicked_out || ui.input(|i| i.key_pressed(egui::Key::Escape))
}

fn remove_entry(state: &mut DashboardState, id: &str) {
    let root = match crate::cache::root() {
        Ok(root) => root,
        Err(e) => return set_error(state, e),
    };
    let outcome = crate::approval_flow::answering::remove(
        &root,
        &crate::run_registry::known_ids(),
        crate::run_registry::approvals,
        id,
    );
    state.approval_notice = approvals::removal_reported(&outcome);
    load_approvals(state);
}

fn answer_entry(state: &mut DashboardState, id: &str, answer: lns_ipc::ApprovalAnswer) {
    let root = match crate::cache::root() {
        Ok(root) => root,
        Err(e) => return set_error(state, e),
    };
    let outcome = crate::approval_flow::answering::decide(
        &root,
        &crate::run_registry::known_ids(),
        crate::run_registry::approvals,
        id,
        answer,
    );
    state.approval_notice = approvals::reported(&outcome);
    load_approvals(state);
}

const SELECT_FONT: f32 = 15.0;
const SELECT_WIDTH: f32 = 172.0;

fn kind_chooser(ui: &mut egui::Ui, state: &mut DashboardState) {
    let label = match state.kinds.len() {
        0 => "all kinds".to_string(),
        1 => state.kinds.iter().next().cloned().unwrap_or_default(),
        n => format!("{n} kinds"),
    };
    let control = control_button(ui, &label, state.kind_open);
    if control.clicked() {
        state.kind_open = !state.kind_open;
        state.kind_query.clear();
    }
    if !state.kind_open {
        return;
    }
    let popup = egui::Area::new(egui::Id::new("dashboard-kind-popup"))
        .order(egui::Order::Foreground)
        .fixed_pos(control.rect.left_bottom() + vec2(0.0, 4.0))
        .constrain(true)
        .show(ui.ctx(), |ui| {
            kind_popup_body(ui, state);
        });
    if dismissed(ui, &popup.response, &control) {
        state.kind_open = false;
    }
}

fn search_button(ui: &mut egui::Ui) -> egui::Response {
    Frame::new()
        .fill(INPUT_FILL)
        .stroke(Stroke::new(1.0_f32, BORDER))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(10, 7))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                glyph(ui, icons::ICON_SEARCH, TEXT_MUTED, 16.0);
                ui.label(
                    RichText::new("Search")
                        .size(SELECT_FONT)
                        .color(TEXT_PRIMARY),
                );
            });
        })
        .response
        .interact(Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text("Search every sandbox's audit trail")
}

fn control_button(ui: &mut egui::Ui, label: &str, open: bool) -> egui::Response {
    let border = if open { CATEGORY } else { BORDER };
    Frame::new()
        .fill(INPUT_FILL)
        .stroke(Stroke::new(1.0_f32, border))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(10, 7))
        .show(ui, |ui| {
            ui.set_width(SELECT_WIDTH);
            ui.horizontal(|ui| {
                ui.label(RichText::new(label).size(SELECT_FONT).color(TEXT_PRIMARY));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    glyph(ui, icons::ICON_EXPAND_MORE, TEXT_MUTED, 18.0);
                });
            });
        })
        .response
        .interact(Sense::click())
        .on_hover_cursor(CursorIcon::PointingHand)
}

fn kind_popup_body(ui: &mut egui::Ui, state: &mut DashboardState) {
    popup_body(ui, |ui| {
        ui.add(
            egui::TextEdit::singleline(&mut state.kind_query)
                .hint_text("Filter…")
                .margin(Margin::symmetric(8, 6))
                .desired_width(f32::INFINITY),
        )
        .request_focus();
        ui.add_space(4.0);
        let q = state.kind_query.trim().to_lowercase();
        egui::ScrollArea::vertical()
            .max_height(300.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if (q.is_empty() || "all kinds".contains(&q))
                    && dropdown_item(ui, "all kinds", state.kinds.is_empty())
                {
                    state.kinds.clear();
                }
                for k in KINDS {
                    if !q.is_empty() && !k.contains(q.as_str()) {
                        continue;
                    }
                    if dropdown_item(ui, k, state.kinds.contains(k)) && !state.kinds.remove(k) {
                        state.kinds.insert(k.to_string());
                    }
                }
            });
    });
}

/// One choice of a single-select chooser: a tick where a multi-select paints a box, so the two do not read alike.
fn dropdown_pick(ui: &mut egui::Ui, label: &str, picked: bool) -> bool {
    dropdown_row(
        ui,
        label,
        if picked {
            (icons::ICON_CHECK, CATEGORY)
        } else {
            (icons::ICON_CHECK, Color32::TRANSPARENT)
        },
    )
}

fn dropdown_item(ui: &mut egui::Ui, label: &str, checked: bool) -> bool {
    dropdown_row(
        ui,
        label,
        if checked {
            (icons::ICON_CHECK_BOX, CATEGORY)
        } else {
            (icons::ICON_CHECK_BOX_OUTLINE_BLANK, TEXT_MUTED)
        },
    )
}

fn dropdown_row(ui: &mut egui::Ui, label: &str, mark: (MaterialIcon, Color32)) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
    if ui.is_rect_visible(rect) {
        if resp.hovered() {
            ui.painter()
                .rect_filled(rect, CornerRadius::same(5), HOVER_FILL);
        }
        let (icon, color) = mark;
        let cy = rect.center().y;
        ui.painter().text(
            egui::pos2(rect.left() + 10.0, cy),
            Align2::LEFT_CENTER,
            icon.codepoint,
            FontId::new(18.0, icon.font_family()),
            color,
        );
        ui.painter().text(
            egui::pos2(rect.left() + 36.0, cy),
            Align2::LEFT_CENTER,
            label,
            FontId::new(SELECT_FONT, egui::FontFamily::Proportional),
            TEXT_PRIMARY,
        );
    }
    resp.on_hover_cursor(CursorIcon::PointingHand).clicked()
}

fn event_row(ui: &mut egui::Ui, state: &mut DashboardState, i: usize) {
    let selected = state.selected == Some(i);
    let row = &state.rows[i];
    let when = format::friendly_time(now_unix_secs(), &row.ts);
    let short = lns_ipc::short_run_id(&row.run).to_string();
    let kind = row.kind.clone();
    let detail = row.detail.clone();
    let icon = kind_icon(&kind);
    let accent = kind_color(&kind);

    let fill = if selected {
        SELECT_FILL
    } else {
        Color32::TRANSPARENT
    };
    let response = Frame::new()
        .fill(fill)
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(6, 5))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(
                    vec2(ICON_COL, ROW_HEIGHT),
                    Layout::left_to_right(Align::Center),
                    |ui| glyph(ui, icon, accent, 16.0),
                );
                cell(
                    ui,
                    W_TIME,
                    RichText::new(when).size(FS_SECONDARY).color(TEXT_MUTED),
                );
                ui.add(
                    egui::Label::new(RichText::new(detail).size(FS_BODY).color(TEXT_PRIMARY))
                        .truncate(),
                );
            });
        })
        .response
        .interact(Sense::click());
    row_click(&response);
    let response = response.on_hover_text(format!("run {short}"));
    if response.clicked() {
        state.selected = Some(i);
        state.detail_row = Some(state.rows[i].clone());
    }
}

fn detail_panel(ui: &mut egui::Ui, state: &mut DashboardState, reveal: f32) {
    let Some(row) = state.detail_row.clone() else {
        return;
    };
    egui::Panel::right("dashboard-detail")
        .resizable(false)
        .exact_size(DETAIL_WIDTH * reveal)
        .show_separator_line(true)
        .frame(Frame::new().fill(CHROME_FILL))
        .show_inside(ui, |ui| {
            ui.set_opacity(reveal);
            egui::Frame::new()
                .inner_margin(Margin::same(theme::STACK_MARGIN))
                .show(ui, |ui| {
                    ui.set_width(DETAIL_WIDTH - 2.0 * theme::STACK_MARGIN as f32);
                    detail_body(ui, state, &row);
                });
        });
}

fn detail_body(ui: &mut egui::Ui, state: &mut DashboardState, row: &TimelineRow) {
    let accent = kind_color(&row.kind);
    ui.horizontal(|ui| {
        glyph(ui, kind_icon(&row.kind), accent, 18.0);
        ui.add_space(8.0);
        ui.label(
            RichText::new(row.kind.to_uppercase())
                .size(FS_LABEL)
                .color(accent),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if icon_button(ui, icons::ICON_CLOSE)
                .on_hover_text("Close")
                .clicked()
            {
                state.selected = None;
            }
        });
    });
    ui.add_space(12.0);

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            field(
                ui,
                state,
                "When",
                &format!(
                    "{} ({})",
                    row.when,
                    format::relative_time(now_unix_secs(), &row.ts)
                ),
                Some(&row.ts),
            );
            sandbox_field(ui, state, &row.run);
            if let Some(obj) = row.raw.as_object() {
                for (key, value) in obj {
                    if RAW_SKIP.contains(&key.as_str()) || format::is_empty_value(value) {
                        continue;
                    }
                    if format::is_structured(value) {
                        structured_field(ui, &format::field_label(key), value);
                    } else {
                        field(
                            ui,
                            state,
                            &format::field_label(key),
                            &format::render_value(value),
                            None,
                        );
                    }
                }
            }
            ui.add_space(6.0);
            ui.label(RichText::new("RAW").size(FS_LABEL).color(TEXT_MUTED));
            ui.add_space(4.0);
            code_block(
                ui,
                &serde_json::to_string_pretty(&row.raw).unwrap_or_default(),
            );
        });
}

const RAW_SKIP: &[&str] = &[
    "prev_hash",
    "ts",
    "type",
    "event",
    "run",
    "microvm",
    "time",
    "cloud",
    "metadata",
    "unmapped",
    "activity_id",
    "category_uid",
    "class_uid",
    "type_uid",
    "severity_id",
    "status_id",
    "disposition_id",
];

fn field(
    ui: &mut egui::Ui,
    state: &mut DashboardState,
    label: &str,
    value: &str,
    copy: Option<&str>,
) {
    ui.label(RichText::new(label).size(FS_LABEL).color(TEXT_MUTED));
    ui.add_space(2.0);
    match copy {
        Some(text) => {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(RichText::new(value).size(FS_BODY).color(TEXT_PRIMARY));
                copy_control(
                    ui,
                    state,
                    egui::Id::new(("dashboard-copy", label)),
                    text,
                    "Copy",
                );
            });
        }
        None => {
            ui.label(RichText::new(value).size(FS_BODY).color(TEXT_PRIMARY));
        }
    }
    ui.add_space(10.0);
}

fn sandbox_field(ui: &mut egui::Ui, state: &mut DashboardState, run: &str) {
    ui.label(RichText::new("Sandbox").size(FS_LABEL).color(TEXT_MUTED));
    ui.add_space(2.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.label(
            RichText::new(lns_ipc::short_run_id(run))
                .monospace()
                .size(FS_BODY)
                .color(TEXT_PRIMARY),
        );
        copy_control(
            ui,
            state,
            egui::Id::new("dashboard-copy-sandbox"),
            run,
            "Copy full run id",
        );
    });
    ui.add_space(10.0);
}

fn structured_field(ui: &mut egui::Ui, label: &str, value: &serde_json::Value) {
    ui.label(RichText::new(label).size(FS_LABEL).color(TEXT_MUTED));
    ui.add_space(4.0);
    code_block(ui, &serde_json::to_string_pretty(value).unwrap_or_default());
    ui.add_space(10.0);
}

fn code_block(ui: &mut egui::Ui, text: &str) {
    Frame::new()
        .fill(INPUT_FILL)
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::same(10))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(
                egui::Label::new(
                    RichText::new(text)
                        .monospace()
                        .size(FS_LABEL)
                        .color(TEXT_MUTED),
                )
                .selectable(true),
            );
        });
}

const COPIED_FEEDBACK_SECS: f64 = 1.3;

fn copy_control(
    ui: &mut egui::Ui,
    state: &mut DashboardState,
    id: egui::Id,
    text: &str,
    hover: &str,
) {
    let now = ui.input(|i| i.time);
    let just_copied = state
        .copied
        .is_some_and(|(cid, at)| cid == id && now - at < COPIED_FEEDBACK_SECS);
    let icon = if just_copied {
        icons::ICON_CHECK
    } else {
        icons::ICON_CONTENT_COPY
    };
    let color = if just_copied {
        ACCENT_GREEN
    } else {
        TEXT_MUTED
    };
    let glyph = RichText::new(icon.codepoint)
        .font(FontId::new(14.0, icon.font_family()))
        .color(color);
    let response = ui
        .add(
            egui::Button::new(glyph)
                .frame(false)
                .min_size(Vec2::splat(20.0)),
        )
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text(if just_copied { "Copied!" } else { hover });
    if response.clicked() {
        ui.ctx().copy_text(text.to_string());
        state.copied = Some((id, now));
    }
    if just_copied {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(150));
    }
}

fn now_unix_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn search_modal(ui: &mut egui::Ui, state: &mut DashboardState, reveal: f32) {
    let mut pick: Option<usize> = None;
    let backdrop_alpha = (14.0 * reveal) as u8;
    let shadow = egui::epaint::Shadow {
        offset: [0, 10],
        blur: 32,
        spread: 2,
        color: Color32::from_black_alpha((160.0 * reveal) as u8),
    };
    let modal = egui::Modal::new(egui::Id::new("dashboard-search"))
        .backdrop_color(Color32::from_rgba_premultiplied(0, 0, 0, backdrop_alpha))
        .frame(
            Frame::new()
                .fill(MODAL_FILL)
                .stroke(Stroke::new(1.0_f32, BORDER))
                .corner_radius(CornerRadius::same(12))
                .shadow(shadow)
                .inner_margin(Margin::same(14)),
        )
        .show(ui.ctx(), |ui| {
            ui.set_opacity(reveal);
            ui.set_width(560.0);
            ui.add(
                egui::TextEdit::singleline(&mut state.search_query)
                    .hint_text("Search audit logs across all sandboxes")
                    .background_color(MODAL_FILL)
                    .font(FontId::new(18.0, egui::FontFamily::Proportional))
                    .margin(Margin::symmetric(2, 8))
                    .desired_width(f32::INFINITY),
            )
            .request_focus();
            ui.add_space(10.0);
            let filters = Filters {
                search: state.search_query.clone(),
                ..Default::default()
            };
            let results = visible_indices(&state.rows, &filters);
            if state.search_query.trim().is_empty() {
                ui.colored_label(TEXT_MUTED, "Type to search every sandbox's audit trail.");
            } else if results.is_empty() {
                ui.colored_label(TEXT_MUTED, "No matching events.");
            }
            egui::ScrollArea::vertical()
                .max_height(360.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    for &i in &results {
                        if search_result_row(ui, &state.rows[i]).clicked() {
                            pick = Some(i);
                        }
                    }
                });
        });
    if let Some(i) = pick {
        state.selected_sandbox = Some(state.rows[i].run.clone());
        state.selected = Some(i);
        state.detail_row = Some(state.rows[i].clone());
        state.search_open = false;
    } else if state.search_open && modal.should_close() {
        state.search_open = false;
    }
}

fn search_result_row(ui: &mut egui::Ui, row: &TimelineRow) -> egui::Response {
    let icon = kind_icon(&row.kind);
    let accent = kind_color(&row.kind);
    let short = lns_ipc::short_run_id(&row.run).to_string();
    let response = Frame::new()
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(6, 5))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                glyph(ui, icon, accent, 15.0);
                ui.add_space(6.0);
                cell(
                    ui,
                    104.0,
                    RichText::new(short)
                        .monospace()
                        .size(FS_SECONDARY)
                        .color(TEXT_MUTED),
                );
                ui.add(
                    egui::Label::new(RichText::new(&row.detail).size(FS_BODY).color(TEXT_PRIMARY))
                        .truncate(),
                );
            });
        })
        .response
        .interact(Sense::click());
    row_click(&response);
    response
}

fn row_click(response: &egui::Response) {
    response.clone().on_hover_cursor(CursorIcon::PointingHand);
    if response.has_focus() {
        response.surrender_focus();
    }
}

fn cell(ui: &mut egui::Ui, width: f32, text: RichText) {
    ui.allocate_ui_with_layout(
        vec2(width, ROW_HEIGHT),
        Layout::left_to_right(Align::Center),
        |ui| {
            // Without a floor the cell is as wide as its own text, which leaves every head over the wrong column and the table short of the window.
            ui.set_min_size(vec2(width, ROW_HEIGHT));
            ui.add(egui::Label::new(text).truncate());
        },
    );
}

fn icon_button(ui: &mut egui::Ui, icon: MaterialIcon) -> egui::Response {
    let text = RichText::new(icon.codepoint)
        .font(FontId::new(18.0, icon.font_family()))
        .color(TEXT_MUTED);
    ui.add(
        egui::Button::new(text)
            .frame(false)
            .min_size(Vec2::splat(24.0)),
    )
    .on_hover_cursor(CursorIcon::PointingHand)
}

fn glyph(ui: &mut egui::Ui, icon: MaterialIcon, color: Color32, size: f32) -> egui::Response {
    ui.label(
        RichText::new(icon.codepoint)
            .font(FontId::new(size, icon.font_family()))
            .color(color),
    )
}

fn status_dot(ui: &mut egui::Ui, status: &str) {
    let color = if status.eq_ignore_ascii_case("running") {
        ACCENT_GREEN
    } else {
        TEXT_MUTED
    };
    glyph(ui, icons::ICON_FIBER_MANUAL_RECORD, color, 12.0);
}

fn kind_icon(kind: &str) -> MaterialIcon {
    match kind {
        "launch" => icons::ICON_ROCKET_LAUNCH,
        "egress" => icons::ICON_SWAP_HORIZ,
        "env" => icons::ICON_TUNE,
        "volume" => icons::ICON_STORAGE,
        "bind" => icons::ICON_FOLDER,
        "approval" => icons::ICON_GAVEL,
        "connection" => icons::ICON_LINK,
        "credential" => icons::ICON_KEY,
        _ => icons::ICON_RECEIPT_LONG,
    }
}

fn kind_color(kind: &str) -> Color32 {
    match kind {
        "approval" => STATUS_WARNING,
        "connection" => ACCENT_GREEN,
        _ => CATEGORY,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use egui_kittest::kittest::Queryable as _;

    use super::*;
    use crate::approval_flow::entries::{EntryKind, EntryState};

    const WINDOW: Vec2 = Vec2 { x: 960.0, y: 640.0 };
    const OVER_THE_LIST: egui::Pos2 = egui::Pos2 { x: 600.0, y: 400.0 };
    const OVER_THE_SIDEBAR: egui::Pos2 = egui::Pos2 { x: 120.0, y: 400.0 };

    fn asked_about(host: &str) -> Entry {
        answered(host, EntryState::Undecided)
    }

    fn answered(host: &str, state: EntryState) -> Entry {
        Entry::new(
            Some("dapper_thistle".to_string()),
            EntryKind::Destination {
                destination: host.to_string(),
                action: format!("CONNECT {host}:443"),
                raw: false,
            },
            state,
        )
    }

    fn run(i: usize) -> Sandbox {
        Sandbox {
            id: format!("{i:032x}"),
            name: format!("run_{i:02}"),
            image: "alpine:latest".into(),
            status: "running".into(),
        }
    }

    fn logged(detail: &str) -> TimelineRow {
        TimelineRow {
            ts: "2026-06-29T13:30:00Z".into(),
            when: "2026-06-29 13:30:00".into(),
            run: "dapper_thistle".into(),
            kind: "egress".into(),
            detail: detail.to_string(),
            connector: None,
            raw: serde_json::json!({ "message": detail }),
        }
    }

    fn state_of(view: View, listed: usize, runs: usize) -> DashboardState {
        DashboardState {
            view,
            approvals: (0..listed)
                .map(|i| asked_about(&format!("api{i:02}.linear.app")))
                .collect(),
            sandboxes: (0..runs).map(run).collect(),
            rows: (0..listed)
                .map(|i| logged(&format!("GET /{i:02}")))
                .collect(),
            ..DashboardState::new()
        }
    }

    /// Drives the real window headless. The icon font binds on a first pass that draws nothing, because egui applies an added font on the pass after it is given.
    fn window(mut state: DashboardState, timeline: &Arc<AtomicBool>) -> egui_kittest::Harness<'_> {
        let switch = timeline.clone();
        let mut prepared = false;
        let mut harness = egui_kittest::Harness::builder()
            .with_size(WINDOW)
            .build_ui(move |ui| {
                if !prepared {
                    crate::approval_flow::window::install_icon_font(ui.ctx());
                    prepared = true;
                    return;
                }
                if switch.load(Ordering::Relaxed) {
                    state.view = View::Timeline;
                }
                render(ui, &mut state);
            });
        harness.run();
        harness
    }

    fn wheel(harness: &mut egui_kittest::Harness<'_>, at: egui::Pos2, notches: usize) {
        harness
            .input_mut()
            .events
            .push(egui::Event::PointerMoved(at));
        harness.run();
        for _ in 0..notches {
            harness.input_mut().events.push(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: vec2(0.0, -120.0),
                modifiers: egui::Modifiers::NONE,
                phase: egui::TouchPhase::Move,
            });
            harness.run();
        }
    }

    fn top_of(harness: &egui_kittest::Harness<'_>, label: &str) -> f32 {
        harness.get_by_label(label).rect().min.y
    }

    #[test]
    fn the_approvals_list_scrolls_under_the_wheel() {
        // A list longer than the window is a list of questions nobody can reach, and the ones out of reach are the ones a run is still waiting on.
        let still = Arc::new(AtomicBool::new(false));
        let mut harness = window(state_of(View::Approvals, 40, 1), &still);

        let before = top_of(&harness, "api05.linear.app");
        wheel(&mut harness, OVER_THE_LIST, 2);
        let after = top_of(&harness, "api05.linear.app");

        assert!(
            after < before - 20.0,
            "the row did not move: it sat at {before} before the wheel and at {after} after"
        );
    }

    #[test]
    fn the_sidebar_scrolls_when_a_machine_holds_more_runs_than_fit() {
        // The panel is one window tall and a machine holds as many runs as it likes; the last ones were unreachable.
        let still = Arc::new(AtomicBool::new(false));
        let mut harness = window(state_of(View::Approvals, 1, 30), &still);

        let before = top_of(&harness, "run_04");
        wheel(&mut harness, OVER_THE_SIDEBAR, 2);
        let after = top_of(&harness, "run_04");

        assert!(
            after < before - 20.0,
            "the sandbox did not move: it sat at {before} before the wheel and at {after} after"
        );
    }

    #[test]
    fn opening_the_approvals_view_closes_the_audit_detail() {
        // The detail panel is not view-gated: an audit row left open stayed over the approvals list and took a third of its width.
        let still = Arc::new(AtomicBool::new(false));
        let mut harness = window(
            DashboardState {
                selected: Some(0),
                detail_row: Some(logged("GET /00")),
                ..state_of(View::Timeline, 40, 1)
            },
            &still,
        );

        assert!(
            harness.query_by_label("When").is_some(),
            "the detail panel is not open, so this pins nothing"
        );
        harness.get_by_label("Approvals").click();
        harness.run();

        assert!(harness.query_by_label("When").is_none());
    }

    #[test]
    fn an_archived_row_waits_behind_one_click() {
        // The live list is what a run is waiting on. Everything settled is a record, and a record that fills the window buries the work.
        let still = Arc::new(AtomicBool::new(false));
        let mut harness = window(
            DashboardState {
                approvals: vec![
                    asked_about("api00.linear.app"),
                    answered("api01.linear.app", EntryState::AlwaysAllowed),
                ],
                ..state_of(View::Approvals, 0, 1)
            },
            &still,
        );

        assert!(
            harness.query_by_label("api00.linear.app").is_some(),
            "the question with no answer is the one list that needs no click"
        );
        assert!(harness.query_by_label("api01.linear.app").is_none());

        harness.get_by_label("Archive (1)").click();
        harness.run();

        assert!(harness.query_by_label("api01.linear.app").is_some());
    }

    #[test]
    fn the_archive_closes_when_a_run_has_nothing_waiting() {
        // With nothing to answer the archive opens itself, and a heading that draws a chevron and takes the click has to answer it.
        let still = Arc::new(AtomicBool::new(false));
        let mut harness = window(
            DashboardState {
                approvals: vec![answered("api01.linear.app", EntryState::AlwaysAllowed)],
                ..state_of(View::Approvals, 0, 1)
            },
            &still,
        );

        assert!(
            harness.query_by_label("api01.linear.app").is_some(),
            "with nothing waiting the archive is the view"
        );
        harness.get_by_label("Archive (1)").click();
        harness.run();

        assert!(harness.query_by_label("api01.linear.app").is_none());
    }

    #[test]
    fn search_belongs_to_the_view_it_searches() {
        // Search reads the audit trail alone, so beside the two views it read as a third one — and it did nothing for the list it sat next to.
        let still = Arc::new(AtomicBool::new(false));
        let approvals = window(state_of(View::Approvals, 4, 1), &still);

        assert!(
            approvals.query_by_label("Search").is_none(),
            "the approvals view offers no control that cannot search it"
        );

        let timeline = Arc::new(AtomicBool::new(true));
        let mut audit = window(state_of(View::Timeline, 4, 1), &timeline);
        audit.run();

        audit.get_by_label("Search").click();
        audit.run();

        assert!(
            audit
                .query_by_label("Type to search every sandbox's audit trail.")
                .is_some(),
            "the audit view's own control opens the search over its own rows"
        );
    }

    #[test]
    fn a_searched_row_opens_the_event_it_names() {
        // The pick selected a row and left the panel reading whichever event was open before it, or nothing at all.
        let timeline = Arc::new(AtomicBool::new(true));
        let mut harness = window(
            DashboardState {
                selected: Some(0),
                detail_row: Some(logged("GET /before-the-search")),
                ..state_of(View::Timeline, 4, 1)
            },
            &timeline,
        );
        harness.get_by_label("Search").click();
        harness.run();

        // The row is in the timeline behind the modal as well as in the results, and the modal draws last.
        let result = harness
            .query_all_by_label("GET /02")
            .last()
            .expect("the search lists the row it matched");
        result.click();
        harness.run();

        assert!(
            harness.query_by_label("When").is_some(),
            "the panel opened on nothing"
        );
        // The panel prints the event's own payload, which is the one place the two rows read differently.
        assert!(
            harness
                .query_all_by_label("GET /before-the-search")
                .next()
                .is_none(),
            "the panel is still reading the event that was open before the search"
        );
        assert!(
            harness
                .query_all_by_label(
                    r#"{
  "message": "GET /02"
}"#
                )
                .next()
                .is_some(),
            "the panel does not show the event the developer picked"
        );
    }

    #[test]
    fn each_list_keeps_its_own_place() {
        // Both lists sit in one panel, so with one scroll id between them the timeline opened at wherever the approvals list had been left — above its own first row, which reads as an empty window.
        let still = Arc::new(AtomicBool::new(false));
        let untouched = window(state_of(View::Timeline, 40, 1), &still);
        let top = top_of(&untouched, "GET /00");

        let switch = Arc::new(AtomicBool::new(false));
        let mut moved = window(state_of(View::Approvals, 40, 1), &switch);
        wheel(&mut moved, OVER_THE_LIST, 3);
        switch.store(true, Ordering::Relaxed);
        moved.run();

        assert_eq!(
            top_of(&moved, "GET /00"),
            top,
            "the timeline opened where the approvals list was left"
        );
    }
}
