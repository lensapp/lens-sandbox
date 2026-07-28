use std::time::Duration;

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, CursorIcon, FontId, Frame, Layout, Margin,
    RichText, Sense, Stroke, vec2,
};
use egui_material_icons::icons;
use lns_audit::TimelineRow;
use serde_json::json;

use super::{
    ACCENT_GREEN, BORDER, CATEGORY, CHROME_FILL, CONTENT_FILL, CredentialBinding,
    CredentialOperation, CredentialStatus, CredentialSummary, DETAIL_WIDTH, FS_BODY, FS_LABEL,
    FS_SECONDARY, INPUT_FILL, MODAL_FILL, SELECT_FILL, SIDEBAR_WIDTH, STATUS_WARNING, TEXT_MUTED,
    TEXT_PRIMARY, TRAFFIC_LIGHT_INSET, dashboard_banner, dashboard_section_label, detail_panel,
    glyph, icon_button, row_click, status_dot,
};
use crate::ui::button::{Button, ButtonKind};
use crate::ui::theme;

const VALUE_REVEAL_SECONDS: f64 = 15.0;
const TABLE_CHEVRON_WIDTH: f32 = 28.0;
const TABLE_PRIMARY_MIN: f32 = 220.0;
const TABLE_MACHINE_MIN: f32 = 120.0;
const TABLE_ACCESS_MIN: f32 = 135.0;
const TABLE_ACTIVITY_MIN: f32 = 105.0;
const TABLE_SANDBOX_MIN: f32 = 140.0;
const TABLE_WHEN_MIN: f32 = 110.0;
const TABLE_RESIZER_WIDTH: f32 = 10.0;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DashboardView {
    #[default]
    Credentials,
    Audit,
}

impl DashboardView {
    fn label(self) -> &'static str {
        match self {
            Self::Credentials => "Credentials",
            Self::Audit => "Audit",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashboardSandbox {
    pub id: String,
    pub name: String,
    pub project: String,
    pub image: String,
    pub status: String,
    pub run_ids: Vec<String>,
}

#[derive(Clone, PartialEq, Eq)]
pub struct DashboardCredential {
    pub summary: CredentialSummary,
    pub value: Option<String>,
    pub placeholder: Option<String>,
}

impl std::fmt::Debug for DashboardCredential {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DashboardCredential")
            .field("summary", &self.summary)
            .field("value", &self.value.as_ref().map(|_| "<redacted>"))
            .field("placeholder", &self.placeholder)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RevealedKind {
    Value,
    Placeholder,
}

#[derive(Debug, Clone)]
struct RevealedValue {
    connector_id: String,
    kind: RevealedKind,
    at: f64,
}

#[derive(Debug)]
struct TableColumnWidths {
    primary: Option<f32>,
    credential_machine: f32,
    credential_access: f32,
    audit_sandbox: f32,
}

impl Default for TableColumnWidths {
    fn default() -> Self {
        Self {
            primary: None,
            credential_machine: 160.0,
            credential_access: 170.0,
            audit_sandbox: 180.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DashboardMode {
    Preview,
    Live,
}

pub enum CredentialReviewChoice {
    UseHost,
    UseValue(String),
    Connect,
    Deny,
}

impl std::fmt::Debug for CredentialReviewChoice {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UseHost => formatter.write_str("UseHost"),
            Self::UseValue(_) => formatter.write_str("UseValue(<redacted>)"),
            Self::Connect => formatter.write_str("Connect"),
            Self::Deny => formatter.write_str("Deny"),
        }
    }
}

impl Drop for CredentialReviewChoice {
    fn drop(&mut self) {
        if let Self::UseValue(value) = self {
            clear_secret(value);
        }
    }
}

pub enum DashboardCommand {
    ReviewCredential {
        request_id: String,
        choice: CredentialReviewChoice,
    },
    ReplaceCredential {
        connector_id: String,
        value: String,
    },
    RemoveCredential {
        connector_id: String,
    },
    RevokeSandbox {
        connector_id: String,
        sandbox_id: String,
    },
}

impl std::fmt::Debug for DashboardCommand {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ReviewCredential { request_id, choice } => formatter
                .debug_struct("ReviewCredential")
                .field("request_id", request_id)
                .field("choice", choice)
                .finish(),
            Self::ReplaceCredential { connector_id, .. } => formatter
                .debug_struct("ReplaceCredential")
                .field("connector_id", connector_id)
                .field("value", &"<redacted>")
                .finish(),
            Self::RemoveCredential { connector_id } => formatter
                .debug_struct("RemoveCredential")
                .field("connector_id", connector_id)
                .finish(),
            Self::RevokeSandbox {
                connector_id,
                sandbox_id,
            } => formatter
                .debug_struct("RevokeSandbox")
                .field("connector_id", connector_id)
                .field("sandbox_id", sandbox_id)
                .finish(),
        }
    }
}

impl Drop for DashboardCommand {
    fn drop(&mut self) {
        if let Self::ReplaceCredential { value, .. } = self {
            clear_secret(value);
        }
    }
}

pub struct UnifiedDashboardState {
    pub view: DashboardView,
    pub sandboxes: Vec<DashboardSandbox>,
    pub credentials: Vec<DashboardCredential>,
    pub audit: super::DashboardState,
    pub selected_sandbox: Option<String>,
    pub selected_connector: Option<String>,
    pub sidebar_open: bool,
    pub notice: Option<String>,
    pub confirmation: Option<CredentialOperation>,
    reviewing_request: Option<String>,
    replacing_connector: Option<String>,
    replacement_value: String,
    review_value: String,
    pending_command: Option<DashboardCommand>,
    revealed: Option<RevealedValue>,
    table_columns: TableColumnWidths,
    mode: DashboardMode,
}

impl UnifiedDashboardState {
    pub fn seeded(
        sandboxes: Vec<DashboardSandbox>,
        credentials: Vec<DashboardCredential>,
        rows: Vec<TimelineRow>,
        warnings: Vec<String>,
    ) -> Self {
        Self {
            view: DashboardView::Credentials,
            sandboxes,
            credentials,
            audit: super::DashboardState::seeded(rows, warnings, Vec::new()),
            selected_sandbox: None,
            selected_connector: None,
            sidebar_open: true,
            notice: None,
            confirmation: None,
            reviewing_request: None,
            replacing_connector: None,
            replacement_value: String::new(),
            review_value: String::new(),
            pending_command: None,
            revealed: None,
            table_columns: TableColumnWidths::default(),
            mode: DashboardMode::Preview,
        }
    }

    pub fn live(
        sandboxes: Vec<DashboardSandbox>,
        credentials: Vec<DashboardCredential>,
        rows: Vec<TimelineRow>,
        warnings: Vec<String>,
        last_error: Option<String>,
    ) -> Self {
        let mut state = Self::seeded(sandboxes, credentials, rows, warnings);
        state.mode = DashboardMode::Live;
        state.audit.last_error = last_error;
        state
    }

    pub fn replace_live_data(
        &mut self,
        sandboxes: Vec<DashboardSandbox>,
        credentials: Vec<DashboardCredential>,
        rows: Vec<TimelineRow>,
        warnings: Vec<String>,
        last_error: Option<String>,
    ) {
        self.sandboxes = sandboxes;
        self.credentials = credentials;
        self.audit.rows = rows;
        self.audit.warnings = warnings;
        self.audit.last_error = last_error;
        self.mode = DashboardMode::Live;
        if self
            .selected_sandbox
            .as_ref()
            .is_some_and(|selected| !self.sandboxes.iter().any(|sandbox| &sandbox.id == selected))
        {
            self.selected_sandbox = None;
        }
        if self.selected_connector.as_ref().is_some_and(|selected| {
            !self
                .credentials
                .iter()
                .any(|credential| &credential.summary.connector_id == selected)
        }) {
            self.selected_connector = None;
            self.revealed = None;
        }
        if self
            .audit
            .selected
            .is_some_and(|selected| selected >= self.audit.rows.len())
        {
            self.audit.selected = None;
            self.audit.detail_row = None;
        }
    }

    fn selected_sandbox(&self) -> Option<&DashboardSandbox> {
        let selected = self.selected_sandbox.as_deref()?;
        self.sandboxes.iter().find(|sandbox| sandbox.id == selected)
    }

    fn selected_credential(&self) -> Option<&DashboardCredential> {
        let selected = self.selected_connector.as_deref()?;
        self.credentials
            .iter()
            .find(|credential| credential.summary.connector_id == selected)
    }

    fn credential_matches_scope(&self, credential: &CredentialSummary) -> bool {
        let Some(selected) = self.selected_sandbox.as_deref() else {
            return true;
        };
        credential
            .sandboxes
            .iter()
            .any(|access| access.sandbox_id == selected)
            || credential
                .pending
                .as_ref()
                .is_some_and(|request| request.sandbox_id == selected)
    }

    fn audit_matches_scope(&self, row: &TimelineRow) -> bool {
        self.selected_sandbox()
            .is_none_or(|sandbox| sandbox.run_ids.iter().any(|run| run == &row.run))
    }

    fn pending_count(&self, sandbox_id: Option<&str>) -> usize {
        self.credentials
            .iter()
            .filter_map(|credential| credential.summary.pending.as_ref())
            .filter(|request| sandbox_id.is_none_or(|id| request.sandbox_id == id))
            .count()
    }
}

pub enum UnifiedDashboardAction {
    None,
    Refresh,
    Command(DashboardCommand),
}

pub fn unified_viewport_builder() -> egui::ViewportBuilder {
    super::viewport_builder()
        .with_title("Lens Sandbox — Dashboard")
        .with_inner_size([1120.0, 720.0])
        .with_min_inner_size([760.0, 480.0])
}

pub fn render_unified_dashboard(
    ui: &mut egui::Ui,
    state: &mut UnifiedDashboardState,
) -> UnifiedDashboardAction {
    ui.ctx().set_visuals(super::dashboard_visuals());
    expire_revealed_value(ui.ctx(), state);
    if ui
        .ctx()
        .input(|input| input.modifiers.command && input.key_pressed(egui::Key::K))
    {
        state.audit.search_open = true;
    }
    unified_sidebar_toggle(ui, state);
    let action = unified_refresh_button(ui, state.mode);
    if state.sidebar_open {
        unified_sidebar(ui, state);
    }
    match state.view {
        DashboardView::Credentials if state.selected_connector.is_some() => {
            credential_detail_panel(ui, state);
        }
        DashboardView::Audit if state.audit.selected.is_some() => {
            let reveal = ui.ctx().animate_bool_with_time(
                egui::Id::new("unified-audit-detail-anim"),
                state.audit.selected.is_some(),
                0.16,
            );
            let credential_link = state
                .audit
                .detail_row
                .as_ref()
                .and_then(|row| row.connector.as_deref())
                .is_some_and(|connector| {
                    state
                        .credentials
                        .iter()
                        .any(|credential| credential.summary.connector_id == connector)
                });
            if let Some(connector) = detail_panel(ui, &mut state.audit, reveal, credential_link) {
                state.view = DashboardView::Credentials;
                state.selected_connector = Some(connector);
                state.audit.selected = None;
                state.audit.detail_row = None;
                state.revealed = None;
            }
        }
        _ => {}
    }
    unified_central(ui, state);
    let search_reveal = ui.ctx().animate_bool_with_time(
        egui::Id::new("unified-dashboard-search-anim"),
        state.audit.search_open,
        0.12,
    );
    if state.audit.search_open || search_reveal > 0.002 {
        unified_search_modal(ui, state, search_reveal);
    }
    if state.confirmation.is_some() {
        confirmation_modal(ui, state);
    }
    if state.reviewing_request.is_some() {
        review_modal(ui, state);
    }
    if state.replacing_connector.is_some() {
        replace_modal(ui, state);
    }
    state
        .pending_command
        .take()
        .map_or(action, UnifiedDashboardAction::Command)
}

fn expire_revealed_value(ctx: &egui::Context, state: &mut UnifiedDashboardState) {
    let now = ctx.input(|input| input.time);
    let lost_focus = ctx.input(|input| input.viewport().focused == Some(false));
    if state
        .revealed
        .as_ref()
        .is_some_and(|revealed| lost_focus || now - revealed.at >= VALUE_REVEAL_SECONDS)
    {
        state.revealed = None;
    }
    if state.revealed.is_some() {
        ctx.request_repaint_after(Duration::from_millis(250));
    }
}

fn unified_refresh_button(ui: &mut egui::Ui, mode: DashboardMode) -> UnifiedDashboardAction {
    let clicked = egui::Area::new(egui::Id::new("unified-dashboard-refresh"))
        .anchor(Align2::RIGHT_TOP, vec2(-6.0, 5.0))
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            icon_button(ui, icons::ICON_REFRESH)
                .on_hover_text(if mode == DashboardMode::Preview {
                    "Refresh preview"
                } else {
                    "Refresh"
                })
                .clicked()
        })
        .inner;
    if clicked {
        UnifiedDashboardAction::Refresh
    } else {
        UnifiedDashboardAction::None
    }
}

