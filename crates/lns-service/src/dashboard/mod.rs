mod filter;
pub mod live;
mod sandboxes;

pub use filter::{Filters, KINDS, visible_indices};

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, CursorIcon, FontId, Frame, Layout, Margin,
    RichText, Sense, Stroke, Vec2, vec2,
};
use egui_material_icons::{MaterialIcon, icons};
use lns_audit::TimelineRow;

use crate::approval_flow::window::{
    ACCENT_GREEN, BG_PRIMARY, BG_SECONDARY, BG_TERTIARY, BORDER, CATEGORY, STATUS_WARNING,
    TEXT_MUTED, TEXT_PRIMARY,
};
use crate::ui::theme;

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
    pub status: String,
}

#[derive(Debug, Default)]
pub struct DashboardState {
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
    v.window_stroke = Stroke::new(1.0, BORDER);
    v.window_corner_radius = CornerRadius::same(10);
    v.extreme_bg_color = INPUT_FILL;
    v.faint_bg_color = CHROME_FILL;
    v.selection.bg_fill = CATEGORY;
    v.selection.stroke = Stroke::new(0.0, Color32::WHITE);
    let radius = CornerRadius::same(4);
    for w in [
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = radius;
        w.bg_stroke = Stroke::NONE;
        w.fg_stroke = Stroke::new(1.0, TEXT_PRIMARY);
        w.weak_bg_fill = INPUT_FILL;
        w.bg_fill = INPUT_FILL;
    }
    v.widgets.hovered.weak_bg_fill = HOVER_FILL;
    v.widgets.hovered.bg_fill = HOVER_FILL;
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, HOVER_LINE);
    v.widgets.active.fg_stroke = Stroke::new(1.0, DRAG_LINE);
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, WEAK_BORDER);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_PRIMARY);
    v
}

pub fn load(state: &mut DashboardState) {
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
        .with_title("Lens Sandbox — Audit")
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
    central(ui, state);
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
            if menu_item(ui, icons::ICON_SEARCH, "Search", state.search_open).clicked() {
                state.search_open = true;
            }
            if menu_item(
                ui,
                icons::ICON_DNS,
                "All sandboxes",
                state.selected_sandbox.is_none(),
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
            for sb in &sandboxes {
                sidebar_item(ui, state, Some(&sb.id), &sb.name, &sb.id, &sb.status);
            }
        });
}

fn menu_item(ui: &mut egui::Ui, icon: MaterialIcon, label: &str, selected: bool) -> egui::Response {
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
            kind_chooser(ui, state);
            ui.add_space(12.0);
            let filters = Filters {
                kinds: state.kinds.iter().cloned().collect(),
                sandbox: state.selected_sandbox.clone().unwrap_or_default(),
                search: String::new(),
            };
            let visible = visible_indices(&state.rows, &filters);
            egui::ScrollArea::vertical()
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
    let clicked_out = ui.input(|i| i.pointer.any_pressed())
        && !popup.response.contains_pointer()
        && !control.contains_pointer();
    if clicked_out || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        state.kind_open = false;
    }
}

fn control_button(ui: &mut egui::Ui, label: &str, open: bool) -> egui::Response {
    let border = if open { CATEGORY } else { BORDER };
    Frame::new()
        .fill(INPUT_FILL)
        .stroke(Stroke::new(1.0, border))
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
    Frame::new()
        .fill(MODAL_FILL)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(6))
        .show(ui, |ui| {
            ui.set_width(SELECT_WIDTH + 8.0);
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

fn dropdown_item(ui: &mut egui::Ui, label: &str, checked: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 30.0), Sense::click());
    if ui.is_rect_visible(rect) {
        if resp.hovered() {
            ui.painter()
                .rect_filled(rect, CornerRadius::same(5), HOVER_FILL);
        }
        let (icon, color) = if checked {
            (icons::ICON_CHECK_BOX, CATEGORY)
        } else {
            (icons::ICON_CHECK_BOX_OUTLINE_BLANK, TEXT_MUTED)
        };
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
    let when = friendly_time(&row.ts);
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
                "When",
                &format!("{} ({})", row.when, relative_time(&row.ts)),
                Some(&row.ts),
            );
            sandbox_field(ui, &row.run);
            if let Some(integration) = &row.integration {
                field(ui, "Integration", integration, None);
            }
            if let Some(obj) = row.raw.as_object() {
                for (key, value) in obj {
                    if RAW_SKIP.contains(&key.as_str()) {
                        continue;
                    }
                    field(ui, &humanize(key), &render_value(value), None);
                }
            }
            ui.add_space(6.0);
            ui.label(RichText::new("RAW").size(FS_LABEL).color(TEXT_MUTED));
            ui.add_space(4.0);
            let json = serde_json::to_string_pretty(&row.raw).unwrap_or_default();
            Frame::new()
                .fill(INPUT_FILL)
                .corner_radius(CornerRadius::same(6))
                .inner_margin(Margin::same(10))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.add(
                        egui::Label::new(
                            RichText::new(json)
                                .monospace()
                                .size(FS_LABEL)
                                .color(TEXT_MUTED),
                        )
                        .selectable(true),
                    );
                });
        });
}

