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
    Model,
    Fileset,
}

impl Family {
    pub const ALL: [Family; 10] = [
        Family::Agent,
        Family::Policy,
        Family::Tool,
        Family::Workflow,
        Family::Sandbox,
        Family::Knowledge,
        Family::Integration,
        Family::Bundle,
        Family::Model,
        Family::Fileset,
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
            Family::Model => "model",
            Family::Fileset => "fileset",
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
            Family::Model => "models",
            Family::Fileset => "filesets",
        }
    }

    /// Application-layer families carry content the agent consumes, mounted into its filesystem; runtime-layer families are applied around the agent.
    pub fn is_application_layer(self) -> bool {
        matches!(
            self,
            Family::Model | Family::Tool | Family::Knowledge | Family::Fileset
        )
    }

    /// The canonical guest mount path for an application-layer artifact (overridden by an explicit envelope `mount`); `None` for fileset (explicit path required) and runtime-layer families.
    pub fn default_mount_path(self, name: &str) -> Option<String> {
        match self {
            Family::Model => Some("/etc/agent/model".to_string()),
            Family::Tool => Some(format!("/etc/agent/tools/{name}")),
            Family::Knowledge => Some(format!("/etc/agent/knowledge/{name}")),
            _ => None,
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

/// The optional top-level mount on an application-layer artifact's envelope (the registry rejects it on runtime-layer families).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Mount {
    pub path: String,
    #[serde(default)]
    pub read_only: Option<bool>,
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

/// An `ArtifactRef {ref, digest?}` — a reference to another artifact inside a bundle's component list.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ComponentRef {
    #[serde(rename = "ref")]
    pub reference: String,
    #[serde(default)]
    pub digest: Option<String>,
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
    pub model: Option<ComponentRef>,
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
    #[serde(default)]
    pub supervisor_version: Option<String>,
    #[serde(default)]
    pub capabilities: Option<Vec<String>>,
    #[serde(default)]
    pub base_image: Option<String>,
}

impl SandboxArtifact {
    pub fn from_config_blob(blob: &[u8]) -> io::Result<SandboxArtifact> {
        serde_json::from_slice(blob).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
}

/// The parsed `model` artifact (`kind: Model`) — application-layer; its config blob is mounted at `/etc/agent/model`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelArtifact {
    pub api_version: String,
    pub kind: String,
    pub metadata: ArtifactMetadata,
    #[serde(default)]
    pub mount: Option<Mount>,
    pub spec: ModelSpec,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ModelSpec {
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub parameters: Option<serde_json::Value>,
}

impl ModelArtifact {
    pub fn from_config_blob(blob: &[u8]) -> io::Result<ModelArtifact> {
        let artifact: ModelArtifact = serde_json::from_slice(blob)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        if artifact.kind != "Model" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected a Model artifact, got kind {:?}", artifact.kind),
            ));
        }
        if artifact.spec.provider.trim().is_empty() || artifact.spec.model.trim().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "model spec.provider and spec.model must not be empty",
            ));
        }
        Ok(artifact)
    }
}

/// The parsed `fileset` artifact (`kind: FileSet`) — application-layer; content lives in the OCI layers, mounted at the required `mount.path`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilesetArtifact {
    pub api_version: String,
    pub kind: String,
    pub metadata: ArtifactMetadata,
    pub mount: Mount,
}

impl FilesetArtifact {
    pub fn from_config_blob(blob: &[u8]) -> io::Result<FilesetArtifact> {
        let artifact: FilesetArtifact = serde_json::from_slice(blob)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        if artifact.kind != "FileSet" {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected a FileSet artifact, got kind {:?}", artifact.kind),
            ));
        }
        Ok(artifact)
    }
}

/// The envelope fields the resolver needs to mount any application-layer artifact (name + optional mount), without modelling each family's full spec.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct MountedArtifact {
    pub metadata: ArtifactMetadata,
    #[serde(default)]
    pub mount: Option<Mount>,
}

