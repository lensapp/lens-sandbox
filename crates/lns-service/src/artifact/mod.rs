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

/// Map a flat `kind: sandbox` definition onto a resolved run: its base image plus the inline config, with no component graph to assemble. A definition that ships neither a network policy nor connectors plans with no policy baseline, so the directory's overlay governs verbatim.
pub fn resolved_from_sandbox(def: &lns_artifact::sandbox::Definition) -> ResolvedSandbox {
    let ships_policy =
        def.spec.policy != lns_policy::NetworkPolicy::default() || !def.spec.connectors.is_empty();
    ResolvedSandbox {
        base_image: def.spec.image.clone(),
        user: def.spec.user.clone(),
        local_filesets: def
            .spec
            .filesets
            .iter()
            .filter_map(|fileset| {
                fileset.path.as_ref().map(|path| assembly::LocalFileset {
                    source: path.clone(),
                    mount_path: fileset.mount_path.clone(),
                    owner: fileset.owner,
                })
            })
            .collect(),
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
        filesets: def
            .spec
            .filesets
            .iter()
            .filter_map(|fileset| {
                fileset
                    .reference
                    .as_ref()
                    .map(|reference| assembly::ResolvedFileset {
                        name: fileset.mount_path.clone(),
                        paths: vec![fileset.mount_path.clone()],
                        reference: reference.clone(),
                        owner: fileset.owner,
                    })
            })
            .collect(),
        command: def.spec.command.clone(),
        env: def.spec.env.clone(),
        resources: def.spec.resources.clone(),
        policy: ships_policy.then(|| lns_policy::Policy {
            network: def.spec.policy.clone(),
            connectors: def.spec.connectors.clone(),
        }),
        credentials: def.spec.credentials.clone(),
        tools: def.spec.tools.clone(),
    }
}

/// The digest-pin trust gate for a published sandbox's filesets: a local path has no meaning off the author's machine, and a floating ref defeats pinning — both refuse the plan.
pub fn published_fileset_problems(resolved: &ResolvedSandbox) -> Vec<String> {
    let mut problems: Vec<String> = resolved
        .local_filesets
        .iter()
        .map(|local| {
            format!(
                "published sandbox declares a local path fileset {}; publish pins filesets by digest",
                local.source
            )
        })
        .collect();
    problems.extend(
        resolved
            .filesets
            .iter()
            .filter(|fileset| !lns_artifact::spec::is_digest_pinned_image(&fileset.reference))
            .map(|fileset| {
                format!(
                    "fileset ref {} is not digest-pinned; refusing to run it",
                    fileset.reference
                )
            }),
    );
    problems
}