const RAW_SKIP: [&str; 6] = ["prev_hash", "ts", "type", "event", "run", "microvm"];

fn field(ui: &mut egui::Ui, label: &str, value: &str, copy: Option<&str>) {
    ui.label(RichText::new(label).size(FS_LABEL).color(TEXT_MUTED));
    ui.add_space(2.0);
    match copy {
        Some(text) => {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(RichText::new(value).size(FS_BODY).color(TEXT_PRIMARY));
                if copy_button(ui).clicked() {
                    ui.ctx().copy_text(text.to_string());
                }
            });
        }
        None => {
            ui.label(RichText::new(value).size(FS_BODY).color(TEXT_PRIMARY));
        }
    }
    ui.add_space(10.0);
}

fn sandbox_field(ui: &mut egui::Ui, run: &str) {
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
        if copy_button(ui).on_hover_text("Copy full run id").clicked() {
            ui.ctx().copy_text(run.to_string());
        }
    });
    ui.add_space(10.0);
}

fn copy_button(ui: &mut egui::Ui) -> egui::Response {
    let glyph = RichText::new(icons::ICON_CONTENT_COPY.codepoint)
        .font(FontId::new(14.0, icons::ICON_CONTENT_COPY.font_family()))
        .color(TEXT_MUTED);
    ui.add(
        egui::Button::new(glyph)
            .frame(false)
            .min_size(Vec2::splat(20.0)),
    )
    .on_hover_cursor(CursorIcon::PointingHand)
}

fn render_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(items) => items
            .iter()
            .map(render_value)
            .collect::<Vec<_>>()
            .join(", "),
        other => other.to_string(),
    }
}

fn humanize(key: &str) -> String {
    let mut chars = key.replace('_', " ").chars().collect::<Vec<_>>();
    if let Some(first) = chars.first_mut() {
        first.make_ascii_uppercase();
    }
    chars.into_iter().collect()
}

fn friendly_time(ts: &str) -> String {
    let Some(event) = rfc3339_to_unix(ts) else {
        return ts.to_string();
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(event);
    let delta = now - event;
    if delta < 45 {
        "just now".to_string()
    } else if delta < 5_400 {
        format!("{}m ago", (delta / 60).max(1))
    } else if delta < 86_400 {
        format!("{}h ago", delta / 3_600)
    } else if delta < 6 * 86_400 {
        format!("{}d ago", delta / 86_400)
    } else {
        ts.get(0..10).unwrap_or(ts).to_string()
    }
}

fn relative_time(ts: &str) -> String {
    let Some(event) = rfc3339_to_unix(ts) else {
        return String::new();
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(event);
    let delta = (now - event).max(0);
    if delta < 45 {
        "just now".to_string()
    } else if delta < 5_400 {
        format!("{}m ago", (delta / 60).max(1))
    } else if delta < 86_400 {
        format!("{}h ago", delta / 3_600)
    } else {
        format!("{}d ago", delta / 86_400)
    }
}

fn rfc3339_to_unix(ts: &str) -> Option<i64> {
    let field = |range: std::ops::Range<usize>| ts.get(range).and_then(|s| s.parse::<i64>().ok());
    let year = field(0..4)?;
    let month = field(5..7)?;
    let day = field(8..10)?;
    let hour = field(11..13)?;
    let minute = field(14..16)?;
    let second = field(17..19)?;
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second)
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
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
                .stroke(Stroke::new(1.0, BORDER))
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

fn glyph(ui: &mut egui::Ui, icon: MaterialIcon, color: Color32, size: f32) {
    ui.label(
        RichText::new(icon.codepoint)
            .font(FontId::new(size, icon.font_family()))
            .color(color),
    );
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
