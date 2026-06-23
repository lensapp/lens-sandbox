//! The lns-registry typed-artifact contract: the family taxonomy, its OCI media types, and the file→config-blob conversion. The registry is the schema authority (it validates the blob on push); this module only produces a well-formed blob and names the right media types.

use std::io;

use serde::Deserialize;

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

/// A named credential the agent needs, mapped to the env var it is injected as.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CredentialRef {
    pub name: String,
    pub env: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ArtifactMetadata {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PortMapping {
    pub host: u16,
    pub container: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeMapping {
    pub name: String,
    pub target: String,
    #[serde(default)]
    pub read_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSpec {
    pub image: String,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub isolation: Option<String>,
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub ports: Vec<PortMapping>,
    #[serde(default)]
    pub volumes: Vec<VolumeMapping>,
    #[serde(default)]
    pub credentials: Vec<CredentialRef>,
}

/// The parsed `agent` artifact (`kind: Agent`) — the runtime-bearing artifact `lns run` resolves.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentArtifact {
    pub api_version: String,
    pub kind: String,
    pub metadata: ArtifactMetadata,
    pub spec: AgentSpec,
}

impl AgentArtifact {
    pub fn from_config_blob(blob: &[u8]) -> io::Result<AgentArtifact> {
        let artifact: AgentArtifact = serde_json::from_slice(blob)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        if artifact.kind != "Agent" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected an Agent artifact, got kind {:?}", artifact.kind),
            ));
        }
        if artifact.spec.image.trim().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "agent spec.image must not be empty",
            ));
        }
        Ok(artifact)
    }
}

/// A reference to another artifact inside a bundle's component list.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ComponentRef {
    #[serde(rename = "ref")]
    pub reference: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Components {
    #[serde(default)]
    pub agents: Vec<ComponentRef>,
    #[serde(default)]
    pub policies: Vec<ComponentRef>,
    #[serde(default)]
    pub tools: Vec<ComponentRef>,
    #[serde(default)]
    pub sandbox: Option<ComponentRef>,
    #[serde(default)]
    pub knowledge: Vec<ComponentRef>,
}

/// The parsed `bundle` artifact (`kind: AgentSystem`) — a manifest of component references.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BundleArtifact {
    pub api_version: String,
    pub kind: String,
    pub metadata: ArtifactMetadata,
    #[serde(default)]
    pub components: Components,
}

impl BundleArtifact {
    pub fn from_config_blob(blob: &[u8]) -> io::Result<BundleArtifact> {
        let bundle: BundleArtifact = serde_json::from_slice(blob)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        if bundle.kind != "AgentSystem" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected an AgentSystem bundle, got kind {:?}", bundle.kind),
            ));
        }
        Ok(bundle)
    }
}

/// A resource quantity as the registry schema allows it: a number or a string (e.g. `3072` or `"3g"`); the consumer interprets it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum Quantity {
    Unsigned(u64),
    Float(f64),
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SandboxResources {
    #[serde(default)]
    pub cpu: Option<Quantity>,
    #[serde(default)]
    pub memory: Option<Quantity>,
}

/// The parsed `sandbox` artifact — the runtime envelope; `lns run` reads its `resources` for sizing.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SandboxArtifact {
    pub name: String,
    #[serde(default)]
    pub isolation: Option<String>,
    #[serde(default)]
    pub resources: Option<SandboxResources>,
}

