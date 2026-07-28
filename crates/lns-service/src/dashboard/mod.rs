mod credentials;
mod filter;
mod format;
pub mod live;
mod sandboxes;
mod snapshot;
mod unified;

pub use credentials::{
    CredentialBinding, CredentialDashboardState, CredentialFilter, CredentialOperation,
    CredentialSandbox, CredentialSection, CredentialStatus, CredentialSummary,
    PendingCredentialRequest, SandboxAccess,
};
pub use filter::{Filters, KINDS, visible_indices};
pub use unified::{
    CredentialReviewChoice, DashboardCommand, DashboardCredential, DashboardSandbox, DashboardView,
    UnifiedDashboardAction, UnifiedDashboardState, render_unified_dashboard,
    unified_viewport_builder,
};

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, CursorIcon, FontId, Frame, Layout, Margin,
    RichText, Sense, Stroke, Vec2, vec2,
};
use egui_material_icons::{MaterialIcon, icons};
use lns_audit::TimelineRow;

use crate::approval_flow::window::{
    ACCENT_GREEN, BG_PRIMARY, BG_SECONDARY, BG_TERTIARY, BORDER, CATEGORY, STATUS_CRITICAL,
    STATUS_WARNING, TEXT_MUTED, TEXT_PRIMARY,
};
use crate::ui::button::{Button, ButtonKind};
use crate::ui::theme;

