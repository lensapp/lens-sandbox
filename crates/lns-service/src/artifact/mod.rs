use anyhow::{Context, Result, bail};

pub mod assembly;
pub mod audit;
pub mod credential_boot;
pub mod fileset;
pub mod inspect;
pub mod mixin;
pub mod mixin_dir;
pub mod policy;
pub mod real;
pub mod resources;

pub use lns_artifact::spec;

use assembly::ResolvedSandbox;
use spec::Kind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunPath {
    SingleImage,
    Sandbox,
}

pub fn dispatch(
    artifact_type: Option<&str>,
    config_media_type: Option<&str>,
) -> Result<Option<Kind>> {
    let artifact_type = artifact_type.filter(|t| !t.is_empty());
    let config_media_type = config_media_type.filter(|t| !t.is_empty());
    // Fall back to the config-blob media type only when artifactType is absent (the oras case), never to second-guess a present-but-unrecognized one.
    let kind = match artifact_type {
        Some(t) => Kind::from_artifact_type(t),
        None => config_media_type.and_then(Kind::from_config_media_type),
    };
    match (kind, artifact_type) {
        (Some(kind), _) => Ok(Some(kind)),
        (None, Some(unknown)) => bail!(
            "unsupported artifact type {unknown}; \
             lns run launches a sandbox"
        ),
        (None, None) => Ok(None),
    }
}

/// Classify a run reference and, when the CLI asked to verify it, refuse a plain OCI image that is not a sandbox — pointing at `lns init`. A local sandbox's base image passes `verify=false` and runs directly.
pub fn dispatch_run(
    artifact_type: Option<&str>,
    config_media_type: Option<&str>,
    reference: &str,
    verify: bool,
) -> Result<RunPath> {
    let path = match dispatch(artifact_type, config_media_type)? {
        Some(Kind::Sandbox) => RunPath::Sandbox,
        Some(other) => bail!(
            "a {} artifact is not directly runnable; lns run takes a published sandbox",
            other.as_str()
        ),
        None => RunPath::SingleImage,
    };
    if verify && path == RunPath::SingleImage {
        bail!(
            "{reference} is not a sandbox; run `lns init` to author an lns.yaml, \
             or pass a published sandbox reference"
        );
    }
    Ok(path)
}

/// The baseline the guest's gate folds this directory's live decisions over. A source that ships neither egress nor connectors leaves no baseline at all, so the directory's decisions govern verbatim.
fn baseline_policy(
    egress: &lns_policy::Egress,
    connectors: &[String],
) -> Option<lns_policy::Policy> {
    let ships_policy = *egress != lns_policy::Egress::default() || !connectors.is_empty();
    ships_policy.then(|| lns_policy::Policy {
        network: lns_policy::NetworkPolicy {
            egress: egress.clone(),
        },
        connectors: connectors.to_vec(),
        ..lns_policy::Policy::default()
    })
}

/// Re-base a plan's policy on the egress every source but the directory's own decided, because the gate folds this directory's live decisions over that baseline rather than over the copy of them the resolved document carries (`docs/sandbox-spec.md` §8.1).
pub fn with_authored_baseline(
    mut resolved: ResolvedSandbox,
    authored: &lns_policy::Egress,
) -> ResolvedSandbox {
    let connectors = resolved
        .policy
        .as_ref()
        .map(|policy| policy.connectors.clone())
        .unwrap_or_default();
    resolved.policy = baseline_policy(authored, &connectors);
    resolved
}

/// The baseline a preflight resolved, as it travelled to the run that boots it.
pub fn authored_egress(json: &str) -> Result<lns_policy::Egress> {
    serde_json::from_str(json).context("reading the egress this run's preflight resolved")
}

/// Where the document being planned came from, which is what decides whether its own `path` filesets name a layer of a pulled artifact or a directory on this machine.
#[derive(Debug, Clone, Copy)]
pub enum RootSource<'a> {
    Pulled(&'a str),
    Local,
}

/// Which artifact ships a `path` fileset's layer, or `None` when the declaring document is a file on this machine and the directory is read directly.
///
/// Only a digest-pinned label names an artifact, so anything else — a directory mixin's path, the decisions file's own name — is read off this machine rather than fetched. A label is a name, not a promise: routing anything unpinned to a registry would fetch whatever squats that name.
fn packed_source(
    origin: Option<&lns_ipc::FilesetOrigin>,
    root: RootSource<'_>,
) -> Option<(String, usize)> {
    let origin = origin?;
    if origin.source == lns_artifact::merge::ROOT_LABEL {
        return match root {
            RootSource::Pulled(reference) => Some((reference.to_string(), origin.layer)),
            RootSource::Local => None,
        };
    }
    lns_artifact::spec::is_digest_pinned_image(&origin.source)
        .then(|| (origin.source.clone(), origin.layer))
}

