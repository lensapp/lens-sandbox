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
    /// The document the directory's own decisions file holds, for a scenario about the source §3.3.2 puts last.
    pub local_mixin: Option<String>,
    /// The document the resolution produced, so a scenario can see what did and did not get merged into it.
    pub resolved_document: Option<String>,
    /// Pinned `--mixin` references this run is composed of; the rig grants the bare identity, so a composed launch models a grant an earlier bare run earned.
    pub composed_mixins: Vec<String>,
    /// The tools the resolved document asks for, so a scenario can see what a mixin contributed.
    pub tools: Vec<String>,
    /// What a reference resolves to, so a scenario can publish a mixin under a tag.
    pub mixin_pins: std::collections::BTreeMap<String, String>,
    /// The packed fileset layers each published mixin's artifact carries, keyed by reference.
    pub mixin_layers: std::collections::BTreeMap<String, Vec<lns_service::artifact::PackedLayer>>,
    /// Which artifact the launch decided each packed fileset is pulled from.
    pub packed_filesets: lns_service::artifact::PackedFilesets,
    /// Every source the resolution merged, as the disclosure names them.
    pub resolved_mixins: Vec<String>,
    /// Which source decided each entry of the merged document, as the disclosure reads them.
    pub contributions: Vec<lns_ipc::SourceContribution>,
    /// The pins for the references the user named, in the order they named them.
    pub pinned_extra: Vec<String>,
    /// Where a local definition sits, since a directory it names roots there.
    pub project_dir: Option<String>,
}
