use eframe::egui::Color32;
use lns_audit::TimelineRow;
use serde_json::json;
use zeroize::{Zeroize, Zeroizing};

use super::{
    CredentialBinding, CredentialOperation, CredentialStatus, CredentialSummary,
    PendingCredentialRequest, STATUS_WARNING, SandboxAccess, TEXT_MUTED, TEXT_PRIMARY,
    credential_status_color,
};

pub(super) const VALUE_REVEAL_SECONDS: f64 = 15.0;
pub(super) const TABLE_CHEVRON_WIDTH: f32 = 28.0;
pub(super) const TABLE_RESIZER_WIDTH: f32 = 10.0;
const TABLE_PRIMARY_MIN: f32 = 220.0;
const TABLE_MACHINE_MIN: f32 = 120.0;
const TABLE_ACCESS_MIN: f32 = 135.0;
const TABLE_ACTIVITY_MIN: f32 = 105.0;
const TABLE_SANDBOX_MIN: f32 = 140.0;
const TABLE_WHEN_MIN: f32 = 110.0;
const WIDE_TABLE_WIDTH: f32 = 680.0;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DashboardView {
    #[default]
    Credentials,
    Audit,
}

impl DashboardView {
    pub(super) fn label(self) -> &'static str {
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

/// `value` is the real machine credential, held so the detail panel can reveal it on request; `Zeroizing` scrubs it when the snapshot is replaced, and the `Debug` impl below keeps it out of the trace stream.
#[derive(Clone, PartialEq, Eq)]
pub struct DashboardCredential {
    pub summary: CredentialSummary,
    pub value: Option<Zeroizing<String>>,
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

/// Which of a credential's two values a reveal is showing, so revealing one hides the other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RevealedKind {
    Value,
    Placeholder,
}

#[derive(Debug, Clone)]
pub(super) struct RevealedValue {
    pub(super) connector_id: String,
    pub(super) kind: RevealedKind,
    pub(super) at: f64,
}

impl RevealedValue {
    /// A revealed value is hidden again by time or by the window losing focus, whichever comes first — a dashboard left open on a second screen must not keep a secret on it.
    pub(super) fn is_expired(&self, now: f64, focused: bool) -> bool {
        !focused || now - self.at >= VALUE_REVEAL_SECONDS
    }
}

#[derive(Debug)]
pub(super) struct TableColumnWidths {
    pub(super) primary: Option<f32>,
    pub(super) credential_machine: f32,
    pub(super) credential_access: f32,
    pub(super) audit_sandbox: f32,
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
pub(super) enum DashboardMode {
    Preview,
    Live,
}

/// `UseBound` grants the value already bound on this machine; `Connect` signs in afresh and replaces it.
pub enum CredentialReviewChoice {
    UseHost,
    UseBound,
    UseValue(Zeroizing<String>),
    Connect,
    Deny,
}

impl std::fmt::Debug for CredentialReviewChoice {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UseHost => formatter.write_str("UseHost"),
            Self::UseBound => formatter.write_str("UseBound"),
            Self::UseValue(_) => formatter.write_str("UseValue(<redacted>)"),
            Self::Connect => formatter.write_str("Connect"),
            Self::Deny => formatter.write_str("Deny"),
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
        value: Zeroizing<String>,
    },
    RemoveCredential {
        connector_id: String,
    },
    DisconnectProject {
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
            Self::DisconnectProject {
                connector_id,
                sandbox_id,
            } => formatter
                .debug_struct("DisconnectProject")
                .field("connector_id", connector_id)
                .field("sandbox_id", sandbox_id)
                .finish(),
        }
    }
}

pub enum UnifiedDashboardAction {
    None,
    Refresh,
    Command(DashboardCommand),
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
    pub(super) reviewing_request: Option<String>,
    pub(super) replacing_connector: Option<String>,
    pub(super) replacement_value: Zeroizing<String>,
    pub(super) review_value: Zeroizing<String>,
    pub(super) pending_command: Option<DashboardCommand>,
    pub(super) revealed: Option<RevealedValue>,
    pub(super) table_columns: TableColumnWidths,
    pub(super) mode: DashboardMode,
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
            replacement_value: Zeroizing::new(String::new()),
            review_value: Zeroizing::new(String::new()),
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

    pub(super) fn select_sandbox(&mut self, id: Option<String>) {
        self.selected_sandbox = id;
        self.audit.selected = None;
        self.audit.detail_row = None;
        self.select_connector(None);
    }

    pub(super) fn select_connector(&mut self, id: Option<String>) {
        self.selected_connector = id;
        self.revealed = None;
    }

    /// Reveals a value, or hides the one already showing; `at` is the frame time the countdown runs from.
    pub(super) fn toggle_reveal(&mut self, connector_id: &str, kind: RevealedKind, at: f64) {
        if self.is_revealed(connector_id, kind) {
            self.revealed = None;
        } else {
            self.revealed = Some(RevealedValue {
                connector_id: connector_id.to_string(),
                kind,
                at,
            });
        }
    }

    pub(super) fn is_revealed(&self, connector_id: &str, kind: RevealedKind) -> bool {
        self.revealed
            .as_ref()
            .is_some_and(|revealed| revealed.connector_id == connector_id && revealed.kind == kind)
    }

    pub(super) fn hide_expired_reveal(&mut self, now: f64, focused: bool) -> bool {
        if self
            .revealed
            .as_ref()
            .is_some_and(|revealed| revealed.is_expired(now, focused))
        {
            self.revealed = None;
        }
        self.revealed.is_some()
    }

    pub(super) fn selected_sandbox(&self) -> Option<&DashboardSandbox> {
        let selected = self.selected_sandbox.as_deref()?;
        self.sandboxes.iter().find(|sandbox| sandbox.id == selected)
    }

    pub(super) fn selected_credential(&self) -> Option<&DashboardCredential> {
        let selected = self.selected_connector.as_deref()?;
        self.credentials
            .iter()
            .find(|credential| credential.summary.connector_id == selected)
    }

