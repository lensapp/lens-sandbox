//! The lns-registry typed-artifact contract: the family taxonomy, its OCI media types, and the file→config-blob conversion. The registry is the schema authority (it validates the blob on push); this module only produces a well-formed blob and names the right media types.

use std::io;

/// One of the registry's typed-artifact families (`GET /ext/v1/types`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Agent,
    Policy,
    Tool,
    Workflow,
    Sandbox,
    Knowledge,
    Integration,
    Bundle,
}

impl Family {
    pub const ALL: [Family; 8] = [
        Family::Agent,
        Family::Policy,
        Family::Tool,
        Family::Workflow,
        Family::Sandbox,
        Family::Knowledge,
        Family::Integration,
        Family::Bundle,
    ];

    pub fn slug(self) -> &'static str {
        match self {
            Family::Agent => "agent",
            Family::Policy => "policy",
            Family::Tool => "tool",
            Family::Workflow => "workflow",
            Family::Sandbox => "sandbox",
            Family::Knowledge => "knowledge",
            Family::Integration => "integration",
            Family::Bundle => "bundle",
        }
    }

    /// The repository path segment convention (`org/<org>/<segment>/<name>`); plural except the mass-noun `knowledge`.
    pub fn path_segment(self) -> &'static str {
        match self {
            Family::Agent => "agents",
            Family::Policy => "policies",
            Family::Tool => "tools",
            Family::Workflow => "workflows",
            Family::Sandbox => "sandboxes",
            Family::Knowledge => "knowledge",
            Family::Integration => "integrations",
            Family::Bundle => "bundles",
        }
    }

    pub fn artifact_type(self) -> String {
        format!("application/vnd.lens.{}.v1+json", self.slug())
    }

    pub fn config_media_type(self) -> String {
        format!("application/vnd.lens.{}.config.v1+json", self.slug())
    }

    pub fn from_slug(slug: &str) -> Option<Family> {
        Family::ALL.into_iter().find(|f| f.slug() == slug)
    }

    pub fn from_path_segment(segment: &str) -> Option<Family> {
        Family::ALL
            .into_iter()
            .find(|f| f.path_segment() == segment)
    }

    pub fn from_artifact_type(media_type: &str) -> Option<Family> {
        let slug = media_type
            .strip_prefix("application/vnd.lens.")?
            .strip_suffix(".v1+json")?;
        Family::from_slug(slug)
    }

    pub fn from_config_media_type(media_type: &str) -> Option<Family> {
        let slug = media_type
            .strip_prefix("application/vnd.lens.")?
            .strip_suffix(".config.v1+json")?;
        Family::from_slug(slug)
    }

    /// Infers the family from a registry reference's repository path (`…/<segment>/<name>[:tag|@digest]`); the trailing name segment is skipped, so its tag/digest is irrelevant.
    pub fn infer_from_reference(reference: &str) -> Option<Family> {
        reference
            .split('/')
            .rev()
            .skip(1)
            .find_map(Family::from_path_segment)
    }
}

/// Normalizes an authored artifact file (YAML or JSON) into the canonical JSON config blob the registry stores and validates.
pub fn to_config_blob(file_bytes: &[u8]) -> io::Result<Vec<u8>> {
    let value: serde_json::Value = serde_yaml::from_slice(file_bytes)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    serde_json::to_vec(&value).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn media_types_match_the_registry_formula_for_every_family() {
        assert_eq!(
            Family::Policy.artifact_type(),
            "application/vnd.lens.policy.v1+json"
        );
        assert_eq!(
            Family::Policy.config_media_type(),
            "application/vnd.lens.policy.config.v1+json"
        );
        assert_eq!(
            Family::Agent.artifact_type(),
            "application/vnd.lens.agent.v1+json"
        );
        assert_eq!(
            Family::Bundle.config_media_type(),
            "application/vnd.lens.bundle.config.v1+json"
        );
        // Every family round-trips slug <-> media types.
        for f in Family::ALL {
            assert_eq!(Family::from_artifact_type(&f.artifact_type()), Some(f));
            assert_eq!(
                Family::from_config_media_type(&f.config_media_type()),
                Some(f)
            );
            assert_eq!(Family::from_slug(f.slug()), Some(f));
            assert_eq!(Family::from_path_segment(f.path_segment()), Some(f));
        }
    }

    #[test]
    fn media_type_parsers_reject_foreign_types() {
        assert_eq!(
            Family::from_config_media_type("application/vnd.oci.image.config.v1+json"),
            None
        );
        assert_eq!(
            Family::from_artifact_type("application/vnd.lens.ghost.v1+json"),
            None
        );
        assert_eq!(Family::from_slug("ghost"), None);
        assert_eq!(Family::from_path_segment("widgets"), None);
    }

    #[test]
    fn infer_from_reference_reads_the_family_segment_before_the_name() {
        assert_eq!(
            Family::infer_from_reference("registry.example.com/org/acme/policies/pii:v1"),
            Some(Family::Policy)
        );
        assert_eq!(
            Family::infer_from_reference("localhost:5000/org/acme/agents/hermes@sha256:abc"),
            Some(Family::Agent)
        );
        assert_eq!(
            Family::infer_from_reference("localhost:5000/org/acme/agents/hermes"),
            Some(Family::Agent)
        );
    }

    #[test]
    fn infer_from_reference_is_none_without_a_known_segment() {
        assert_eq!(
            Family::infer_from_reference("docker.io/library/alpine:3.20"),
            None
        );
        assert_eq!(
            Family::infer_from_reference("localhost:5000/just-a-name"),
            None
        );
    }

    #[test]
    fn to_config_blob_accepts_yaml_and_emits_json() {
        let blob = to_config_blob(b"network:\n  defaultVerdict: ask\n").unwrap();
        let v: serde_json::Value = serde_json::from_slice(&blob).unwrap();
        assert_eq!(v, json!({"network": {"defaultVerdict": "ask"}}));
    }

    #[test]
    fn to_config_blob_accepts_json_unchanged_in_meaning() {
        let blob = to_config_blob(br#"{"name":"pii","network":{}}"#).unwrap();
        let v: serde_json::Value = serde_json::from_slice(&blob).unwrap();
        assert_eq!(v, json!({"name": "pii", "network": {}}));
    }

    #[test]
    fn to_config_blob_rejects_a_malformed_file() {
        let err = to_config_blob(b": : not yaml or json : :").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }
}