fn unified_sidebar_toggle(ui: &mut egui::Ui, state: &mut UnifiedDashboardState) {
    let icon = if state.sidebar_open {
        icons::ICON_LEFT_PANEL_CLOSE
    } else {
        icons::ICON_LEFT_PANEL_OPEN
    };
    let clicked = egui::Area::new(egui::Id::new("unified-dashboard-sidebar-toggle"))
        .fixed_pos(egui::pos2(TRAFFIC_LIGHT_INSET, 5.0))
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            icon_button(ui, icon)
                .on_hover_text("Toggle sandbox list")
                .clicked()
        })
        .inner;
    if clicked {
        state.sidebar_open = !state.sidebar_open;
    }
}

fn unified_sidebar(ui: &mut egui::Ui, state: &mut UnifiedDashboardState) {
    let sandboxes = state.sandboxes.clone();
    egui::Panel::left("unified-dashboard-sidebar")
        .resizable(true)
        .default_size(SIDEBAR_WIDTH)
        .min_size(190.0)
        .max_size(340.0)
        .show_separator_line(true)
        .frame(Frame::new().fill(CHROME_FILL).inner_margin(Margin::same(8)))
        .show_inside(ui, |ui| {
            ui.add_space(26.0);
            if search_sidebar_row(ui).clicked() {
                state.audit.search_open = true;
            }
            ui.add_space(2.0);
            if sidebar_row(
                ui,
                icons::ICON_DNS,
                "All sandboxes",
                "",
                state.selected_sandbox.is_none(),
                None,
                None,
            )
            .clicked()
            {
                select_sandbox(state, None);
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                ui.label(RichText::new("SANDBOXES").size(FS_LABEL).color(TEXT_MUTED));
            });
            ui.add_space(2.0);
            for sandbox in sandboxes {
                let pending = state.pending_count(Some(&sandbox.id));
                let selected = state.selected_sandbox.as_deref() == Some(&sandbox.id);
                if sidebar_row(
                    ui,
                    icons::ICON_DNS,
                    &sandbox.name,
                    &sandbox.project,
                    selected,
                    Some(&sandbox.status),
                    (pending > 0).then_some(pending),
                )
                .clicked()
                {
                    select_sandbox(state, Some(sandbox.id));
                }
            }
        });
}

fn search_sidebar_row(ui: &mut egui::Ui) -> egui::Response {
    let response = Frame::new()
        .fill(Color32::TRANSPARENT)
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(8, 7))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                glyph(ui, icons::ICON_SEARCH, TEXT_MUTED, 18.0);
                ui.add_space(7.0);
                ui.label(RichText::new("Search").size(FS_BODY).color(TEXT_PRIMARY));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new("⌘K").size(FS_LABEL).color(TEXT_MUTED));
                });
            });
        })
        .response
        .interact(Sense::click());
    row_click(&response);
    response
}

fn sidebar_row(
    ui: &mut egui::Ui,
    icon: egui_material_icons::MaterialIcon,
    label: &str,
    detail: &str,
    selected: bool,
    status: Option<&str>,
    attention: Option<usize>,
) -> egui::Response {
    let response = Frame::new()
        .fill(if selected {
            SELECT_FILL
        } else {
            Color32::TRANSPARENT
        })
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(8, 7))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                if let Some(status) = status {
                    status_dot(ui, status);
                } else {
                    glyph(ui, icon, TEXT_MUTED, 18.0);
                }
                ui.add_space(7.0);
                ui.vertical(|ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(label).size(FS_BODY).color(TEXT_PRIMARY));
                        if let Some(count) = attention {
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                compact_attention_badge(ui, count);
                            });
                        }
                    });
                    if !detail.is_empty() {
                        ui.add(
                            egui::Label::new(
                                RichText::new(detail).size(FS_LABEL).color(TEXT_MUTED),
                            )
                            .truncate(),
                        );
                    }
                });
            });
        })
        .response
        .interact(Sense::click());
    row_click(&response);
    response
}

fn select_sandbox(state: &mut UnifiedDashboardState, id: Option<String>) {
    state.selected_sandbox = id;
    state.selected_connector = None;
    state.audit.selected = None;
    state.audit.detail_row = None;
    state.revealed = None;
}

fn unified_central(ui: &mut egui::Ui, state: &mut UnifiedDashboardState) {
    egui::CentralPanel::default()
        .frame(
            Frame::new()
                .fill(CONTENT_FILL)
                .inner_margin(Margin::same(theme::STACK_MARGIN)),
        )
        .show_inside(ui, |ui| {
            ui.add_space(12.0);
            view_tabs(ui, state);
            if let Some(notice) = state.notice.as_deref() {
                ui.add_space(12.0);
                inline_notice(ui, notice);
            }
            ui.add_space(18.0);
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| match state.view {
                    DashboardView::Credentials => credentials(ui, state),
                    DashboardView::Audit => audit(ui, state),
                });
        });
}