    pub(super) fn credential_matches_scope(&self, credential: &CredentialSummary) -> bool {
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

    pub(super) fn audit_matches_scope(&self, row: &TimelineRow) -> bool {
        self.selected_sandbox()
            .is_none_or(|sandbox| sandbox.run_ids.iter().any(|run| run == &row.run))
    }

    pub(super) fn pending_count(&self, sandbox_id: &str) -> usize {
        self.credentials
            .iter()
            .filter_map(|credential| credential.summary.pending.as_ref())
            .filter(|request| request.sandbox_id == sandbox_id)
            .count()
    }

    /// Rows in the order the credential table draws them: what needs attention first, then alphabetical so the list doesn't reshuffle on every refresh.
    pub(super) fn visible_credentials(&self) -> Vec<usize> {
        let mut indices: Vec<usize> = self
            .credentials
            .iter()
            .enumerate()
            .filter(|(_, credential)| self.credential_matches_scope(&credential.summary))
            .map(|(index, _)| index)
            .collect();
        indices.sort_by(|left, right| {
            credential_rank(self.credentials[*left].summary.status)
                .cmp(&credential_rank(self.credentials[*right].summary.status))
                .then_with(|| {
                    self.credentials[*left]
                        .summary
                        .display_name
                        .cmp(&self.credentials[*right].summary.display_name)
                })
        });
        indices
    }

    pub(super) fn visible_audit(&self) -> Vec<usize> {
        self.audit
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| self.audit_matches_scope(row))
            .map(|(index, _)| index)
            .collect()
    }

    pub(super) fn search_results(&self, query: &str) -> SearchResults {
        let needle = query.trim().to_lowercase();
        SearchResults {
            credentials: self
                .credentials
                .iter()
                .enumerate()
                .filter(|(_, credential)| self.credential_matches_scope(&credential.summary))
                .filter(|(_, credential)| {
                    needle.is_empty() || credential_contains(&credential.summary, &needle)
                })
                .map(|(index, _)| index)
                .collect(),
            audit: self
                .audit
                .rows
                .iter()
                .enumerate()
                .filter(|(_, row)| self.audit_matches_scope(row))
                .filter(|(_, row)| needle.is_empty() || audit_row_contains(row, &needle))
                .map(|(index, _)| index)
                .collect(),
        }
    }

    pub(super) fn sandbox_label_for_run(&self, run: &str) -> String {
        self.sandboxes
            .iter()
            .find(|sandbox| sandbox.run_ids.iter().any(|id| id == run))
            .map_or_else(
                || lns_ipc::short_run_id(run).to_string(),
                |sandbox| sandbox.name.clone(),
            )
    }

