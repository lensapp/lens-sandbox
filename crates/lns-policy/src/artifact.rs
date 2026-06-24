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
#[serde(rename_all = "camelCase")]
pub struct AgentResources {
    #[serde(default)]
    pub cpus: Option<u32>,
    #[serde(default)]
    pub memory_mib: Option<u32>,
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
    pub resources: Option<AgentResources>,
    #[serde(default)]
    pub ports: Vec<PortMapping>,
    #[serde(default)]
    pub volumes: Vec<VolumeMapping>,
    #[serde(default)]
    pub credentials: Vec<CredentialRef>,
    #[serde(default)]
    pub mcp: Option<McpInjection>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpInjection {
    pub config_path: String,
    #[serde(default)]
    pub format: McpConfigFormat,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum McpConfigFormat {
    #[default]
    McpServers,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolKind {
    Mcp,
    Api,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum McpTransport {
    Stdio,
    Sse,
    Http,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpSpec {
    pub transport: McpTransport,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub url: Option<String>,
}

impl McpSpec {
    fn connection_error(&self) -> Option<&'static str> {
        let blank = |s: &Option<String>| s.as_deref().unwrap_or_default().trim().is_empty();
        match self.transport {
            McpTransport::Stdio if blank(&self.command) => {
                Some("a stdio mcp tool requires `mcp.command`")
            }
            McpTransport::Sse | McpTransport::Http if blank(&self.url) => {
                Some("an http/sse mcp tool requires `mcp.url`")
            }
            _ => None,
        }
    }
}

/// The parsed `tool` artifact — an MCP server or an API the agent may use; mirrors the registry's tool schema.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolArtifact {
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
    pub kind: ToolKind,
    #[serde(default)]
    pub mcp: Option<McpSpec>,
    #[serde(default)]
    pub required_integrations: Vec<String>,
}

impl ToolArtifact {
    pub fn from_config_blob(blob: &[u8]) -> io::Result<ToolArtifact> {
        let tool: ToolArtifact = serde_json::from_slice(blob)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        if tool.name.trim().is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "tool name must not be empty",
            ));
        }
        match tool.kind {
            ToolKind::Mcp => {
                let mcp = tool.mcp.as_ref().ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        "an mcp tool requires an `mcp` block",
                    )
                })?;
                if let Some(msg) = mcp.connection_error() {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, msg));
                }
            }
            ToolKind::Api if tool.remote_url().unwrap_or_default().trim().is_empty() => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "an api tool requires a url",
                ));
            }
            ToolKind::Api => {}
        }
        Ok(tool)
    }

    /// The network endpoint a remote tool is reached at (folds into the run policy); `None` for a launched stdio server.
    pub fn remote_url(&self) -> Option<&str> {
        let url = self.mcp.as_ref().and_then(|m| m.url.as_deref());
        match self.kind {
            ToolKind::Api => url,
            ToolKind::Mcp => match self.mcp.as_ref().map(|m| m.transport) {
                Some(McpTransport::Sse | McpTransport::Http) => url,
                _ => None,
            },
        }
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
    fn agent_artifact_parses_resources_user_ports_and_volumes() {
        let blob = agent_blob(
            "apiVersion: lens.dev/v1alpha1\nkind: Agent\n\
             metadata:\n  name: some-agent\n\
             spec:\n  image: some-image:1\n  user: runner\n  \
             resources:\n    cpus: 2\n    memoryMib: 3072\n  \
             ports:\n    - { host: 9119, container: 9119 }\n  \
             volumes:\n    - { name: somedata, target: /opt/data }\n",
        );
        let agent = AgentArtifact::from_config_blob(&blob).unwrap();
        assert_eq!(agent.spec.user.as_deref(), Some("runner"));
        let resources = agent.spec.resources.unwrap();
        assert_eq!(resources.cpus, Some(2));
        assert_eq!(resources.memory_mib, Some(3072));
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
        assert!(agent.spec.resources.is_none());
        assert!(agent.spec.ports.is_empty());
        assert!(agent.spec.volumes.is_empty());
        assert!(agent.spec.mcp.is_none());
    }

    #[test]
    fn agent_artifact_parses_the_mcp_injection_point_with_a_default_format() {
        let blob = agent_blob(
            "apiVersion: lens.dev/v1alpha1\nkind: Agent\n\
             metadata:\n  name: some-agent\n\
             spec:\n  image: some-image:1\n  mcp:\n    configPath: /home/agent/.mcp.json\n",
        );
        let mcp = AgentArtifact::from_config_blob(&blob)
            .unwrap()
            .spec
            .mcp
            .unwrap();
        assert_eq!(mcp.config_path, "/home/agent/.mcp.json");
        assert_eq!(mcp.format, McpConfigFormat::McpServers);
    }

    #[test]
    fn agent_artifact_parses_an_explicit_mcp_format() {
        let blob = agent_blob(
            "apiVersion: lens.dev/v1alpha1\nkind: Agent\n\
             metadata:\n  name: some-agent\n\
             spec:\n  image: some-image:1\n  mcp:\n    \
             configPath: /etc/mcp.json\n    format: mcpServers\n",
        );
        let mcp = AgentArtifact::from_config_blob(&blob)
            .unwrap()
            .spec
            .mcp
            .unwrap();
        assert_eq!(mcp.format, McpConfigFormat::McpServers);
    }

    fn tool_blob(yaml: &str) -> Vec<u8> {
        to_config_blob(yaml.as_bytes()).unwrap()
    }

    #[test]
    fn tool_artifact_parses_a_stdio_mcp_server() {
        let blob = tool_blob(
            "name: fmt\nversion: '1'\nkind: mcp\n\
             mcp:\n  transport: stdio\n  command: some-mcp-server\n  args: ['--flag']\n\
             requiredIntegrations: [some-oauth]\n",
        );
        let tool = ToolArtifact::from_config_blob(&blob).unwrap();
        assert_eq!(tool.name, "fmt");
        assert_eq!(tool.kind, ToolKind::Mcp);
        let mcp = tool.mcp.as_ref().unwrap();
        assert_eq!(mcp.transport, McpTransport::Stdio);
        assert_eq!(mcp.command.as_deref(), Some("some-mcp-server"));
        assert_eq!(mcp.args, vec!["--flag".to_string()]);
        assert_eq!(tool.required_integrations, vec!["some-oauth".to_string()]);
        assert_eq!(tool.remote_url(), None);
    }

    #[test]
    fn tool_artifact_parses_an_http_mcp_server_as_remote() {
        let blob = tool_blob(
            "name: search\nkind: mcp\n\
             mcp:\n  transport: http\n  url: https://api.some-provider.example/mcp\n",
        );
        let tool = ToolArtifact::from_config_blob(&blob).unwrap();
        assert_eq!(
            tool.remote_url(),
            Some("https://api.some-provider.example/mcp")
        );
    }

    #[test]
    fn tool_artifact_parses_an_sse_mcp_server_as_remote() {
        let blob = tool_blob(
            "name: events\nkind: mcp\n\
             mcp:\n  transport: sse\n  url: https://api.some-provider.example/sse\n",
        );
        let tool = ToolArtifact::from_config_blob(&blob).unwrap();
        assert_eq!(
            tool.remote_url(),
            Some("https://api.some-provider.example/sse")
        );
    }

    #[test]
    fn tool_artifact_parses_an_api_tool_as_remote() {
        let blob = tool_blob(
            "name: weather\nkind: api\n\
             mcp:\n  transport: http\n  url: https://api.example.test/v1\n",
        );
        let tool = ToolArtifact::from_config_blob(&blob).unwrap();
        assert_eq!(tool.kind, ToolKind::Api);
        assert_eq!(tool.remote_url(), Some("https://api.example.test/v1"));
    }

    #[test]
    fn tool_artifact_rejects_an_empty_name() {
        let err =
            ToolArtifact::from_config_blob(&tool_blob("name: '  '\nkind: api\n")).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(format!("{err}").contains("name"), "got: {err}");
    }

    #[test]
    fn tool_artifact_rejects_an_mcp_tool_without_an_mcp_block() {
        let err = ToolArtifact::from_config_blob(&tool_blob("name: x\nkind: mcp\n")).unwrap_err();
        assert!(format!("{err}").contains("mcp` block"), "got: {err}");
    }

    #[test]
    fn tool_artifact_rejects_a_stdio_tool_without_a_command() {
        let blob = tool_blob("name: x\nkind: mcp\nmcp:\n  transport: stdio\n");
        let err = ToolArtifact::from_config_blob(&blob).unwrap_err();
        assert!(format!("{err}").contains("command"), "got: {err}");
    }

    #[test]
    fn tool_artifact_rejects_a_remote_mcp_tool_without_a_url() {
        let blob = tool_blob("name: x\nkind: mcp\nmcp:\n  transport: http\n");
        let err = ToolArtifact::from_config_blob(&blob).unwrap_err();
        assert!(format!("{err}").contains("url"), "got: {err}");
    }

    #[test]
    fn tool_artifact_rejects_an_api_tool_without_a_url() {
        let err = ToolArtifact::from_config_blob(&tool_blob("name: x\nkind: api\n")).unwrap_err();
        assert!(format!("{err}").contains("url"), "got: {err}");
    }

    #[test]
    fn tool_artifact_rejects_malformed_json() {
        let err = ToolArtifact::from_config_blob(b"not json").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
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
}
