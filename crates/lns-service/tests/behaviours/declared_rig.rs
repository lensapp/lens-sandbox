use lns_policy::Policy;
use lns_policy::credentials::CredentialStateFile;
use lns_policy::integrations::Integration;
use lns_service::artifact::credential_boot::ConnectPrompt;

/// Drives a sandbox definition through the launch path: catalog + definition + overlay + machine store in, armed providers (from the overlay's connected integrations and credential slots) + the ids offered for a reactive connect + running policy (or a blocked/refused launch) out.
#[derive(Debug, Default)]
pub struct DeclaredRig {
    pub catalog: Vec<Integration>,
    pub definition: Option<String>,
    pub overlay: Policy,
    pub store: CredentialStateFile,
    /// Armed providers as (integration id, env var, placeholder).
    pub providers: Vec<(String, String, String)>,
    /// Integration ids offered for a reactive connect on first use (never armed at launch).
    pub offered: Vec<String>,
    pub running_policy: Option<Policy>,
    /// The sign-in the launch is blocked on, when the gate said AwaitConnect.
    pub pending: Option<ConnectPrompt>,
    pub error: Option<String>,
    /// The definition bytes as authored, for pinning that nothing writes back to it.
    pub definition_snapshot: Option<String>,
}