/// Map a flat `kind: sandbox` definition onto a resolved run: its base image plus the inline config, with no component graph to assemble. A definition that ships neither a network policy nor connectors plans with no policy baseline, so the directory's overlay governs verbatim.
pub fn resolved_from_sandbox(
    def: &lns_artifact::sandbox::Definition,
    origins: &[lns_ipc::FilesetOrigin],
    root: RootSource<'_>,
) -> ResolvedSandbox {
    let mut local_filesets = Vec::new();
    let mut packed_filesets = Vec::new();
    for fileset in def.spec.filesets.iter() {
        let Some(path) = &fileset.path else {
            continue;
        };
        let origin = origins
            .iter()
            .find(|origin| origin.mount_path == fileset.mount_path);
        match packed_source(origin, root) {
            Some((reference, layer)) => packed_filesets.push(assembly::PackedFileset {
                mount_path: fileset.mount_path.clone(),
                reference,
                layer,
                owner: fileset.owner,
            }),
            None => local_filesets.push(assembly::LocalFileset {
                source: path.clone(),
                mount_path: fileset.mount_path.clone(),
                owner: fileset.owner,
            }),
        }
    }
    ResolvedSandbox {
        base_image: def.spec.image.clone(),
        user: def.spec.user.clone(),
        local_filesets,
        packed_filesets,
        host_filesets: def
            .spec
            .filesets
            .iter()
            .filter_map(|fileset| {
                fileset
                    .host_path
                    .as_ref()
                    .map(|source| assembly::HostFileset {
                        source: source.clone(),
                        mount_path: fileset.mount_path.clone(),
                        owner: fileset.owner,
                        optional: fileset.optional,
                    })
            })
            .collect(),
        inline_filesets: def
            .spec
            .filesets
            .iter()
            .filter_map(|fileset| {
                fileset
                    .inline
                    .as_ref()
                    .map(|files| assembly::InlineFileset {
                        files: files.clone(),
                        mount_path: fileset.mount_path.clone(),
                        owner: fileset.owner,
                    })
            })
            .collect(),
        command: def.spec.command.clone(),
        env: def.spec.env.clone(),
        resources: def.spec.resources.clone(),
        policy: baseline_policy(&def.spec.egress, &def.spec.connectors),
        credentials: def.spec.credentials.clone(),
        tools: def.spec.tools.clone(),
    }
}

/// The trust gate for a published run's filesets: every `path` entry must draw its files from a digest-pinned artifact, since a directory means nothing off the machine it was written on. `packed_source` routes only a pinned label, so an entry that reached `local_filesets` here is one no artifact ships.
pub fn published_fileset_problems(resolved: &ResolvedSandbox) -> Vec<String> {
    resolved
        .local_filesets
        .iter()
        .map(|local| {
            format!(
                "no digest-pinned artifact ships the fileset mounted at {} (declared as {}); refusing to run it",
                local.mount_path, local.source
            )
        })
        .collect()
}

/// Plan a published sandbox's config blob: the one place the pulled path turns a document into a run, so every guard between the two applies to a stranger's artifact as much as to a local file.
pub fn plan_published_sandbox(
    config_json: &[u8],
    image_ref: &str,
    origins: &[lns_ipc::FilesetOrigin],
) -> Result<ResolvedSandbox> {
    let def = lns_artifact::sandbox::parse(config_json)
        .with_context(|| format!("parsing published sandbox {image_ref}"))?;
    if !def.spec.mixins.is_empty() {
        anyhow::bail!(
            "published sandbox {image_ref} reached the plan without being resolved; it still declares mixins ({}), and running it now would drop what they contribute",
            def.spec.mixins.join(", ")
        );
    }
    Ok(resolved_from_sandbox(
        &def,
        origins,
        RootSource::Pulled(image_ref),
    ))
}

/// Resolution is what empties this list, so a definition still carrying one never went through it and would boot without what its mixins contribute.
pub fn refuse_unresolved_local_mixins(def: &lns_artifact::sandbox::Definition) -> Result<()> {
    if def.spec.mixins.is_empty() {
        return Ok(());
    }
    anyhow::bail!(
        "this definition reached the plan without being resolved; it still declares mixins ({})",
        def.spec.mixins.join(", ")
    )
}

