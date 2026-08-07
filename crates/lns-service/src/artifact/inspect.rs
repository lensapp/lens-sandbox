use crate::artifact::{RunPath, dispatch, resolved_from_sandbox};
use anyhow::{Context, Result};
use lns_ipc::{ArtifactInspection, ImageView, SandboxMount, SandboxMountKind};

fn declared_view_ports(ports: &[lns_artifact::spec::Port]) -> Result<Vec<lns_ipc::SandboxPort>> {
    ports
        .iter()
        .map(|port| {
            Ok(lns_ipc::SandboxPort {
                host: port
                    .host
                    .map(u16::try_from)
                    .transpose()
                    .with_context(|| format!("declared host port {:?} out of range", port.host))?,
                container: u16::try_from(port.container).with_context(|| {
                    format!("declared container port {} out of range", port.container)
                })?,
            })
        })
        .collect()
}

/// Project an already-peeked manifest into the pre-run inspection: a plain image reports its digest, a published sandbox reports its base image, mounts, filesets, declared connectors, and any over-broad-policy flags.
pub(crate) fn project_inspection(
    image_ref: &str,
    digest: String,
    artifact_type: Option<&str>,
    config_media_type: &str,
    config_json: &str,
) -> Result<ArtifactInspection> {
    match dispatch(artifact_type, Some(config_media_type))? {
        RunPath::SingleImage => Ok(ArtifactInspection::Image(ImageView {
            reference: image_ref.to_string(),
            digest,
        })),
        RunPath::Sandbox => {
            let def = lns_artifact::sandbox::parse(config_json.as_bytes())
                .with_context(|| format!("inspecting sandbox {image_ref}"))?;
            let resolved = resolved_from_sandbox(&def);
            let (declared_size, _) =
                lns_artifact::resources::DeclaredSize::from_resources(def.spec.resources.as_ref());
            Ok(ArtifactInspection::Sandbox(Box::new(
                lns_ipc::SandboxView {
                    reference: image_ref.to_string(),
                    digest,
                    cpus: declared_size.cpus,
                    mem_mib: declared_size.mem_mib,
                    image: resolved.base_image,
                    workdir: def.spec.workdir.clone(),
                    user: def.spec.user.clone(),
                    mounts: def
                        .spec
                        .volumes
                        .iter()
                        .map(|volume| SandboxMount {
                            kind: if volume.is_bind() {
                                SandboxMountKind::Bind
                            } else {
                                SandboxMountKind::Volume
                            },
                            source: volume.source().to_string(),
                            target: volume.target.clone(),
                            read_only: volume.read_only(),
                        })
                        .collect(),
                    ports: declared_view_ports(&def.spec.ports)?,
                    filesets: def
                        .spec
                        .filesets
                        .iter()
                        .map(|fileset| lns_ipc::SandboxFileset {
                            path: fileset.path.clone(),
                            reference: fileset.reference.clone(),
                            inline: fileset.inline.is_some(),
                            mount_path: fileset.mount_path.clone(),
                            owner: match fileset.owner {
                                lns_artifact::sandbox::FilesetOwner::Workload => {
                                    lns_ipc::SandboxFilesetOwner::Workload
                                }
                                lns_artifact::sandbox::FilesetOwner::Root => {
                                    lns_ipc::SandboxFilesetOwner::Root
                                }
                            },
                        })
                        .collect(),
                    connectors: def.spec.connectors,
                    env: def
                        .spec
                        .env
                        .iter()
                        .map(|(key, value)| format!("{key}={value}"))
                        .collect(),
                    credentials: def
                        .spec
                        .credentials
                        .into_iter()
                        .map(|credential| lns_ipc::SandboxCredential {
                            name: credential.name,
                            env: credential.env,
                            required: credential.required,
                        })
                        .collect(),
                    tools: def.spec.tools,
                    policy_flags: resolved
                        .policy
                        .as_ref()
                        .map(|p| {
                            crate::artifact::policy::guardrail_flags(p)
                                .iter()
                                .map(|f| f.message().to_string())
                                .collect()
                        })
                        .unwrap_or_default(),
                },
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lns_ipc::{SandboxCredential, SandboxFileset, SandboxPort, SandboxView};

    fn digest() -> String {
        format!("sha256:{}", "a".repeat(64))
    }

    fn project_sandbox(config: &str) -> Result<ArtifactInspection> {
        let artifact_type = lns_artifact::spec::Kind::Sandbox.artifact_type();
        let config_media_type = lns_artifact::spec::Kind::Sandbox.config_media_type();
        project_inspection(
            "registry.example.test/team/sandbox:latest",
            digest(),
            Some(&artifact_type),
            &config_media_type,
            config,
        )
    }

    #[test]
    fn a_plain_image_projects_to_an_image_view_carrying_its_digest() {
        let inspection = project_inspection(
            "registry.example.test/team/app:latest",
            digest(),
            None,
            "application/vnd.oci.image.config.v1+json",
            "{}",
        )
        .unwrap();

        assert_eq!(
            inspection,
            ArtifactInspection::Image(ImageView {
                reference: "registry.example.test/team/app:latest".into(),
                digest: digest(),
            })
        );
    }

    #[test]
    fn an_unknown_artifact_type_is_refused() {
        let err = project_inspection(
            "registry.example.test/team/app:latest",
            digest(),
            Some("application/vnd.acme.thing"),
            "application/vnd.oci.image.config.v1+json",
            "{}",
        )
        .unwrap_err();

        assert!(
            format!("{err:#}").contains("unsupported artifact type"),
            "got: {err:#}"
        );
    }

    #[test]
    fn a_corrupt_published_sandbox_config_surfaces_a_parse_error() {
        let err = project_sandbox("not json").unwrap_err();
        assert!(
            format!("{err:#}").contains("inspecting sandbox"),
            "got: {err:#}"
        );
    }

    /// The projection of a bare sandbox that declares nothing but an image, so a test can vary one field and compare whole values.
    fn bare_sandbox_view(
        cpus: Option<u8>,
        mem_mib: Option<usize>,
        user: Option<&str>,
    ) -> ArtifactInspection {
        ArtifactInspection::Sandbox(Box::new(SandboxView {
            reference: "registry.example.test/team/sandbox:latest".into(),
            digest: digest(),
            image: "registry.example.test/runtime:1".into(),
            workdir: None,
            user: user.map(str::to_string),
            mounts: Vec::new(),
            ports: Vec::new(),
            filesets: Vec::new(),
            connectors: Vec::new(),
            env: Vec::new(),
            credentials: Vec::new(),
            tools: Vec::new(),
            policy_flags: Vec::new(),
            cpus,
            mem_mib,
        }))
    }

    #[test]
    fn a_sandbox_projects_the_run_as_user_it_declared_so_a_pull_can_disclose_it() {
        assert_eq!(
            project_sandbox(
                r#"{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{"name":"some-sandbox"},"spec":{"image":"registry.example.test/runtime:1","user":"root"}}"#
            )
            .unwrap(),
            bare_sandbox_view(None, None, Some("root")),
            "without this a pulled artifact asking for root is invisible before it boots"
        );
    }

    #[test]
    fn a_sandbox_projects_the_size_it_declared_so_a_pulled_run_can_report_it() {
        assert_eq!(
            project_sandbox(
                r#"{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{"name":"some-sandbox"},"spec":{"image":"registry.example.test/runtime:1","resources":{"cpu":3,"memory":"6Gi"}}}"#
            )
            .unwrap(),
            bare_sandbox_view(Some(3), Some(6144), None),
            "without this the summary of a pulled run falls back to the default size"
        );
    }

    #[test]
    fn a_sandbox_that_declares_no_size_projects_none_not_the_default() {
        assert_eq!(
            project_sandbox(
                r#"{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{"name":"some-sandbox"},"spec":{"image":"registry.example.test/runtime:1"}}"#
            )
            .unwrap(),
            bare_sandbox_view(None, None, None),
            "None must stay distinguishable from a declared default, or a flag can no longer outrank a declaration"
        );
    }

    #[test]
    fn a_sandbox_projects_its_volumes_ports_filesets_and_over_broad_policy_flag() {
        let config = r#"{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{"name":"some-sandbox"},"spec":{"image":"registry.example.test/runtime:1","workdir":"/work","volumes":[{"type":"bind","source":".","target":"/workspace"},{"type":"volume","name":"cache","target":"/root/.cache","readOnly":true}],"ports":[{"container":8080},{"host":9090,"container":3000}],"filesets":[{"ref":"registry.example.test/team/skills@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","mountPath":"/root/.agent/skills"},{"inline":{"settings.json":"do-not-print"},"mountPath":"/etc/agent","owner":"root"}],"policy":{"egress":{"http":[{"match":"*","verdict":"allow"}]}}}}"#;

        let inspection = project_sandbox(config).unwrap();

        assert_eq!(
            inspection,
            ArtifactInspection::Sandbox(Box::new(SandboxView {
                reference: "registry.example.test/team/sandbox:latest".into(),
                digest: digest(),
                image: "registry.example.test/runtime:1".into(),
                workdir: Some("/work".into()),
                user: None,
                mounts: vec![
                    SandboxMount {
                        kind: SandboxMountKind::Bind,
                        source: ".".into(),
                        target: "/workspace".into(),
                        read_only: false,
                    },
                    SandboxMount {
                        kind: SandboxMountKind::Volume,
                        source: "cache".into(),
                        target: "/root/.cache".into(),
                        read_only: true,
                    },
                ],
                ports: vec![
                    SandboxPort {
                        host: None,
                        container: 8080,
                    },
                    SandboxPort {
                        host: Some(9090),
                        container: 3000,
                    },
                ],
                filesets: vec![
                    SandboxFileset {
                        path: None,
                        reference: Some(
                            "registry.example.test/team/skills@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
                                .into()
                        ),
                        inline: false,
                        mount_path: "/root/.agent/skills".into(),
                        owner: lns_ipc::SandboxFilesetOwner::Workload,
                    },
                    SandboxFileset {
                        path: None,
                        reference: None,
                        inline: true,
                        mount_path: "/etc/agent".into(),
                        owner: lns_ipc::SandboxFilesetOwner::Root,
                    },
                ],
                connectors: vec![],
                env: vec![],
                credentials: vec![],
                tools: vec![],
                policy_flags: vec![
                    "wildcard allow — a catch-all or whole-suffix host pattern is permitted".into()
                ],
                cpus: None,
                mem_mib: None,
            }))
        );
    }

    #[test]
    fn a_sandbox_projects_declared_credentials_and_no_flags_without_a_policy() {
        let config = r#"{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{"name":"some-sandbox"},"spec":{"image":"registry.example.test/runtime:1","credentials":[{"name":"some-provider","env":"SOME_TOKEN","required":true}]}}"#;

        let inspection = project_sandbox(config).unwrap();

        assert_eq!(
            inspection,
            ArtifactInspection::Sandbox(Box::new(SandboxView {
                reference: "registry.example.test/team/sandbox:latest".into(),
                digest: digest(),
                image: "registry.example.test/runtime:1".into(),
                workdir: None,
                user: None,
                mounts: vec![],
                ports: vec![],
                filesets: vec![],
                connectors: vec![],
                env: vec![],
                credentials: vec![SandboxCredential {
                    name: "some-provider".into(),
                    env: "SOME_TOKEN".into(),
                    required: true,
                }],
                tools: vec![],
                policy_flags: vec![],
                cpus: None,
                mem_mib: None,
            }))
        );
    }

    #[test]
    fn a_sandbox_projects_its_declared_tools() {
        let config = r#"{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{"name":"some-sandbox"},"spec":{"image":"registry.example.test/runtime:1","tools":["node@22.11.0","python@3.12.6"]}}"#;

        let inspection = project_sandbox(config).unwrap();

        assert_eq!(
            inspection,
            ArtifactInspection::Sandbox(Box::new(SandboxView {
                reference: "registry.example.test/team/sandbox:latest".into(),
                digest: digest(),
                image: "registry.example.test/runtime:1".into(),
                workdir: None,
                user: None,
                mounts: vec![],
                ports: vec![],
                filesets: vec![],
                connectors: vec![],
                env: vec![],
                credentials: vec![],
                tools: vec!["node@22.11.0".into(), "python@3.12.6".into()],
                policy_flags: vec![],
                cpus: None,
                mem_mib: None,
            }))
        );
    }

    #[test]
    fn a_sandbox_projects_env_sorted_by_key() {
        let config = r#"{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{"name":"some-sandbox"},"spec":{"image":"registry.example.test/runtime:1","env":{"SHELL":"/bin/sh","FOO":"bar"}}}"#;

        let inspection = project_sandbox(config).unwrap();

        assert_eq!(
            inspection,
            ArtifactInspection::Sandbox(Box::new(SandboxView {
                reference: "registry.example.test/team/sandbox:latest".into(),
                digest: digest(),
                image: "registry.example.test/runtime:1".into(),
                workdir: None,
                user: None,
                mounts: vec![],
                ports: vec![],
                filesets: vec![],
                connectors: vec![],
                env: vec!["FOO=bar".into(), "SHELL=/bin/sh".into()],
                credentials: vec![],
                tools: vec![],
                policy_flags: vec![],
                cpus: None,
                mem_mib: None,
            }))
        );
    }

    #[test]
    fn a_declared_container_port_out_of_u16_range_is_refused() {
        let ports = [lns_artifact::spec::Port {
            host: None,
            container: 70_000,
        }];
        let err = declared_view_ports(&ports).unwrap_err();
        assert!(
            format!("{err:#}").contains("declared container port 70000 out of range"),
            "got: {err:#}"
        );
    }

    #[test]
    fn a_declared_host_port_out_of_u16_range_is_refused() {
        let ports = [lns_artifact::spec::Port {
            host: Some(70_000),
            container: 3000,
        }];
        let err = declared_view_ports(&ports).unwrap_err();
        assert!(
            format!("{err:#}").contains("declared host port"),
            "got: {err:#}"
        );
    }
}