const TRAFFIC_LIGHT_INSET: f32 = 80.0;
const SIDEBAR_WIDTH: f32 = 216.0;
const ROW_HEIGHT: f32 = 38.0;
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialDashboardAction {
    None,
    Refresh,
    ReviewRequest(String),
    Confirm(CredentialOperation),
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

pub fn load_unified(
    window_state: &crate::approval_flow::window::WindowState,
) -> UnifiedDashboardState {
    let data = collect_unified_data(window_state);
    UnifiedDashboardState::live(
        data.sandboxes,
        data.credentials,
        data.rows,
        data.warnings,
        data.last_error,
    )
}

pub fn reload_unified(
    state: &mut UnifiedDashboardState,
    window_state: &crate::approval_flow::window::WindowState,
) {
    let data = collect_unified_data(window_state);
    state.replace_live_data(
        data.sandboxes,
        data.credentials,
        data.rows,
        data.warnings,
        data.last_error,
    );
}

pub fn execute_unified_command(
    command: &DashboardCommand,
    window_state: &crate::approval_flow::window::WindowState,
) -> anyhow::Result<String> {
    use anyhow::{Context, bail};
    use lns_policy::credentials::{CredentialEntry, CredentialStore};

    let notice = match command {
        DashboardCommand::ReviewCredential { request_id, choice } => {
            if let Some(connector_id) = request_id.strip_prefix("sign-in:") {
                let (resolved, notice) = match choice {
                    CredentialReviewChoice::Deny => (
                        window_state.cancel_sign_in(connector_id),
                        format!("Sign-in to {connector_id} was cancelled."),
                    ),
                    CredentialReviewChoice::UseValue(value) => (
                        window_state.pivot_sign_in(connector_id, value.clone()),
                        format!("{connector_id} will use the supplied token."),
                    ),
                    CredentialReviewChoice::UseHost | CredentialReviewChoice::Connect => {
                        bail!("sign-in for {connector_id} is already in progress")
                    }
                };
                if !resolved {
                    bail!("credential request is no longer pending");
                }
                notice
            } else {
                let request = match choice {
                    CredentialReviewChoice::UseHost | CredentialReviewChoice::Connect => {
                        crate::credential_flow::session::CredentialDecisionRequest::Allow(
                            CredentialEntry::HostDetect,
                        )
                    }
                    CredentialReviewChoice::UseValue(value) => {
                        crate::credential_flow::session::CredentialDecisionRequest::Allow(
                            CredentialEntry::Stored {
                                value: value.clone(),
                            },
                        )
                    }
                    CredentialReviewChoice::Deny => {
                        crate::credential_flow::session::CredentialDecisionRequest::Deny
                    }
                };
                if !window_state.decide_credential(request_id, request) {
                    bail!("credential request is no longer pending");
                }
                match choice {
                    CredentialReviewChoice::Deny => "Credential request denied.".to_string(),
                    CredentialReviewChoice::Connect => "Sign-in started.".to_string(),
                    CredentialReviewChoice::UseHost | CredentialReviewChoice::UseValue(_) => {
                        "Credential request allowed.".to_string()
                    }
                }
            }
        }
        DashboardCommand::ReplaceCredential {
            connector_id,
            value,
        } => {
            let path = lns_policy::credentials::default_credentials_path();
            let store = lns_policy::credentials::JsonFileCredentialStore::new(path.clone());
            let mut state = store
                .load()
                .with_context(|| format!("reading credential state {}", path.display()))?;
            replace_saved_credential(&mut state, connector_id, value)?;
            store
                .save(&state)
                .with_context(|| format!("saving credential state {}", path.display()))?;
            crate::dashboard::live::note_write();
            format!("{connector_id} was replaced.")
        }
        DashboardCommand::RemoveCredential { connector_id } => {
            let path = lns_policy::credentials::default_credentials_path();
            let store = lns_policy::credentials::JsonFileCredentialStore::new(path.clone());
            let mut state = store
                .load()
                .with_context(|| format!("reading credential state {}", path.display()))?;
            remove_saved_credential(&mut state, connector_id)?;
            store
                .save(&state)
                .with_context(|| format!("saving credential state {}", path.display()))?;
            crate::dashboard::live::note_write();
            format!("{connector_id} was removed from this machine.")
        }
        DashboardCommand::RevokeSandbox {
            connector_id,
            sandbox_id,
        } => {
            if crate::run_registry::credential_slots(sandbox_id)
                .iter()
                .any(|slot| slot.name == *connector_id)
            {
                bail!("{connector_id} is declared by the sandbox definition and cannot be revoked");
            }
            let details = crate::run_registry::inspect(sandbox_id)
                .with_context(|| format!("sandbox {sandbox_id} is no longer available"))?;
            let policy_path = details
                .config
                .policy_path
                .map(std::path::PathBuf::from)
                .context("sandbox has no project policy")?;
            let mut policy = lns_policy::Policy::load_or_default(&policy_path)
                .with_context(|| format!("reading policy {}", policy_path.display()))?;
            if !policy.disconnect(connector_id) {
                bail!("{connector_id} is no longer connected to this project");
            }
            policy
                .save_atomic(&policy_path)
                .with_context(|| format!("saving policy {}", policy_path.display()))?;
            crate::dashboard::live::note_write();
            format!(
                "{connector_id} was disconnected from {}.",
                project_label(&policy_path)
            )
        }
    };
    Ok(notice)
}

fn replace_saved_credential(
    state: &mut lns_policy::credentials::CredentialStateFile,
    connector_id: &str,
    value: &str,
) -> anyhow::Result<()> {
    use anyhow::bail;
    use lns_policy::credentials::CredentialEntry;

    match state.get_mut(connector_id) {
        Some(CredentialEntry::Stored { value: stored }) => stored.replace_range(.., value),
        Some(_) => bail!("{connector_id} is not a replaceable stored credential"),
        None => bail!("{connector_id} is no longer stored on this machine"),
    }
    Ok(())
}

fn remove_saved_credential(
    state: &mut lns_policy::credentials::CredentialStateFile,
    connector_id: &str,
) -> anyhow::Result<()> {
    anyhow::ensure!(
        state.remove(connector_id).is_some(),
        "{connector_id} is no longer stored on this machine"
    );
    Ok(())
}

struct UnifiedData {
    sandboxes: Vec<DashboardSandbox>,
    credentials: Vec<DashboardCredential>,
    rows: Vec<TimelineRow>,
    warnings: Vec<String>,
    last_error: Option<String>,
}

fn collect_unified_data(window_state: &crate::approval_flow::window::WindowState) -> UnifiedData {
    use lns_policy::credentials::CredentialStore;

    let mut warnings = Vec::new();
    let (rows, audit_warnings, last_error) = collect_audit_rows();
    warnings.extend(audit_warnings);

    let user_catalog_path = lns_policy::connectors::default_connectors_path();
    let user_catalog = match lns_policy::connectors::Catalog::load_or_default(&user_catalog_path) {
        Ok(catalog) => catalog,
        Err(error) => {
            warnings.push(format!(
                "Could not read connector catalog {}: {error}",
                user_catalog_path.display()
            ));
            lns_policy::connectors::Catalog::default()
        }
    };
    let catalog = lns_policy::connectors::effective_connectors(&user_catalog);

    let credential_path = lns_policy::credentials::default_credentials_path();
    let credential_store =
        lns_policy::credentials::JsonFileCredentialStore::new(credential_path.clone());
    let credential_state = match credential_store.load() {
        Ok(state) => state,
        Err(error) => {
            warnings.push(format!(
                "Could not read credential state {}: {error}",
                credential_path.display()
            ));
            lns_policy::credentials::CredentialStateFile::new()
        }
    };

    let (active, projects, run_access, policy_warnings) = active_dashboard_sources();
    warnings.extend(policy_warnings);
    let merged = sandboxes::merge_sandboxes(&active, &rows);
    let sandboxes = merged
        .into_iter()
        .map(|sandbox| DashboardSandbox {
            project: projects.get(&sandbox.id).cloned().unwrap_or_default(),
            run_ids: vec![sandbox.id.clone()],
            id: sandbox.id,
            name: sandbox.name,
            image: sandbox.image,
            status: sandbox.status,
        })
        .collect::<Vec<_>>();
    let window_snapshot = window_state.snapshot();
    let pending = window_snapshot
        .pending_credentials
        .into_iter()
        .map(|prompt| {
            let origin = prompt.origin.unwrap_or_else(unknown_prompt_origin);
            snapshot::PendingCredential {
                id: prompt.id,
                connector_id: prompt.credential_id,
                action: prompt.action,
                sandbox_id: origin.sandbox_id,
                sandbox_name: origin.sandbox_name,
                project: display_project(&origin.project),
                host_value_available: prompt.host_value_available,
                oauth: prompt.oauth_display_name.is_some(),
                token_fallback: prompt.token_fallback.is_some(),
                verification_uri: None,
                user_code: None,
            }
        })
        .chain(window_snapshot.sign_ins.into_iter().map(|sign_in| {
            let origin = sign_in.origin.unwrap_or_else(unknown_prompt_origin);
            snapshot::PendingCredential {
                id: format!("sign-in:{}", sign_in.credential_id),
                connector_id: sign_in.credential_id,
                action: "complete sign-in".to_string(),
                sandbox_id: origin.sandbox_id,
                sandbox_name: origin.sandbox_name,
                project: display_project(&origin.project),
                host_value_available: false,
                oauth: true,
                token_fallback: sign_in.token_fallback.is_some(),
                verification_uri: Some(sign_in.verification_uri),
                user_code: sign_in.user_code,
            }
        }))
        .collect();
    let host_values = catalog
        .iter()
        .filter(|connector| {
            matches!(
                credential_state.get(&connector.id),
                Some(lns_policy::credentials::CredentialEntry::HostDetect)
            )
        })
        .filter_map(|connector| {
            let env_var = match connector.auth_kind {
                lns_policy::connectors::AuthKind::Credential => connector
                    .credential
                    .as_ref()
                    .map(|credential| credential.env_var.as_str()),
                lns_policy::connectors::AuthKind::Oauth => {
                    connector.oauth.as_ref().map(|oauth| oauth.env_var.as_str())
                }
            }?;
            std::env::var(env_var)
                .ok()
                .filter(|value| !value.is_empty())
                .map(|value| (connector.id.clone(), value))
        })
        .collect();
    let credentials = snapshot::build_credentials(snapshot::SnapshotInput {
        catalog: &catalog,
        credential_state: &credential_state,
        sandboxes: sandboxes.clone(),
        run_access,
        pending,
        host_values,
        rows: &rows,
        now: now_unix_secs().max(0) as u64,
    });
    UnifiedData {
        sandboxes,
        credentials,
        rows,
        warnings,
        last_error,
    }
}

fn collect_audit_rows() -> (Vec<TimelineRow>, Vec<String>, Option<String>) {
    let runs = match lns_ipc::audit_runs_root() {
        Ok(path) => path,
        Err(error) => return (Vec::new(), Vec::new(), Some(error.to_string())),
    };
    let ledger = match lns_ipc::connection_ledger() {
        Ok(path) => path,
        Err(error) => return (Vec::new(), Vec::new(), Some(error.to_string())),
    };
    match lns_audit::collect_timeline(&runs, &ledger, None) {
        Ok(timeline) => (timeline.rows, timeline.warnings, None),
        Err(error) => (Vec::new(), Vec::new(), Some(format!("{error:#}"))),
    }
}

fn active_dashboard_sources() -> (
    Vec<Sandbox>,
    std::collections::HashMap<String, String>,
    Vec<snapshot::RunCredentialAccess>,
    Vec<String>,
) {
    let summaries = crate::run_registry::snapshot();
    let mut active = Vec::with_capacity(summaries.len());
    let mut projects = std::collections::HashMap::new();
    let mut access = Vec::new();
    let mut warnings = Vec::new();
    for summary in summaries {
        let id = summary.id.clone();
        active.push(Sandbox {
            id: id.clone(),
            name: summary.name,
            image: summary.image,
            status: status_word(&summary.status),
        });
        let Some(details) = crate::run_registry::inspect(&id) else {
            continue;
        };
        let mut grants = Vec::new();
        if let Some(policy_path) = details.config.policy_path.map(std::path::PathBuf::from) {
            projects.insert(id.clone(), project_label(&policy_path));
            match lns_policy::Policy::load_or_default(&policy_path) {
                Ok(policy) => grants.extend(policy.connectors.into_iter().map(|connector_id| {
                    snapshot::RunCredentialGrant {
                        connector_id,
                        reason: "Connected by project policy".to_string(),
                        revocable: true,
                    }
                })),
                Err(error) => warnings.push(format!(
                    "Could not read policy {}: {error}",
                    policy_path.display()
                )),
            }
        }
        for slot in crate::run_registry::credential_slots(&id) {
            grants.retain(|grant| grant.connector_id != slot.name);
            grants.push(snapshot::RunCredentialGrant {
                connector_id: slot.name,
                reason: if slot.required {
                    "Required by sandbox definition".to_string()
                } else {
                    "Declared by sandbox definition".to_string()
                },
                revocable: false,
            });
        }
        if !grants.is_empty() {
            access.push(snapshot::RunCredentialAccess {
                sandbox_id: id,
                grants,
            });
        }
    }
    (active, projects, access, warnings)
}

fn project_label(policy_path: &std::path::Path) -> String {
    let project = policy_path.parent().unwrap_or(policy_path);
    display_project_path(project)
}

fn display_project(project: &str) -> String {
    display_project_path(std::path::Path::new(project))
}

fn display_project_path(project: &std::path::Path) -> String {
    let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else {
        return project.display().to_string();
    };
    project.strip_prefix(&home).map_or_else(
        |_| project.display().to_string(),
        |relative| {
            if relative.as_os_str().is_empty() {
                "~".to_string()
            } else {
                format!("~/{}", relative.display())
            }
        },
    )
}

fn unknown_prompt_origin() -> crate::approval_flow::window::CredentialPromptOrigin {
    crate::approval_flow::window::CredentialPromptOrigin {
        sandbox_id: String::new(),
        sandbox_name: "A sandbox".to_string(),
        project: "this machine".to_string(),
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
        .with_title("Lens Sandbox — Audit")
        .with_fullsize_content_view(true)
        .with_titlebar_shown(false)
        .with_title_shown(false)
        .with_titlebar_buttons_shown(true)
        .with_resizable(true)
        .with_inner_size([960.0, 640.0])
        .with_min_inner_size([640.0, 400.0])
}

pub fn credential_viewport_builder() -> egui::ViewportBuilder {
    viewport_builder().with_title("Lens Sandbox — Credential access")
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
        let _ = detail_panel(ui, state, detail_reveal, false);
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
            ui.add_space(12.0);
            dashboard_page_header(
                ui,
                "Audit",
                "Review sandbox activity and decisions across every run.",
            );
            if let Some(error) = state.last_error.as_deref() {
                ui.add_space(10.0);
                dashboard_banner(ui, error, STATUS_CRITICAL);
            }
            for warning in &state.warnings {
                ui.add_space(10.0);
                dashboard_banner(ui, warning, STATUS_WARNING);
            }
            ui.add_space(14.0);
            kind_chooser(ui, state);
            ui.add_space(14.0);
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
                    dashboard_section_label(ui, "ACTIVITY");
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
        .inner_margin(Margin::symmetric(6, 8))
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

fn detail_panel(
    ui: &mut egui::Ui,
    state: &mut DashboardState,
    reveal: f32,
    credential_link: bool,
) -> Option<String> {
    let row = state.detail_row.clone()?;
    let mut open_credential = None;
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
                    detail_body(ui, state, &row, credential_link, &mut open_credential);
                });
        });
    open_credential
}

