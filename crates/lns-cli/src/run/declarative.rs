use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use anyhow::{Result, bail};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountDefault {
    pub bind: bool,
    pub source: String,
    pub target: String,
    pub read_only: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Defaults {
    pub workdir: Option<String>,
    pub mounts: Vec<MountDefault>,
}

impl Defaults {
    pub fn from_definition(definition: &lns_artifact::sandbox::Definition) -> Self {
        Self {
            workdir: definition.spec.workdir.clone(),
            mounts: definition
                .spec
                .volumes
                .iter()
                .map(|volume| MountDefault {
                    bind: volume.is_bind(),
                    source: volume.source().to_string(),
                    target: volume.target.clone(),
                    read_only: volume.read_only(),
                })
                .collect(),
        }
    }

    pub fn from_view(view: &lns_ipc::SandboxView) -> Self {
        Self {
            workdir: view.workdir.clone(),
            mounts: view
                .mounts
                .iter()
                .map(|mount| MountDefault {
                    bind: mount.kind == lns_ipc::SandboxMountKind::Bind,
                    source: mount.source.clone(),
                    target: mount.target.clone(),
                    read_only: mount.read_only,
                })
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub workdir: Option<String>,
    pub mounts: Vec<lns_ipc::MountSpec>,
}

pub fn resolve(
    defaults: &Defaults,
    project_dir: &Path,
    explicit_workdir: Option<String>,
    explicit_mounts: Vec<lns_ipc::MountSpec>,
) -> Result<Resolved> {
    let overridden_targets: BTreeSet<&str> = explicit_mounts
        .iter()
        .map(lns_ipc::MountSpec::target)
        .collect();
    let mut mounts = defaults
        .mounts
        .iter()
        .filter(|mount| !overridden_targets.contains(mount.target.as_str()))
        .map(|mount| resolve_mount(mount, project_dir))
        .collect::<Result<Vec<_>>>()?;
    mounts.extend(explicit_mounts);
    Ok(Resolved {
        workdir: explicit_workdir.or_else(|| defaults.workdir.clone()),
        mounts,
    })
}

fn resolve_mount(mount: &MountDefault, project_dir: &Path) -> Result<lns_ipc::MountSpec> {
    lns_ipc::validate_volume_target(&mount.target).map_err(anyhow::Error::msg)?;
    if mount.bind {
        let source = resolve_bind_source(&mount.source, project_dir)?;
        lns_ipc::validate_bind_source(&source).map_err(anyhow::Error::msg)?;
        return Ok(lns_ipc::MountSpec::Bind(lns_ipc::BindSpec {
            host_source: source,
            target: mount.target.clone(),
            read_only: mount.read_only,
        }));
    }
    lns_ipc::validate_volume_name(&mount.source).map_err(anyhow::Error::msg)?;
    Ok(lns_ipc::MountSpec::Named(lns_ipc::VolumeMount {
        name: mount.source.clone(),
        target: mount.target.clone(),
        read_only: mount.read_only,
    }))
}

fn resolve_bind_source(source: &str, project_dir: &Path) -> Result<String> {
    let source = Path::new(source);
    let joined = if source.is_absolute() {
        source.to_path_buf()
    } else {
        project_dir.join(source)
    };
    let normalized = normalize_absolute(&joined)?;
    normalized
        .into_os_string()
        .into_string()
        .map_err(|_| anyhow::anyhow!("bind source is not valid UTF-8"))
}

fn normalize_absolute(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() {
        bail!("project directory must be absolute: {}", path.display());
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                if !normalized.pop() {
                    bail!(
                        "bind source escapes the filesystem root: {}",
                        path.display()
                    );
                }
            }
            _ => normalized.push(component.as_os_str()),
        }
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_bind_sources_are_normalized_under_the_project_directory() {
        assert_eq!(
            resolve_bind_source("./src", Path::new("/work/project")).unwrap(),
            "/work/project/src"
        );
    }

    #[test]
    fn absolute_bind_sources_are_normalized_without_using_the_project_directory() {
        assert_eq!(
            resolve_bind_source("/work/./project", Path::new("/elsewhere")).unwrap(),
            "/work/project"
        );
    }

    #[test]
    fn resolution_rejects_a_relative_project_directory() {
        let err = resolve_bind_source(".", Path::new("relative")).unwrap_err();
        assert!(format!("{err:#}").contains("project directory must be absolute"));
    }

    #[test]
    fn normalization_rejects_parent_traversal_above_root() {
        let err = normalize_absolute(Path::new("/../work")).unwrap_err();
        assert!(format!("{err:#}").contains("escapes"));
    }
}