/// Plan a published sandbox's config blob: the one place the pulled path turns a document into a run, so every guard between the two applies to a stranger's artifact as much as to a local file.
pub fn plan_published_sandbox(config_json: &[u8], image_ref: &str) -> Result<ResolvedSandbox> {
    let def = lns_artifact::sandbox::parse(config_json)
        .with_context(|| format!("parsing published sandbox {image_ref}"))?;
    if !def.spec.mixins.is_empty() {
        anyhow::bail!(
            "published sandbox {image_ref} reached the plan without being resolved; it still declares mixins ({}), and running it now would drop what they contribute",
            def.spec.mixins.join(", ")
        );
    }
    Ok(resolved_from_sandbox(&def))
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
pub fn plan_local_sandbox(config_json: &[u8]) -> Result<ResolvedSandbox> {
    let def = lns_artifact::sandbox::parse(config_json)
        .context("parsing the local sandbox definition")?;
    refuse_unresolved_local_mixins(&def)?;
    Ok(resolved_from_sandbox(&def))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolved_from_sandbox_splits_path_and_ref_filesets() {
        let def = lns_artifact::sandbox::parse(
            br#"{"apiVersion":"lns.run/v1","kind":"sandbox","metadata":{"name":"s"},"spec":{"image":"x:1","filesets":[{"path":"/work/skills","mountPath":"/a"},{"ref":"reg/skills@sha256:abc","mountPath":"/b","owner":"root"}]}}"#,
        )
        .unwrap();
        let resolved = resolved_from_sandbox(&def);
        assert_eq!(
            resolved.local_filesets,
            [assembly::LocalFileset {
                source: "/work/skills".into(),
                mount_path: "/a".into(),
                owner: lns_artifact::sandbox::FilesetOwner::Workload,
            }]
        );
        assert_eq!(resolved.filesets.len(), 1);
        assert_eq!(resolved.filesets[0].reference, "reg/skills@sha256:abc");
        assert_eq!(resolved.filesets[0].paths, ["/b"]);
        assert_eq!(
            resolved.filesets[0].owner,
            lns_artifact::sandbox::FilesetOwner::Root,
            "a declared owner must survive resolution"
        );
    }

    #[test]
    fn published_fileset_problems_refuse_paths_and_floating_refs_but_pass_pinned_refs() {
        let def = lns_artifact::sandbox::parse(
            br#"{"apiVersion":"lns.run/v1","kind":"sandbox","metadata":{"name":"s"},"spec":{"image":"x:1","filesets":[{"path":"./skills","mountPath":"/a"},{"ref":"reg/skills:latest","mountPath":"/b"},{"ref":"reg/settings@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","mountPath":"/c"}]}}"#,
        )
        .unwrap();
        let problems = published_fileset_problems(&resolved_from_sandbox(&def));
        assert_eq!(problems.len(), 2, "got: {problems:?}");
        assert!(problems[0].contains("local path fileset ./skills"));
        assert!(problems[1].contains("reg/skills:latest is not digest-pinned"));
    }

    #[test]
    fn published_fileset_problems_refuses_a_truncated_or_malformed_digest_ref() {
        let def = lns_artifact::sandbox::parse(
            br#"{"apiVersion":"lns.run/v1","kind":"sandbox","metadata":{"name":"s"},"spec":{"image":"x:1","filesets":[{"ref":"reg/skills@sha256:abc","mountPath":"/a"}]}}"#,
        )
        .unwrap();
        let problems = published_fileset_problems(&resolved_from_sandbox(&def));
        assert!(
            !problems.is_empty(),
            "a fileset ref with a truncated @sha256 digest is not digest-pinned and must be refused by the trust gate, not admitted by a loose contains(\"@sha256:\") check: {problems:?}"
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
            br#"{"apiVersion":"lns.run/v1","kind":"sandbox","metadata":{"name":"hermes"},"spec":{"image":"ghcr.io/team/base@sha256:abc","command":"agent --serve","env":{"MODE":"research"},"policy":{"egress":{"http":[{"match":"*","verdict":"deny"}]}},"connectors":["some-provider"],"user":"root"}}"#,
        )
        .unwrap();
        let resolved = resolved_from_sandbox(&def);
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
        assert!(resolved.filesets.is_empty());
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
    fn a_definition_shipping_no_policy_or_connectors_plans_without_a_baseline() {
        let def = lns_artifact::sandbox::parse(
            br#"{"apiVersion":"lns.run/v1","kind":"sandbox","metadata":{"name":"hermes"},"spec":{"image":"ghcr.io/team/base:1"}}"#,
        )
        .unwrap();
        assert_eq!(
            resolved_from_sandbox(&def).policy,
            None,
            "a plain definition must leave the directory overlay governing verbatim"
        );
    }

    #[test]
    fn plan_local_sandbox_resolves_the_definition_like_a_published_one() {
        let resolved = plan_local_sandbox(
            br#"{"apiVersion":"lns.run/v1","kind":"sandbox","metadata":{"name":"hermes"},"spec":{"image":"ghcr.io/team/base:1","connectors":["some-provider"],"resources":{"cpu":2,"memory":"1Gi"}}}"#,
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
        let err = plan_local_sandbox(br#"{"apiVersion":"lns.run/v1","kind":"sandbox","metadata":{"name":"hermes"},"spec":{}}"#)
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
                r#"{{"apiVersion":"lns.run/v1","kind":"sandbox","metadata":{{"name":"hermes"}},"spec":{{"image":"ghcr.io/team/base:1","mixins":["{pinned}"]}}}}"#
            )
            .as_bytes(),
            "registry.example.test/team/sandbox:1",
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
