use lns_policy::Policy;

/// Drives a sandbox definition through the launch path: definition + overlay in, running policy (or a refused launch) out.
#[derive(Debug, Default)]
pub struct DeclaredRig {
    pub definition: Option<String>,
    pub overlay: Policy,
    pub running_policy: Option<Policy>,
    pub error: Option<String>,
    /// Mixin documents this machine can resolve, keyed by reference.
    pub mixins: std::collections::BTreeMap<String, String>,
    /// The document the directory's own decisions file holds, for a scenario about the source §3.3.2 puts last.
    pub local_mixin: Option<String>,
    /// The document the resolution produced, so a scenario can see what did and did not get merged into it.
    pub resolved_document: Option<String>,
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