fn view_tabs(ui: &mut egui::Ui, state: &mut UnifiedDashboardState) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 24.0;
        for view in [DashboardView::Credentials, DashboardView::Audit] {
            if view_tab(ui, view, state.view == view).clicked() {
                state.view = view;
                state.selected_connector = None;
                state.audit.selected = None;
                state.revealed = None;
            }
        }
    });
}

fn view_tab(ui: &mut egui::Ui, view: DashboardView, selected: bool) -> egui::Response {
    let response = ui
        .add(
            egui::Button::new(
                RichText::new(view.label())
                    .size(FS_BODY)
                    .color(if selected { TEXT_PRIMARY } else { TEXT_MUTED }),
            )
            .frame(false)
            .sense(Sense::click()),
        )
        .on_hover_cursor(CursorIcon::PointingHand);
    if selected {
        ui.painter().line_segment(
            [response.rect.left_bottom(), response.rect.right_bottom()],
            Stroke::new(2.0, CATEGORY),
        );
    }
    response
}

fn inline_notice(ui: &mut egui::Ui, notice: &str) {
    ui.horizontal(|ui| {
        glyph(ui, icons::ICON_CHECK_CIRCLE, ACCENT_GREEN, 15.0);
        ui.label(RichText::new(notice).size(FS_SECONDARY).color(TEXT_MUTED));
    });
}

fn credentials(ui: &mut egui::Ui, state: &mut UnifiedDashboardState) {
    let mut indices: Vec<usize> = state
        .credentials
        .iter()
        .enumerate()
        .filter(|(_, credential)| state.credential_matches_scope(&credential.summary))
        .map(|(index, _)| index)
        .collect();
    if indices.is_empty() {
        ui.colored_label(TEXT_MUTED, "No credentials match this sandbox.");
        return;
    }
    indices.sort_by(|left, right| {
        credential_rank(state.credentials[*left].summary.status)
            .cmp(&credential_rank(state.credentials[*right].summary.status))
            .then_with(|| {
                state.credentials[*left]
                    .summary
                    .display_name
                    .cmp(&state.credentials[*right].summary.display_name)
            })
    });
    let table_width = ui.available_width() - 12.0;
    let columns = CredentialColumns::for_width(table_width, &state.table_columns);
    credential_table_header(ui, columns, &mut state.table_columns);
    ui.separator();
    for index in indices {
        let credential = &state.credentials[index].summary;
        let selected = state.selected_connector.as_deref() == Some(&credential.connector_id);
        let response = credential_table_row(ui, state, credential, columns, selected);
        if response.clicked() {
            state.selected_connector = Some(credential.connector_id.clone());
            state.revealed = None;
        }
        ui.separator();
    }
}

fn credential_rank(status: CredentialStatus) -> u8 {
    match status {
        CredentialStatus::Pending => 0,
        CredentialStatus::Expiring | CredentialStatus::Expired => 1,
        CredentialStatus::Active => 2,
        CredentialStatus::Denied | CredentialStatus::Unavailable => 3,
    }
}

#[derive(Clone, Copy)]
struct CredentialColumns {
    connector: f32,
    machine: f32,
    access: f32,
    activity: f32,
    wide: bool,
}

impl CredentialColumns {
    fn for_width(width: f32, stored: &TableColumnWidths) -> Self {
        let wide = width >= 680.0;
        let connector = shared_primary_width(width, wide, stored.primary);
        let remaining = width - connector - TABLE_CHEVRON_WIDTH;
        let machine = if wide {
            stored.credential_machine.clamp(
                TABLE_MACHINE_MIN,
                remaining - TABLE_ACCESS_MIN - TABLE_ACTIVITY_MIN,
            )
        } else {
            0.0
        };
        let access = if wide {
            stored
                .credential_access
                .clamp(TABLE_ACCESS_MIN, remaining - machine - TABLE_ACTIVITY_MIN)
        } else {
            remaining
        };
        let activity = if wide {
            remaining - machine - access
        } else {
            0.0
        };
        Self {
            connector,
            machine,
            access,
            activity,
            wide,
        }
    }
}

fn shared_primary_width(width: f32, wide: bool, stored: Option<f32>) -> f32 {
    let default = if wide {
        width - 160.0 - 170.0 - 125.0 - TABLE_CHEVRON_WIDTH
    } else {
        width - 175.0 - TABLE_CHEVRON_WIDTH
    };
    let maximum = if wide {
        width - TABLE_MACHINE_MIN - TABLE_ACCESS_MIN - TABLE_ACTIVITY_MIN - TABLE_CHEVRON_WIDTH
    } else {
        width - TABLE_ACCESS_MIN - TABLE_CHEVRON_WIDTH
    }
    .max(120.0);
    stored
        .unwrap_or(default)
        .clamp(TABLE_PRIMARY_MIN.min(maximum), maximum)
}

fn credential_table_header(
    ui: &mut egui::Ui,
    columns: CredentialColumns,
    stored: &mut TableColumnWidths,
) {
    Frame::new()
        .inner_margin(Margin::symmetric(6, 0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                exact_cell(
                    ui,
                    columns.connector - TABLE_RESIZER_WIDTH,
                    18.0,
                    Layout::left_to_right(Align::Center),
                    |ui| table_header_label(ui, "Credential"),
                );
                if let Some(width) =
                    table_column_resizer(ui, "shared-primary-column", columns.connector)
                {
                    stored.primary = Some(width);
                }
                if columns.wide {
                    table_header_cell(ui, "On this machine", columns.machine - TABLE_RESIZER_WIDTH);
                    if let Some(width) =
                        table_column_resizer(ui, "credential-machine-column", columns.machine)
                    {
                        stored.credential_machine = width;
                    }
                }
                table_header_cell(
                    ui,
                    "Sandbox access",
                    columns.access
                        - if columns.wide {
                            TABLE_RESIZER_WIDTH
                        } else {
                            0.0
                        },
                );
                if columns.wide {
                    if let Some(width) =
                        table_column_resizer(ui, "credential-access-column", columns.access)
                    {
                        stored.credential_access = width;
                    }
                    table_header_cell(ui, "Last activity", columns.activity);
                }
                exact_cell(
                    ui,
                    TABLE_CHEVRON_WIDTH,
                    18.0,
                    Layout::left_to_right(Align::Center),
                    |_| {},
                );
            });
        });
    ui.add_space(3.0);
}

fn table_header_cell(ui: &mut egui::Ui, label: &str, width: f32) {
    exact_cell(
        ui,
        width,
        18.0,
        Layout::left_to_right(Align::Center),
        |ui| table_header_label(ui, label),
    );
}

fn table_header_label(ui: &mut egui::Ui, label: &str) {
    ui.label(RichText::new(label).size(FS_LABEL).color(TEXT_MUTED));
}

fn table_column_resizer(ui: &mut egui::Ui, id: &'static str, current_width: f32) -> Option<f32> {
    let (rect, response) = ui
        .push_id(id, |ui| {
            ui.allocate_exact_size(vec2(TABLE_RESIZER_WIDTH, 18.0), Sense::drag())
        })
        .inner;
    let response = response.on_hover_cursor(CursorIcon::ResizeHorizontal);
    let color = if response.hovered() || response.dragged() {
        TEXT_MUTED
    } else {
        BORDER
    };
    ui.painter()
        .vline(rect.center().x, rect.y_range(), Stroke::new(1.0, color));
    if response.dragged() {
        Some(current_width + response.drag_delta().x)
    } else {
        None
    }
}

fn exact_cell(
    ui: &mut egui::Ui,
    width: f32,
    height: f32,
    layout: Layout,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .id_salt((rect.min.x.to_bits(), rect.min.y.to_bits()))
            .max_rect(rect)
            .layout(layout),
    );
    child.set_clip_rect(rect.intersect(ui.clip_rect()));
    add_contents(&mut child);
}

