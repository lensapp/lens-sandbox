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

/// `bound_value_available` mirrors the approval card's own flag: the machine already holds a usable value, so the request can be answered by granting it rather than by pasting one or signing in again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingCredentialRequest {
    pub id: String,
    pub sandbox_id: String,
    pub sandbox_name: String,
    pub project: String,
    pub action: String,
    pub host_value_available: bool,
    pub bound_value_available: bool,
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

/// `sandbox_id` names the run whose policy file the disconnect rewrites; the decision itself is project-wide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CredentialOperation {
    DisconnectProject {
        connector_id: String,
        sandbox_id: String,
        project: String,
    },
    ForgetEverywhere {
        connector_id: String,
    },
}

impl CredentialOperation {
    pub fn title(&self) -> &'static str {
        match self {
            Self::DisconnectProject { .. } => "Disconnect from project?",
            Self::ForgetEverywhere { .. } => "Forget saved access everywhere?",
        }
    }

    pub fn confirm_label(&self) -> &'static str {
        match self {
            Self::DisconnectProject { .. } => "Disconnect",
            Self::ForgetEverywhere { .. } => "Forget access",
        }
    }

    pub fn description(&self) -> String {
        match self {
            Self::DisconnectProject {
                connector_id,
                project,
                ..
            } => format!(
                "{connector_id} will be removed from {project}. Every sandbox in this project will lose access and future use will ask again."
            ),
            Self::ForgetEverywhere { connector_id } => format!(
                "Every running sandbox will lose {connector_id} now. This does not revoke authorization at the provider."
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_describe_the_binding_and_status_a_user_sees() {
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
    }

    #[test]
    fn only_running_sandboxes_count_as_active_access() {
        let access = |sandbox_id: &str, active: bool| SandboxAccess {
            sandbox_id: sandbox_id.into(),
            sandbox_name: sandbox_id.into(),
            project: "/projects/example".into(),
            reason: "Connected by project policy".into(),
            active,
            revocable: true,
        };
        let summary = CredentialSummary {
            connector_id: "some-provider".into(),
            display_name: "Some Provider".into(),
            binding: CredentialBinding::Stored,
            status: CredentialStatus::Active,
            account: None,
            scopes: vec![],
            expires_at: None,
            environment_variable: None,
            destinations: vec![],
            sandboxes: vec![access("run-1", true), access("run-2", false)],
            recent_activity: None,
            pending: None,
        };
        assert_eq!(summary.active_sandbox_count(), 1);
    }

    #[test]
    fn operation_copy_names_scope_and_impact() {
        let disconnect = CredentialOperation::DisconnectProject {
            connector_id: "some-provider".into(),
            sandbox_id: "run-1".into(),
            project: "~/projects/example".into(),
        };
        assert_eq!(disconnect.title(), "Disconnect from project?");
        assert_eq!(disconnect.confirm_label(), "Disconnect");
        assert!(disconnect.description().contains("~/projects/example"));
        assert!(disconnect.description().contains("will ask again"));

        let forget = CredentialOperation::ForgetEverywhere {
            connector_id: "some-provider".into(),
        };
        assert_eq!(forget.title(), "Forget saved access everywhere?");
        assert_eq!(forget.confirm_label(), "Forget access");
        assert!(forget.description().contains("does not revoke"));
    }
}
