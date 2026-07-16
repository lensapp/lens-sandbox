use anyhow::{Context, Result, bail};

pub mod assembly;
pub mod audit;
pub mod credential_boot;
pub mod fetch;
pub mod fileset;
pub mod policy;
pub mod real;
pub mod resolve;
pub mod resources;
pub mod signature;

pub use lns_artifact::spec;

use assembly::{Override, ResolvedBundle};
use resolve::{BundleSpec, ComponentFetcher, DeclaredComponent};
use spec::{ArtifactRef, BundleComponents, Kind};

pub const BUNDLE_ARTIFACT_TYPE: &str = "application/vnd.lens.bundle.v1+json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunPath {
    SingleImage,
    AssembleBundle,
    Sandbox,
}

pub fn dispatch(artifact_type: Option<&str>, config_media_type: Option<&str>) -> Result<RunPath> {
    let artifact_type = artifact_type.filter(|t| !t.is_empty());
    let config_media_type = config_media_type.filter(|t| !t.is_empty());
    // Fall back to the config-blob media type only when artifactType is absent (the oras case), never to second-guess a present-but-unrecognized one.
    let kind = match artifact_type {
        Some(t) => Kind::from_artifact_type(t),
        None => config_media_type.and_then(Kind::from_config_media_type),
    };
    match kind {
        Some(Kind::Sandbox) => Ok(RunPath::Sandbox),
        Some(Kind::AgentSystem) => Ok(RunPath::AssembleBundle),
        Some(other) => bail!(
            "a {} artifact is not directly runnable; \
             lns run takes a published sandbox or an AgentSystem bundle",
            other.as_str()
        ),
        None => match artifact_type {
            Some(unknown) => bail!(
                "unsupported artifact type {unknown}; \
                 lns run launches a sandbox"
            ),
            None => Ok(RunPath::SingleImage),
        },
    }
}

/// Classify a run reference and, when the CLI asked to verify it, refuse a plain OCI image that is not a sandbox — pointing at `lns init`. A local sandbox's base image passes `verify=false` and runs directly.
pub fn dispatch_run(
    artifact_type: Option<&str>,
    config_media_type: Option<&str>,
    reference: &str,
    verify: bool,
) -> Result<RunPath> {
    let path = dispatch(artifact_type, config_media_type)?;
    if verify && path == RunPath::SingleImage {
        bail!(
            "{reference} is not a sandbox; run `lns init` to author an lns.yaml, \
             or pass a published sandbox reference"
        );
    }
    Ok(path)
}

fn declared(name: String, reference: &ArtifactRef) -> DeclaredComponent {
    let reference = match &reference.digest {
        Some(digest) => format!("{}@{}", reference.reference, digest),
        None => reference.reference.clone(),
    };
    DeclaredComponent { name, reference }
}

fn flatten(components: &BundleComponents) -> BundleSpec {
    let mut declared_components = Vec::new();
    if let Some(sandbox) = &components.sandbox {
        declared_components.push(declared("sandbox".to_string(), sandbox));
    }
    for (i, agent) in components.agents.iter().enumerate() {
        declared_components.push(declared(format!("agent-{i}"), agent));
    }
    for (i, fileset) in components.filesets.iter().enumerate() {
        declared_components.push(declared(format!("fileset-{i}"), fileset));
    }
    for (i, policy) in components.policies.iter().enumerate() {
        declared_components.push(declared(format!("policy-{i}"), policy));
    }
    BundleSpec {
        components: declared_components,
    }
}

/// Map a flat `kind: Sandbox` definition onto a resolved run: its base image plus the inline config, with no component graph to assemble.
pub fn resolved_from_sandbox(def: &lns_artifact::sandbox::Definition) -> ResolvedBundle {
    ResolvedBundle {
        base_image: def.spec.image.clone(),
        base_paths: Vec::new(),
        filesets: Vec::new(),
        command: def.spec.command.clone(),
        env: def.spec.env.clone(),
        resources: def.spec.resources.clone(),
        policy: Some(lns_policy::Policy {
            network: def.spec.policy.clone(),
            integrations: def.spec.integrations.clone(),
        }),
    }
}