fn detail_body(
    ui: &mut egui::Ui,
    state: &mut DashboardState,
    row: &TimelineRow,
    credential_link: bool,
    open_credential: &mut Option<String>,
) {
    let accent = kind_color(&row.kind);
    ui.horizontal(|ui| {
        glyph(ui, kind_icon(&row.kind), accent, 18.0);
        ui.add_space(8.0);
        ui.vertical(|ui| {
            ui.add(
                egui::Label::new(RichText::new(&row.detail).size(FS_BODY).color(TEXT_PRIMARY))
                    .truncate(),
            );
            ui.label(
                RichText::new(row.kind.to_uppercase())
                    .size(FS_LABEL)
                    .color(accent),
            );
        });
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
            if let Some(connector) = &row.connector {
                if credential_link && credential_field(ui, connector, accent) {
                    *open_credential = Some(connector.clone());
                } else if !credential_link {
                    field(ui, state, "Connector", connector, None);
                }
            }
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

fn credential_field(ui: &mut egui::Ui, connector: &str, accent: Color32) -> bool {
    ui.label(RichText::new("Credential").size(FS_LABEL).color(TEXT_MUTED));
    ui.add_space(2.0);
    let response = ui
        .add(
            egui::Label::new(RichText::new(connector).size(FS_BODY).color(accent))
                .sense(Sense::click()),
        )
        .on_hover_cursor(CursorIcon::PointingHand)
        .on_hover_text("Open credential");
    ui.add_space(10.0);
    response.clicked()
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

pub fn render_credentials(
    ui: &mut egui::Ui,
    state: &mut CredentialDashboardState,
) -> CredentialDashboardAction {
    ui.ctx().set_visuals(dashboard_visuals());
    credential_sidebar_toggle(ui, state);
    let detail_reveal = ui.ctx().animate_bool_with_time(
        egui::Id::new("credential-dashboard-detail-anim"),
        state.selected_connector.is_some(),
        0.16,
    );
    let detail_open = state.selected_connector.is_some() || detail_reveal > 0.002;
    let mut action = if detail_open {
        CredentialDashboardAction::None
    } else {
        credential_refresh_button(ui)
    };
    if state.sidebar_open {
        credential_sidebar(ui, state);
    }
    if detail_open {
        credential_detail_panel(ui, state, detail_reveal, &mut action);
    }
    credential_central(ui, state);
    if state.confirmation.is_some() {
        credential_confirmation(ui, state, &mut action);
    }
    action
}

fn credential_refresh_button(ui: &mut egui::Ui) -> CredentialDashboardAction {
    let clicked = egui::Area::new(egui::Id::new("credential-dashboard-refresh"))
        .anchor(Align2::RIGHT_TOP, vec2(-6.0, 5.0))
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            icon_button(ui, icons::ICON_REFRESH)
                .on_hover_text("Refresh")
                .clicked()
        })
        .inner;
    if clicked {
        CredentialDashboardAction::Refresh
    } else {
        CredentialDashboardAction::None
    }
}

fn credential_sidebar_toggle(ui: &mut egui::Ui, state: &mut CredentialDashboardState) {
    let icon = if state.sidebar_open {
        icons::ICON_LEFT_PANEL_CLOSE
    } else {
        icons::ICON_LEFT_PANEL_OPEN
    };
    let clicked = egui::Area::new(egui::Id::new("credential-dashboard-sidebar-toggle"))
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

fn credential_sidebar(ui: &mut egui::Ui, state: &mut CredentialDashboardState) {
    let sandboxes = state.sandboxes.clone();
    egui::Panel::left("credential-dashboard-sidebar")
        .resizable(true)
        .default_size(SIDEBAR_WIDTH)
        .min_size(170.0)
        .max_size(360.0)
        .show_separator_line(true)
        .frame(Frame::new().fill(CHROME_FILL).inner_margin(Margin::same(8)))
        .show_inside(ui, |ui| {
            ui.add_space(26.0);
            if credential_filter_item(
                ui,
                "All credentials",
                state.count(CredentialFilter::All),
                state.selected_filter == CredentialFilter::All,
            )
            .clicked()
            {
                state.selected_filter = CredentialFilter::All;
                state.selected_connector = None;
            }
            if credential_filter_item(
                ui,
                "Pending",
                state.count(CredentialFilter::Pending),
                state.selected_filter == CredentialFilter::Pending,
            )
            .clicked()
            {
                state.selected_filter = CredentialFilter::Pending;
                state.selected_connector = None;
            }
            if credential_filter_item(
                ui,
                "Denied",
                state.count(CredentialFilter::Denied),
                state.selected_filter == CredentialFilter::Denied,
            )
            .clicked()
            {
                state.selected_filter = CredentialFilter::Denied;
                state.selected_connector = None;
            }
            if !sandboxes.is_empty() {
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    ui.label(RichText::new("SANDBOXES").size(FS_LABEL).color(TEXT_MUTED));
                });
                ui.add_space(2.0);
                let all_selected = state.selected_sandbox.is_none();
                if credential_sandbox_item(ui, "All sandboxes", "Every project", "", all_selected)
                    .clicked()
                {
                    state.selected_sandbox = None;
                    state.selected_connector = None;
                }
                for sandbox in sandboxes {
                    let selected = state.selected_sandbox.as_deref() == Some(&sandbox.id);
                    if credential_sandbox_item(
                        ui,
                        &sandbox.name,
                        &sandbox.project,
                        &sandbox.status,
                        selected,
                    )
                    .clicked()
                    {
                        state.selected_sandbox = Some(sandbox.id);
                        state.selected_connector = None;
                    }
                }
            }
        });
}

fn credential_filter_item(
    ui: &mut egui::Ui,
    label: &str,
    count: usize,
    selected: bool,
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
                glyph(ui, icons::ICON_KEY, TEXT_MUTED, 18.0);
                ui.add_space(8.0);
                ui.label(RichText::new(label).size(FS_BODY).color(TEXT_PRIMARY));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(count.to_string())
                            .size(FS_LABEL)
                            .color(TEXT_MUTED),
                    );
                });
            });
        })
        .response
        .interact(Sense::click());
    row_click(&response);
    response
}