fn credential_table_row(
    ui: &mut egui::Ui,
    state: &UnifiedDashboardState,
    credential: &CredentialSummary,
    columns: CredentialColumns,
    selected: bool,
) -> egui::Response {
    let response = Frame::new()
        .fill(if selected {
            SELECT_FILL
        } else {
            Color32::TRANSPARENT
        })
        .corner_radius(CornerRadius::same(4))
        .inner_margin(Margin::symmetric(6, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                exact_cell(
                    ui,
                    columns.connector,
                    38.0,
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        glyph(
                            ui,
                            icons::ICON_KEY,
                            credential_row_icon_color(credential.status),
                            17.0,
                        );
                        ui.add_space(7.0);
                        ui.vertical(|ui| {
                            ui.label(
                                RichText::new(&credential.display_name)
                                    .size(FS_BODY)
                                    .color(TEXT_PRIMARY),
                            );
                            ui.label(
                                RichText::new(&credential.connector_id)
                                    .size(FS_LABEL)
                                    .monospace()
                                    .color(TEXT_MUTED),
                            );
                        });
                    },
                );
                if columns.wide {
                    let (machine, machine_color) = credential_machine_label(credential);
                    exact_cell(
                        ui,
                        columns.machine,
                        38.0,
                        Layout::left_to_right(Align::Center),
                        |ui| {
                            ui.label(
                                RichText::new(machine)
                                    .size(FS_SECONDARY)
                                    .color(machine_color),
                            );
                        },
                    );
                }
                let (access, access_color) = credential_access_label(state, credential);
                exact_cell(
                    ui,
                    columns.access,
                    38.0,
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        ui.label(RichText::new(access).size(FS_SECONDARY).color(access_color));
                    },
                );
                if columns.wide {
                    exact_cell(
                        ui,
                        columns.activity,
                        38.0,
                        Layout::left_to_right(Align::Center),
                        |ui| {
                            ui.label(
                                RichText::new(
                                    credential.recent_activity.as_deref().unwrap_or("Never"),
                                )
                                .size(FS_SECONDARY)
                                .color(TEXT_MUTED),
                            );
                        },
                    );
                }
                exact_cell(
                    ui,
                    TABLE_CHEVRON_WIDTH,
                    38.0,
                    Layout::centered_and_justified(egui::Direction::LeftToRight),
                    |ui| glyph(ui, icons::ICON_CHEVRON_RIGHT, TEXT_MUTED, 16.0),
                );
            });
        })
        .response
        .interact(Sense::click());
    row_click(&response);
    response
}

fn credential_row_icon_color(status: CredentialStatus) -> Color32 {
    match status {
        CredentialStatus::Active => TEXT_MUTED,
        CredentialStatus::Pending | CredentialStatus::Expiring => STATUS_WARNING,
        CredentialStatus::Expired | CredentialStatus::Denied | CredentialStatus::Unavailable => {
            super::credential_status_color(status)
        }
    }
}

fn credential_machine_label(credential: &CredentialSummary) -> (String, Color32) {
    match credential.status {
        CredentialStatus::Pending => ("Not configured".into(), TEXT_MUTED),
        CredentialStatus::Expiring => ("Expiring soon".into(), STATUS_WARNING),
        CredentialStatus::Expired => (
            "Expired".into(),
            super::credential_status_color(credential.status),
        ),
        CredentialStatus::Denied => (
            "Blocked".into(),
            super::credential_status_color(credential.status),
        ),
        CredentialStatus::Unavailable => (
            "Unavailable".into(),
            super::credential_status_color(credential.status),
        ),
        CredentialStatus::Active => {
            let label = match credential.binding {
                CredentialBinding::OAuth => "Signed in",
                CredentialBinding::Stored => "Stored",
                CredentialBinding::HostDetected => "Host value",
                CredentialBinding::Unbound => "Not configured",
                CredentialBinding::Denied => "Blocked",
            };
            (label.into(), TEXT_PRIMARY)
        }
    }
}

fn credential_access_label(
    state: &UnifiedDashboardState,
    credential: &CredentialSummary,
) -> (String, Color32) {
    if let Some(request) = credential.pending.as_ref()
        && state
            .selected_sandbox
            .as_deref()
            .is_none_or(|sandbox| sandbox == request.sandbox_id)
    {
        return ("Review request".into(), STATUS_WARNING);
    }
    if let Some(sandbox) = state.selected_sandbox.as_deref() {
        return credential
            .sandboxes
            .iter()
            .find(|access| access.sandbox_id == sandbox)
            .map(|access| {
                if access.active {
                    ("Authorized".into(), TEXT_PRIMARY)
                } else {
                    ("Not authorized".into(), TEXT_MUTED)
                }
            })
            .unwrap_or_else(|| ("Not authorized".into(), TEXT_MUTED));
    }
    let active = credential.active_sandbox_count();
    if active == 0 {
        ("No sandbox access".into(), TEXT_MUTED)
    } else if active == 1 {
        ("1 sandbox".into(), TEXT_PRIMARY)
    } else {
        (format!("{active} sandboxes"), TEXT_PRIMARY)
    }
}

fn audit(ui: &mut egui::Ui, state: &mut UnifiedDashboardState) {
    for warning in &state.audit.warnings {
        dashboard_banner(ui, warning, STATUS_WARNING);
        ui.add_space(10.0);
    }
    let indices: Vec<usize> = state
        .audit
        .rows
        .iter()
        .enumerate()
        .filter(|(_, row)| state.audit_matches_scope(row))
        .map(|(index, _)| index)
        .collect();
    if indices.is_empty() {
        ui.colored_label(TEXT_MUTED, "No audit events for this sandbox.");
        return;
    }
    let table_width = ui.available_width() - 12.0;
    let columns = AuditColumns::for_width(table_width, &state.table_columns);
    audit_table_header(ui, columns, &mut state.table_columns);
    ui.separator();
    for index in indices {
        audit_table_row(ui, state, index, columns);
        ui.separator();
    }
}

#[derive(Clone, Copy)]
struct AuditColumns {
    event: f32,
    sandbox: f32,
    when: f32,
    wide: bool,
}

impl AuditColumns {
    fn for_width(width: f32, stored: &TableColumnWidths) -> Self {
        let wide = width >= 680.0;
        let event = shared_primary_width(width, wide, stored.primary);
        let remaining = width - event - TABLE_CHEVRON_WIDTH;
        let sandbox = if wide {
            stored
                .audit_sandbox
                .clamp(TABLE_SANDBOX_MIN, remaining - TABLE_WHEN_MIN)
        } else {
            0.0
        };
        let when = remaining - sandbox;
        Self {
            event,
            sandbox,
            when,
            wide,
        }
    }
}

fn audit_table_header(ui: &mut egui::Ui, columns: AuditColumns, stored: &mut TableColumnWidths) {
    Frame::new()
        .inner_margin(Margin::symmetric(6, 0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                exact_cell(
                    ui,
                    columns.event - TABLE_RESIZER_WIDTH,
                    18.0,
                    Layout::left_to_right(Align::Center),
                    |ui| table_header_label(ui, "Event"),
                );
                if let Some(width) =
                    table_column_resizer(ui, "shared-primary-column", columns.event)
                {
                    stored.primary = Some(width);
                }
                if columns.wide {
                    table_header_cell(ui, "Sandbox", columns.sandbox - TABLE_RESIZER_WIDTH);
                    if let Some(width) =
                        table_column_resizer(ui, "audit-sandbox-column", columns.sandbox)
                    {
                        stored.audit_sandbox = width;
                    }
                }
                table_header_cell(ui, "When", columns.when);
                exact_cell(
                    ui,
                    TABLE_CHEVRON_WIDTH,
                    18.0,
                    Layout::left_to_right(Align::Center),
                    |_| {},
                );
            });
        });
    ui.add_space(3.0);
}

fn audit_table_row(
    ui: &mut egui::Ui,
    state: &mut UnifiedDashboardState,
    index: usize,
    columns: AuditColumns,
) {
    let row = state.audit.rows[index].clone();
    let selected = state.audit.selected == Some(index);
    let sandbox = state
        .sandboxes
        .iter()
        .find(|sandbox| sandbox.run_ids.iter().any(|run| run == &row.run))
        .map_or_else(
            || lns_ipc::short_run_id(&row.run).to_string(),
            |sandbox| sandbox.name.clone(),
        );
    let when = super::format::friendly_time(super::now_unix_secs(), &row.ts);
    let response = Frame::new()
        .fill(if selected {
            SELECT_FILL
        } else {
            Color32::TRANSPARENT
        })
        .corner_radius(CornerRadius::same(4))
        .inner_margin(Margin::symmetric(6, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                exact_cell(
                    ui,
                    columns.event,
                    38.0,
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        glyph(
                            ui,
                            super::kind_icon(&row.kind),
                            super::kind_color(&row.kind),
                            16.0,
                        );
                        ui.add_space(7.0);
                        ui.add(
                            egui::Label::new(
                                RichText::new(&row.detail).size(FS_BODY).color(TEXT_PRIMARY),
                            )
                            .truncate(),
                        );
                    },
                );
                if columns.wide {
                    exact_cell(
                        ui,
                        columns.sandbox,
                        38.0,
                        Layout::left_to_right(Align::Center),
                        |ui| {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&sandbox).size(FS_SECONDARY).color(TEXT_MUTED),
                                )
                                .truncate(),
                            );
                        },
                    );
                }
                exact_cell(
                    ui,
                    columns.when,
                    38.0,
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        ui.label(RichText::new(&when).size(FS_SECONDARY).color(TEXT_MUTED));
                    },
                );
                exact_cell(
                    ui,
                    TABLE_CHEVRON_WIDTH,
                    38.0,
                    Layout::centered_and_justified(egui::Direction::LeftToRight),
                    |ui| glyph(ui, icons::ICON_CHEVRON_RIGHT, TEXT_MUTED, 16.0),
                );
            });
        })
        .response
        .interact(Sense::click());
    row_click(&response);
    if response.clicked() {
        state.audit.selected = Some(index);
        state.audit.detail_row = Some(row);
    }
}