/// Plan a local `lns.yaml` definition through the same path a published sandbox takes, so its policy, connectors, and resources apply identically.
pub fn plan_local_sandbox(
    config_json: &[u8],
    origins: &[lns_ipc::FilesetOrigin],
) -> Result<ResolvedSandbox> {
    let def = lns_artifact::sandbox::parse(config_json)
        .context("parsing the local sandbox definition")?;
    refuse_unresolved_local_mixins(&def)?;
    Ok(resolved_from_sandbox(&def, origins, RootSource::Local))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin(mount_path: &str, source: &str, layer: usize) -> lns_ipc::FilesetOrigin {
        lns_ipc::FilesetOrigin {
            mount_path: mount_path.into(),
            source: source.into(),
            layer,
        }
    }

    #[test]
    fn a_pulled_documents_path_fileset_resolves_to_a_layer_of_the_artifact_that_ships_it() {
        let def = lns_artifact::sandbox::parse(
            br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"s","spec":{"image":"x:1","filesets":[{"path":"./skills","mountPath":"/a"},{"path":"./prompts","mountPath":"/b","owner":"root"}]}}"#,
        )
        .unwrap();
        let origins = [
            origin("/a", lns_artifact::merge::ROOT_LABEL, 0),
            origin(
                "/b",
                "reg/tools@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                0,
            ),
        ];
        let resolved = resolved_from_sandbox(&def, &origins, RootSource::Pulled("reg/s@sha256:d"));
        assert!(
            resolved.local_filesets.is_empty(),
            "nothing on a pulled document names a directory on this machine"
        );
        assert_eq!(
            resolved.packed_filesets,
            [
                assembly::PackedFileset {
                    mount_path: "/a".into(),
                    reference: "reg/s@sha256:d".into(),
                    layer: 0,
                    owner: lns_artifact::sandbox::FilesetOwner::Workload,
                },
                assembly::PackedFileset {
                    mount_path: "/b".into(),
                    reference: "reg/tools@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
                    layer: 0,
                    owner: lns_artifact::sandbox::FilesetOwner::Root,
                },
            ],
            "the sandbox's own entry comes from the sandbox artifact; a mixin's comes from that mixin's"
        );
    }

    #[test]
    fn a_local_documents_path_fileset_is_read_from_the_directory_it_names() {
        let def = lns_artifact::sandbox::parse(
            br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"s","spec":{"image":"x:1","filesets":[{"path":"/work/skills","mountPath":"/a"},{"path":"/work/mixins/tools/prompts","mountPath":"/b"}]}}"#,
        )
        .unwrap();
        let origins = [
            origin("/a", lns_artifact::merge::ROOT_LABEL, 0),
            origin("/b", "./mixins/tools", 0),
        ];
        let resolved = resolved_from_sandbox(&def, &origins, RootSource::Local);
        assert_eq!(
            resolved.local_filesets.len(),
            2,
            "a directory mixin ships no artifact, so its files are read where they sit"
        );
        assert!(resolved.packed_filesets.is_empty());
    }

    #[test]
    fn published_fileset_problems_refuse_a_path_no_pinned_artifact_ships() {
        let def = lns_artifact::sandbox::parse(
            br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"s","spec":{"image":"x:1","filesets":[{"path":"./skills","mountPath":"/a"},{"path":"./tools","mountPath":"/b"},{"path":"./ok","mountPath":"/c"}]}}"#,
        )
        .unwrap();
        let pinned = format!("reg/settings@sha256:{}", "a".repeat(64));
        let origins = [
            origin("/b", "reg/skills:latest", 0),
            origin("/c", &pinned, 0),
        ];
        let resolved = resolved_from_sandbox(&def, &origins, RootSource::Local);
        let problems = published_fileset_problems(&resolved);
        assert_eq!(problems.len(), 2, "got: {problems:?}");
        assert!(
            problems.iter().any(|p| p.contains("at /a")),
            "an entry no source claims ships nothing: {problems:?}"
        );
        assert!(
            problems.iter().any(|p| p.contains("at /b")),
            "a floating tag names no artifact, so it must not pass as one: {problems:?}"
        );
        assert!(
            resolved
                .packed_filesets
                .iter()
                .all(|fileset| fileset.reference == pinned),
            "only the digest-pinned source may be fetched: {:?}",
            resolved.packed_filesets
        );
    }

    #[test]
    fn a_truncated_or_malformed_digest_never_becomes_a_fetchable_source() {
        let def = lns_artifact::sandbox::parse(
            br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"s","spec":{"image":"x:1","filesets":[{"path":"./skills","mountPath":"/a"}]}}"#,
        )
        .unwrap();
        let origins = [origin("/a", "reg/skills@sha256:abc", 0)];
        let resolved = resolved_from_sandbox(&def, &origins, RootSource::Local);
        assert!(
            resolved.packed_filesets.is_empty(),
            "a truncated @sha256 is not a pin, so it must never be routed to a registry, not admitted by a loose contains(\"@sha256:\") check: {:?}",
            resolved.packed_filesets
        );
        assert!(
            !published_fileset_problems(&resolved).is_empty(),
            "and the run is refused rather than silently reading the publisher's path off this machine"
        );
    }

    #[test]
    fn an_empty_artifact_type_and_config_type_is_treated_as_a_plain_image() {
        assert_eq!(dispatch(Some(""), Some("")).unwrap(), None);
    }

    #[test]
    fn dispatch_run_refuses_a_plain_image_reference_pointing_at_lns_init() {
        let err = dispatch_run(None, None, "alpine:3.20", true).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("not a sandbox"), "got: {msg}");
        assert!(msg.contains("lns init"), "got: {msg}");
    }

    #[test]
    fn resolved_from_sandbox_carries_the_base_image_command_env_and_policy() {
        let def = lns_artifact::sandbox::parse(
            br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"ghcr.io/team/base@sha256:abc","command":"agent --serve","env":{"MODE":"research"},"egress":{"http":[{"match":"*","verdict":"deny"}]},"connectors":["some-provider"],"user":"root"}}"#,
        )
        .unwrap();
        let resolved = resolved_from_sandbox(&def, &[], RootSource::Local);
        assert_eq!(resolved.base_image, "ghcr.io/team/base@sha256:abc");
        assert_eq!(resolved.command.as_deref(), Some("agent --serve"));
        assert_eq!(
            assembly::assemble(&resolved).user.as_deref(),
            Some("root"),
            "the run-as user the definition asked for has to survive assembly, or the launch resolves without it"
        );
        assert_eq!(
            resolved.env.get("MODE").map(String::as_str),
            Some("research")
        );
        assert!(resolved.packed_filesets.is_empty());
        let policy = resolved
            .policy
            .expect("a flat sandbox carries its inline policy");
        assert_eq!(
            policy.network.egress.http[0].match_pattern, "*",
            "the baseline's lockdown is a catch-all deny it carries in the table"
        );
        assert_eq!(policy.connectors, vec!["some-provider".to_string()]);
    }

    #[test]
    fn a_plan_re_based_on_the_authored_egress_leaves_out_what_the_directory_decided() {
        let def = lns_artifact::sandbox::parse(
            br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"ghcr.io/team/base:1","connectors":["some-provider"],"egress":{"http":[{"match":"docs.some-vendor.example","verdict":"allow"},{"match":"api.some-vendor.example","verdict":"deny"}]}}}"#,
        )
        .unwrap();
        let authored = authored_egress(
            r#"{"http":[{"match":"api.some-vendor.example","verdict":"deny"}],"tcp":[]}"#,
        )
        .expect("the baseline a preflight resolved parses");

        let policy = with_authored_baseline(
            resolved_from_sandbox(&def, &[], RootSource::Local),
            &authored,
        )
        .policy
        .expect("a sandbox that ships egress keeps a baseline");

        assert_eq!(
            policy
                .network
                .egress
                .http
                .iter()
                .map(|rule| rule.match_pattern.as_str())
                .collect::<Vec<_>>(),
            ["api.some-vendor.example"],
            "the gate folds the live decisions file over this, and the developer's own allow frozen into it would outlive them deleting the rule"
        );
        assert_eq!(
            policy.connectors,
            vec!["some-provider".to_string()],
            "no mixin can name a connector, so re-basing the egress must not drop the list the sandbox itself declared"
        );
    }

    #[test]
    fn a_plan_whose_only_egress_was_the_directorys_own_leaves_the_gate_no_baseline() {
        let def = lns_artifact::sandbox::parse(
            br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"ghcr.io/team/base:1","egress":{"http":[{"match":"docs.some-vendor.example","verdict":"allow"}]}}}"#,
        )
        .unwrap();

        assert_eq!(
            with_authored_baseline(
                resolved_from_sandbox(&def, &[], RootSource::Local),
                &lns_policy::Egress::default()
            )
            .policy,
            None,
            "nothing but the developer decided anything here, so the file governs verbatim rather than through a merge with itself"
        );
    }

    #[test]
    fn a_definition_that_shipped_nothing_still_leaves_the_gate_no_baseline_to_fold_over() {
        let def = lns_artifact::sandbox::parse(
            br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"ghcr.io/team/base:1"}}"#,
        )
        .unwrap();

        assert_eq!(
            with_authored_baseline(
                resolved_from_sandbox(&def, &[], RootSource::Local),
                &lns_policy::Egress::default()
            )
            .policy,
            None,
            "a directory whose decisions are the only word on the network has nothing to layer them over, and inventing an empty baseline would merge the file with itself"
        );
    }

    #[test]
    fn a_baseline_that_is_not_an_egress_table_refuses_rather_than_governing_as_nothing() {
        let err = authored_egress("not json").unwrap_err();
        assert!(
            format!("{err:#}").contains("the egress this run's preflight resolved"),
            "a baseline read as empty would silently drop every rule the sandbox ships; got: {err:#}"
        );
    }

    #[test]
    fn a_definition_shipping_no_policy_or_connectors_plans_without_a_baseline() {
        let def = lns_artifact::sandbox::parse(
            br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"ghcr.io/team/base:1"}}"#,
        )
        .unwrap();
        assert_eq!(
            resolved_from_sandbox(&def, &[], RootSource::Local).policy,
            None,
            "a plain definition must leave the directory overlay governing verbatim"
        );
    }

    #[test]
    fn plan_local_sandbox_resolves_the_definition_like_a_published_one() {
        let resolved = plan_local_sandbox(
            br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"ghcr.io/team/base:1","connectors":["some-provider"],"resources":{"cpu":2,"memory":"1Gi"}}}"#,
            &[],
        )
        .unwrap();
        assert_eq!(resolved.base_image, "ghcr.io/team/base:1");
        assert!(
            resolved.resources.is_some(),
            "resources must survive the plan"
        );
        let policy = resolved.policy.expect("the plan carries the inline policy");
        assert_eq!(policy.connectors, vec!["some-provider".to_string()]);
    }

    #[test]
    fn plan_local_sandbox_surfaces_a_broken_definition() {
        let err = plan_local_sandbox(
            br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{}}"#,
            &[],
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("must carry an image"),
            "got: {err:#}"
        );
    }

    #[test]
    fn a_published_document_reaching_the_plan_unresolved_refuses_rather_than_dropping_its_mixins() {
        let pinned = format!("ghcr.io/acme/postgres-tools@sha256:{}", "c".repeat(64));
        let err = plan_published_sandbox(
            format!(
                r#"{{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{{"image":"ghcr.io/team/base:1","mixins":["{pinned}"]}}}}"#
            )
            .as_bytes(),
            "registry.example.test/team/sandbox:1",
            &[],
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("reached the plan without being resolved"),
            "resolution is what empties this list, so a document that still carries one never went through it and would boot without what its mixins contribute; got: {err:#}"
        );
    }

    #[test]
    fn dispatch_runs_a_published_sandbox_artifact_directly() {
        let sandbox_type = Kind::Sandbox.artifact_type();
        assert_eq!(
            dispatch(Some(&sandbox_type), None).unwrap(),
            Some(Kind::Sandbox)
        );
        assert_eq!(
            dispatch_run(Some(&sandbox_type), None, "ghcr.io/team/hermes:1", true).unwrap(),
            RunPath::Sandbox,
            "a published sandbox is the one runnable noun, verified or not"
        );
    }

    #[test]
    fn dispatch_run_runs_a_local_base_image_directly_when_verification_is_off() {
        assert_eq!(
            dispatch_run(None, None, "docker.io/library/alpine:3.20", false).unwrap(),
            RunPath::SingleImage,
            "a local sandbox's plain base image must run directly, not be refused",
        );
    }

    #[test]
    fn a_present_unknown_artifact_type_is_refused_even_with_a_known_config_media_type() {
        let err = dispatch(
            Some("application/vnd.oci.image.config.v1+json"),
            Some(&Kind::Sandbox.config_media_type()),
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("unsupported artifact type"),
            "a real artifactType we don't know must not be overridden by the config blob: {err:#}"
        );
    }
}