impl MountedArtifact {
    pub fn from_config_blob(blob: &[u8]) -> io::Result<MountedArtifact> {
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
        assert_eq!(
            Family::Model.artifact_type(),
            "application/vnd.lens.model.v1+json"
        );
        assert_eq!(
            Family::Fileset.config_media_type(),
            "application/vnd.lens.fileset.config.v1+json"
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
        // a stray `isolation` on the agent (now sandbox-only) is harmlessly ignored — the clean break
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

    #[test]
    fn sandbox_artifact_parses_supervisor_capabilities_and_base_image() {
        let blob = to_config_blob(
            "name: r\nisolation: microvm\nsupervisorVersion: 1.2.3\n\
             capabilities: [net]\nbaseImage: img@sha256:abc\n"
                .as_bytes(),
        )
        .unwrap();
        let s = SandboxArtifact::from_config_blob(&blob).unwrap();
        assert_eq!(s.supervisor_version.as_deref(), Some("1.2.3"));
        assert_eq!(s.capabilities, Some(vec!["net".to_string()]));
        assert_eq!(s.base_image.as_deref(), Some("img@sha256:abc"));
    }

    #[test]
    fn family_application_layer_split_and_default_mounts() {
        assert!(Family::Model.is_application_layer());
        assert!(Family::Tool.is_application_layer());
        assert!(Family::Knowledge.is_application_layer());
        assert!(Family::Fileset.is_application_layer());
        assert!(!Family::Agent.is_application_layer());
        assert!(!Family::Sandbox.is_application_layer());
        assert_eq!(
            Family::Model.default_mount_path("hermes"),
            Some("/etc/agent/model".to_string())
        );
        assert_eq!(
            Family::Tool.default_mount_path("fs"),
            Some("/etc/agent/tools/fs".to_string())
        );
        assert_eq!(
            Family::Knowledge.default_mount_path("rb"),
            Some("/etc/agent/knowledge/rb".to_string())
        );
        assert_eq!(Family::Fileset.default_mount_path("x"), None);
        assert_eq!(Family::Agent.default_mount_path("x"), None);
    }

    #[test]
    fn mount_parses_path_and_optional_read_only() {
        let m: Mount = serde_json::from_str(r#"{"path":"/etc/agent/model"}"#).unwrap();
        assert_eq!(m.path, "/etc/agent/model");
        assert_eq!(m.read_only, None);
        let m: Mount = serde_json::from_str(r#"{"path":"/p","readOnly":true}"#).unwrap();
        assert_eq!(m.read_only, Some(true));
    }

    #[test]
    fn component_ref_parses_optional_digest() {
        let bare: ComponentRef = serde_json::from_str(r#"{"ref":"a/b:v1"}"#).unwrap();
        assert_eq!(bare.reference, "a/b:v1");
        assert_eq!(bare.digest, None);
        let pinned: ComponentRef =
            serde_json::from_str(r#"{"ref":"a/b:v1","digest":"sha256:abc"}"#).unwrap();
        assert_eq!(pinned.digest.as_deref(), Some("sha256:abc"));
    }

    #[test]
    fn components_parses_a_single_model_ref() {
        let blob = to_config_blob(
            "apiVersion: lens.dev/v1alpha1\nkind: AgentSystem\n\
             metadata:\n  name: sys\ncomponents:\n  \
             agents:\n    - { ref: org/acme/agents/a:v1 }\n  \
             model:\n    ref: org/acme/models/m:v1\n"
                .as_bytes(),
        )
        .unwrap();
        let bundle = BundleArtifact::from_config_blob(&blob).unwrap();
        assert_eq!(
            bundle.components.model.unwrap().reference,
            "org/acme/models/m:v1"
        );
    }

    fn model_blob(extra: &str) -> Vec<u8> {
        to_config_blob(
            format!(
                "apiVersion: lens.dev/v1alpha1\nkind: Model\n\
                 metadata:\n  name: some-model\n{extra}\
                 spec:\n  provider: anthropic\n  model: claude-sonnet-4-6\n"
            )
            .as_bytes(),
        )
        .unwrap()
    }

    #[test]
    fn model_artifact_parses_provider_model_and_default_mount() {
        let m = ModelArtifact::from_config_blob(&model_blob("")).unwrap();
        assert_eq!(m.spec.provider, "anthropic");
        assert_eq!(m.spec.model, "claude-sonnet-4-6");
        assert!(m.mount.is_none());
    }

    #[test]
    fn model_artifact_honors_an_explicit_mount_override() {
        let m = ModelArtifact::from_config_blob(&model_blob("mount:\n  path: /custom/model\n"))
            .unwrap();
        assert_eq!(m.mount.unwrap().path, "/custom/model");
    }

    #[test]
    fn model_artifact_rejects_empty_provider_and_wrong_kind() {
        let empty = to_config_blob(
            "apiVersion: lens.dev/v1alpha1\nkind: Model\nmetadata:\n  name: m\n\
             spec:\n  provider: ' '\n  model: x\n"
                .as_bytes(),
        )
        .unwrap();
        assert_eq!(
            ModelArtifact::from_config_blob(&empty).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        let wrong = to_config_blob(
            "apiVersion: lens.dev/v1alpha1\nkind: Agent\nmetadata:\n  name: m\n\
             spec:\n  provider: a\n  model: b\n"
                .as_bytes(),
        )
        .unwrap();
        assert!(
            format!("{}", ModelArtifact::from_config_blob(&wrong).unwrap_err()).contains("Model")
        );
    }

    #[test]
    fn model_artifact_rejects_malformed_json() {
        assert_eq!(
            ModelArtifact::from_config_blob(b"nope").unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn fileset_artifact_requires_a_mount() {
        let ok = to_config_blob(
            "apiVersion: lens.dev/v1alpha1\nkind: FileSet\nmetadata:\n  name: cfg\n\
             mount:\n  path: /opt/data/config.yaml\n  readOnly: false\nspec: {}\n"
                .as_bytes(),
        )
        .unwrap();
        let fs = FilesetArtifact::from_config_blob(&ok).unwrap();
        assert_eq!(fs.mount.path, "/opt/data/config.yaml");
        assert_eq!(fs.mount.read_only, Some(false));

        let no_mount = to_config_blob(
            "apiVersion: lens.dev/v1alpha1\nkind: FileSet\nmetadata:\n  name: cfg\n".as_bytes(),
        )
        .unwrap();
        assert_eq!(
            FilesetArtifact::from_config_blob(&no_mount)
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn fileset_artifact_rejects_wrong_kind() {
        let blob = to_config_blob(
            "apiVersion: lens.dev/v1alpha1\nkind: Tool\nmetadata:\n  name: t\n\
             mount:\n  path: /x\n"
                .as_bytes(),
        )
        .unwrap();
        assert!(
            format!("{}", FilesetArtifact::from_config_blob(&blob).unwrap_err())
                .contains("FileSet")
        );
    }

    #[test]
    fn mounted_artifact_reads_name_and_optional_mount() {
        let tool = to_config_blob(
            "apiVersion: lens.dev/v1alpha1\nkind: Tool\nmetadata:\n  name: filesystem\n\
             spec:\n  kind: mcp\n"
                .as_bytes(),
        )
        .unwrap();
        let m = MountedArtifact::from_config_blob(&tool).unwrap();
        assert_eq!(m.metadata.name, "filesystem");
        assert!(m.mount.is_none());

        let knowledge = to_config_blob(
            "apiVersion: lens.dev/v1alpha1\nkind: Knowledge\nmetadata:\n  name: rb\n\
             mount:\n  path: /custom/kb\nspec:\n  format: runbook\n"
                .as_bytes(),
        )
        .unwrap();
        let m = MountedArtifact::from_config_blob(&knowledge).unwrap();
        assert_eq!(m.mount.unwrap().path, "/custom/kb");
    }
}