/// Plan a local `lns.yaml` definition through the same path a published sandbox takes, so its policy, integrations, and resources apply identically.
pub fn plan_local_sandbox(config_json: &[u8]) -> Result<ResolvedBundle> {
    let def = lns_artifact::sandbox::parse(config_json)
        .context("parsing the local sandbox definition")?;
    Ok(resolved_from_sandbox(&def))
}

pub async fn plan_bundle<F: ComponentFetcher>(
    config_json: &[u8],
    fetcher: &F,
    host_arch: &str,
    overrides: &[Override],
) -> Result<ResolvedBundle> {
    let bundle = spec::parse_bundle(config_json)?;
    let mut spec = flatten(&bundle.spec.components);
    for (i, over) in overrides.iter().enumerate() {
        spec.components.push(DeclaredComponent {
            name: format!("with-{i}"),
            reference: over.reference.clone(),
        });
    }
    resolve::resolve(&spec, fetcher, host_arch)
        .await
        .with_context(|| format!("resolving bundle {}", bundle.metadata.name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact::resolve::{FetchError, FetchedComponent};
    use std::collections::HashMap;

    struct MapFetcher(HashMap<String, FetchedComponent>);

    impl ComponentFetcher for MapFetcher {
        async fn fetch(
            &self,
            reference: &str,
        ) -> std::result::Result<FetchedComponent, FetchError> {
            self.0.get(reference).cloned().ok_or(FetchError::NotFound)
        }
    }

    fn bundle_json(components: &str) -> Vec<u8> {
        format!(
            r#"{{"apiVersion":"lens.dev/v1alpha1","kind":"AgentSystem","metadata":{{"name":"some-bundle"}},"spec":{{"components":{components}}}}}"#
        )
        .into_bytes()
    }

    fn sandbox(reference: &str) -> (String, FetchedComponent) {
        (
            reference.to_string(),
            FetchedComponent {
                kind: "Sandbox".into(),
                name: "some-sandbox".into(),
                base_image: Some("registry.example.test/base@sha256:abc".into()),
                ..Default::default()
            },
        )
    }

    fn agent(reference: &str) -> (String, FetchedComponent) {
        (
            reference.to_string(),
            FetchedComponent {
                kind: "Agent".into(),
                name: "some-agent".into(),
                command: Some("agent --serve".into()),
                ..Default::default()
            },
        )
    }

    fn policy(reference: &str) -> (String, FetchedComponent) {
        (
            reference.to_string(),
            FetchedComponent {
                kind: "Policy".into(),
                name: "some-policy".into(),
                ..Default::default()
            },
        )
    }

    fn fileset(reference: &str, name: &str, path: &str) -> (String, FetchedComponent) {
        (
            reference.to_string(),
            FetchedComponent {
                kind: "FileSet".into(),
                name: name.into(),
                mount_path: Some(path.into()),
                ..Default::default()
            },
        )
    }

    #[tokio::test]
    async fn plan_bundle_resolves_a_config_into_a_composed_workload() {
        let config = bundle_json(
            r#"{"sandbox":{"ref":"reg/base:1"},"agents":[{"ref":"reg/agent:1"}],"filesets":[{"ref":"reg/skills:1"}],"policies":[{"ref":"reg/policy:1"}]}"#,
        );
        let fetcher = MapFetcher(HashMap::from([
            sandbox("reg/base:1"),
            agent("reg/agent:1"),
            fileset("reg/skills:1", "skills", "/root/.some-agent/skills"),
            policy("reg/policy:1"),
        ]));
        let resolved = plan_bundle(&config, &fetcher, "test-arch", &[])
            .await
            .unwrap();
        assert_eq!(resolved.base_image, "registry.example.test/base@sha256:abc");
        assert_eq!(resolved.command.as_deref(), Some("agent --serve"));
        assert!(resolved.filesets.iter().any(|f| f.name == "skills"));
    }

    #[tokio::test]
    async fn plan_bundle_flattens_a_digest_pinned_ref_into_a_pinned_fetch() {
        let config = bundle_json(
            r#"{"sandbox":{"ref":"reg/base","digest":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},"agents":[{"ref":"reg/agent:1"}]}"#,
        );
        let pinned =
            "reg/base@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let fetcher = MapFetcher(HashMap::from([sandbox(pinned), agent("reg/agent:1")]));
        let resolved = plan_bundle(&config, &fetcher, "test-arch", &[])
            .await
            .unwrap();
        assert_eq!(resolved.base_image, "registry.example.test/base@sha256:abc");
    }

    #[tokio::test]
    async fn plan_bundle_gives_multiple_filesets_distinct_names_so_none_collide() {
        let config = bundle_json(
            r#"{"sandbox":{"ref":"reg/base:1"},"agents":[{"ref":"reg/agent:1"}],"filesets":[{"ref":"reg/a:1"},{"ref":"reg/b:1"}]}"#,
        );
        let fetcher = MapFetcher(HashMap::from([
            sandbox("reg/base:1"),
            agent("reg/agent:1"),
            fileset("reg/a:1", "skills", "/a"),
            fileset("reg/b:1", "settings", "/b"),
        ]));
        let resolved = plan_bundle(&config, &fetcher, "test-arch", &[])
            .await
            .unwrap();
        assert_eq!(
            resolved.filesets.len(),
            2,
            "both filesets must survive flattening"
        );
    }

    #[tokio::test]
    async fn plan_bundle_folds_a_with_override_ref_in_as_a_trailing_fileset() {
        let config = bundle_json(
            r#"{"sandbox":{"ref":"reg/base:1"},"agents":[{"ref":"reg/agent:1"}],"filesets":[{"ref":"reg/shipped:1"}]}"#,
        );
        let fetcher = MapFetcher(HashMap::from([
            sandbox("reg/base:1"),
            agent("reg/agent:1"),
            fileset("reg/shipped:1", "shipped", "/shared"),
            fileset("reg/override:1", "override", "/shared"),
        ]));
        let over = Override {
            reference: "reg/override:1".into(),
        };
        let resolved = plan_bundle(&config, &fetcher, "test-arch", std::slice::from_ref(&over))
            .await
            .unwrap();
        let last = resolved.filesets.last().expect("a resolved fileset");
        assert_eq!(
            last.name, "override",
            "the --with override must land last so it overlays the bundle fileset"
        );
        assert_eq!(
            assembly::assemble(&resolved).source_of("/shared"),
            Some(&assembly::FileSource::Fileset("override".into()))
        );
    }

    #[tokio::test]
    async fn plan_bundle_refuses_a_with_override_that_is_not_a_mountable_component() {
        let config =
            bundle_json(r#"{"sandbox":{"ref":"reg/base:1"},"agents":[{"ref":"reg/agent:1"}]}"#);
        let fetcher = MapFetcher(HashMap::from([
            sandbox("reg/base:1"),
            agent("reg/agent:1"),
            sandbox("reg/second-sandbox:1"),
        ]));
        let over = Override {
            reference: "reg/second-sandbox:1".into(),
        };
        let err = plan_bundle(&config, &fetcher, "test-arch", std::slice::from_ref(&over))
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("sandbox"),
            "a non-mountable --with override must be refused: {err:#}"
        );
    }

    #[tokio::test]
    async fn plan_bundle_refuses_a_bundle_shipping_more_than_one_policy() {
        let config = bundle_json(
            r#"{"sandbox":{"ref":"reg/base:1"},"agents":[{"ref":"reg/agent:1"}],"policies":[{"ref":"reg/p1:1"},{"ref":"reg/p2:1"}]}"#,
        );
        let fetcher = MapFetcher(HashMap::from([
            sandbox("reg/base:1"),
            agent("reg/agent:1"),
            policy("reg/p1:1"),
            policy("reg/p2:1"),
        ]));
        let err = plan_bundle(&config, &fetcher, "test-arch", &[])
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("exactly one policy"),
            "a bundle must not ship an ambiguous second policy: {err:#}"
        );
    }

    #[tokio::test]
    async fn plan_bundle_propagates_a_parse_refusal() {
        let config = bundle_json(r#"{"model":{"ref":"reg/model:1"}}"#);
        let fetcher = MapFetcher(HashMap::new());
        let err = plan_bundle(&config, &fetcher, "test-arch", &[])
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("unimplemented component kind"));
    }

    #[tokio::test]
    async fn plan_bundle_propagates_a_resolution_refusal_with_bundle_context() {
        let config = bundle_json(r#"{"agents":[{"ref":"reg/agent:1"}]}"#);
        let fetcher = MapFetcher(HashMap::from([agent("reg/agent:1")]));
        let err = plan_bundle(&config, &fetcher, "test-arch", &[])
            .await
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("resolving bundle some-bundle"), "got: {msg}");
        assert!(msg.contains("exactly one sandbox"), "got: {msg}");
    }

    #[test]
    fn an_empty_artifact_type_and_config_type_is_treated_as_a_plain_image() {
        assert_eq!(dispatch(Some(""), Some("")).unwrap(), RunPath::SingleImage);
    }

    #[test]
    fn dispatch_run_refuses_a_plain_image_reference_pointing_at_lns_init() {
        let err = dispatch_run(None, None, "alpine:3.20", true).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("not a sandbox"), "got: {msg}");
        assert!(msg.contains("lns init"), "got: {msg}");
    }

    #[test]
    fn dispatch_run_lets_a_bundle_reference_through_for_assembly() {
        assert_eq!(
            dispatch_run(
                Some(BUNDLE_ARTIFACT_TYPE),
                None,
                "ghcr.io/team/hermes:1",
                true
            )
            .unwrap(),
            RunPath::AssembleBundle
        );
    }

    #[test]
    fn resolved_from_sandbox_carries_the_base_image_command_env_and_policy() {
        let def = lns_artifact::sandbox::parse(
            br#"{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{"name":"hermes"},"spec":{"image":"ghcr.io/team/base@sha256:abc","command":"agent --serve","env":{"MODE":"research"},"policy":{"defaultVerdict":"deny"},"integrations":["some-provider"]}}"#,
        )
        .unwrap();
        let resolved = resolved_from_sandbox(&def);
        assert_eq!(resolved.base_image, "ghcr.io/team/base@sha256:abc");
        assert_eq!(resolved.command.as_deref(), Some("agent --serve"));
        assert_eq!(
            resolved.env.get("MODE").map(String::as_str),
            Some("research")
        );
        assert!(resolved.filesets.is_empty());
        let policy = resolved
            .policy
            .expect("a flat sandbox carries its inline policy");
        assert_eq!(policy.network.default_verdict, lns_policy::Verdict::Deny);
        assert_eq!(policy.integrations, vec!["some-provider".to_string()]);
    }

    #[test]
    fn plan_local_sandbox_resolves_the_definition_like_a_published_one() {
        let resolved = plan_local_sandbox(
            br#"{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{"name":"hermes"},"spec":{"image":"ghcr.io/team/base:1","integrations":["some-provider"],"resources":{"cpu":2,"memory":"1Gi"}}}"#,
        )
        .unwrap();
        assert_eq!(resolved.base_image, "ghcr.io/team/base:1");
        assert!(
            resolved.resources.is_some(),
            "resources must survive the plan"
        );
        let policy = resolved.policy.expect("the plan carries the inline policy");
        assert_eq!(policy.integrations, vec!["some-provider".to_string()]);
    }

    #[test]
    fn plan_local_sandbox_surfaces_a_broken_definition() {
        let err = plan_local_sandbox(br#"{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{"name":"hermes"},"spec":{}}"#)
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("must carry an image"),
            "got: {err:#}"
        );
    }

    #[test]
    fn dispatch_runs_a_published_sandbox_artifact_directly() {
        let sandbox_type = Kind::Sandbox.artifact_type();
        assert_eq!(
            dispatch(Some(&sandbox_type), None).unwrap(),
            RunPath::Sandbox
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
    fn a_present_unknown_artifact_type_is_refused_even_with_a_bundle_config_media_type() {
        let err = dispatch(
            Some("application/vnd.oci.image.config.v1+json"),
            Some("application/vnd.lens.bundle.config.v1+json"),
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("unsupported artifact type"),
            "a real artifactType we don't know must not be overridden by the config blob: {err:#}"
        );
    }
}
