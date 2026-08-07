use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountDefault {
    pub bind: bool,
    pub source: String,
    pub target: String,
    pub read_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PortDefault {
    pub host: Option<i64>,
    pub container: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Defaults {
    pub workdir: Option<String>,
    pub mounts: Vec<MountDefault>,
    pub ports: Vec<PortDefault>,
    pub size: lns_artifact::resources::DeclaredSize,
}

impl Defaults {
    pub fn from_definition(definition: &lns_artifact::sandbox::Definition) -> Self {
        let (size, _) = lns_artifact::resources::DeclaredSize::from_resources(
            definition.spec.resources.as_ref(),
        );
        Self {
            size,
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
            ports: definition
                .spec
                .ports
                .iter()
                .map(|port| PortDefault {
                    host: port.host,
                    container: port.container,
                })
                .collect(),
        }
    }

    pub fn from_view(view: &lns_ipc::SandboxView) -> Self {
        Self {
            size: lns_artifact::resources::DeclaredSize {
                cpus: view.cpus,
                mem_mib: view.mem_mib,
            },
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
            ports: view
                .ports
                .iter()
                .map(|port| PortDefault {
                    host: port.host.map(i64::from),
                    container: i64::from(port.container),
                })
                .collect(),
        }
    }
}

#[derive(Debug)]
pub struct ComposedPorts {
    pub published: Vec<lns_ipc::PortPublish>,
    pub declared_unpublished: Vec<u16>,
}

/// Explicit -p entries win a container-port conflict; without publish_declared the declared set is disclosure only.
pub fn compose_ports(
    declared: &[PortDefault],
    explicit: Vec<lns_ipc::PortPublish>,
    publish_declared: bool,
) -> Result<ComposedPorts> {
    let explicit_containers: BTreeSet<u16> =
        explicit.iter().map(|port| port.container_port).collect();
    let mut published = Vec::new();
    let mut declared_unpublished = Vec::new();
    let mut substituted = BTreeSet::new();
    for port in declared {
        let container = declared_port(port.container, "container")?;
        if explicit_containers.contains(&container) {
            published.extend(
                explicit
                    .iter()
                    .filter(|explicit| explicit.container_port == container)
                    .copied(),
            );
            substituted.insert(container);
        } else if publish_declared {
            published.push(lns_ipc::PortPublish {
                host_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
                host_port: match port.host {
                    Some(host) => declared_port(host, "host")?,
                    None => container,
                },
                container_port: container,
                protocol: lns_ipc::Protocol::Tcp,
            });
        } else {
            declared_unpublished.push(container);
        }
    }
    published.extend(
        explicit
            .into_iter()
            .filter(|port| !substituted.contains(&port.container_port)),
    );
    Ok(ComposedPorts {
        published,
        declared_unpublished,
    })
}

fn declared_port(value: i64, side: &str) -> Result<u16> {
    u16::try_from(value)
        .ok()
        .filter(|port| *port != 0)
        .with_context(|| format!("declared {side} port {value} is out of range (1-65535)"))
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

pub(crate) fn resolve_bind_source(source: &str, project_dir: &Path) -> Result<String> {
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

    fn declared(host: Option<i64>, container: i64) -> PortDefault {
        PortDefault { host, container }
    }

    #[test]
    fn composition_rejects_an_out_of_range_declared_container_port() {
        let err = compose_ports(&[declared(None, 70000)], Vec::new(), true).unwrap_err();
        assert!(
            format!("{err:#}").contains("declared container port 70000 is out of range"),
            "got: {err:#}"
        );
    }

    #[test]
    fn composition_rejects_an_out_of_range_declared_host_port() {
        let err = compose_ports(&[declared(Some(0), 3003)], Vec::new(), true).unwrap_err();
        assert!(
            format!("{err:#}").contains("declared host port 0 is out of range"),
            "got: {err:#}"
        );
    }

    #[test]
    fn an_explicit_entry_also_settles_an_unpublished_declared_port() {
        let explicit = vec![lns_ipc::PortPublish {
            host_ip: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            host_port: 4000,
            container_port: 3003,
            protocol: lns_ipc::Protocol::Tcp,
        }];
        let composed = compose_ports(&[declared(None, 3003)], explicit, false).unwrap();
        assert_eq!(composed.published.len(), 1);
        assert!(composed.declared_unpublished.is_empty());
    }
}