fn credential_sandbox_item(
    ui: &mut egui::Ui,
    name: &str,
    project: &str,
    status: &str,
    selected: bool,
) -> egui::Response {
    let fill = if selected {
        SELECT_FILL
    } else {
        Color32::TRANSPARENT
    };
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
                    ui.add(
                        egui::Label::new(RichText::new(project).size(FS_LABEL).color(TEXT_MUTED))
                            .truncate(),
                    );
                });
            });
        })
        .response
        .interact(Sense::click());
    row_click(&response);
    response
}

fn credential_central(ui: &mut egui::Ui, state: &mut CredentialDashboardState) {
    egui::CentralPanel::default()
        .frame(
            Frame::new()
                .fill(CONTENT_FILL)
                .inner_margin(Margin::same(theme::STACK_MARGIN)),
        )
        .show_inside(ui, |ui| {
            ui.add_space(12.0);
            dashboard_page_header(
                ui,
                "Credential access",
                "See what sandboxes can use. Credential values are never shown.",
            );
            if let Some(notice) = state.notice.as_deref() {
                ui.add_space(10.0);
                dashboard_banner(ui, notice, STATUS_WARNING);
            }
            ui.add_space(14.0);
            let visible = credentials::visible_indices(state);
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if visible.is_empty() {
                        ui.add_space(8.0);
                        ui.colored_label(TEXT_MUTED, credential_empty_message(state));
                        return;
                    }
                    for section in [
                        CredentialSection::NeedsAttention,
                        CredentialSection::Active,
                        CredentialSection::Denied,
                    ] {
                        let section_rows: Vec<usize> = visible
                            .iter()
                            .copied()
                            .filter(|index| state.credentials[*index].status.section() == section)
                            .collect();
                        if section_rows.is_empty() {
                            continue;
                        }
                        ui.label(
                            RichText::new(section.label())
                                .size(FS_LABEL)
                                .color(TEXT_MUTED),
                        );
                        ui.add_space(3.0);
                        for index in section_rows {
                            credential_row(ui, state, index);
                        }
                        ui.add_space(14.0);
                    }
                });
        });
}

