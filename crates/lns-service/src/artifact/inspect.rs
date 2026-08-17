use crate::artifact::{dispatch, resolved_from_sandbox};
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

/// One projection for both kinds, since a mixin's mounts, filesets and env are the same shapes a sandbox's are.
fn declared_view_mounts(spec: &lns_artifact::sandbox::SandboxSpec) -> Vec<SandboxMount> {
    spec.volumes
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
            exclude: volume.exclude().to_vec(),
            optional: volume.optional(),
        })
        .collect()
}

fn declared_view_filesets(
    spec: &lns_artifact::sandbox::SandboxSpec,
) -> Vec<lns_ipc::SandboxFileset> {
    spec.filesets
        .iter()
        .map(|fileset| lns_ipc::SandboxFileset {
            path: fileset.path.clone(),
            inline: fileset.inline.is_some(),
            host_path: fileset.host_path.clone(),
            optional: fileset.optional,
            mount_path: fileset.mount_path.clone(),
            owner: match fileset.owner {
                lns_artifact::sandbox::FilesetOwner::Workload => {
                    lns_ipc::SandboxFilesetOwner::Workload
                }
                lns_artifact::sandbox::FilesetOwner::Root => lns_ipc::SandboxFilesetOwner::Root,
            },
        })
        .collect()
}

fn declared_view_env(spec: &lns_artifact::sandbox::SandboxSpec) -> Vec<String> {
    spec.env
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect()
}

fn declared_policy_flags(policy: &lns_policy::Policy) -> Vec<String> {
    crate::artifact::policy::guardrail_flags(policy)
        .iter()
        .map(|f| f.message().to_string())
        .collect()
}