#[derive(Debug, Clone)]
enum UnifiedSearchPick {
    Credential(String),
    Audit(usize),
}

fn unified_search_modal(ui: &mut egui::Ui, state: &mut UnifiedDashboardState, reveal: f32) {
    let mut pick = None;
    let backdrop_alpha = (14.0 * reveal) as u8;
    let shadow = egui::epaint::Shadow {
        offset: [0, 10],
        blur: 32,
        spread: 2,
        color: Color32::from_black_alpha((160.0 * reveal) as u8),
    };
    let modal = egui::Modal::new(egui::Id::new("unified-dashboard-search"))
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
                egui::TextEdit::singleline(&mut state.audit.search_query)
                    .hint_text("Search credentials and activity")
                    .background_color(MODAL_FILL)
                    .font(FontId::new(18.0, egui::FontFamily::Proportional))
                    .margin(Margin::symmetric(2, 8))
                    .desired_width(f32::INFINITY),
            )
            .request_focus();
            ui.add_space(10.0);
            let needle = state.audit.search_query.trim().to_lowercase();
            let credential_results: Vec<usize> = state
                .credentials
                .iter()
                .enumerate()
                .filter(|(_, credential)| state.credential_matches_scope(&credential.summary))
                .filter(|(_, credential)| {
                    needle.is_empty() || credential_contains(&credential.summary, &needle)
                })
                .map(|(index, _)| index)
                .collect();
            let audit_results: Vec<usize> = state
                .audit
                .rows
                .iter()
                .enumerate()
                .filter(|(_, row)| state.audit_matches_scope(row))
                .filter(|(_, row)| needle.is_empty() || audit_row_contains(row, &needle))
                .map(|(index, _)| index)
                .collect();
            if credential_results.is_empty() && audit_results.is_empty() {
                let message = if needle.is_empty() {
                    "No credentials or activity."
                } else {
                    "No matching credentials or activity."
                };
                ui.colored_label(TEXT_MUTED, message);
            }
            egui::ScrollArea::vertical()
                .max_height(360.0)
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if !credential_results.is_empty() {
                        search_group_label(ui, "Credentials");
                        for &index in &credential_results {
                            if search_credential_row(ui, &state.credentials[index].summary)
                                .clicked()
                            {
                                pick = Some(UnifiedSearchPick::Credential(
                                    state.credentials[index].summary.connector_id.clone(),
                                ));
                            }
                        }
                    }
                    if !audit_results.is_empty() {
                        if !credential_results.is_empty() {
                            ui.add_space(8.0);
                        }
                        search_group_label(ui, "Audit");
                        for &index in &audit_results {
                            if search_audit_row(ui, state, &state.audit.rows[index]).clicked() {
                                pick = Some(UnifiedSearchPick::Audit(index));
                            }
                        }
                    }
                });
        });
    match pick {
        Some(UnifiedSearchPick::Credential(connector_id)) => {
            state.view = DashboardView::Credentials;
            state.selected_connector = Some(connector_id);
            state.audit.selected = None;
            state.audit.detail_row = None;
            state.audit.search_open = false;
        }
        Some(UnifiedSearchPick::Audit(index)) => {
            state.view = DashboardView::Audit;
            state.selected_connector = None;
            state.audit.selected = Some(index);
            state.audit.detail_row = Some(state.audit.rows[index].clone());
            state.audit.search_open = false;
        }
        None if state.audit.search_open && modal.should_close() => {
            state.audit.search_open = false;
        }
        None => {}
    }
}

fn search_group_label(ui: &mut egui::Ui, label: &str) {
    ui.label(RichText::new(label).size(FS_LABEL).color(TEXT_MUTED));
    ui.add_space(3.0);
}

fn search_credential_row(ui: &mut egui::Ui, credential: &CredentialSummary) -> egui::Response {
    search_row(
        ui,
        icons::ICON_KEY,
        super::credential_status_color(credential.status),
        &credential.display_name,
        &credential.connector_id,
        "Credential",
    )
}

fn search_audit_row(
    ui: &mut egui::Ui,
    state: &UnifiedDashboardState,
    row: &TimelineRow,
) -> egui::Response {
    let sandbox = state
        .sandboxes
        .iter()
        .find(|sandbox| sandbox.run_ids.iter().any(|run| run == &row.run))
        .map_or_else(
            || lns_ipc::short_run_id(&row.run).to_string(),
            |sandbox| sandbox.name.clone(),
        );
    search_row(
        ui,
        super::kind_icon(&row.kind),
        super::kind_color(&row.kind),
        &row.detail,
        &sandbox,
        "Audit",
    )
}

fn search_row(
    ui: &mut egui::Ui,
    icon: egui_material_icons::MaterialIcon,
    color: Color32,
    title: &str,
    detail: &str,
    kind: &str,
) -> egui::Response {
    let response = Frame::new()
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(6, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                glyph(ui, icon, color, 16.0);
                ui.add_space(7.0);
                ui.vertical(|ui| {
                    ui.add(
                        egui::Label::new(RichText::new(title).size(FS_BODY).color(TEXT_PRIMARY))
                            .truncate(),
                    );
                    ui.label(
                        RichText::new(detail)
                            .size(FS_LABEL)
                            .monospace()
                            .color(TEXT_MUTED),
                    );
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(kind).size(FS_LABEL).color(TEXT_MUTED));
                });
            });
        })
        .response
        .interact(Sense::click());
    row_click(&response);
    response
}

fn credential_contains(credential: &CredentialSummary, needle: &str) -> bool {
    contains(&credential.display_name, needle)
        || contains(&credential.connector_id, needle)
        || contains(credential.binding.label(), needle)
        || contains(credential.status.label(), needle)
        || credential
            .account
            .as_deref()
            .is_some_and(|value| contains(value, needle))
        || credential
            .environment_variable
            .as_deref()
            .is_some_and(|value| contains(value, needle))
        || credential
            .recent_activity
            .as_deref()
            .is_some_and(|value| contains(value, needle))
        || credential
            .destinations
            .iter()
            .any(|value| contains(value, needle))
        || credential.sandboxes.iter().any(|access| {
            contains(&access.sandbox_name, needle) || contains(&access.project, needle)
        })
        || credential.pending.as_ref().is_some_and(|request| {
            contains(&request.sandbox_name, needle)
                || contains(&request.project, needle)
                || contains(&request.action, needle)
        })
}

fn audit_row_contains(row: &TimelineRow, needle: &str) -> bool {
    contains(&row.when, needle)
        || contains(&row.run, needle)
        || contains(&row.kind, needle)
        || contains(&row.detail, needle)
        || row
            .connector
            .as_deref()
            .is_some_and(|connector| contains(connector, needle))
}

fn contains(value: &str, needle: &str) -> bool {
    value.to_lowercase().contains(needle)
}

fn credential_detail_panel(ui: &mut egui::Ui, state: &mut UnifiedDashboardState) {
    let reveal = ui.ctx().animate_bool_with_time(
        egui::Id::new("unified-credential-detail-anim"),
        state.selected_connector.is_some(),
        0.16,
    );
    let Some(credential) = state.selected_credential().cloned() else {
        return;
    };
    egui::Panel::right("unified-credential-detail")
        .resizable(false)
        .exact_size(DETAIL_WIDTH * reveal)
        .show_separator_line(true)
        .frame(Frame::new().fill(CHROME_FILL))
        .show_inside(ui, |ui| {
            ui.set_opacity(reveal);
            Frame::new()
                .inner_margin(Margin::same(theme::STACK_MARGIN))
                .show(ui, |ui| {
                    ui.set_width(DETAIL_WIDTH - 2.0 * theme::STACK_MARGIN as f32);
                    credential_detail(ui, state, &credential);
                });
        });
}