fn dashboard_page_header(ui: &mut egui::Ui, title: &str, description: &str) {
    ui.label(RichText::new(title).size(20.0).color(TEXT_PRIMARY));
    ui.label(
        RichText::new(description)
            .size(FS_SECONDARY)
            .color(TEXT_MUTED),
    );
}

fn dashboard_banner(ui: &mut egui::Ui, text: &str, color: Color32) {
    Frame::new()
        .fill(Color32::from_rgba_unmultiplied(
            color.r(),
            color.g(),
            color.b(),
            20,
        ))
        .stroke(Stroke::new(1.0, color))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(text).size(FS_SECONDARY).color(TEXT_PRIMARY));
        });
}

fn credential_empty_message(state: &CredentialDashboardState) -> &'static str {
    if state.selected_sandbox.is_some() {
        "No credential access matches this sandbox."
    } else {
        match state.selected_filter {
            CredentialFilter::All => "No credential access has been recorded.",
            CredentialFilter::Pending => "No credential requests are waiting for you.",
            CredentialFilter::Denied => "No credentials are denied or unavailable.",
        }
    }
}

fn credential_row(ui: &mut egui::Ui, state: &mut CredentialDashboardState, index: usize) {
    let credential = state.credentials[index].clone();
    let selected = state.selected_connector.as_deref() == Some(&credential.connector_id);
    let show_activity = ui.available_width() >= 620.0;
    let activity_width = if show_activity { 166.0 } else { 0.0 };
    let body_width =
        (ui.available_width() - ICON_COL - activity_width - if show_activity { 20.0 } else { 8.0 })
            .max(120.0);
    let fill = if selected {
        SELECT_FILL
    } else {
        Color32::TRANSPARENT
    };
    let response = Frame::new()
        .fill(fill)
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(6, 7))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.allocate_ui_with_layout(
                    vec2(ICON_COL, 38.0),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        glyph(
                            ui,
                            icons::ICON_KEY,
                            credential_status_color(credential.status),
                            17.0,
                        );
                    },
                );
                ui.allocate_ui_with_layout(
                    vec2(body_width, 38.0),
                    Layout::top_down(Align::Min),
                    |ui| {
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&credential.display_name)
                                        .size(FS_BODY)
                                        .color(TEXT_PRIMARY),
                                )
                                .truncate(),
                            );
                            credential_status_badge(ui, credential.status);
                        });
                        ui.add(
                            egui::Label::new(
                                RichText::new(credential_row_detail(&credential))
                                    .size(FS_SECONDARY)
                                    .color(TEXT_MUTED),
                            )
                            .truncate(),
                        );
                    },
                );
                if show_activity {
                    ui.allocate_ui_with_layout(
                        vec2(activity_width, 38.0),
                        Layout::right_to_left(Align::Center),
                        |ui| {
                            if let Some(activity) = credential.recent_activity.as_deref() {
                                ui.add(
                                    egui::Label::new(
                                        RichText::new(activity)
                                            .size(FS_SECONDARY)
                                            .color(TEXT_MUTED),
                                    )
                                    .truncate(),
                                );
                            }
                        },
                    );
                }
            });
        })
        .response
        .interact(Sense::click());
    row_click(&response);
    if response.clicked() {
        state.selected_connector = Some(credential.connector_id);
    }
}

