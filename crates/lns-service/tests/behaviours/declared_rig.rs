use lns_policy::Policy;
use lns_policy::connectors::Connector;
use lns_policy::credentials::CredentialStateFile;
use lns_service::approval_flow::protocol::Credential;
use lns_service::artifact::credential_boot::ConnectPrompt;

/// Drives a sandbox definition through the launch path: catalog + definition + overlay + machine store in, armed providers (from the overlay's connected connectors and credential slots) + the ids offered for a reactive connect + running policy (or a blocked/refused launch) out.
#[derive(Debug, Default)]
pub struct DeclaredRig {
    pub catalog: Vec<Connector>,
    pub definition: Option<String>,
    pub overlay: Policy,
    pub store: CredentialStateFile,
    /// Armed providers as (connector id, env var, placeholder).
    pub providers: Vec<(String, String, String)>,
    /// The run's wire credentials as the boundary would receive them at boot.
    pub wire: Vec<Credential>,
    /// Connector ids offered for a reactive connect on first use (never armed at launch).
    pub offered: Vec<String>,
    pub running_policy: Option<Policy>,
    /// The sign-in the launch is blocked on, when the gate said AwaitConnect.
    pub pending: Option<ConnectPrompt>,
    pub error: Option<String>,
    /// The definition bytes as authored, for pinning that nothing writes back to it.
    pub definition_snapshot: Option<String>,
}