fn credential_detail(
    ui: &mut egui::Ui,
    state: &mut UnifiedDashboardState,
    credential: &DashboardCredential,
) {
    ui.horizontal(|ui| {
        glyph(
            ui,
            icons::ICON_KEY,
            super::credential_status_color(credential.summary.status),
            18.0,
        );
        ui.add_space(8.0);
        ui.vertical(|ui| {
            ui.label(
                RichText::new(&credential.summary.display_name)
                    .size(FS_BODY)
                    .color(TEXT_PRIMARY),
            );
            ui.label(
                RichText::new(&credential.summary.connector_id)
                    .size(FS_LABEL)
                    .monospace()
                    .color(TEXT_MUTED),
            );
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if icon_button(ui, icons::ICON_CLOSE)
                .on_hover_text("Close credential details")
                .clicked()
            {
                state.selected_connector = None;
                state.revealed = None;
            }
        });
    });
    ui.add_space(10.0);
    super::credential_status_badge(ui, credential.summary.status);
    ui.add_space(14.0);
    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if let Some(request) = credential.summary.pending.as_ref() {
                pending_detail(ui, state, request);
                ui.add_space(16.0);
            }
            dashboard_section_label(ui, "EFFECTIVE ACCESS");
            detail_field(ui, "Machine credential", credential.summary.binding.label());
            detail_field(
                ui,
                "Sandbox authorization",
                &authorization_detail(state, &credential.summary),
            );
            if let Some(env) = credential.summary.environment_variable.as_deref() {
                detail_field(ui, "Environment", env);
            }
            if let Some(account) = credential.summary.account.as_deref() {
                detail_field(ui, "Account", account);
            }
            if !credential.summary.scopes.is_empty() {
                detail_field(ui, "Scopes", &credential.summary.scopes.join(", "));
            }
            if let Some(value) = credential.value.as_deref() {
                sensitive_value(
                    ui,
                    state,
                    &credential.summary.connector_id,
                    "Real machine credential",
                    value,
                    RevealedKind::Value,
                );
            }
            if let Some(placeholder) = credential.placeholder.as_deref() {
                sensitive_value(
                    ui,
                    state,
                    &credential.summary.connector_id,
                    "Fake workload placeholder",
                    placeholder,
                    RevealedKind::Placeholder,
                );
            }
            if !credential.summary.sandboxes.is_empty() {
                ui.add_space(8.0);
                dashboard_section_label(ui, "SANDBOX ACCESS");
                let mut open_sandbox = None;
                for access in &credential.summary.sandboxes {
                    let navigable = state
                        .sandboxes
                        .iter()
                        .any(|sandbox| sandbox.id == access.sandbox_id);
                    if super::credential_access_row(ui, access, navigable).clicked() {
                        open_sandbox = Some(access.sandbox_id.clone());
                    }
                }
                if let Some(sandbox_id) = open_sandbox {
                    select_sandbox(state, Some(sandbox_id));
                }
            }
            if !credential.summary.destinations.is_empty() {
                ui.add_space(8.0);
                dashboard_section_label(ui, "DESTINATIONS");
                crate::ui::badge::badges(ui, &credential.summary.destinations);
                ui.add_space(10.0);
            }
            credential_actions(ui, state, credential);
        });
}

fn authorization_detail(state: &UnifiedDashboardState, credential: &CredentialSummary) -> String {
    let selected = state.selected_sandbox.as_deref();
    let access = credential
        .sandboxes
        .iter()
        .find(|access| selected.is_none_or(|id| access.sandbox_id == id));
    match access {
        Some(access) if access.active => format!("Authorized · {}", access.reason),
        Some(access) => format!("Not authorized · {}", access.reason),
        None => "Not authorized for this sandbox".to_string(),
    }
}

fn detail_field(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.label(RichText::new(label).size(FS_LABEL).color(TEXT_MUTED));
    ui.label(RichText::new(value).size(FS_BODY).color(TEXT_PRIMARY));
    ui.add_space(9.0);
}

fn sensitive_value(
    ui: &mut egui::Ui,
    state: &mut UnifiedDashboardState,
    connector_id: &str,
    label: &str,
    value: &str,
    kind: RevealedKind,
) {
    let revealed = state
        .revealed
        .as_ref()
        .is_some_and(|revealed| revealed.connector_id == connector_id && revealed.kind == kind);
    ui.label(RichText::new(label).size(FS_LABEL).color(TEXT_MUTED));
    Frame::new()
        .fill(INPUT_FILL)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(9, 7))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(
                egui::Label::new(
                    RichText::new(if revealed {
                        value
                    } else {
                        "••••••••••••••••"
                    })
                    .size(FS_SECONDARY)
                    .monospace()
                    .color(TEXT_PRIMARY),
                )
                .selectable(revealed),
            );
            ui.add_space(5.0);
            ui.horizontal(|ui| {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if Button::new("Copy", ButtonKind::Secondary)
                        .show(ui)
                        .clicked()
                    {
                        ui.ctx().copy_text(value.to_string());
                        state.notice = Some(if state.mode == DashboardMode::Preview {
                            format!("{label} copied from synthetic preview data.")
                        } else {
                            format!("{label} copied.")
                        });
                    }
                    let reveal_label = if revealed { "Hide" } else { "Reveal" };
                    if Button::new(reveal_label, ButtonKind::Secondary)
                        .show(ui)
                        .clicked()
                    {
                        if revealed {
                            state.revealed = None;
                        } else {
                            state.revealed = Some(RevealedValue {
                                connector_id: connector_id.to_string(),
                                kind,
                                at: ui.input(|input| input.time),
                            });
                        }
                    }
                });
            });
        });
    ui.add_space(9.0);
}

fn pending_detail(
    ui: &mut egui::Ui,
    state: &mut UnifiedDashboardState,
    request: &super::PendingCredentialRequest,
) {
    Frame::new()
        .fill(Color32::from_rgba_unmultiplied(
            STATUS_WARNING.r(),
            STATUS_WARNING.g(),
            STATUS_WARNING.b(),
            18,
        ))
        .stroke(Stroke::new(1.0, STATUS_WARNING))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new("Waiting for you")
                    .size(FS_BODY)
                    .color(STATUS_WARNING),
            );
            ui.label(
                RichText::new(format!(
                    "{} wants to {} · {}",
                    request.sandbox_name, request.action, request.requested_at
                ))
                .size(FS_SECONDARY)
                .color(TEXT_PRIMARY),
            );
            ui.add_space(10.0);
            if Button::new("Review", ButtonKind::Primary)
                .show(ui)
                .clicked()
            {
                state.reviewing_request = Some(request.id.clone());
            }
        });
}

fn credential_actions(
    ui: &mut egui::Ui,
    state: &mut UnifiedDashboardState,
    credential: &DashboardCredential,
) {
    if credential.summary.pending.is_some() {
        return;
    }
    ui.add_space(6.0);
    dashboard_section_label(ui, "ACTIONS");
    let can_replace = credential.summary.binding == CredentialBinding::Stored;
    if can_replace
        && Button::new("Replace", ButtonKind::Secondary)
            .min_size(vec2(ui.available_width(), theme::BUTTON_MIN_HEIGHT))
            .show(ui)
            .clicked()
    {
        state.replacing_connector = Some(credential.summary.connector_id.clone());
        state.replacement_value.clear();
        state.revealed = None;
    }
    let access = state
        .selected_sandbox
        .as_ref()
        .and_then(|id| {
            credential
                .summary
                .sandboxes
                .iter()
                .find(|access| &access.sandbox_id == id && access.active && access.revocable)
        })
        .or_else(|| {
            credential
                .summary
                .sandboxes
                .iter()
                .find(|access| access.active && access.revocable)
        });
    if let Some(access) = access {
        ui.add_space(6.0);
        if Button::new("Disconnect", ButtonKind::Danger)
            .min_size(vec2(ui.available_width(), theme::BUTTON_MIN_HEIGHT))
            .show(ui)
            .clicked()
        {
            state.confirmation = Some(CredentialOperation::RevokeSandbox {
                connector_id: credential.summary.connector_id.clone(),
                sandbox_id: access.sandbox_id.clone(),
                sandbox_name: access.sandbox_name.clone(),
                project: access.project.clone(),
            });
        }
    }
    if credential.summary.binding != CredentialBinding::Unbound {
        ui.add_space(6.0);
        if Button::new("Remove credential", ButtonKind::Danger)
            .min_size(vec2(ui.available_width(), theme::BUTTON_MIN_HEIGHT))
            .show(ui)
            .clicked()
        {
            state.confirmation = Some(CredentialOperation::ForgetEverywhere {
                connector_id: credential.summary.connector_id.clone(),
            });
        }
    }
}

fn confirmation_modal(ui: &mut egui::Ui, state: &mut UnifiedDashboardState) {
    let Some(operation) = state.confirmation.clone() else {
        return;
    };
    let mut confirmed = false;
    let modal = egui::Modal::new(egui::Id::new("unified-dashboard-confirmation"))
        .backdrop_color(Color32::from_black_alpha(80))
        .frame(modal_frame())
        .show(ui.ctx(), |ui| {
            ui.set_width(400.0);
            ui.label(
                RichText::new(operation.title())
                    .size(18.0)
                    .color(TEXT_PRIMARY),
            );
            ui.add_space(8.0);
            ui.label(
                RichText::new(operation.description())
                    .size(FS_BODY)
                    .color(TEXT_MUTED),
            );
            ui.add_space(16.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if Button::new(operation.confirm_label(), ButtonKind::Danger)
                    .show(ui)
                    .clicked()
                {
                    confirmed = true;
                }
                if Button::new("Cancel", ButtonKind::Secondary)
                    .show(ui)
                    .clicked()
                {
                    state.confirmation = None;
                }
            });
        });
    if confirmed {
        if state.mode == DashboardMode::Preview {
            apply_operation(state, operation);
        } else {
            state.pending_command = operation_command(&operation);
        }
        state.confirmation = None;
    } else if modal.should_close() {
        state.confirmation = None;
    }
}