    pub(super) fn pending_request(
        &self,
        request_id: &str,
    ) -> Option<(String, String, PendingCredentialRequest)> {
        self.credentials.iter().find_map(|credential| {
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
    }

    /// Live mode hands the decision to the service; preview mode mutates the synthetic snapshot so the interaction can be seen without a running sandbox.
    pub(super) fn resolve_review(
        &mut self,
        request: &PendingCredentialRequest,
        choice: CredentialReviewChoice,
    ) {
        let choice = match choice {
            CredentialReviewChoice::UseValue(value) => {
                CredentialReviewChoice::UseValue(trimmed(value))
            }
            other => other,
        };
        let connector_id = self
            .credentials
            .iter()
            .find(|credential| {
                credential
                    .summary
                    .pending
                    .as_ref()
                    .is_some_and(|pending| pending.id == request.id)
            })
            .map(|credential| credential.summary.connector_id.clone());
        match self.mode {
            DashboardMode::Preview => {
                if let Some(connector_id) = connector_id {
                    let allow = !matches!(choice, CredentialReviewChoice::Deny);
                    self.apply_review_decision(&connector_id, request, allow);
                }
            }
            DashboardMode::Live => {
                self.pending_command = Some(DashboardCommand::ReviewCredential {
                    request_id: request.id.clone(),
                    choice,
                });
            }
        }
        self.reviewing_request = None;
        self.review_value.zeroize();
    }

    pub(super) fn resolve_replacement(&mut self, connector_id: &str) {
        let value = trimmed(std::mem::take(&mut self.replacement_value));
        match self.mode {
            DashboardMode::Preview => {
                if let Some(credential) = self
                    .credentials
                    .iter_mut()
                    .find(|credential| credential.summary.connector_id == connector_id)
                {
                    credential.value = Some(value);
                    credential.summary.binding = CredentialBinding::Stored;
                    credential.summary.status = CredentialStatus::Active;
                }
                self.notice = Some(format!(
                    "The synthetic stored value for {connector_id} was replaced."
                ));
                self.record_preview_event(
                    connector_id,
                    format!("Replaced the stored value for {connector_id}"),
                );
            }
            DashboardMode::Live => {
                self.pending_command = Some(DashboardCommand::ReplaceCredential {
                    connector_id: connector_id.to_string(),
                    value,
                });
            }
        }
        self.replacing_connector = None;
    }

    pub(super) fn resolve_confirmation(&mut self, operation: CredentialOperation) {
        match self.mode {
            DashboardMode::Preview => self.apply_operation(operation),
            DashboardMode::Live => self.pending_command = Some(operation_command(&operation)),
        }
        self.confirmation = None;
    }

    fn apply_review_decision(
        &mut self,
        connector_id: &str,
        request: &PendingCredentialRequest,
        allow: bool,
    ) {
        if let Some(credential) = self
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
                credential.summary.sandboxes.push(SandboxAccess {
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
        self.notice = Some(format!(
            "{connector_id} was {result} for {}.",
            request.sandbox_name
        ));
        self.record_preview_event(
            connector_id,
            format!("{result} {connector_id} for {}", request.sandbox_name),
        );
    }

    fn apply_operation(&mut self, operation: CredentialOperation) {
        match operation {
            CredentialOperation::DisconnectProject {
                connector_id,
                project,
                ..
            } => {
                if let Some(credential) = self
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
                self.notice = Some(format!(
                    "{connector_id} was disconnected from {project}. The machine credential is unchanged."
                ));
                self.record_preview_event(
                    &connector_id,
                    format!("Disconnected {connector_id} from {project}"),
                );
            }
            CredentialOperation::ForgetEverywhere { connector_id } => {
                if let Some(credential) = self
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
                self.notice = Some(format!(
                    "{connector_id} was removed from this machine. Provider authorization was not revoked."
                ));
                self.revealed = None;
                self.record_preview_event(
                    &connector_id,
                    format!("Removed the machine credential for {connector_id}"),
                );
            }
        }
    }

    fn record_preview_event(&mut self, connector_id: &str, detail: String) {
        let run = self
            .selected_sandbox()
            .or_else(|| self.sandboxes.first())
            .and_then(|sandbox| sandbox.run_ids.first().cloned())
            .unwrap_or_else(|| "preview".to_string());
        self.audit.rows.insert(
            0,
            TimelineRow {
                ts: PREVIEW_EVENT_TS.into(),
                when: PREVIEW_EVENT_WHEN.into(),
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
}

const PREVIEW_EVENT_TS: &str = "2026-07-27T12:00:00Z";
const PREVIEW_EVENT_WHEN: &str = "2026-07-27 12:00:00";

/// Every path a typed value takes into a store trims it, so a pasted trailing newline can't become part of the credential; the untrimmed original is zeroized on drop.
fn trimmed(value: Zeroizing<String>) -> Zeroizing<String> {
    Zeroizing::new(value.trim().to_string())
}

pub(super) struct SearchResults {
    pub(super) credentials: Vec<usize>,
    pub(super) audit: Vec<usize>,
}

pub(super) fn operation_command(operation: &CredentialOperation) -> DashboardCommand {
    match operation {
        CredentialOperation::DisconnectProject {
            connector_id,
            sandbox_id,
            ..
        } => DashboardCommand::DisconnectProject {
            connector_id: connector_id.clone(),
            sandbox_id: sandbox_id.clone(),
        },
        CredentialOperation::ForgetEverywhere { connector_id } => {
            DashboardCommand::RemoveCredential {
                connector_id: connector_id.clone(),
            }
        }
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

pub(super) fn credential_row_icon_color(status: CredentialStatus) -> Color32 {
    match status {
        CredentialStatus::Active => TEXT_MUTED,
        CredentialStatus::Pending | CredentialStatus::Expiring => STATUS_WARNING,
        CredentialStatus::Expired | CredentialStatus::Denied | CredentialStatus::Unavailable => {
            credential_status_color(status)
        }
    }
}

pub(super) fn credential_machine_label(credential: &CredentialSummary) -> (&'static str, Color32) {
    match credential.status {
        CredentialStatus::Pending => ("Not configured", TEXT_MUTED),
        CredentialStatus::Expiring => ("Expiring soon", STATUS_WARNING),
        CredentialStatus::Expired => ("Expired", credential_status_color(credential.status)),
        CredentialStatus::Denied => ("Blocked", credential_status_color(credential.status)),
        CredentialStatus::Unavailable => {
            ("Unavailable", credential_status_color(credential.status))
        }
        CredentialStatus::Active => {
            let label = match credential.binding {
                CredentialBinding::OAuth => "Signed in",
                CredentialBinding::Stored => "Stored",
                CredentialBinding::HostDetected => "Host value",
                CredentialBinding::Unbound => "Not configured",
                CredentialBinding::Denied => "Blocked",
            };
            (label, TEXT_PRIMARY)
        }
    }
}

pub(super) fn credential_access_label(
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
        let active = credential
            .sandboxes
            .iter()
            .any(|access| access.sandbox_id == sandbox && access.active);
        return if active {
            ("Authorized".into(), TEXT_PRIMARY)
        } else {
            ("Not authorized".into(), TEXT_MUTED)
        };
    }
    match credential.active_sandbox_count() {
        0 => ("No sandbox access".into(), TEXT_MUTED),
        1 => ("1 sandbox".into(), TEXT_PRIMARY),
        count => (format!("{count} sandboxes"), TEXT_PRIMARY),
    }
}

pub(super) fn authorization_detail(
    state: &UnifiedDashboardState,
    credential: &CredentialSummary,
) -> String {
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

#[derive(Clone, Copy)]
pub(super) struct CredentialColumns {
    pub(super) connector: f32,
    pub(super) machine: f32,
    pub(super) access: f32,
    pub(super) activity: f32,
    pub(super) wide: bool,
}

impl CredentialColumns {
    pub(super) fn for_width(width: f32, stored: &TableColumnWidths) -> Self {
        let wide = width >= WIDE_TABLE_WIDTH;
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

#[derive(Clone, Copy)]
pub(super) struct AuditColumns {
    pub(super) event: f32,
    pub(super) sandbox: f32,
    pub(super) when: f32,
    pub(super) wide: bool,
}

impl AuditColumns {
    pub(super) fn for_width(width: f32, stored: &TableColumnWidths) -> Self {
        let wide = width >= WIDE_TABLE_WIDTH;
        let event = shared_primary_width(width, wide, stored.primary);
        let remaining = width - event - TABLE_CHEVRON_WIDTH;
        let sandbox = if wide {
            stored
                .audit_sandbox
                .clamp(TABLE_SANDBOX_MIN, remaining - TABLE_WHEN_MIN)
        } else {
            0.0
        };
        Self {
            event,
            sandbox,
            when: remaining - sandbox,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(id: &str, status: CredentialStatus) -> CredentialSummary {
        CredentialSummary {
            connector_id: id.into(),
            display_name: id.into(),
            binding: CredentialBinding::Stored,
            status,
            account: None,
            scopes: Vec::new(),
            expires_at: None,
            environment_variable: None,
            destinations: Vec::new(),
            sandboxes: Vec::new(),
            recent_activity: None,
            pending: None,
        }
    }

    fn credential(id: &str, status: CredentialStatus) -> DashboardCredential {
        DashboardCredential {
            summary: summary(id, status),
            value: Some(Zeroizing::new("some-secret".into())),
            placeholder: Some("safe-placeholder".into()),
        }
    }

    fn access(sandbox_id: &str, active: bool) -> SandboxAccess {
        SandboxAccess {
            sandbox_id: sandbox_id.into(),
            sandbox_name: format!("sandbox-{sandbox_id}"),
            project: "/projects/example".into(),
            reason: "Connected by project policy".into(),
            active,
            revocable: true,
        }
    }

    fn request(id: &str, sandbox_id: &str) -> PendingCredentialRequest {
        PendingCredentialRequest {
            id: id.into(),
            sandbox_id: sandbox_id.into(),
            sandbox_name: format!("sandbox-{sandbox_id}"),
            project: "~/projects/example".into(),
            action: "use of some-provider placeholder".into(),
            host_value_available: false,
            bound_value_available: false,
            oauth: false,
            token_fallback: false,
            verification_uri: None,
            user_code: None,
        }
    }

    fn sandbox(id: &str) -> DashboardSandbox {
        DashboardSandbox {
            id: id.into(),
            name: format!("sandbox-{id}"),
            project: "/projects/example".into(),
            image: "example:latest".into(),
            status: "running".into(),
            run_ids: vec![id.into()],
        }
    }

    fn row(run: &str, connector: &str, detail: &str) -> TimelineRow {
        TimelineRow {
            ts: "2026-07-27T11:00:00Z".into(),
            when: "2026-07-27 11:00:00".into(),
            run: run.into(),
            kind: "credential".into(),
            detail: detail.into(),
            raw: serde_json::Value::Null,
            connector: Some(connector.into()),
        }
    }

    fn state() -> UnifiedDashboardState {
        UnifiedDashboardState::seeded(
            vec![sandbox("run-1"), sandbox("run-2")],
            vec![
                credential("beta", CredentialStatus::Active),
                credential("alpha", CredentialStatus::Active),
                credential("waiting", CredentialStatus::Pending),
                credential("stale", CredentialStatus::Expired),
                credential("blocked", CredentialStatus::Denied),
            ],
            vec![
                row("run-1", "alpha", "used alpha"),
                row("run-2", "beta", "used beta"),
            ],
            Vec::new(),
        )
    }

    #[test]
    fn view_labels_name_the_two_tabs() {
        assert_eq!(DashboardView::Credentials.label(), "Credentials");
        assert_eq!(DashboardView::Audit.label(), "Audit");
        assert_eq!(DashboardView::default(), DashboardView::Credentials);
    }

    #[test]
    fn attention_sorts_before_availability_then_alphabetically() {
        let state = state();
        let order: Vec<&str> = state
            .visible_credentials()
            .into_iter()
            .map(|index| state.credentials[index].summary.connector_id.as_str())
            .collect();
        assert_eq!(order, ["waiting", "stale", "alpha", "beta", "blocked"]);
    }

    #[test]
    fn selecting_a_sandbox_scopes_credentials_audit_and_pending_counts() {
        let mut state = state();
        state.credentials[1].summary.sandboxes = vec![access("run-1", true)];
        state.credentials[2].summary.pending = Some(request("request-1", "run-2"));

        state.select_sandbox(Some("run-1".into()));
        let visible: Vec<&str> = state
            .visible_credentials()
            .into_iter()
            .map(|index| state.credentials[index].summary.connector_id.as_str())
            .collect();
        assert_eq!(visible, ["alpha"]);
        assert_eq!(state.visible_audit(), [0]);
        assert_eq!(state.pending_count("run-1"), 0);
        assert_eq!(state.pending_count("run-2"), 1);

        state.select_sandbox(Some("run-2".into()));
        let visible: Vec<&str> = state
            .visible_credentials()
            .into_iter()
            .map(|index| state.credentials[index].summary.connector_id.as_str())
            .collect();
        assert_eq!(visible, ["waiting"]);
        assert_eq!(state.visible_audit(), [1]);
    }

    #[test]
    fn changing_the_sandbox_scope_clears_what_was_open_inside_it() {
        let mut state = state();
        state.selected_connector = Some("beta".into());
        state.audit.selected = Some(1);
        state.audit.detail_row = Some(row("run-2", "beta", "used beta"));

        state.select_sandbox(Some("run-1".into()));

        assert_eq!(state.selected_sandbox.as_deref(), Some("run-1"));
        assert!(state.selected_connector.is_none());
        assert!(state.audit.selected.is_none());
        assert!(state.audit.detail_row.is_none());
    }

    #[test]
    fn the_detail_panel_follows_the_selection_and_closes_when_it_goes_stale() {
        let mut state = state();
        assert!(state.selected_credential().is_none());
        assert!(state.selected_sandbox().is_none());

        state.selected_connector = Some("alpha".into());
        state.selected_sandbox = Some("run-1".into());
        assert_eq!(
            state
                .selected_credential()
                .map(|credential| credential.summary.connector_id.as_str()),
            Some("alpha")
        );
        assert_eq!(
            state
                .selected_sandbox()
                .map(|sandbox| sandbox.name.as_str()),
            Some("sandbox-run-1")
        );

        state.selected_connector = Some("gone".into());
        state.selected_sandbox = Some("gone".into());
        assert!(state.selected_credential().is_none());
        assert!(state.selected_sandbox().is_none());
    }

    #[test]
    fn a_credential_debug_redacts_the_usable_value_and_keeps_the_placeholder() {
        let debug = format!("{:?}", credential("alpha", CredentialStatus::Active));
        assert!(!debug.contains("some-secret"));
        assert!(debug.contains("<redacted>"));
        assert!(debug.contains("safe-placeholder"));
    }

    #[test]
    fn revealing_one_value_hides_the_other_and_toggling_hides_it_again() {
        let mut state = state();
        state.toggle_reveal("alpha", RevealedKind::Value, 10.0);
        assert!(state.is_revealed("alpha", RevealedKind::Value));
        assert!(!state.is_revealed("alpha", RevealedKind::Placeholder));
        assert!(!state.is_revealed("beta", RevealedKind::Value));

        state.toggle_reveal("alpha", RevealedKind::Placeholder, 11.0);
        assert!(state.is_revealed("alpha", RevealedKind::Placeholder));
        assert!(!state.is_revealed("alpha", RevealedKind::Value));

        state.toggle_reveal("alpha", RevealedKind::Placeholder, 12.0);
        assert!(state.revealed.is_none());
    }

    #[test]
    fn a_revealed_value_is_hidden_by_time_or_by_losing_focus() {
        let mut state = state();
        state.toggle_reveal("alpha", RevealedKind::Value, 100.0);

        assert!(state.hide_expired_reveal(100.0 + VALUE_REVEAL_SECONDS - 0.5, true));
        assert!(state.is_revealed("alpha", RevealedKind::Value));

        assert!(!state.hide_expired_reveal(100.5, false));
        assert!(state.revealed.is_none());

        state.toggle_reveal("alpha", RevealedKind::Value, 100.0);
        assert!(!state.hide_expired_reveal(100.0 + VALUE_REVEAL_SECONDS, true));
        assert!(state.revealed.is_none());
        assert!(!state.hide_expired_reveal(200.0, true));
    }

    #[test]
    fn a_revealed_value_never_outlives_the_panel_that_showed_it() {
        let mut state = state();
        state.select_connector(Some("alpha".into()));
        state.toggle_reveal("alpha", RevealedKind::Value, 10.0);
        state.select_connector(Some("beta".into()));
        assert!(state.revealed.is_none());

        state.toggle_reveal("beta", RevealedKind::Value, 10.0);
        state.select_sandbox(Some("run-1".into()));
        assert!(state.revealed.is_none());
        assert!(state.selected_connector.is_none());

        state.select_connector(Some("beta".into()));
        state.toggle_reveal("beta", RevealedKind::Value, 10.0);
        state.replace_live_data(Vec::new(), Vec::new(), Vec::new(), Vec::new(), None);
        assert!(state.revealed.is_none());
    }

    #[test]
    fn search_matches_credentials_and_audit_rows_within_the_selected_sandbox() {
        let mut state = state();
        state.credentials[1].summary.sandboxes = vec![access("run-1", true)];

        let all = state.search_results("");
        assert_eq!(all.credentials.len(), 5);
        assert_eq!(all.audit.len(), 2);

        let by_name = state.search_results("  ALPHA ");
        assert_eq!(by_name.credentials, [1]);
        assert_eq!(by_name.audit, [0]);

        let by_project = state.search_results("projects/example");
        assert_eq!(by_project.credentials, [1]);
        assert!(by_project.audit.is_empty());

        let nothing = state.search_results("no-such-thing");
        assert!(nothing.credentials.is_empty());
        assert!(nothing.audit.is_empty());

        state.select_sandbox(Some("run-2".into()));
        assert!(state.search_results("alpha").credentials.is_empty());
    }

    #[test]
    fn search_reaches_every_disclosed_field_of_a_credential() {
        let mut state = state();
        let credential = &mut state.credentials[0].summary;
        credential.binding = CredentialBinding::OAuth;
        credential.status = CredentialStatus::Expiring;
        credential.account = Some("person@example.test".into());
        credential.environment_variable = Some("SOME_TOKEN".into());
        credential.recent_activity = Some("2m ago".into());
        credential.destinations = vec!["api.some-provider.example".into()];
        credential.sandboxes = vec![access("run-1", true)];
        state.credentials[2].summary.pending = Some(request("request-1", "run-2"));

        for needle in [
            "signed in",
            "expiring soon",
            "person@example.test",
            "some_token",
            "2m ago",
            "api.some-provider.example",
            "sandbox-run-1",
        ] {
            assert_eq!(
                state.search_results(needle).credentials,
                [0],
                "expected {needle} to match the first credential"
            );
        }
        assert_eq!(state.search_results("placeholder").credentials, [2]);
        assert_eq!(state.search_results("sandbox-run-2").credentials, [2]);
    }

    #[test]
    fn search_reaches_every_field_of_an_audit_row() {
        let state = state();
        for needle in ["11:00:00", "run-1", "credential", "used alpha", "alpha"] {
            assert!(
                !state.search_results(needle).audit.is_empty(),
                "expected {needle} to match an audit row"
            );
        }
    }

    #[test]
    fn an_unknown_run_falls_back_to_its_short_id() {
        let state = state();
        assert_eq!(state.sandbox_label_for_run("run-1"), "sandbox-run-1");
        assert_eq!(
            state.sandbox_label_for_run("9e8d7c6b0000000000000000000000aa"),
            lns_ipc::short_run_id("9e8d7c6b0000000000000000000000aa")
        );
    }

    #[test]
    fn the_machine_column_reports_the_binding_only_once_the_value_is_usable() {
        let mut summary = summary("some-provider", CredentialStatus::Active);
        for (binding, expected) in [
            (CredentialBinding::OAuth, "Signed in"),
            (CredentialBinding::Stored, "Stored"),
            (CredentialBinding::HostDetected, "Host value"),
            (CredentialBinding::Unbound, "Not configured"),
            (CredentialBinding::Denied, "Blocked"),
        ] {
            summary.binding = binding;
            assert_eq!(credential_machine_label(&summary).0, expected);
        }
        for (status, expected) in [
            (CredentialStatus::Pending, "Not configured"),
            (CredentialStatus::Expiring, "Expiring soon"),
            (CredentialStatus::Expired, "Expired"),
            (CredentialStatus::Denied, "Blocked"),
            (CredentialStatus::Unavailable, "Unavailable"),
        ] {
            summary.status = status;
            assert_eq!(credential_machine_label(&summary).0, expected);
        }
    }

    #[test]
    fn the_row_icon_warns_before_it_alarms() {
        assert_eq!(
            credential_row_icon_color(CredentialStatus::Active),
            TEXT_MUTED
        );
        assert_eq!(
            credential_row_icon_color(CredentialStatus::Pending),
            STATUS_WARNING
        );
        assert_eq!(
            credential_row_icon_color(CredentialStatus::Expiring),
            STATUS_WARNING
        );
        for status in [
            CredentialStatus::Expired,
            CredentialStatus::Denied,
            CredentialStatus::Unavailable,
        ] {
            assert_eq!(
                credential_row_icon_color(status),
                credential_status_color(status)
            );
        }
    }

    #[test]
    fn the_access_column_answers_for_the_sandbox_in_scope() {
        let mut state = state();
        state.credentials[0].summary.sandboxes = vec![access("run-1", true), access("run-2", true)];
        let credential = state.credentials[0].summary.clone();
        assert_eq!(
            credential_access_label(&state, &credential).0,
            "2 sandboxes"
        );

        state.credentials[0].summary.sandboxes = vec![access("run-1", true)];
        let credential = state.credentials[0].summary.clone();
        assert_eq!(credential_access_label(&state, &credential).0, "1 sandbox");

        state.select_sandbox(Some("run-1".into()));
        assert_eq!(credential_access_label(&state, &credential).0, "Authorized");
        state.select_sandbox(Some("run-2".into()));
        assert_eq!(
            credential_access_label(&state, &credential).0,
            "Not authorized"
        );

        state.select_sandbox(None);
        let mut waiting = credential.clone();
        waiting.sandboxes.clear();
        assert_eq!(
            credential_access_label(&state, &waiting).0,
            "No sandbox access"
        );
        waiting.pending = Some(request("request-1", "run-2"));
        assert_eq!(
            credential_access_label(&state, &waiting).0,
            "Review request"
        );
        state.select_sandbox(Some("run-1".into()));
        assert_eq!(
            credential_access_label(&state, &waiting).0,
            "Not authorized"
        );
    }

    #[test]
    fn the_authorization_field_names_why_a_sandbox_has_access() {
        let mut state = state();
        let mut credential = summary("some-provider", CredentialStatus::Active);
        assert_eq!(
            authorization_detail(&state, &credential),
            "Not authorized for this sandbox"
        );

        credential.sandboxes = vec![access("run-1", true)];
        assert_eq!(
            authorization_detail(&state, &credential),
            "Authorized · Connected by project policy"
        );

        credential.sandboxes = vec![access("run-1", false)];
        assert_eq!(
            authorization_detail(&state, &credential),
            "Not authorized · Connected by project policy"
        );

        state.select_sandbox(Some("run-2".into()));
        assert_eq!(
            authorization_detail(&state, &credential),
            "Not authorized for this sandbox"
        );
    }

    #[test]
    fn live_refresh_preserves_navigation_and_drops_stale_selections() {
        let mut state = UnifiedDashboardState::live(
            vec![sandbox("run-1")],
            vec![credential("alpha", CredentialStatus::Active)],
            vec![row("run-1", "alpha", "used alpha")],
            Vec::new(),
            None,
        );
        state.selected_sandbox = Some("run-1".into());
        state.selected_connector = Some("alpha".into());
        state.audit.selected = Some(0);
        state.audit.detail_row = Some(row("run-1", "alpha", "used alpha"));

        state.replace_live_data(
            vec![sandbox("run-1")],
            vec![credential("alpha", CredentialStatus::Active)],
            vec![row("run-1", "alpha", "used alpha")],
            Vec::new(),
            None,
        );
        assert_eq!(state.selected_sandbox.as_deref(), Some("run-1"));
        assert_eq!(state.selected_connector.as_deref(), Some("alpha"));
        assert_eq!(state.audit.selected, Some(0));

        state.replace_live_data(
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec!["warn".into()],
            Some("boom".into()),
        );
        assert_eq!(state.mode, DashboardMode::Live);
        assert_eq!(state.selected_sandbox, None);
        assert_eq!(state.selected_connector, None);
        assert_eq!(state.audit.selected, None);
        assert!(state.audit.detail_row.is_none());
        assert_eq!(state.audit.warnings, ["warn"]);
        assert_eq!(state.audit.last_error.as_deref(), Some("boom"));
    }

    #[test]
    fn live_review_hands_the_choice_to_the_service_and_scrubs_the_typed_value() {
        let mut state = UnifiedDashboardState::live(
            vec![sandbox("run-1")],
            vec![credential("alpha", CredentialStatus::Pending)],
            Vec::new(),
            Vec::new(),
            None,
        );
        let pending = request("request-1", "run-1");
        state.credentials[0].summary.pending = Some(pending.clone());
        state.reviewing_request = Some("request-1".into());
        state.review_value = Zeroizing::new("some-secret".into());
        let typed = state.review_value.clone();

        state.resolve_review(&pending, CredentialReviewChoice::UseValue(typed));

        assert!(state.review_value.is_empty());
        assert!(state.reviewing_request.is_none());
        let command = state.pending_command.take().expect("review command");
        assert!(matches!(
            &command,
            DashboardCommand::ReviewCredential { request_id, choice: CredentialReviewChoice::UseValue(value) }
                if request_id == "request-1" && value.as_str() == "some-secret"
        ));
        assert!(state.credentials[0].summary.pending.is_some());
    }

    #[test]
    fn preview_review_resolves_the_synthetic_request_both_ways() {
        let mut state = state();
        let pending = request("request-1", "run-1");
        state.credentials[2].summary.pending = Some(pending.clone());

        state.resolve_review(&pending, CredentialReviewChoice::UseBound);
        assert!(state.pending_command.is_none());
        let allowed = &state.credentials[2].summary;
        assert_eq!(allowed.status, CredentialStatus::Active);
        assert_eq!(allowed.binding, CredentialBinding::Stored);
        assert_eq!(allowed.sandboxes.len(), 1);
        assert!(allowed.sandboxes[0].active);
        assert!(
            state
                .notice
                .as_deref()
                .is_some_and(|n| n.contains("allowed"))
        );
        assert_eq!(state.audit.rows[0].connector.as_deref(), Some("waiting"));

        state.credentials[2].summary.pending = Some(pending.clone());
        state.resolve_review(&pending, CredentialReviewChoice::Deny);
        let denied = &state.credentials[2].summary;
        assert_eq!(denied.status, CredentialStatus::Denied);
        assert_eq!(denied.binding, CredentialBinding::Denied);
        assert_eq!(denied.sandboxes.len(), 1);
        assert!(
            state
                .notice
                .as_deref()
                .is_some_and(|n| n.contains("denied"))
        );
    }

    #[test]
    fn a_review_for_a_request_that_vanished_changes_nothing() {
        let mut state = state();
        state.reviewing_request = Some("request-1".into());
        state.resolve_review(&request("request-1", "run-1"), CredentialReviewChoice::Deny);
        assert!(state.reviewing_request.is_none());
        assert!(state.notice.is_none());
        assert!(state.audit.rows.iter().all(|row| row.detail != "denied"));
    }

    #[test]
    fn the_pending_request_lookup_finds_its_connector_and_display_name() {
        let mut state = state();
        state.credentials[2].summary.pending = Some(request("request-1", "run-1"));
        let (connector_id, display_name, found) =
            state.pending_request("request-1").expect("pending request");
        assert_eq!(connector_id, "waiting");
        assert_eq!(display_name, "waiting");
        assert_eq!(found.sandbox_id, "run-1");
        assert!(state.pending_request("missing").is_none());
    }

    #[test]
    fn live_replacement_hands_the_typed_value_over_without_touching_the_snapshot() {
        let mut state = UnifiedDashboardState::live(
            Vec::new(),
            vec![credential("alpha", CredentialStatus::Active)],
            Vec::new(),
            Vec::new(),
            None,
        );
        state.replacing_connector = Some("alpha".into());
        state.replacement_value = Zeroizing::new("new-secret".into());

        state.resolve_replacement("alpha");

        assert!(state.replacement_value.is_empty());
        assert!(state.replacing_connector.is_none());
        let command = state.pending_command.take().expect("replace command");
        assert!(matches!(
            &command,
            DashboardCommand::ReplaceCredential { connector_id, value }
                if connector_id == "alpha" && value.as_str() == "new-secret"
        ));
        assert_eq!(
            state.credentials[0].value.as_deref().map(String::as_str),
            Some("some-secret"),
            "the live snapshot changes only when the service reports the write"
        );
    }

    #[test]
    fn a_typed_value_is_trimmed_before_it_leaves_the_dashboard() {
        let mut preview = state();
        preview.replacement_value = Zeroizing::new(" new-secret ".into());
        preview.resolve_replacement("beta");
        assert_eq!(
            preview.credentials[0].value.as_deref().map(String::as_str),
            Some("new-secret")
        );

        let mut state = UnifiedDashboardState::live(
            vec![sandbox("run-1")],
            vec![credential("alpha", CredentialStatus::Pending)],
            Vec::new(),
            Vec::new(),
            None,
        );
        let pending = request("request-1", "run-1");
        state.credentials[0].summary.pending = Some(pending.clone());
        state.resolve_review(
            &pending,
            CredentialReviewChoice::UseValue(Zeroizing::new(" some-secret\n".into())),
        );
        assert!(matches!(
            state.pending_command.take().expect("review command"),
            DashboardCommand::ReviewCredential { choice: CredentialReviewChoice::UseValue(value), .. }
                if value.as_str() == "some-secret"
        ));

        state.replacement_value = Zeroizing::new("\tnew-secret \n".into());
        state.resolve_replacement("alpha");
        assert!(matches!(
            state.pending_command.take().expect("replace command"),
            DashboardCommand::ReplaceCredential { value, .. } if value.as_str() == "new-secret"
        ));
    }

    #[test]
    fn preview_replacement_swaps_the_synthetic_value() {
        let mut state = state();
        state.credentials[3].summary.status = CredentialStatus::Unavailable;
        state.credentials[3].summary.binding = CredentialBinding::Unbound;
        state.replacement_value = Zeroizing::new("new-secret".into());

        state.resolve_replacement("stale");

        assert!(state.pending_command.is_none());
        assert!(state.replacement_value.is_empty());
        assert_eq!(
            state.credentials[3].value.as_deref().map(String::as_str),
            Some("new-secret")
        );
        assert_eq!(
            state.credentials[3].summary.status,
            CredentialStatus::Active
        );
        assert_eq!(
            state.credentials[3].summary.binding,
            CredentialBinding::Stored
        );
        assert!(state.notice.as_deref().is_some_and(|n| n.contains("stale")));
        assert_eq!(state.audit.rows[0].connector.as_deref(), Some("stale"));
    }

    #[test]
    fn a_replacement_for_a_credential_that_vanished_changes_nothing() {
        let mut state = state();
        state.replacement_value = Zeroizing::new("new-secret".into());
        state.resolve_replacement("missing");
        assert!(state.replacing_connector.is_none());
        assert!(state.notice.is_some());
        assert!(state.credentials.iter().all(|credential| {
            credential.value.as_deref().map(String::as_str) == Some("some-secret")
        }));
    }

    #[test]
    fn live_confirmation_forwards_the_authoritative_target_ids() {
        let mut state = UnifiedDashboardState::live(
            Vec::new(),
            vec![credential("alpha", CredentialStatus::Active)],
            Vec::new(),
            Vec::new(),
            None,
        );
        state.resolve_confirmation(CredentialOperation::DisconnectProject {
            connector_id: "alpha".into(),
            sandbox_id: "run-1".into(),
            project: "~/projects/example".into(),
        });
        assert!(state.confirmation.is_none());
        assert!(matches!(
            state.pending_command.take().expect("disconnect command"),
            DashboardCommand::DisconnectProject { connector_id, sandbox_id }
                if connector_id == "alpha" && sandbox_id == "run-1"
        ));

        state.resolve_confirmation(CredentialOperation::ForgetEverywhere {
            connector_id: "alpha".into(),
        });
        assert!(matches!(
            state.pending_command.take().expect("remove command"),
            DashboardCommand::RemoveCredential { connector_id } if connector_id == "alpha"
        ));
    }

    #[test]
    fn preview_disconnect_only_touches_the_named_project() {
        let mut state = state();
        state.credentials[0].summary.sandboxes = vec![access("run-1", true), access("run-2", true)];
        state.credentials[0].summary.sandboxes[1].project = "/projects/other".into();

        state.resolve_confirmation(CredentialOperation::DisconnectProject {
            connector_id: "beta".into(),
            sandbox_id: "run-1".into(),
            project: "/projects/example".into(),
        });

        let sandboxes = &state.credentials[0].summary.sandboxes;
        assert!(!sandboxes[0].active);
        assert_eq!(sandboxes[0].reason, "Disconnected in this preview");
        assert!(sandboxes[1].active);
        assert!(
            state
                .notice
                .as_deref()
                .is_some_and(|n| n.contains("machine credential is unchanged"))
        );
        assert_eq!(state.audit.rows[0].run, "run-1");
    }

    #[test]
    fn preview_forget_unbinds_the_credential_and_every_grant() {
        let mut state = state();
        state.credentials[0].summary.sandboxes = vec![access("run-1", true)];
        state.toggle_reveal("beta", RevealedKind::Value, 10.0);

        state.resolve_confirmation(CredentialOperation::ForgetEverywhere {
            connector_id: "beta".into(),
        });

        let credential = &state.credentials[0];
        assert!(credential.value.is_none());
        assert!(state.revealed.is_none());
        assert_eq!(credential.summary.binding, CredentialBinding::Unbound);
        assert_eq!(credential.summary.status, CredentialStatus::Unavailable);
        assert!(!credential.summary.sandboxes[0].active);
        assert!(
            state
                .notice
                .as_deref()
                .is_some_and(|n| n.contains("was not revoked"))
        );
    }

    #[test]
    fn a_preview_event_without_any_sandbox_still_records_a_run() {
        let mut state = UnifiedDashboardState::seeded(
            Vec::new(),
            vec![credential("alpha", CredentialStatus::Active)],
            Vec::new(),
            Vec::new(),
        );
        state.resolve_confirmation(CredentialOperation::ForgetEverywhere {
            connector_id: "alpha".into(),
        });
        assert_eq!(state.audit.rows[0].run, "preview");
        assert_eq!(state.audit.rows[0].ts, PREVIEW_EVENT_TS);
        assert_eq!(state.audit.rows[0].when, PREVIEW_EVENT_WHEN);
    }

    #[test]
    fn debug_output_redacts_every_typed_value() {
        let commands = format!(
            "{:?} {:?} {:?} {:?}",
            DashboardCommand::ReplaceCredential {
                connector_id: "alpha".into(),
                value: Zeroizing::new("some-secret".into()),
            },
            DashboardCommand::ReviewCredential {
                request_id: "request-1".into(),
                choice: CredentialReviewChoice::UseValue(Zeroizing::new("some-token".into())),
            },
            DashboardCommand::RemoveCredential {
                connector_id: "alpha".into(),
            },
            DashboardCommand::DisconnectProject {
                connector_id: "alpha".into(),
                sandbox_id: "run-1".into(),
            },
        );
        assert!(!commands.contains("some-secret"));
        assert!(!commands.contains("some-token"));
        assert!(commands.contains("<redacted>"));
        assert!(commands.contains("UseValue(<redacted>)"));

        let choices = format!(
            "{:?} {:?} {:?} {:?}",
            CredentialReviewChoice::UseHost,
            CredentialReviewChoice::UseBound,
            CredentialReviewChoice::Connect,
            CredentialReviewChoice::Deny,
        );
        assert_eq!(choices, "UseHost UseBound Connect Deny");
    }

    #[test]
    fn credential_and_audit_share_the_primary_column_width() {
        let stored = TableColumnWidths::default();
        assert_eq!(
            CredentialColumns::for_width(900.0, &stored).connector,
            AuditColumns::for_width(900.0, &stored).event
        );
    }

    #[test]
    fn a_wide_table_spends_its_slack_on_the_primary_column() {
        let stored = TableColumnWidths::default();
        let columns = CredentialColumns::for_width(900.0, &stored);
        assert!(columns.wide);
        assert_eq!(columns.machine, stored.credential_machine);
        assert_eq!(columns.access, stored.credential_access);
        assert!(columns.activity > 0.0);
        assert_eq!(
            columns.connector + columns.machine + columns.access + columns.activity,
            900.0 - TABLE_CHEVRON_WIDTH
        );

        let audit = AuditColumns::for_width(900.0, &stored);
        assert_eq!(audit.sandbox, stored.audit_sandbox);
        assert_eq!(
            audit.event + audit.sandbox + audit.when,
            900.0 - TABLE_CHEVRON_WIDTH
        );
    }

    #[test]
    fn a_narrow_table_drops_the_optional_columns() {
        let stored = TableColumnWidths::default();
        let columns = CredentialColumns::for_width(600.0, &stored);
        assert!(!columns.wide);
        assert_eq!(columns.machine, 0.0);
        assert_eq!(columns.activity, 0.0);
        assert_eq!(
            columns.connector + columns.access,
            600.0 - TABLE_CHEVRON_WIDTH
        );

        let audit = AuditColumns::for_width(600.0, &stored);
        assert!(!audit.wide);
        assert_eq!(audit.sandbox, 0.0);
        assert_eq!(audit.event + audit.when, 600.0 - TABLE_CHEVRON_WIDTH);
    }

    #[test]
    fn a_dragged_column_cannot_starve_its_neighbours() {
        let stored = TableColumnWidths {
            primary: Some(10_000.0),
            credential_machine: 10_000.0,
            credential_access: 10_000.0,
            audit_sandbox: 10_000.0,
        };
        let columns = CredentialColumns::for_width(900.0, &stored);
        assert_eq!(columns.machine, TABLE_MACHINE_MIN);
        assert_eq!(columns.access, TABLE_ACCESS_MIN);
        assert!(columns.activity >= TABLE_ACTIVITY_MIN);
        let audit = AuditColumns::for_width(900.0, &stored);
        assert!(audit.sandbox >= TABLE_SANDBOX_MIN);
        assert!(audit.when >= TABLE_WHEN_MIN);

        let starved = TableColumnWidths {
            primary: Some(0.0),
            ..TableColumnWidths::default()
        };
        assert_eq!(
            CredentialColumns::for_width(900.0, &starved).connector,
            TABLE_PRIMARY_MIN
        );
        assert_eq!(
            CredentialColumns::for_width(200.0, &starved).connector,
            120.0
        );
    }
}
