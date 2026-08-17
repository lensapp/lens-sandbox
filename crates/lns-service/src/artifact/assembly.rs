use std::collections::BTreeMap;

/// A local definition's path fileset: an absolute host directory snapshotted into the guest at launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalFileset {
    pub source: String,
    pub mount_path: String,
    pub owner: lns_artifact::sandbox::FilesetOwner,
}

/// A definition's hostPath fileset: one file on the machine that runs it, snapshotted into the guest at launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostFileset {
    pub source: String,
    pub mount_path: String,
    pub owner: lns_artifact::sandbox::FilesetOwner,
    pub optional: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineFileset {
    pub files: BTreeMap<String, String>,
    pub mount_path: String,
    pub owner: lns_artifact::sandbox::FilesetOwner,
}

#[derive(Debug, Default, Clone)]
pub struct ResolvedSandbox {
    pub base_image: String,
    pub local_filesets: Vec<LocalFileset>,
    pub host_filesets: Vec<HostFileset>,
    pub inline_filesets: Vec<InlineFileset>,
    pub packed_filesets: Vec<PackedFileset>,
    pub command: Option<String>,
    pub user: Option<String>,
    pub env: BTreeMap<String, String>,
    pub resources: Option<crate::artifact::spec::Resources>,
    pub policy: Option<lns_policy::Policy>,
    pub credentials: Vec<lns_spec::Credential>,
    pub tools: Vec<String>,
}

/// A `path` fileset whose files live in a pulled artifact: the document that shipped it, and which of that artifact's layers holds them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackedFileset {
    pub mount_path: String,
    /// The digest-pinned OCI reference whose layer the content is pulled from at materialization.
    pub reference: String,
    pub layer: usize,
    pub owner: lns_artifact::sandbox::FilesetOwner,
}

#[derive(Debug, Clone)]
pub struct AssembledWorkload {
    pub base_image: String,
    pub command: Option<String>,
    pub user: Option<String>,
    pub env: BTreeMap<String, String>,
    pub resources: Option<crate::artifact::spec::Resources>,
    pub policy: Option<lns_policy::Policy>,
    pub credentials: Vec<lns_spec::Credential>,
    pub tools: Vec<String>,
}

pub fn assemble(sandbox: &ResolvedSandbox) -> AssembledWorkload {
    AssembledWorkload {
        base_image: sandbox.base_image.clone(),
        command: sandbox.command.clone(),
        user: sandbox.user.clone(),
        env: sandbox.env.clone(),
        resources: sandbox.resources.clone(),
        policy: sandbox.policy.clone(),
        credentials: sandbox.credentials.clone(),
        tools: sandbox.tools.clone(),
    }
}