fn operation_command(operation: &CredentialOperation) -> Option<DashboardCommand> {
    match operation {
        CredentialOperation::RevokeSandbox {
            connector_id,
            sandbox_id,
            ..
        } => Some(DashboardCommand::RevokeSandbox {
            connector_id: connector_id.clone(),
            sandbox_id: sandbox_id.clone(),
        }),
        CredentialOperation::ForgetEverywhere { connector_id } => {
            Some(DashboardCommand::RemoveCredential {
                connector_id: connector_id.clone(),
            })
        }
        CredentialOperation::DisconnectProject { .. } => None,
    }
}

fn apply_operation(state: &mut UnifiedDashboardState, operation: CredentialOperation) {
    match operation {
        CredentialOperation::RevokeSandbox {
            connector_id,
            sandbox_id: _,
            sandbox_name: _,
            project,
        } => {
            if let Some(credential) = state
                .credentials
                .iter_mut()
                .find(|credential| credential.summary.connector_id == connector_id)
            {
                for access in credential
                    .summary
                    .sandboxes
                    .iter_mut()
                    .filter(|access| access.project == project)
                {
                    access.active = false;
                    access.reason = "Disconnected in this preview".into();
                }
            }
            state.notice = Some(format!(
                "{connector_id} was disconnected from {project}. The machine credential is unchanged."
            ));
            record_preview_event(
                state,
                &connector_id,
                format!("Disconnected {connector_id} from {project}"),
            );
        }
        CredentialOperation::DisconnectProject {
            connector_id,
            project,
        } => {
            if let Some(credential) = state
                .credentials
                .iter_mut()
                .find(|credential| credential.summary.connector_id == connector_id)
            {
                for access in &mut credential.summary.sandboxes {
                    if access.project == project {
                        access.active = false;
                        access.reason = "Disconnected in this preview".into();
                    }
                }
            }
            state.notice = Some(format!("{connector_id} was disconnected from {project}."));
            record_preview_event(
                state,
                &connector_id,
                format!("Disconnected {connector_id} from {project}"),
            );
        }
        CredentialOperation::ForgetEverywhere { connector_id } => {
            if let Some(credential) = state
                .credentials
                .iter_mut()
                .find(|credential| credential.summary.connector_id == connector_id)
            {
                credential.summary.binding = CredentialBinding::Unbound;
                credential.summary.status = CredentialStatus::Unavailable;
                credential.value = None;
                for access in &mut credential.summary.sandboxes {
                    access.active = false;
                    access.reason = "Machine credential removed in this preview".into();
                }
            }
            state.notice = Some(format!(
                "{connector_id} was removed from this machine. Provider authorization was not revoked."
            ));
            state.revealed = None;
            record_preview_event(
                state,
                &connector_id,
                format!("Removed the machine credential for {connector_id}"),
            );
        }
    }
}

fn review_modal(ui: &mut egui::Ui, state: &mut UnifiedDashboardState) {
    let Some(request_id) = state.reviewing_request.clone() else {
        return;
    };
    let Some((connector_id, display_name, request)) =
        state.credentials.iter().find_map(|credential| {
            credential
                .summary
                .pending
                .as_ref()
                .filter(|request| request.id == request_id)
                .map(|request| {
                    (
                        credential.summary.connector_id.clone(),
                        credential.summary.display_name.clone(),
                        request.clone(),
                    )
                })
        })
    else {
        state.reviewing_request = None;
        return;
    };
    let mut decision = None;
    let modal = egui::Modal::new(egui::Id::new("unified-dashboard-review"))
        .backdrop_color(Color32::from_black_alpha(80))
        .frame(modal_frame())
        .show(ui.ctx(), |ui| {
            ui.set_width(440.0);
            ui.label(
                RichText::new(format!("Review {display_name}"))
                    .size(18.0)
                    .color(TEXT_PRIMARY),
            );
            ui.add_space(8.0);
            ui.label(
                RichText::new(format!(
                    "{} in {} wants to {}.",
                    request.sandbox_name, request.project, request.action
                ))
                .size(FS_BODY)
                .color(TEXT_PRIMARY),
            );
            ui.label(
                RichText::new(format!(
                    "{} · {} held request{}",
                    request.requested_at,
                    request.held_requests,
                    if request.held_requests == 1 { "" } else { "s" }
                ))
                .size(FS_SECONDARY)
                .color(TEXT_MUTED),
            );
            if let Some(code) = &request.user_code {
                ui.add_space(10.0);
                ui.label(
                    RichText::new(code)
                        .size(18.0)
                        .monospace()
                        .color(TEXT_PRIMARY),
                );
            }
            if let Some(uri) = &request.verification_uri {
                ui.hyperlink_to("Open sign-in", uri);
            }
            ui.add_space(16.0);
            if !request.oauth || request.token_fallback {
                ui.add(
                    egui::TextEdit::singleline(&mut state.review_value)
                        .password(true)
                        .hint_text("Credential value")
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(12.0);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if request.oauth
                    && request.verification_uri.is_none()
                    && Button::new("Connect", ButtonKind::Primary)
                        .show(ui)
                        .clicked()
                {
                    decision = Some(CredentialReviewChoice::Connect);
                }
                if request.oauth
                    && request.token_fallback
                    && !state.review_value.trim().is_empty()
                    && Button::new("Use token", ButtonKind::Secondary)
                        .show(ui)
                        .clicked()
                {
                    decision = Some(CredentialReviewChoice::UseValue(state.review_value.clone()));
                }
                if !request.oauth {
                    if request.host_value_available
                        && Button::new("Use host", ButtonKind::Primary)
                            .show(ui)
                            .clicked()
                    {
                        decision = Some(CredentialReviewChoice::UseHost);
                    }
                    if !state.review_value.trim().is_empty()
                        && Button::new("Use value", ButtonKind::Primary)
                            .show(ui)
                            .clicked()
                    {
                        decision =
                            Some(CredentialReviewChoice::UseValue(state.review_value.clone()));
                    }
                }
                if Button::new("Deny", ButtonKind::Danger).show(ui).clicked() {
                    decision = Some(CredentialReviewChoice::Deny);
                }
                if Button::new("Cancel", ButtonKind::Secondary)
                    .show(ui)
                    .clicked()
                {
                    state.reviewing_request = None;
                    clear_secret(&mut state.review_value);
                }
            });
        });
    if let Some(choice) = decision {
        if state.mode == DashboardMode::Preview {
            let allow = !matches!(&choice, CredentialReviewChoice::Deny);
            apply_review_decision(state, &connector_id, &request, allow);
        } else {
            state.pending_command = Some(DashboardCommand::ReviewCredential {
                request_id: request.id.clone(),
                choice,
            });
        }
        state.reviewing_request = None;
        clear_secret(&mut state.review_value);
    } else if modal.should_close() {
        state.reviewing_request = None;
        clear_secret(&mut state.review_value);
    }
}

fn apply_review_decision(
    state: &mut UnifiedDashboardState,
    connector_id: &str,
    request: &super::PendingCredentialRequest,
    allow: bool,
) {
    if let Some(credential) = state
        .credentials
        .iter_mut()
        .find(|credential| credential.summary.connector_id == connector_id)
    {
        credential.summary.pending = None;
        credential.summary.status = if allow {
            CredentialStatus::Active
        } else {
            CredentialStatus::Denied
        };
        credential.summary.binding = if allow {
            CredentialBinding::Stored
        } else {
            CredentialBinding::Denied
        };
        if allow
            && !credential
                .summary
                .sandboxes
                .iter()
                .any(|access| access.sandbox_id == request.sandbox_id)
        {
            credential.summary.sandboxes.push(super::SandboxAccess {
                sandbox_id: request.sandbox_id.clone(),
                sandbox_name: request.sandbox_name.clone(),
                project: request.project.clone(),
                reason: "Approved in this preview".into(),
                active: true,
                revocable: true,
            });
        }
    }
    let result = if allow { "allowed" } else { "denied" };
    state.notice = Some(format!(
        "{connector_id} was {result} for {}. The synthetic held request was resolved.",
        request.sandbox_name
    ));
    record_preview_event(
        state,
        connector_id,
        format!("{result} {connector_id} for {}", request.sandbox_name),
    );
}

