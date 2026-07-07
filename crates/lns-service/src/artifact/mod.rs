use anyhow::{Context, Result, bail};

pub mod assembly;
pub mod fetch;
pub mod real;
pub mod resolve;
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
        Some(Kind::AgentSystem) => Ok(RunPath::AssembleBundle),
        Some(other) => bail!(
            "a {} artifact is not directly runnable; \
             lns run takes a plain OCI image or an AgentSystem bundle",
            other.as_str()
        ),
        None => match artifact_type {
            Some(unknown) => bail!(
                "unsupported artifact type {unknown}; \
                 lns run can launch a plain OCI image or an AgentSystem bundle"
            ),
            None => Ok(RunPath::SingleImage),
        },
    }
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

pub async fn plan_bundle<F: ComponentFetcher>(
    config_json: &[u8],
    fetcher: &F,
    host_arch: &str,
    overrides: &[Override],
) -> Result<ResolvedBundle> {
    let bundle = spec::parse_bundle(config_json)?;
    let spec = flatten(&bundle.spec.components);
    let resolved = resolve::resolve(&spec, fetcher, host_arch)
        .await
        .with_context(|| format!("resolving bundle {}", bundle.metadata.name))?;
    assembly::apply_with(resolved, overrides)
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
    async fn plan_bundle_applies_a_with_override() {
        let config =
            bundle_json(r#"{"sandbox":{"ref":"reg/base:1"},"agents":[{"ref":"reg/agent:1"}]}"#);
        let fetcher = MapFetcher(HashMap::from([sandbox("reg/base:1"), agent("reg/agent:1")]));
        let over = Override {
            kind: "FileSet".into(),
            name: "extra".into(),
            mount_path: Some("/root/.some-agent/extra".into()),
        };
        let resolved = plan_bundle(&config, &fetcher, "test-arch", std::slice::from_ref(&over))
            .await
            .unwrap();
        assert!(resolved.filesets.iter().any(|f| f.name == "extra"));
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
