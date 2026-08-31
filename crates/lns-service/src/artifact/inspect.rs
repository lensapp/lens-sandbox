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
            size_bytes: volume.size_bytes(),
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
            guest_path: fileset.guest_path.clone(),
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

/// A script travels whole, because a consumer approving one has to be able to read it (§1.5).
fn declared_view_scripts(spec: &lns_artifact::sandbox::SandboxSpec) -> Vec<lns_ipc::SandboxScript> {
    spec.scripts
        .iter()
        .map(|script| lns_ipc::SandboxScript {
            when: script.when.as_str().to_string(),
            run: script.run.clone(),
            user: script.user.clone(),
            description: script.description.clone(),
        })
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
        Some(lns_artifact::spec::Kind::Connector) => anyhow::bail!(
            "{image_ref} is a connector; a connector is installed on this machine rather than inspected as a project's artifact"
        ),
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
                scripts: declared_view_scripts(&mixin.spec),
                policy_flags: declared_policy_flags(&lns_policy::Policy {
                    network: lns_policy::NetworkPolicy {
                        egress: mixin.spec.egress.clone(),
                    },
                    ..Default::default()
                }),
            })))
        }
        Some(lns_artifact::spec::Kind::Sandbox) => {
            let def = lns_artifact::sandbox::parse_resolved(&resolution.document)
                .with_context(|| format!("inspecting sandbox {image_ref}"))?;
            let resolved = resolved_from_sandbox(&def, &crate::artifact::PackedFilesets::new());
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
                    disk_bytes: declared_size.disk_bytes,
                    image: resolved.base_image,
                    workdir: def.spec.workdir.clone(),
                    user: def.spec.user.clone(),
                    mounts: declared_view_mounts(&def.spec),
                    ports: declared_view_ports(&def.spec.ports)?,
                    filesets: declared_view_filesets(&def.spec),
                    credentials: def.spec.credentials.clone(),
                    env: declared_view_env(&def.spec),
                    tools: def.spec.tools.clone(),
                    scripts: declared_view_scripts(&def.spec),
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
            declared_path_filesets: std::collections::BTreeMap::new(),
            carriers: std::collections::BTreeMap::new(),
        }
    }

    #[test]
    fn a_sandbox_reports_every_script_it_declares_with_the_user_it_asks_for() {
        let spec: lns_artifact::sandbox::SandboxSpec = serde_json::from_str(
            r#"{"image":"registry.example.test/runtime:1","scripts":[{"when":"pre-start","user":"root","run":"apt-get install -y psql","description":"the psql the prompts assume"},{"when":"pre-start","run":"npm ci"}]}"#,
        )
        .expect("a valid fixture");
        assert_eq!(
            declared_view_scripts(&spec),
            vec![
                lns_ipc::SandboxScript {
                    when: "pre-start".into(),
                    run: "apt-get install -y psql".into(),
                    user: Some("root".into()),
                    description: Some("the psql the prompts assume".into()),
                },
                lns_ipc::SandboxScript {
                    when: "pre-start".into(),
                    run: "npm ci".into(),
                    user: None,
                    description: None,
                },
            ],
            "a consumer approving a script has to be able to read it, so the body travels whole and an absent user stays absent"
        );
    }

    #[test]
    fn the_preflight_projects_every_script_the_merge_appended() {
        let entries: Vec<String> = (0..=lns_artifact::sandbox::MAX_SCRIPT_STEPS)
            .map(|n| format!(r#"{{"when":"pre-start","run":"echo {n}"}}"#))
            .collect();
        let config = format!(
            r#"{{"apiVersion":"lns.run/v1","kind":"sandbox","name":"some-sandbox","spec":{{"image":"registry.example.test/runtime:1","scripts":[{}]}}}}"#,
            entries.join(",")
        );
        let projected = serde_json::to_value(project_sandbox(&config).expect(
            "the preflight reads the resolved document, so it must not hold it to an authoring ceiling",
        ))
        .expect("an inspection encodes");
        assert_eq!(
            projected["scripts"].as_array().map(Vec::len),
            Some(lns_artifact::sandbox::MAX_SCRIPT_STEPS + 1),
            "the disclosure is what the consumer approves, so it has to list every script the run will actually execute"
        );
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

    /// The same projection with a declared disk, so a test can compare whole values rather than reach into the enum.
    fn sandbox_view_with_disk(disk_bytes: u64) -> ArtifactInspection {
        sandbox_view_of(None, None, None, Vec::new(), Some(disk_bytes))
    }

    fn sandbox_view_with(
        cpus: Option<u8>,
        mem_mib: Option<usize>,
        user: Option<&str>,
        mounts: Vec<SandboxMount>,
    ) -> ArtifactInspection {
        sandbox_view_of(cpus, mem_mib, user, mounts, None)
    }

    fn sandbox_view_of(
        cpus: Option<u8>,
        mem_mib: Option<usize>,
        user: Option<&str>,
        mounts: Vec<SandboxMount>,
        disk_bytes: Option<u64>,
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
            credentials: Vec::new(),
            env: Vec::new(),
            tools: Vec::new(),
            scripts: Vec::new(),
            policy_flags: Vec::new(),
            cpus,
            mem_mib,
            disk_bytes,
        }))
    }

    /// The bare projection with declared credentials, so a test compares a whole value rather than reaching into the enum.
    fn sandbox_view_with_credentials(credentials: Vec<lns_spec::Credential>) -> ArtifactInspection {
        ArtifactInspection::Sandbox(Box::new(SandboxView {
            credentials,
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
            filesets: Vec::new(),
            env: Vec::new(),
            tools: Vec::new(),
            scripts: Vec::new(),
            policy_flags: Vec::new(),
            cpus: None,
            mem_mib: None,
            disk_bytes: None,
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
            credentials: Vec::new(),
            env: Vec::new(),
            tools: Vec::new(),
            scripts: Vec::new(),
            policy_flags: Vec::new(),
            cpus: None,
            mem_mib: None,
            disk_bytes: None,
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
            credentials: Vec::new(),
            env: Vec::new(),
            tools: Vec::new(),
            scripts: Vec::new(),
            policy_flags: Vec::new(),
            cpus: None,
            mem_mib: None,
            disk_bytes: None,
        }))
    }

    #[test]
    fn a_projected_sandbox_carries_the_credentials_it_declared() {
        // §1.5: the disclosure is of the resolved sandbox, so a secret path the document wrote down has to reach the reader — with the domains, which are what they approve.
        assert_eq!(
            project_inspection(
                "registry.example.test/team/sandbox:latest",
                digest(),
                Some(&lns_artifact::spec::Kind::Sandbox.artifact_type()),
                &lns_artifact::spec::Kind::Sandbox.config_media_type(),
                &resolution(
                    r#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"some-sandbox","spec":{"image":"registry.example.test/runtime:1","credentials":[{"envVar":"SOME_TOKEN","placeholder":"some_LNSPLACEHOLDER0000000000","injections":[{"kind":"bearer_header","domain":"api.some-provider.example"}]}]}}"#,
                    &[],
                ),
                None,
            )
            .unwrap(),
            sandbox_view_with_credentials(vec![lns_spec::Credential {
                env_var: Some("SOME_TOKEN".into()),
                placeholder: "some_LNSPLACEHOLDER0000000000".into(),
                field: None,
                injections: vec![lns_spec::InjectionDef {
                    kind: lns_spec::InjectionKind::BearerHeader,
                    domain: "api.some-provider.example".into(),
                    header: None,
                }],
            }])
        );
    }

    #[test]
    fn a_published_mixin_projects_as_the_document_its_author_wrote() {
        let pinned = format!("ghcr.io/acme/base@sha256:{}", "a".repeat(64));
        let document = format!(
            r#"{{"apiVersion":"lns.run/v1","kind":"mixin","name":"obs-tools","spec":{{"mixins":["{pinned}"],"tools":["node@22"],"env":{{"MODE":"research"}},"credentials":[{{"envVar":"SOME_TOKEN","placeholder":"some_LNSPLACEHOLDER0000000000","injections":[{{"kind":"bearer_header","domain":"api.some-provider.example"}}]}}]}}}}"#
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
                // A mixin's own credentials merge into the sandbox, so an inspect of the mixin alone has to carry them (§3.1.7).
                credentials: vec![lns_spec::Credential {
                    env_var: Some("SOME_TOKEN".into()),
                    placeholder: "some_LNSPLACEHOLDER0000000000".into(),
                    field: None,
                    injections: vec![lns_spec::InjectionDef {
                        kind: lns_spec::InjectionKind::BearerHeader,
                        domain: "api.some-provider.example".into(),
                        header: None,
                    }],
                }],
                env: vec!["MODE=research".into()],
                tools: vec!["node@22".into()],
                scripts: Vec::new(),
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
    fn a_connector_artifact_is_not_a_projects_artifact_to_inspect() {
        let err = project_inspection(
            "ghcr.io/acme/some-provider:1",
            digest(),
            Some(&lns_artifact::spec::Kind::Connector.artifact_type()),
            &lns_artifact::spec::Kind::Connector.config_media_type(),
            &resolution(
                r#"{"apiVersion":"lns.run/v1","kind":"connector","name":"some-provider","spec":{}}"#,
                &[],
            ),
            None,
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("is a connector"),
            "a connector is installed on this machine rather than resolved into a project, so inspecting one as a project's artifact has no answer; got: {err:#}"
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
    fn a_sandbox_projects_the_disk_it_declared_so_the_disclosure_can_name_it() {
        assert_eq!(
            project_sandbox(
                r#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"some-sandbox","spec":{"image":"registry.example.test/runtime:1","resources":{"disk":"40Gi"}}}"#
            )
            .unwrap(),
            sandbox_view_with_disk(40 << 30),
            "without this a pulled run discloses the default disk while booting the declared one"
        );
    }

    #[test]
    fn a_sandbox_projects_the_size_its_volume_declared_so_a_pulled_run_provisions_it() {
        assert_eq!(
            project_sandbox(
                r#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"some-sandbox","spec":{"image":"registry.example.test/runtime:1","volumes":[{"name":"cache","target":"/cache","size":"40Gi"}]}}"#
            )
            .unwrap(),
            sandbox_view_with(
                None,
                None,
                None,
                vec![SandboxMount {
                    kind: SandboxMountKind::Volume,
                    source: "cache".into(),
                    target: "/cache".into(),
                    read_only: false,
                    exclude: Vec::new(),
                    optional: false,
                    size_bytes: Some(40 << 30),
                }]
            ),
            "without this a pulled sandbox's volume is created at the default, not the size its author declared"
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
                    size_bytes: None,
                }]
            ),
            "a pulled sandbox whose exclusions are dropped here publishes masks that never apply"
        );
    }

    #[test]
    fn a_sandbox_discloses_a_host_file_source_and_whether_it_is_optional() {
        assert_eq!(
            project_sandbox(
                r#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"some-sandbox","spec":{"image":"registry.example.test/runtime:1","filesets":[{"hostPath":"~/.gitconfig","guestPath":"/home/agent/.gitconfig","optional":true}]}}"#
            )
            .unwrap(),
            sandbox_view_with_filesets(vec![SandboxFileset {
                path: None,
                inline: false,
                host_path: Some("~/.gitconfig".into()),
                guest_path: "/home/agent/.gitconfig".into(),
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
                    size_bytes: None,
                }]
            ),
            "dropping this makes a pulled optional bind required again, so a consumer without the directory could not run the sandbox at all"
        );
    }

    #[test]
    fn a_sandbox_projects_its_volumes_ports_filesets_and_over_broad_policy_flag() {
        let config = r#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"some-sandbox","spec":{"image":"registry.example.test/runtime:1","workdir":"/work","volumes":[{"type":"bind","source":".","target":"/workspace"},{"type":"volume","name":"cache","target":"/root/.cache","readOnly":true}],"ports":[{"container":8080},{"host":9090,"container":3000}],"filesets":[{"path":"./skills","guestPath":"/root/.agent/skills"},{"inline":{"settings.json":"do-not-print"},"guestPath":"/etc/agent","owner":"root"}],"egress":{"http":[{"match":"*","verdict":"allow"}]}}}"#;

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
                        size_bytes: None,
                    },
                    SandboxMount {
                        kind: SandboxMountKind::Volume,
                        source: "cache".into(),
                        target: "/root/.cache".into(),
                        read_only: true,
                        exclude: Vec::new(),
                        optional: false,
                        size_bytes: None,
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
                        guest_path: "/root/.agent/skills".into(),
                        owner: lns_ipc::SandboxFilesetOwner::Workload,
                    },
                    SandboxFileset {
                        path: None,
                        inline: true,
                        host_path: None,
                        optional: false,
                        guest_path: "/etc/agent".into(),
                        owner: lns_ipc::SandboxFilesetOwner::Root,
                    },
                ],
                credentials: Vec::new(),
                env: vec![],
                tools: vec![],
                scripts: Vec::new(),
                policy_flags: vec![
                    "wildcard allow — a catch-all or whole-suffix host pattern is permitted".into()
                ],
                cpus: None,
                mem_mib: None,
                disk_bytes: None,
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
                credentials: Vec::new(),
                env: vec![],
                tools: vec!["node@22.11.0".into(), "python@3.12.6".into()],
                scripts: Vec::new(),
                policy_flags: vec![],
                cpus: None,
                mem_mib: None,
                disk_bytes: None,
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
                credentials: Vec::new(),
                env: vec!["FOO=bar".into(), "SHELL=/bin/sh".into()],
                tools: vec![],
                scripts: Vec::new(),
                policy_flags: vec![],
                cpus: None,
                mem_mib: None,
                disk_bytes: None,
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