impl SandboxArtifact {
    pub fn from_config_blob(blob: &[u8]) -> io::Result<SandboxArtifact> {
        serde_json::from_slice(blob).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
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

    fn agent_blob(yaml: &str) -> Vec<u8> {
        to_config_blob(yaml.as_bytes()).unwrap()
    }

    #[test]
    fn agent_artifact_parses_image_command_and_credentials() {
        let blob = agent_blob(
            "apiVersion: lens.dev/v1alpha1\n\
             kind: Agent\n\
             metadata:\n  name: some-agent\n  labels:\n    team: demo\n\
             spec:\n  image: localhost:5000/org/acme/images/some-agent:v1\n  \
             command: 'run it'\n  isolation: microvm\n  \
             credentials:\n    - { name: some-provider, env: SOME_TOKEN }\n",
        );
        let agent = AgentArtifact::from_config_blob(&blob).unwrap();
        assert_eq!(agent.metadata.name, "some-agent");
        assert_eq!(
            agent.spec.image,
            "localhost:5000/org/acme/images/some-agent:v1"
        );
        assert_eq!(agent.spec.command.as_deref(), Some("run it"));
        assert_eq!(agent.spec.isolation.as_deref(), Some("microvm"));
        assert_eq!(
            agent.spec.credentials,
            vec![CredentialRef {
                name: "some-provider".into(),
                env: "SOME_TOKEN".into()
            }]
        );
    }

    #[test]
    fn agent_artifact_parses_user_ports_and_volumes() {
        let blob = agent_blob(
            "apiVersion: lens.dev/v1alpha1\nkind: Agent\n\
             metadata:\n  name: some-agent\n\
             spec:\n  image: some-image:1\n  user: runner\n  \
             ports:\n    - { host: 9119, container: 9119 }\n  \
             volumes:\n    - { name: somedata, target: /opt/data }\n",
        );
        let agent = AgentArtifact::from_config_blob(&blob).unwrap();
        assert_eq!(agent.spec.user.as_deref(), Some("runner"));
        assert_eq!(
            agent.spec.ports,
            vec![PortMapping {
                host: 9119,
                container: 9119
            }]
        );
        assert_eq!(
            agent.spec.volumes,
            vec![VolumeMapping {
                name: "somedata".into(),
                target: "/opt/data".into(),
                read_only: false
            }]
        );
    }

    #[test]
    fn agent_artifact_tolerates_missing_command_and_credentials() {
        let blob = agent_blob(
            "apiVersion: lens.dev/v1alpha1\nkind: Agent\n\
             metadata:\n  name: bare\nspec:\n  image: some-image:1\n",
        );
        let agent = AgentArtifact::from_config_blob(&blob).unwrap();
        assert!(agent.spec.command.is_none());
        assert!(agent.spec.credentials.is_empty());
        assert!(agent.spec.user.is_none());
        assert!(agent.spec.ports.is_empty());
        assert!(agent.spec.volumes.is_empty());
    }

    #[test]
    fn agent_artifact_rejects_a_non_agent_kind() {
        let blob = agent_blob(
            "apiVersion: lens.dev/v1alpha1\nkind: Policy\n\
             metadata:\n  name: x\nspec:\n  image: some-image:1\n",
        );
        let err = AgentArtifact::from_config_blob(&blob).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(format!("{err}").contains("kind"), "got: {err}");
    }

    #[test]
    fn agent_artifact_rejects_an_empty_image() {
        let blob = agent_blob(
            "apiVersion: lens.dev/v1alpha1\nkind: Agent\n\
             metadata:\n  name: x\nspec:\n  image: '   '\n",
        );
        let err = AgentArtifact::from_config_blob(&blob).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(format!("{err}").contains("image"), "got: {err}");
    }

    #[test]
    fn agent_artifact_rejects_malformed_json() {
        let err = AgentArtifact::from_config_blob(b"not json").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn bundle_artifact_parses_component_references() {
        let blob = to_config_blob(
            "apiVersion: lens.dev/v1alpha1\nkind: AgentSystem\n\
             metadata:\n  name: some-system\n\
             components:\n  \
             agents:\n    - { ref: org/acme/agents/some-agent:v1 }\n  \
             policies:\n    - { ref: org/acme/policies/some-egress:v1 }\n  \
             sandbox:\n    ref: org/acme/sandboxes/some-runtime:v1\n"
                .as_bytes(),
        )
        .unwrap();
        let bundle = BundleArtifact::from_config_blob(&blob).unwrap();
        assert_eq!(bundle.metadata.name, "some-system");
        assert_eq!(bundle.components.agents.len(), 1);
        assert_eq!(
            bundle.components.agents[0].reference,
            "org/acme/agents/some-agent:v1"
        );
        assert_eq!(
            bundle.components.policies[0].reference,
            "org/acme/policies/some-egress:v1"
        );
        assert_eq!(
            bundle.components.sandbox.as_ref().unwrap().reference,
            "org/acme/sandboxes/some-runtime:v1"
        );
        assert!(bundle.components.tools.is_empty());
    }

    #[test]
    fn bundle_artifact_rejects_a_non_agentsystem_kind() {
        let blob = to_config_blob(
            "apiVersion: lens.dev/v1alpha1\nkind: Agent\nmetadata:\n  name: x\n".as_bytes(),
        )
        .unwrap();
        let err = BundleArtifact::from_config_blob(&blob).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(format!("{err}").contains("AgentSystem"), "got: {err}");
    }

    #[test]
    fn sandbox_artifact_parses_numeric_resources() {
        let blob = to_config_blob(
            "name: some-runtime\nversion: \"1.0\"\n\
             baseImage: localhost:5000/x@sha256:0000000000000000000000000000000000000000000000000000000000000000\n\
             isolation: microvm\nresources:\n  cpu: 2\n  memory: 3072\ncapabilities: []\n"
                .as_bytes(),
        )
        .unwrap();
        let sandbox = SandboxArtifact::from_config_blob(&blob).unwrap();
        assert_eq!(sandbox.name, "some-runtime");
        assert_eq!(sandbox.isolation.as_deref(), Some("microvm"));
        let resources = sandbox.resources.unwrap();
        assert_eq!(resources.cpu, Some(Quantity::Unsigned(2)));
        assert_eq!(resources.memory, Some(Quantity::Unsigned(3072)));
    }

    #[test]
    fn sandbox_artifact_tolerates_string_resources() {
        let blob =
            to_config_blob("name: r\nresources:\n  cpu: \"2\"\n  memory: \"3g\"\n".as_bytes())
                .unwrap();
        let sandbox = SandboxArtifact::from_config_blob(&blob).unwrap();
        let resources = sandbox.resources.unwrap();
        assert_eq!(resources.cpu, Some(Quantity::Text("2".into())));
        assert_eq!(resources.memory, Some(Quantity::Text("3g".into())));
    }

    #[test]
    fn sandbox_artifact_tolerates_a_missing_resources_block() {
        let blob = to_config_blob("name: r\nisolation: microvm\n".as_bytes()).unwrap();
        let sandbox = SandboxArtifact::from_config_blob(&blob).unwrap();
        assert!(sandbox.resources.is_none());
    }

    #[test]
    fn sandbox_artifact_rejects_malformed_json() {
        let err = SandboxArtifact::from_config_blob(b"not json").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }
}