fn replace_modal(ui: &mut egui::Ui, state: &mut UnifiedDashboardState) {
    let Some(connector_id) = state.replacing_connector.clone() else {
        return;
    };
    let mut save = false;
    let modal = egui::Modal::new(egui::Id::new("unified-dashboard-replace"))
        .backdrop_color(Color32::from_black_alpha(80))
        .frame(modal_frame())
        .show(ui.ctx(), |ui| {
            ui.set_width(400.0);
            ui.label(
                RichText::new("Replace stored value")
                    .size(18.0)
                    .color(TEXT_PRIMARY),
            );
            ui.add_space(8.0);
            ui.label(
                RichText::new(format!(
                    "Enter a new {}value for {connector_id}. The previous value will be replaced.",
                    if state.mode == DashboardMode::Preview {
                        "synthetic "
                    } else {
                        ""
                    }
                ))
                .size(FS_BODY)
                .color(TEXT_MUTED),
            );
            ui.add_space(12.0);
            ui.add(
                egui::TextEdit::singleline(&mut state.replacement_value)
                    .password(true)
                    .hint_text(if state.mode == DashboardMode::Preview {
                        "Synthetic credential value"
                    } else {
                        "Credential value"
                    })
                    .desired_width(f32::INFINITY),
            )
            .request_focus();
            ui.add_space(16.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_enabled_ui(!state.replacement_value.trim().is_empty(), |ui| {
                    if Button::new("Replace", ButtonKind::Primary)
                        .show(ui)
                        .clicked()
                    {
                        save = true;
                    }
                });
                if Button::new("Cancel", ButtonKind::Secondary)
                    .show(ui)
                    .clicked()
                {
                    state.replacing_connector = None;
                    clear_secret(&mut state.replacement_value);
                }
            });
        });
    if save {
        let value = std::mem::take(&mut state.replacement_value);
        if state.mode == DashboardMode::Preview {
            if let Some(credential) = state
                .credentials
                .iter_mut()
                .find(|credential| credential.summary.connector_id == connector_id)
            {
                credential.value = Some(value);
                credential.summary.binding = CredentialBinding::Stored;
                credential.summary.status = CredentialStatus::Active;
            }
            state.notice = Some(format!(
                "The synthetic stored value for {connector_id} was replaced."
            ));
            record_preview_event(
                state,
                &connector_id,
                format!("Replaced the stored value for {connector_id}"),
            );
        } else {
            state.pending_command = Some(DashboardCommand::ReplaceCredential {
                connector_id: connector_id.clone(),
                value,
            });
        }
        state.replacing_connector = None;
    } else if modal.should_close() {
        state.replacing_connector = None;
        clear_secret(&mut state.replacement_value);
    }
}

fn clear_secret(value: &mut String) {
    let mut secret = std::mem::take(value);
    // SAFETY: the bytes are overwritten before the string is dropped and no safe code observes the temporary invalid UTF-8.
    unsafe {
        secret.as_bytes_mut().fill(0);
    }
}

fn modal_frame() -> Frame {
    Frame::new()
        .fill(MODAL_FILL)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(18))
}

fn record_preview_event(state: &mut UnifiedDashboardState, connector_id: &str, detail: String) {
    let run = state
        .selected_sandbox()
        .and_then(|sandbox| sandbox.run_ids.first())
        .cloned()
        .or_else(|| {
            state
                .sandboxes
                .first()
                .and_then(|sandbox| sandbox.run_ids.first())
                .cloned()
        })
        .unwrap_or_else(|| "preview".into());
    state.audit.rows.insert(
        0,
        TimelineRow {
            ts: "2026-07-27T12:00:00Z".into(),
            when: "2026-07-27 12:00:00".into(),
            run: run.clone(),
            kind: "credential".into(),
            detail: detail.clone(),
            raw: json!({
                "message": detail,
                "run": run,
                "unmapped": {
                    "lns_kind": "credential",
                    "preview": true
                }
            }),
            connector: Some(connector_id.to_string()),
        },
    );
}

fn compact_attention_badge(ui: &mut egui::Ui, count: usize) {
    Frame::new()
        .fill(Color32::from_rgba_unmultiplied(
            STATUS_WARNING.r(),
            STATUS_WARNING.g(),
            STATUS_WARNING.b(),
            24,
        ))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.label(
                RichText::new(count.to_string())
                    .size(FS_LABEL)
                    .color(STATUS_WARNING),
            )
            .on_hover_text(format!(
                "{count} pending credential request{}",
                if count == 1 { "" } else { "s" }
            ));
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn credential(id: &str, value: &str) -> DashboardCredential {
        DashboardCredential {
            summary: CredentialSummary {
                connector_id: id.into(),
                display_name: id.into(),
                binding: CredentialBinding::Stored,
                status: CredentialStatus::Active,
                account: None,
                scopes: Vec::new(),
                expires_at: None,
                environment_variable: None,
                destinations: Vec::new(),
                sandboxes: Vec::new(),
                recent_activity: None,
                pending: None,
            },
            value: Some(value.into()),
            placeholder: Some("safe-placeholder".into()),
        }
    }

    #[test]
    fn credential_and_audit_share_the_primary_column_width() {
        let stored = TableColumnWidths::default();
        let credentials = CredentialColumns::for_width(900.0, &stored);
        let audit = AuditColumns::for_width(900.0, &stored);

        assert_eq!(credentials.connector, audit.event);
    }

    #[test]
    fn dragging_a_table_divider_changes_its_width() {
        let ctx = egui::Context::default();
        let screen_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(300.0, 100.0));
        let render = |events, resized: &mut Option<f32>| {
            let _ = ctx.run_ui(
                egui::RawInput {
                    events,
                    screen_rect: Some(screen_rect),
                    ..Default::default()
                },
                |ui| {
                    ui.horizontal(|ui| {
                        exact_cell(ui, 90.0, 18.0, Layout::left_to_right(Align::Center), |_| {});
                        *resized = table_column_resizer(ui, "test-column", 100.0);
                    });
                },
            );
        };
        let mut resized = None;
        render(Vec::new(), &mut resized);
        render(
            vec![
                egui::Event::PointerMoved(egui::pos2(95.0, 9.0)),
                egui::Event::PointerButton {
                    pos: egui::pos2(95.0, 9.0),
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            &mut resized,
        );
        render(
            vec![egui::Event::PointerMoved(egui::pos2(115.0, 9.0))],
            &mut resized,
        );

        assert_eq!(resized, Some(120.0));
    }

    #[test]
    fn credential_debug_redacts_the_usable_machine_value() {
        let debug = format!("{:?}", credential("some-provider", "some-secret"));
        assert!(!debug.contains("some-secret"));
        assert!(debug.contains("<redacted>"));
        assert!(debug.contains("safe-placeholder"));
    }

    #[test]
    fn dashboard_commands_redact_typed_values() {
        let replace = DashboardCommand::ReplaceCredential {
            connector_id: "some-provider".into(),
            value: "some-secret".into(),
        };
        let review = DashboardCommand::ReviewCredential {
            request_id: "request-1".into(),
            choice: CredentialReviewChoice::UseValue("some-token".into()),
        };
        let debug = format!("{replace:?} {review:?}");
        assert!(!debug.contains("some-secret"));
        assert!(!debug.contains("some-token"));
        assert!(debug.contains("<redacted>"));
    }

    #[test]
    fn confirmed_operations_preserve_the_authoritative_target_ids() {
        let command = operation_command(&CredentialOperation::RevokeSandbox {
            connector_id: "some-provider".into(),
            sandbox_id: "run-1".into(),
            sandbox_name: "calm-finch".into(),
            project: "~/projects/example".into(),
        })
        .expect("revoke command");
        assert!(matches!(
            &command,
            DashboardCommand::RevokeSandbox {
                connector_id,
                sandbox_id
            } if connector_id == "some-provider" && sandbox_id == "run-1"
        ));
    }

    #[test]
    fn live_refresh_preserves_navigation_and_drops_stale_selections() {
        let sandbox = DashboardSandbox {
            id: "run-1".into(),
            name: "sandbox".into(),
            project: "/project".into(),
            image: "example:latest".into(),
            status: "running".into(),
            run_ids: vec!["run-1".into()],
        };
        let mut state = UnifiedDashboardState::live(
            vec![sandbox],
            vec![credential("some-provider", "some-secret")],
            Vec::new(),
            Vec::new(),
            None,
        );
        state.selected_sandbox = Some("run-1".into());
        state.selected_connector = Some("some-provider".into());
        state.audit.selected = Some(2);
        state.replace_live_data(Vec::new(), Vec::new(), Vec::new(), Vec::new(), None);
        assert_eq!(state.mode, DashboardMode::Live);
        assert_eq!(state.selected_sandbox, None);
        assert_eq!(state.selected_connector, None);
        assert_eq!(state.audit.selected, None);
    }
}
