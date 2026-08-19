use std::collections::BTreeMap;

/// A local definition's path fileset: an absolute host directory snapshotted into the guest at launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalFileset {
    pub source: String,
    pub guest_path: String,
    pub owner: lns_artifact::sandbox::FilesetOwner,
}

/// A definition's hostPath fileset: one file on the machine that runs it, snapshotted into the guest at launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostFileset {
    pub source: String,
    pub guest_path: String,
    pub owner: lns_artifact::sandbox::FilesetOwner,
    pub optional: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InlineFileset {
    pub files: BTreeMap<String, String>,
    pub guest_path: String,
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

/// A `path` fileset that arrived packed into a layer of the artifact declaring it, materialized from that artifact rather than from this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackedFileset {
    pub guest_path: String,
    pub source: super::PackedSource,
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