/// Project an already-peeked manifest into the pre-run inspection: a plain image reports its digest, a published sandbox reports its base image, mounts, filesets, declared connectors, and any over-broad-policy flags.
pub(crate) fn project_inspection(
    image_ref: &str,
    digest: String,
    artifact_type: Option<&str>,
    config_media_type: &str,
    resolution: &crate::artifact::mixin::Resolution,
    host: Option<lns_artifact::resources::HostCapacity>,
) -> Result<ArtifactInspection> {
    match dispatch(artifact_type, Some(config_media_type))? {
        None => Ok(ArtifactInspection::Image(ImageView {
            reference: image_ref.to_string(),
            digest,
        })),
        Some(lns_artifact::spec::Kind::Mixin) => {
            let mixin = lns_artifact::sandbox::parse_mixin(&resolution.document)
                .with_context(|| format!("inspecting mixin {image_ref}"))?;
            Ok(ArtifactInspection::Mixin(Box::new(lns_ipc::MixinView {
                reference: image_ref.to_string(),
                digest,
                mixins: mixin.spec.mixins.clone(),
                mounts: declared_view_mounts(&mixin.spec),
                ports: declared_view_ports(&mixin.spec.ports)?,
                filesets: declared_view_filesets(&mixin.spec),
                env: declared_view_env(&mixin.spec),
                credentials: mixin.spec.credentials.clone(),
                tools: mixin.spec.tools.clone(),
                policy_flags: declared_policy_flags(&lns_policy::Policy {
                    network: lns_policy::NetworkPolicy {
                        egress: mixin.spec.egress.clone(),
                    },
                    ..Default::default()
                }),
            })))
        }
        Some(lns_artifact::spec::Kind::Sandbox) => {
            let def = lns_artifact::sandbox::parse(&resolution.document)
                .with_context(|| format!("inspecting sandbox {image_ref}"))?;
            let resolved = resolved_from_sandbox(
                &def,
                &resolution.fileset_origins,
                crate::artifact::RootSource::Pulled(image_ref),
            );
            let (declared_size, _) = lns_artifact::resources::DeclaredSize::from_resources(
                def.spec.resources.as_ref(),
                host,
            );
            Ok(ArtifactInspection::Sandbox(Box::new(
                lns_ipc::SandboxView {
                    mixins: resolution.mixins.clone(),
                    pinned_mixins: resolution.pinned_extra.clone(),
                    contributions: crate::artifact::mixin::on_the_wire(&resolution.contributions),
                    reference: image_ref.to_string(),
                    digest,
                    cpus: declared_size.cpus,
                    mem_mib: declared_size.mem_mib,
                    image: resolved.base_image,
                    workdir: def.spec.workdir.clone(),
                    user: def.spec.user.clone(),
                    mounts: declared_view_mounts(&def.spec),
                    ports: declared_view_ports(&def.spec.ports)?,
                    filesets: declared_view_filesets(&def.spec),
                    env: declared_view_env(&def.spec),
                    connectors: def.spec.connectors.clone(),
                    credentials: def.spec.credentials.clone(),
                    tools: def.spec.tools.clone(),
                    policy_flags: resolved
                        .policy
                        .as_ref()
                        .map(declared_policy_flags)
                        .unwrap_or_default(),
                },
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lns_ipc::{SandboxFileset, SandboxPort, SandboxView};

    fn digest() -> String {
        format!("sha256:{}", "a".repeat(64))
    }

    const TEST_HOST: lns_artifact::resources::HostCapacity =
        lns_artifact::resources::HostCapacity {
            cpus: 10,
            mem_mib: 16384,
        };

    fn resolution(config: &str, mixins: &[String]) -> crate::artifact::mixin::Resolution {
        crate::artifact::mixin::Resolution {
            document: config.as_bytes().to_vec(),
            mixins: mixins.to_vec(),
            pinned_extra: Vec::new(),
            contributions: Vec::new(),
            authored_egress: Default::default(),
            fileset_origins: Vec::new(),
        }
    }

    fn project_sandbox(config: &str) -> Result<ArtifactInspection> {
        project_sandbox_on(config, None)
    }

    fn project_sandbox_on(
        config: &str,
        host: Option<lns_artifact::resources::HostCapacity>,
    ) -> Result<ArtifactInspection> {
        project_sandbox_resolved_from(config, &[], host)
    }

    fn project_sandbox_resolved_from(
        config: &str,
        mixins: &[String],
        host: Option<lns_artifact::resources::HostCapacity>,
    ) -> Result<ArtifactInspection> {
        let artifact_type = lns_artifact::spec::Kind::Sandbox.artifact_type();
        let config_media_type = lns_artifact::spec::Kind::Sandbox.config_media_type();
        project_inspection(
            "registry.example.test/team/sandbox:latest",
            digest(),
            Some(&artifact_type),
            &config_media_type,
            &resolution(config, mixins),
            host,
        )
    }

    #[test]
    fn a_plain_image_projects_to_an_image_view_carrying_its_digest() {
        let inspection = project_inspection(
            "registry.example.test/team/app:latest",
            digest(),
            None,
            "application/vnd.oci.image.config.v1+json",
            &resolution("{}", &[]),
            None,
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
            &resolution("{}", &[]),
            None,
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
        sandbox_view_with(cpus, mem_mib, user, Vec::new())
    }

    fn sandbox_view_with(
        cpus: Option<u8>,
        mem_mib: Option<usize>,
        user: Option<&str>,
        mounts: Vec<SandboxMount>,
    ) -> ArtifactInspection {
        ArtifactInspection::Sandbox(Box::new(SandboxView {
            mixins: Vec::new(),
            pinned_mixins: Vec::new(),
            contributions: Vec::new(),
            reference: "registry.example.test/team/sandbox:latest".into(),
            digest: digest(),
            image: "registry.example.test/runtime:1".into(),
            workdir: None,
            user: user.map(str::to_string),
            mounts,
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

    fn sandbox_view_with_mixins(mixins: Vec<String>, pinned: Vec<String>) -> ArtifactInspection {
        ArtifactInspection::Sandbox(Box::new(SandboxView {
            mixins,
            pinned_mixins: pinned,
            contributions: Vec::new(),
            reference: "registry.example.test/team/sandbox:latest".into(),
            digest: digest(),
            image: "registry.example.test/runtime:1".into(),
            workdir: None,
            user: None,
            mounts: Vec::new(),
            ports: Vec::new(),
            filesets: Vec::new(),
            connectors: Vec::new(),
            env: Vec::new(),
            credentials: Vec::new(),
            tools: Vec::new(),
            policy_flags: Vec::new(),
            cpus: None,
            mem_mib: None,
        }))
    }

    fn sandbox_view_with_filesets(filesets: Vec<SandboxFileset>) -> ArtifactInspection {
        ArtifactInspection::Sandbox(Box::new(SandboxView {
            mixins: Vec::new(),
            pinned_mixins: Vec::new(),
            contributions: Vec::new(),
            reference: "registry.example.test/team/sandbox:latest".into(),
            digest: digest(),
            image: "registry.example.test/runtime:1".into(),
            workdir: None,
            user: None,
            mounts: Vec::new(),
            ports: Vec::new(),
            filesets,
            connectors: Vec::new(),
            env: Vec::new(),
            credentials: Vec::new(),
            tools: Vec::new(),
            policy_flags: Vec::new(),
            cpus: None,
            mem_mib: None,
        }))
    }

    #[test]
    fn a_published_mixin_projects_as_the_document_its_author_wrote() {
        let pinned = format!("ghcr.io/acme/base@sha256:{}", "a".repeat(64));
        let document = format!(
            r#"{{"apiVersion":"lns.run/v1","kind":"mixin","name":"obs-tools","spec":{{"mixins":["{pinned}"],"tools":["node@22"],"env":{{"MODE":"research"}}}}}}"#
        );
        let projected = project_inspection(
            "ghcr.io/acme/obs-tools:2",
            digest(),
            Some(&lns_artifact::spec::Kind::Mixin.artifact_type()),
            &lns_artifact::spec::Kind::Mixin.config_media_type(),
            &resolution(&document, &[]),
            None,
        )
        .unwrap();
        assert_eq!(
            projected,
            ArtifactInspection::Mixin(Box::new(lns_ipc::MixinView {
                reference: "ghcr.io/acme/obs-tools:2".into(),
                digest: digest(),
                mixins: vec![pinned],
                mounts: Vec::new(),
                ports: Vec::new(),
                filesets: Vec::new(),
                env: vec!["MODE=research".into()],
                credentials: Vec::new(),
                tools: vec!["node@22".into()],
                policy_flags: Vec::new(),
            })),
            "a published mixin is shown unresolved, so what its own graph merges to stays a launch-time answer"
        );
    }

    #[test]
    fn a_mixin_artifact_carrying_a_launch_block_is_refused_at_inspect() {
        let err = project_inspection(
            "ghcr.io/acme/obs-tools:2",
            digest(),
            Some(&lns_artifact::spec::Kind::Mixin.artifact_type()),
            &lns_artifact::spec::Kind::Mixin.config_media_type(),
            &resolution(
                r#"{"apiVersion":"lns.run/v1","kind":"mixin","name":"obs","spec":{"image":"x:1"}}"#,
                &[],
            ),
            None,
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("a mixin must not declare image"),
            "the artifact type says mixin, so the document has to hold to a mixin's rules before a reader trusts it; got: {err:#}"
        );
    }

    #[test]
    fn a_view_separates_what_the_user_named_from_everything_the_merge_reached() {
        let pinned = format!("ghcr.io/acme/obs@sha256:{}", "c".repeat(64));
        let declared = format!("ghcr.io/acme/base@sha256:{}", "a".repeat(64));
        assert_eq!(
            project_inspection(
                "registry.example.test/team/sandbox:latest",
                digest(),
                Some(&lns_artifact::spec::Kind::Sandbox.artifact_type()),
                &lns_artifact::spec::Kind::Sandbox.config_media_type(),
                &crate::artifact::mixin::Resolution {
                    pinned_extra: vec![pinned.clone()],
                    ..resolution(
                        r#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"some-sandbox","spec":{"image":"registry.example.test/runtime:1"}}"#,
                        &[declared.clone(), pinned.clone()],
                    )
                },
                None,
            )
            .unwrap(),
            sandbox_view_with_mixins(vec![declared, pinned.clone()], vec![pinned]),
            "only the references the user named can be shown beside the tag they typed, so the two lists cannot be one"
        );
    }

    #[test]
    fn a_sandbox_projects_the_mixins_it_resolved_into_so_inspect_and_run_answer_alike() {
        let pinned = format!("ghcr.io/acme/postgres-tools@sha256:{}", "c".repeat(64));
        assert_eq!(
            project_sandbox_resolved_from(
                r#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"some-sandbox","spec":{"image":"registry.example.test/runtime:1"}}"#,
                std::slice::from_ref(&pinned),
                None,
            )
            .unwrap(),
            sandbox_view_with_mixins(vec![pinned], Vec::new()),
            "the resolved document declares no mixins of its own, so only the list travelling beside it can tell a reader what this sandbox layers on"
        );
    }

    #[test]
    fn a_sandbox_projects_the_run_as_user_it_declared_so_a_pull_can_disclose_it() {
        assert_eq!(
            project_sandbox(
                r#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"some-sandbox","spec":{"image":"registry.example.test/runtime:1","user":"root"}}"#
            )
            .unwrap(),
            bare_sandbox_view(None, None, Some("root")),
            "without this a pulled artifact asking for root is invisible before it boots"
        );
    }

    #[test]
    fn a_sandbox_declaring_a_share_projects_that_share_of_the_given_host() {
        assert_eq!(
            project_sandbox_on(
                r#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"some-sandbox","spec":{"image":"registry.example.test/runtime:1","resources":{"cpu":"80%","memory":"80%"}}}"#,
                Some(TEST_HOST)
            )
            .unwrap(),
            bare_sandbox_view(Some(8), Some(13107), None),
            "a pulled sandbox sized in percent has to report the share it will actually boot with"
        );
    }

    #[test]
    fn a_sandbox_projects_the_size_it_declared_so_a_pulled_run_can_report_it() {
        assert_eq!(
            project_sandbox(
                r#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"some-sandbox","spec":{"image":"registry.example.test/runtime:1","resources":{"cpu":3,"memory":"6Gi"}}}"#
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
                r#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"some-sandbox","spec":{"image":"registry.example.test/runtime:1"}}"#
            )
            .unwrap(),
            bare_sandbox_view(None, None, None),
            "None must stay distinguishable from a declared default, or a flag can no longer outrank a declaration"
        );
    }

    #[test]
    fn a_sandbox_projects_the_subpaths_its_bind_excluded() {
        assert_eq!(
            project_sandbox(
                r#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"some-sandbox","spec":{"image":"registry.example.test/runtime:1","volumes":[{"type":"bind","source":".","target":"/workspace","exclude":[".cargo","tmp/scratch"]}]}}"#
            )
            .unwrap(),
            sandbox_view_with(
                None,
                None,
                None,
                vec![SandboxMount {
                    kind: SandboxMountKind::Bind,
                    source: ".".into(),
                    target: "/workspace".into(),
                    read_only: false,
                    exclude: vec![".cargo".into(), "tmp/scratch".into()],
                    optional: false,
                }]
            ),
            "a pulled sandbox whose exclusions are dropped here publishes masks that never apply"
        );
    }

    #[test]
    fn a_sandbox_discloses_a_host_file_source_and_whether_it_is_optional() {
        assert_eq!(
            project_sandbox(
                r#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"some-sandbox","spec":{"image":"registry.example.test/runtime:1","filesets":[{"hostPath":"~/.gitconfig","mountPath":"/home/agent/.gitconfig","optional":true}]}}"#
            )
            .unwrap(),
            sandbox_view_with_filesets(vec![SandboxFileset {
                path: None,
                inline: false,
                host_path: Some("~/.gitconfig".into()),
                mount_path: "/home/agent/.gitconfig".into(),
                owner: lns_ipc::SandboxFilesetOwner::Workload,
                optional: true,
            }]),
            "a pulled sandbox that reads a file off the consumer's machine must say which file, before it boots"
        );
    }

    #[test]
    fn a_sandbox_projects_whether_its_bind_is_optional() {
        assert_eq!(
            project_sandbox(
                r#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"some-sandbox","spec":{"image":"registry.example.test/runtime:1","volumes":[{"type":"bind","source":"~/.claude","target":"/home/agent/.claude","optional":true}]}}"#
            )
            .unwrap(),
            sandbox_view_with(
                None,
                None,
                None,
                vec![SandboxMount {
                    kind: SandboxMountKind::Bind,
                    source: "~/.claude".into(),
                    target: "/home/agent/.claude".into(),
                    read_only: false,
                    exclude: Vec::new(),
                    optional: true,
                }]
            ),
            "dropping this makes a pulled optional bind required again, so a consumer without the directory could not run the sandbox at all"
        );
    }

    #[test]
    fn a_sandbox_projects_its_volumes_ports_filesets_and_over_broad_policy_flag() {
        let config = r#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"some-sandbox","spec":{"image":"registry.example.test/runtime:1","workdir":"/work","volumes":[{"type":"bind","source":".","target":"/workspace"},{"type":"volume","name":"cache","target":"/root/.cache","readOnly":true}],"ports":[{"container":8080},{"host":9090,"container":3000}],"filesets":[{"path":"./skills","mountPath":"/root/.agent/skills"},{"inline":{"settings.json":"do-not-print"},"mountPath":"/etc/agent","owner":"root"}],"egress":{"http":[{"match":"*","verdict":"allow"}]}}}"#;

        let inspection = project_sandbox(config).unwrap();

        assert_eq!(
            inspection,
            ArtifactInspection::Sandbox(Box::new(SandboxView {
                mixins: Vec::new(),
                pinned_mixins: Vec::new(),
                contributions: Vec::new(),
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
                        exclude: Vec::new(),
                        optional: false,
                    },
                    SandboxMount {
                        kind: SandboxMountKind::Volume,
                        source: "cache".into(),
                        target: "/root/.cache".into(),
                        read_only: true,
                        exclude: Vec::new(),
                        optional: false,
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
                        path: Some("./skills".into()),
                        inline: false,
                        host_path: None,
                        optional: false,
                        mount_path: "/root/.agent/skills".into(),
                        owner: lns_ipc::SandboxFilesetOwner::Workload,
                    },
                    SandboxFileset {
                        path: None,
                        inline: true,
                        host_path: None,
                        optional: false,
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
        let config = r#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"some-sandbox","spec":{"image":"registry.example.test/runtime:1","credentials":[{"envVar":"SOME_TOKEN","placeholder":"lns-placeholder-some-token","injections":[{"kind":"bearer_header","domain":"api.some-provider.example"}]}]}}"#;

        let inspection = project_sandbox(config).unwrap();

        assert_eq!(
            inspection,
            ArtifactInspection::Sandbox(Box::new(SandboxView {
                mixins: Vec::new(),
                pinned_mixins: Vec::new(),
                contributions: Vec::new(),
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
                credentials: vec![lns_spec::Credential {
                    env_var: "SOME_TOKEN".into(),
                    placeholder: "lns-placeholder-some-token".into(),
                    injections: vec![lns_spec::InjectionDef {
                        kind: lns_spec::InjectionKind::BearerHeader,
                        domain: "api.some-provider.example".into(),
                        header: None,
                    }],
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
        let config = r#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"some-sandbox","spec":{"image":"registry.example.test/runtime:1","tools":["node@22.11.0","python@3.12.6"]}}"#;

        let inspection = project_sandbox(config).unwrap();

        assert_eq!(
            inspection,
            ArtifactInspection::Sandbox(Box::new(SandboxView {
                mixins: Vec::new(),
                pinned_mixins: Vec::new(),
                contributions: Vec::new(),
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
        let config = r#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"some-sandbox","spec":{"image":"registry.example.test/runtime:1","env":{"SHELL":"/bin/sh","FOO":"bar"}}}"#;

        let inspection = project_sandbox(config).unwrap();

        assert_eq!(
            inspection,
            ArtifactInspection::Sandbox(Box::new(SandboxView {
                mixins: Vec::new(),
                pinned_mixins: Vec::new(),
                contributions: Vec::new(),
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