fn credential_row_detail(credential: &CredentialSummary) -> String {
    if let Some(request) = credential.pending.as_ref() {
        return format!("{} · {}", request.sandbox_name, request.requested_at);
    }
    let count = credential.active_sandbox_count();
    let sandboxes = if count == 1 {
        "1 active sandbox".to_string()
    } else {
        format!("{count} active sandboxes")
    };
    format!("{} · {sandboxes}", credential.binding.label())
}

fn credential_status_badge(ui: &mut egui::Ui, status: CredentialStatus) {
    let color = credential_status_color(status);
    Frame::new()
        .fill(Color32::from_rgba_unmultiplied(
            color.r(),
            color.g(),
            color.b(),
            22,
        ))
        .corner_radius(CornerRadius::same(5))
        .inner_margin(Margin::symmetric(6, 2))
        .show(ui, |ui| {
            ui.label(RichText::new(status.label()).size(FS_LABEL).color(color));
        });
}

fn credential_status_color(status: CredentialStatus) -> Color32 {
    match status {
        CredentialStatus::Active => ACCENT_GREEN,
        CredentialStatus::Pending | CredentialStatus::Expiring => STATUS_WARNING,
        CredentialStatus::Expired | CredentialStatus::Denied | CredentialStatus::Unavailable => {
            STATUS_CRITICAL
        }
    }
}

fn credential_detail_panel(
    ui: &mut egui::Ui,
    state: &mut CredentialDashboardState,
    reveal: f32,
    action: &mut CredentialDashboardAction,
) {
    let Some(credential) = state.selected_credential().cloned() else {
        return;
    };
    egui::Panel::right("credential-dashboard-detail")
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
                    credential_detail_body(ui, state, &credential, action);
                });
        });
}

