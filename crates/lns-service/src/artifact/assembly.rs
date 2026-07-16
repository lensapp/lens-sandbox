use std::collections::BTreeMap;

/// A launch-time `--with` override addressed by OCI reference; it resolves through the component graph like any bundle component, landing last so it overlays the base image and bundle filesets.
#[derive(Debug, Clone)]
pub struct Override {
    pub reference: String,
}

/// A local definition's path fileset: an absolute host directory snapshotted into the guest at launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalFileset {
    pub source: String,
    pub mount_path: String,
}

#[derive(Debug, Default, Clone)]
pub struct ResolvedBundle {
    pub base_image: String,
    pub base_paths: Vec<String>,
    pub local_filesets: Vec<LocalFileset>,
    pub filesets: Vec<ResolvedFileset>,
    pub command: Option<String>,
    pub env: BTreeMap<String, String>,
    pub resources: Option<crate::artifact::spec::Resources>,
    pub policy: Option<lns_policy::Policy>,
    pub credentials: Vec<crate::artifact::spec::CredentialSlot>,
}

#[derive(Debug, Clone)]
pub struct ResolvedFileset {
    pub name: String,
    pub paths: Vec<String>,
    /// The digest-pinned OCI reference the fileset's content layer is pulled from at materialization.
    pub reference: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileSource {
    BaseImage,
    Fileset(String),
}

#[derive(Debug, Clone)]
pub struct AssembledWorkload {
    pub base_image: String,
    pub command: Option<String>,
    pub env: BTreeMap<String, String>,
    pub resources: Option<crate::artifact::spec::Resources>,
    pub policy: Option<lns_policy::Policy>,
    pub credentials: Vec<crate::artifact::spec::CredentialSlot>,
    ownership: BTreeMap<String, FileSource>,
}

impl AssembledWorkload {
    pub fn source_of(&self, path: &str) -> Option<&FileSource> {
        self.ownership.get(path)
    }
}

pub fn assemble(bundle: &ResolvedBundle) -> AssembledWorkload {
    let mut ownership = BTreeMap::new();
    for path in &bundle.base_paths {
        ownership.insert(path.clone(), FileSource::BaseImage);
    }
    for fileset in &bundle.filesets {
        for path in &fileset.paths {
            ownership.insert(path.clone(), FileSource::Fileset(fileset.name.clone()));
        }
    }
    AssembledWorkload {
        base_image: bundle.base_image.clone(),
        command: bundle.command.clone(),
        env: bundle.env.clone(),
        resources: bundle.resources.clone(),
        policy: bundle.policy.clone(),
        credentials: bundle.credentials.clone(),
        ownership,
    }
}
