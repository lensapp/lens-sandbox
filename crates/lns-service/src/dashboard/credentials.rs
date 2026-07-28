#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialBinding {
    OAuth,
    Stored,
    HostDetected,
    Unbound,
    Denied,
}

impl CredentialBinding {
    pub fn label(self) -> &'static str {
        match self {
            Self::OAuth => "Signed in",
            Self::Stored => "Stored on this machine",
            Self::HostDetected => "Uses host value",
            Self::Unbound => "Not configured",
            Self::Denied => "Blocked on this machine",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialStatus {
    Pending,
    Active,
    Expiring,
    Expired,
    Denied,
    Unavailable,
}

impl CredentialStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "Waiting for you",
            Self::Active => "Available",
            Self::Expiring => "Expiring soon",
            Self::Expired => "Expired",
            Self::Denied => "Denied",
            Self::Unavailable => "Unavailable",
        }
    }

    pub fn section(self) -> CredentialSection {
        match self {
            Self::Pending | Self::Expiring | Self::Expired => CredentialSection::NeedsAttention,
            Self::Active => CredentialSection::Active,
            Self::Denied | Self::Unavailable => CredentialSection::Denied,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialSection {
    NeedsAttention,
    Active,
    Denied,
}

impl CredentialSection {
    pub fn label(self) -> &'static str {
        match self {
            Self::NeedsAttention => "NEEDS ATTENTION",
            Self::Active => "ACTIVE ACCESS",
            Self::Denied => "DENIED OR UNAVAILABLE",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CredentialFilter {
    #[default]
    All,
    Pending,
    Denied,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialSandbox {
    pub id: String,
    pub name: String,
    pub project: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxAccess {
    pub sandbox_id: String,
    pub sandbox_name: String,
    pub project: String,
    pub reason: String,
    pub active: bool,
    pub revocable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingCredentialRequest {
    pub id: String,
    pub sandbox_id: String,
    pub sandbox_name: String,
    pub project: String,
    pub action: String,
    pub requested_at: String,
    pub held_requests: usize,
    pub host_value_available: bool,
    pub oauth: bool,
    pub token_fallback: bool,
    pub verification_uri: Option<String>,
    pub user_code: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialSummary {
    pub connector_id: String,
    pub display_name: String,
    pub binding: CredentialBinding,
    pub status: CredentialStatus,
    pub account: Option<String>,
    pub scopes: Vec<String>,
    pub expires_at: Option<String>,
    pub environment_variable: Option<String>,
    pub destinations: Vec<String>,
    pub sandboxes: Vec<SandboxAccess>,
    pub recent_activity: Option<String>,
    pub pending: Option<PendingCredentialRequest>,
}

impl CredentialSummary {
    pub fn active_sandbox_count(&self) -> usize {
        self.sandboxes.iter().filter(|access| access.active).count()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialOperation {
    RevokeSandbox {
        connector_id: String,
        sandbox_id: String,
        sandbox_name: String,
        project: String,
    },
    DisconnectProject {
        connector_id: String,
        project: String,
    },
    ForgetEverywhere {
        connector_id: String,
    },
}

impl CredentialOperation {
    pub fn title(&self) -> &'static str {
        match self {
            Self::RevokeSandbox { .. } => "Disconnect from project?",
            Self::DisconnectProject { .. } => "Disconnect from project?",
            Self::ForgetEverywhere { .. } => "Forget saved access everywhere?",
        }
    }

    pub fn confirm_label(&self) -> &'static str {
        match self {
            Self::RevokeSandbox { .. } => "Disconnect",
            Self::DisconnectProject { .. } => "Disconnect",
            Self::ForgetEverywhere { .. } => "Forget access",
        }
    }

    pub fn description(&self) -> String {
        match self {
            Self::RevokeSandbox {
                connector_id,
                sandbox_id: _,
                sandbox_name: _,
                project,
            } => format!(
                "{connector_id} will be removed from {project}. Every sandbox in this project will lose access and future use will ask again."
            ),
            Self::DisconnectProject {
                connector_id,
                project,
            } => format!(
                "{connector_id} will be removed from {project}. Saved machine access remains available to other projects."
            ),
            Self::ForgetEverywhere { connector_id } => format!(
                "Every running sandbox will lose {connector_id} now. This does not revoke authorization at the provider."
            ),
        }
    }
}

#[derive(Debug)]
pub struct CredentialDashboardState {
    pub credentials: Vec<CredentialSummary>,
    pub sandboxes: Vec<CredentialSandbox>,
    pub selected_filter: CredentialFilter,
    pub selected_sandbox: Option<String>,
    pub selected_connector: Option<String>,
    pub sidebar_open: bool,
    pub confirmation: Option<CredentialOperation>,
    pub notice: Option<String>,
}

impl CredentialDashboardState {
    pub fn seeded(credentials: Vec<CredentialSummary>, sandboxes: Vec<CredentialSandbox>) -> Self {
        Self {
            credentials,
            sandboxes,
            selected_filter: CredentialFilter::All,
            selected_sandbox: None,
            selected_connector: None,
            sidebar_open: true,
            confirmation: None,
            notice: None,
        }
    }

    pub fn selected_credential(&self) -> Option<&CredentialSummary> {
        let selected = self.selected_connector.as_deref()?;
        self.credentials
            .iter()
            .find(|credential| credential.connector_id == selected)
    }

    pub fn count(&self, filter: CredentialFilter) -> usize {
        self.credentials
            .iter()
            .filter(|credential| filter_matches(credential, filter))
            .count()
    }
}

pub fn visible_indices(state: &CredentialDashboardState) -> Vec<usize> {
    state
        .credentials
        .iter()
        .enumerate()
        .filter(|(_, credential)| filter_matches(credential, state.selected_filter))
        .filter(|(_, credential)| {
            state.selected_sandbox.as_ref().is_none_or(|selected| {
                credential
                    .sandboxes
                    .iter()
                    .any(|access| &access.sandbox_id == selected)
                    || credential
                        .pending
                        .as_ref()
                        .is_some_and(|request| &request.sandbox_id == selected)
            })
        })
        .map(|(index, _)| index)
        .collect()
}

fn filter_matches(credential: &CredentialSummary, filter: CredentialFilter) -> bool {
    match filter {
        CredentialFilter::All => true,
        CredentialFilter::Pending => credential.status == CredentialStatus::Pending,
        CredentialFilter::Denied => matches!(
            credential.status,
            CredentialStatus::Denied | CredentialStatus::Unavailable
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn credential(
        id: &str,
        status: CredentialStatus,
        sandbox_id: Option<&str>,
    ) -> CredentialSummary {
        CredentialSummary {
            connector_id: id.into(),
            display_name: id.into(),
            binding: CredentialBinding::Stored,
            status,
            account: None,
            scopes: vec![],
            expires_at: None,
            environment_variable: None,
            destinations: vec![],
            sandboxes: sandbox_id
                .map(|id| SandboxAccess {
                    sandbox_id: id.into(),
                    sandbox_name: id.into(),
                    project: "/projects/example".into(),
                    reason: "Connected by project policy".into(),
                    active: true,
                    revocable: true,
                })
                .into_iter()
                .collect(),
            recent_activity: None,
            pending: (status == CredentialStatus::Pending).then(|| PendingCredentialRequest {
                id: format!("{id}-request"),
                sandbox_id: sandbox_id.unwrap_or_default().into(),
                sandbox_name: sandbox_id.unwrap_or_default().into(),
                project: "/projects/example".into(),
                action: "connect".into(),
                requested_at: "just now".into(),
                held_requests: 1,
                host_value_available: false,
                oauth: false,
                token_fallback: false,
                verification_uri: None,
                user_code: None,
            }),
        }
    }

    fn state() -> CredentialDashboardState {
        CredentialDashboardState::seeded(
            vec![
                credential("active", CredentialStatus::Active, Some("sandbox-a")),
                credential("pending", CredentialStatus::Pending, Some("sandbox-b")),
                credential("denied", CredentialStatus::Denied, Some("sandbox-a")),
                credential("unavailable", CredentialStatus::Unavailable, None),
            ],
            vec![],
        )
    }

    #[test]
    fn labels_describe_binding_status_and_sections() {
        assert_eq!(CredentialBinding::OAuth.label(), "Signed in");
        assert_eq!(CredentialBinding::Stored.label(), "Stored on this machine");
        assert_eq!(CredentialBinding::HostDetected.label(), "Uses host value");
        assert_eq!(CredentialBinding::Unbound.label(), "Not configured");
        assert_eq!(CredentialBinding::Denied.label(), "Blocked on this machine");

        assert_eq!(CredentialStatus::Pending.label(), "Waiting for you");
        assert_eq!(CredentialStatus::Active.label(), "Available");
        assert_eq!(CredentialStatus::Expiring.label(), "Expiring soon");
        assert_eq!(CredentialStatus::Expired.label(), "Expired");
        assert_eq!(CredentialStatus::Denied.label(), "Denied");
        assert_eq!(CredentialStatus::Unavailable.label(), "Unavailable");

        assert_eq!(
            CredentialStatus::Pending.section(),
            CredentialSection::NeedsAttention
        );
        assert_eq!(
            CredentialStatus::Expiring.section(),
            CredentialSection::NeedsAttention
        );
        assert_eq!(
            CredentialStatus::Expired.section(),
            CredentialSection::NeedsAttention
        );
        assert_eq!(
            CredentialStatus::Active.section(),
            CredentialSection::Active
        );
        assert_eq!(
            CredentialStatus::Denied.section(),
            CredentialSection::Denied
        );
        assert_eq!(
            CredentialStatus::Unavailable.section(),
            CredentialSection::Denied
        );
        assert_eq!(CredentialSection::NeedsAttention.label(), "NEEDS ATTENTION");
        assert_eq!(CredentialSection::Active.label(), "ACTIVE ACCESS");
        assert_eq!(CredentialSection::Denied.label(), "DENIED OR UNAVAILABLE");
    }

    #[test]
    fn filters_select_pending_and_denied_credentials() {
        let mut state = state();
        assert_eq!(state.count(CredentialFilter::All), 4);
        assert_eq!(state.count(CredentialFilter::Pending), 1);
        assert_eq!(state.count(CredentialFilter::Denied), 2);
        assert_eq!(visible_indices(&state), [0, 1, 2, 3]);

        state.selected_filter = CredentialFilter::Pending;
        assert_eq!(visible_indices(&state), [1]);

        state.selected_filter = CredentialFilter::Denied;
        assert_eq!(visible_indices(&state), [2, 3]);
    }

    #[test]
    fn sandbox_filter_includes_access_and_pending_origin() {
        let mut state = state();
        state.selected_sandbox = Some("sandbox-a".into());
        assert_eq!(visible_indices(&state), [0, 2]);

        state.selected_sandbox = Some("sandbox-b".into());
        assert_eq!(visible_indices(&state), [1]);

        state.selected_sandbox = Some("missing".into());
        assert!(visible_indices(&state).is_empty());
    }

    #[test]
    fn selected_credential_and_active_count_are_derived_without_secret_values() {
        let mut state = state();
        assert!(state.selected_credential().is_none());
        state.selected_connector = Some("active".into());
        let selected = state.selected_credential().expect("selected credential");
        assert_eq!(selected.connector_id, "active");
        assert_eq!(selected.active_sandbox_count(), 1);
    }

    #[test]
    fn operation_copy_names_scope_and_impact() {
        let revoke = CredentialOperation::RevokeSandbox {
            connector_id: "some-provider".into(),
            sandbox_id: "run-1".into(),
            sandbox_name: "calm-finch".into(),
            project: "~/projects/example".into(),
        };
        assert_eq!(revoke.title(), "Disconnect from project?");
        assert_eq!(revoke.confirm_label(), "Disconnect");
        assert!(revoke.description().contains("~/projects/example"));

        let disconnect = CredentialOperation::DisconnectProject {
            connector_id: "some-provider".into(),
            project: "/projects/demo".into(),
        };
        assert_eq!(disconnect.title(), "Disconnect from project?");
        assert_eq!(disconnect.confirm_label(), "Disconnect");
        assert!(disconnect.description().contains("/projects/demo"));

        let forget = CredentialOperation::ForgetEverywhere {
            connector_id: "some-provider".into(),
        };
        assert_eq!(forget.title(), "Forget saved access everywhere?");
        assert_eq!(forget.confirm_label(), "Forget access");
        assert!(forget.description().contains("does not revoke"));
    }
}