fn credential_detail_body(
    ui: &mut egui::Ui,
    state: &mut CredentialDashboardState,
    credential: &CredentialSummary,
    action: &mut CredentialDashboardAction,
) {
    ui.horizontal(|ui| {
        glyph(
            ui,
            icons::ICON_KEY,
            credential_status_color(credential.status),
            18.0,
        );
        ui.add_space(8.0);
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
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if icon_button(ui, icons::ICON_CLOSE)
                .on_hover_text("Close")
                .clicked()
            {
                state.selected_connector = None;
            }
        });
    });
    ui.add_space(10.0);
    credential_status_badge(ui, credential.status);
    ui.add_space(14.0);

    egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            if let Some(request) = credential.pending.as_ref() {
                credential_pending_card(ui, request, action);
                ui.add_space(16.0);
            }
            dashboard_section_label(ui, "ACCESS ON THIS MACHINE");
            credential_detail_field(ui, "Binding", credential.binding.label());
            if let Some(account) = credential.account.as_deref() {
                credential_detail_field(ui, "Account", account);
            }
            if !credential.scopes.is_empty() {
                credential_detail_field(ui, "Scopes", &credential.scopes.join(", "));
            }
            if let Some(expires_at) = credential.expires_at.as_deref() {
                credential_detail_field(ui, "Expires", expires_at);
            }
            if let Some(env_var) = credential.environment_variable.as_deref() {
                credential_detail_field(ui, "Environment", env_var);
            }

            if !credential.sandboxes.is_empty() {
                ui.add_space(6.0);
                dashboard_section_label(ui, "SANDBOX ACCESS");
                for access in &credential.sandboxes {
                    let _ = credential_access_row(ui, access, false);
                }
            }

            if !credential.destinations.is_empty() {
                ui.add_space(10.0);
                dashboard_section_label(ui, "DESTINATIONS");
                crate::ui::badge::badges(ui, &credential.destinations);
                ui.add_space(10.0);
            }

            if let Some(activity) = credential.recent_activity.as_deref() {
                ui.add_space(6.0);
                dashboard_section_label(ui, "RECENT ACTIVITY");
                ui.label(
                    RichText::new(activity)
                        .size(FS_SECONDARY)
                        .color(TEXT_PRIMARY),
                );
                ui.add_space(10.0);
            }

            credential_actions(ui, state, credential);
        });
}

fn credential_pending_card(
    ui: &mut egui::Ui,
    request: &PendingCredentialRequest,
    action: &mut CredentialDashboardAction,
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
            ui.add_space(4.0);
            ui.label(
                RichText::new(format!(
                    "{} in {} wants to {}.",
                    request.sandbox_name, request.project, request.action
                ))
                .size(FS_SECONDARY)
                .color(TEXT_PRIMARY),
            );
            ui.label(
                RichText::new(format!(
                    "{} · {} held request{}",
                    request.requested_at,
                    request.held_requests,
                    if request.held_requests == 1 { "" } else { "s" }
                ))
                .size(FS_LABEL)
                .color(TEXT_MUTED),
            );
            ui.add_space(10.0);
            if Button::new("Review request", ButtonKind::Primary)
                .show(ui)
                .clicked()
            {
                *action = CredentialDashboardAction::ReviewRequest(request.id.clone());
            }
        });
}

fn dashboard_section_label(ui: &mut egui::Ui, label: &str) {
    ui.label(RichText::new(label).size(FS_LABEL).color(TEXT_MUTED));
    ui.add_space(5.0);
}

fn credential_detail_field(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.label(RichText::new(label).size(FS_LABEL).color(TEXT_MUTED));
    ui.label(RichText::new(value).size(FS_BODY).color(TEXT_PRIMARY));
    ui.add_space(9.0);
}

fn credential_access_row(
    ui: &mut egui::Ui,
    access: &SandboxAccess,
    navigable: bool,
) -> egui::Response {
    let response = Frame::new()
        .fill(INPUT_FILL)
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::same(9))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                status_dot(ui, if access.active { "running" } else { "exited" });
                ui.vertical(|ui| {
                    ui.label(
                        RichText::new(&access.sandbox_name)
                            .size(FS_BODY)
                            .color(TEXT_PRIMARY),
                    );
                    ui.add(
                        egui::Label::new(
                            RichText::new(&access.project)
                                .size(FS_LABEL)
                                .color(TEXT_MUTED),
                        )
                        .truncate(),
                    );
                    ui.label(
                        RichText::new(&access.reason)
                            .size(FS_LABEL)
                            .color(TEXT_MUTED),
                    );
                });
                if navigable {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        glyph(ui, icons::ICON_CHEVRON_RIGHT, TEXT_MUTED, 16.0);
                    });
                }
            });
        })
        .response;
    let response = if navigable {
        let response = response.interact(Sense::click());
        row_click(&response);
        response
    } else {
        response
    };
    ui.add_space(6.0);
    response
}

fn credential_actions(
    ui: &mut egui::Ui,
    state: &mut CredentialDashboardState,
    credential: &CredentialSummary,
) {
    if credential.pending.is_some() {
        return;
    }
    ui.add_space(4.0);
    dashboard_section_label(ui, "ACTIONS");
    let selected_access = state
        .selected_sandbox
        .as_ref()
        .and_then(|id| {
            credential
                .sandboxes
                .iter()
                .find(|access| &access.sandbox_id == id)
        })
        .or_else(|| credential.sandboxes.first());
    if let Some(access) = selected_access {
        if Button::new("Revoke access", ButtonKind::Danger)
            .min_size(vec2(ui.available_width(), theme::BUTTON_MIN_HEIGHT))
            .show(ui)
            .clicked()
        {
            state.confirmation = Some(CredentialOperation::RevokeSandbox {
                connector_id: credential.connector_id.clone(),
                sandbox_id: access.sandbox_id.clone(),
                sandbox_name: access.sandbox_name.clone(),
                project: access.project.clone(),
            });
        }
        ui.add_space(6.0);
        if Button::new("Disconnect", ButtonKind::Secondary)
            .min_size(vec2(ui.available_width(), theme::BUTTON_MIN_HEIGHT))
            .show(ui)
            .clicked()
        {
            state.confirmation = Some(CredentialOperation::DisconnectProject {
                connector_id: credential.connector_id.clone(),
                project: access.project.clone(),
            });
        }
        ui.add_space(6.0);
    }
    if credential.binding != CredentialBinding::Unbound
        && Button::new("Remove credential", ButtonKind::Danger)
            .min_size(vec2(ui.available_width(), theme::BUTTON_MIN_HEIGHT))
            .show(ui)
            .clicked()
    {
        state.confirmation = Some(CredentialOperation::ForgetEverywhere {
            connector_id: credential.connector_id.clone(),
        });
    }
}

