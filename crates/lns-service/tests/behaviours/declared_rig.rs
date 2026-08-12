use std::collections::HashSet;

use lns_policy::Policy;
use lns_policy::connectors::Connector;
use lns_policy::credentials::CredentialStateFile;
use lns_service::approval_flow::protocol::Credential;
use lns_service::artifact::credential_boot::ConnectPrompt;

/// Drives a sandbox definition through the launch path: catalog + definition + overlay + machine store in, armed providers (from the overlay's connected connectors and credential slots this workload has granted) + the ids offered for a reactive connect + running policy (or a blocked/refused launch) out.
#[derive(Debug, Default)]
pub struct DeclaredRig {
    pub catalog: Vec<Connector>,
    pub definition: Option<String>,
    pub overlay: Policy,
    pub store: CredentialStateFile,
    /// Ids this workload has NOT granted; the launch defaults to consenting to its applied connectors, so a scenario adds an id here to model a cloned overlay or a slot with no machine-local grant.
    pub withhold_grants: HashSet<String>,
    /// Ids whose boot-gate sign-in the user completed this launch; their consent grants the workload even where a grant was withheld.
    pub signed_in: Vec<String>,
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
    /// Mixin documents this machine can resolve, keyed by reference.
    pub mixins: std::collections::BTreeMap<String, String>,
    /// The tools the resolved document asks for, so a scenario can see what a mixin contributed.
    pub tools: Vec<String>,
}