fn credential_confirmation(
    ui: &mut egui::Ui,
    state: &mut CredentialDashboardState,
    action: &mut CredentialDashboardAction,
) {
    let Some(operation) = state.confirmation.clone() else {
        return;
    };
    let modal = egui::Modal::new(egui::Id::new("credential-dashboard-confirmation"))
        .backdrop_color(Color32::from_black_alpha(80))
        .frame(
            Frame::new()
                .fill(MODAL_FILL)
                .stroke(Stroke::new(1.0, BORDER))
                .corner_radius(CornerRadius::same(12))
                .inner_margin(Margin::same(18)),
        )
        .show(ui.ctx(), |ui| {
            ui.set_width(380.0);
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
                    state.confirmation = None;
                    *action = CredentialDashboardAction::Confirm(operation.clone());
                }
                if Button::new("Cancel", ButtonKind::Secondary)
                    .show(ui)
                    .clicked()
                {
                    state.confirmation = None;
                }
            });
        });
    if modal.should_close() {
        state.confirmation = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approval_flow::window::{CredentialDecisionDelivery, SignInCard, WindowState};
    use crate::credential_flow::session::{CredentialDecisionRequest, CredentialPendingPrompt};
    use crate::credential_flow::store::CredentialEntry;
    use tokio::sync::mpsc;

    fn pending(state: &WindowState) -> mpsc::UnboundedReceiver<CredentialDecisionDelivery> {
        let (tx, rx) = mpsc::unbounded_channel();
        state.insert_credential_pending(
            CredentialPendingPrompt {
                id: "request-1".into(),
                credential_id: "some-provider".into(),
                action: "use of some-provider placeholder".into(),
                oauth_display_name: None,
                token_fallback: None,
                env_var: Some("SOME_TOKEN".into()),
                injection_domains: vec!["api.some-provider.example".into()],
                is_project_defined: false,
            },
            true,
            tx,
        );
        rx
    }

    #[test]
    fn dashboard_review_uses_the_pending_requests_delivery_channel() {
        let state = WindowState::new();
        let mut rx = pending(&state);
        let command = DashboardCommand::ReviewCredential {
            request_id: "request-1".into(),
            choice: CredentialReviewChoice::UseHost,
        };
        let notice = execute_unified_command(&command, &state).expect("review succeeds");
        assert_eq!(notice, "Credential request allowed.");
        assert_eq!(
            rx.try_recv().expect("decision").request,
            CredentialDecisionRequest::Allow(CredentialEntry::HostDetect)
        );
    }

    #[test]
    fn dashboard_review_rejects_a_stale_request() {
        let state = WindowState::new();
        let command = DashboardCommand::ReviewCredential {
            request_id: "missing".into(),
            choice: CredentialReviewChoice::Deny,
        };
        let error = execute_unified_command(&command, &state).expect_err("stale request");
        assert!(error.to_string().contains("no longer pending"));
    }

    #[test]
    fn dashboard_can_cancel_an_in_progress_sign_in() {
        let state = WindowState::new();
        let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel();
        state.insert_sign_in(
            SignInCard {
                credential_id: "some-oauth".into(),
                display_name: "Some OAuth".into(),
                user_code: Some("SOME-CODE".into()),
                verification_uri: "https://api.some-oauth.example/device".into(),
                token_fallback: None,
                env_var: Some("SOME_OAUTH_TOKEN".into()),
                injection_domains: vec!["api.some-oauth.example".into()],
                is_project_defined: false,
                origin: None,
            },
            cancel_tx,
        );
        let command = DashboardCommand::ReviewCredential {
            request_id: "sign-in:some-oauth".into(),
            choice: CredentialReviewChoice::Deny,
        };
        execute_unified_command(&command, &state).expect("cancel succeeds");
        assert!(matches!(
            cancel_rx.try_recv(),
            Ok(crate::oauth::SignInPivot::Cancel)
        ));
    }

    #[test]
    fn stored_credential_mutations_reject_stale_or_incompatible_state() {
        let mut state = lns_policy::credentials::CredentialStateFile::from([
            (
                "stored".into(),
                CredentialEntry::Stored {
                    value: "old-value".into(),
                },
            ),
            ("host".into(), CredentialEntry::HostDetect),
        ]);
        replace_saved_credential(&mut state, "stored", "new-value").expect("replace");
        assert_eq!(
            state.get("stored"),
            Some(&CredentialEntry::Stored {
                value: "new-value".into()
            })
        );
        assert!(replace_saved_credential(&mut state, "host", "value").is_err());
        assert!(replace_saved_credential(&mut state, "missing", "value").is_err());
        remove_saved_credential(&mut state, "stored").expect("remove");
        assert!(remove_saved_credential(&mut state, "stored").is_err());
    }
}
