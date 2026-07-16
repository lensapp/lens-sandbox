use anyhow::{Context, Result, bail};
use lns_policy::NetworkPolicy;
use serde::Deserialize;
use std::collections::BTreeMap;

use crate::spec::{self, CredentialSlot, Metadata, Port, Resources, Volume};

pub const API_VERSION: &str = "lns.run/v1";
pub const KIND: &str = "Sandbox";

/// True when a document declares the user-facing `lns.run/v1` API group.
pub fn is_sandbox_definition(config_json: &[u8]) -> bool {
    #[derive(Deserialize)]
    struct ApiOnly {
        #[serde(rename = "apiVersion", default)]
        api_version: String,
    }
    serde_json::from_slice::<ApiOnly>(config_json)
        .map(|d| d.api_version == API_VERSION)
        .unwrap_or(false)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Definition {
    pub metadata: Metadata,
    pub spec: SandboxSpec,
}

/// The whole sandbox in one document: the base image plus its config, env, embedded network policy, mounts, and the integration ids it needs.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct SandboxSpec {
    #[serde(default)]
    pub image: String,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub resources: Option<Resources>,
    #[serde(default)]
    pub policy: NetworkPolicy,
    #[serde(default)]
    pub integrations: Vec<String>,
    #[serde(default)]
    pub credentials: Vec<CredentialSlot>,
    #[serde(default)]
    pub volumes: Vec<Volume>,
    #[serde(default)]
    pub ports: Vec<Port>,
}

#[derive(Deserialize)]
struct Doc {
    #[serde(rename = "apiVersion", default)]
    api_version: String,
    #[serde(default)]
    kind: String,
    metadata: Metadata,
    #[serde(default)]
    spec: SandboxSpec,
}

/// Parse and cross-field-validate a `lns.run/v1` sandbox definition, offline.
pub fn parse(config_json: &[u8]) -> Result<Definition> {
    let doc: Doc = serde_json::from_slice(config_json).context("parsing sandbox definition")?;
    if doc.api_version != API_VERSION {
        bail!(
            "unexpected apiVersion {:?}; expected {API_VERSION}",
            doc.api_version
        );
    }
    if doc.kind != KIND {
        bail!(
            "expected kind {KIND} but definition declares {:?}",
            doc.kind
        );
    }
    if !spec::is_valid_name(&doc.metadata.name) {
        bail!("invalid metadata.name {:?}", doc.metadata.name);
    }
    if doc.spec.image.trim().is_empty() {
        bail!("sandbox must carry an image; it is the base OCI image the sandbox runs");
    }
    for volume in &doc.spec.volumes {
        spec::validate_mount_path(&volume.target)
            .with_context(|| format!("volume {}", volume.name))?;
    }
    for integration in &doc.spec.integrations {
        if !spec::is_valid_name(integration) {
            bail!("invalid integration id {integration:?}");
        }
    }
    for slot in &doc.spec.credentials {
        if !spec::is_valid_name(&slot.name) {
            bail!("invalid credential integration id {:?}", slot.name);
        }
        if slot.env.trim().is_empty() {
            bail!(
                "credential {:?} must name the env var it is injected as",
                slot.name
            );
        }
    }
    for port in &doc.spec.ports {
        if !(1..=65535).contains(&port.container) {
            bail!(
                "sandbox port container {} is out of range (1-65535)",
                port.container
            );
        }
    }
    Ok(Definition {
        metadata: doc.metadata,
        spec: doc.spec,
    })
}

/// Schema + cross-field guards for a sandbox definition (the secret guard runs separately in `validate`).
pub fn validate(config_json: &[u8]) -> Result<()> {
    parse(config_json).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lns_policy::{Transport, Verdict};

    fn def_json(spec: &str) -> Vec<u8> {
        format!(
            r#"{{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{{"name":"hermes"}},"spec":{spec}}}"#
        )
        .into_bytes()
    }

    #[test]
    fn parse_reads_the_whole_flat_definition() {
        let json = def_json(
            r#"{"image":"ghcr.io/team/base:1","command":"agent --serve","env":{"MODE":"research"},"resources":{"cpu":2,"memory":"1Gi"},"policy":{"defaultVerdict":"deny","allowedRoutes":[{"match":"api.example.test","verdict":"allow"}]},"integrations":["some-provider"],"credentials":[{"name":"some-provider","env":"SOME_TOKEN"}],"volumes":[{"name":"home","target":"/root/.home"}],"ports":[{"container":8080}]}"#,
        );
        let def = parse(&json).unwrap();
        assert_eq!(def.metadata.name, "hermes");
        assert_eq!(def.spec.image, "ghcr.io/team/base:1");
        assert_eq!(def.spec.command.as_deref(), Some("agent --serve"));
        assert_eq!(
            def.spec.env.get("MODE").map(String::as_str),
            Some("research")
        );
        assert_eq!(def.spec.policy.default_verdict, Verdict::Deny);
        assert_eq!(def.spec.policy.allowed_routes.len(), 1);
        assert_eq!(def.spec.integrations, vec!["some-provider".to_string()]);
        assert_eq!(def.spec.credentials[0].env, "SOME_TOKEN");
        assert_eq!(def.spec.volumes[0].target, "/root/.home");
        assert_eq!(def.spec.ports[0].container, 8080);
    }

    #[test]
    fn parse_defaults_the_policy_to_ask_and_direct_when_omitted() {
        let def = parse(&def_json(r#"{"image":"ghcr.io/team/base:1"}"#)).unwrap();
        assert_eq!(def.spec.policy.default_verdict, Verdict::Ask);
        assert_eq!(def.spec.policy.default_transport, Transport::Direct);
        assert!(def.spec.integrations.is_empty());
    }

    #[test]
    fn parse_allows_a_floating_image_locally_pinning_is_a_publish_time_concern() {
        parse(&def_json(r#"{"image":"ghcr.io/team/base:1.4.0"}"#))
            .expect("a local definition may name a floating tag; publish pins it by digest");
    }

    #[test]
    fn parse_rejects_a_foreign_api_version() {
        let json = br#"{"apiVersion":"lens.dev/v1alpha1","kind":"Sandbox","metadata":{"name":"hermes"},"spec":{"image":"x:1"}}"#;
        let err = parse(json).unwrap_err();
        assert!(
            format!("{err:#}").contains("unexpected apiVersion"),
            "got: {err:#}"
        );
    }

    #[test]
    fn parse_rejects_a_wrong_kind() {
        let json = br#"{"apiVersion":"lns.run/v1","kind":"Policy","metadata":{"name":"hermes"},"spec":{"image":"x:1"}}"#;
        let err = parse(json).unwrap_err();
        assert!(
            format!("{err:#}").contains("expected kind Sandbox"),
            "got: {err:#}"
        );
    }

    #[test]
    fn parse_rejects_an_invalid_name() {
        let json = br#"{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{"name":"-bad"},"spec":{"image":"x:1"}}"#;
        let err = parse(json).unwrap_err();
        assert!(format!("{err:#}").contains("metadata.name"), "got: {err:#}");
    }

    #[test]
    fn parse_requires_a_non_empty_image() {
        let err = parse(&def_json(r#"{"image":""}"#)).unwrap_err();
        assert!(
            format!("{err:#}").contains("must carry an image"),
            "got: {err:#}"
        );
        let err = parse(&def_json(r#"{}"#)).unwrap_err();
        assert!(
            format!("{err:#}").contains("must carry an image"),
            "got: {err:#}"
        );
    }

    #[test]
    fn parse_rejects_a_traversing_volume_target() {
        let err = parse(&def_json(
            r#"{"image":"x:1","volumes":[{"name":"home","target":"/root/../etc"}]}"#,
        ))
        .unwrap_err();
        assert!(format!("{err:#}").contains("`..` segment"), "got: {err:#}");
    }

    #[test]
    fn parse_rejects_an_out_of_range_port() {
        let err = parse(&def_json(
            r#"{"image":"x:1","ports":[{"container":70000}]}"#,
        ))
        .unwrap_err();
        assert!(format!("{err:#}").contains("out of range"), "got: {err:#}");
    }

    #[test]
    fn parse_rejects_an_invalid_integration_id() {
        let err = parse(&def_json(r#"{"image":"x:1","integrations":["Bad_Id"]}"#)).unwrap_err();
        assert!(
            format!("{err:#}").contains("invalid integration id"),
            "got: {err:#}"
        );
    }

    #[test]
    fn parse_rejects_an_invalid_credential_slot_integration_id() {
        let err = parse(&def_json(
            r#"{"image":"x:1","credentials":[{"name":"Bad_Id","env":"SOME_TOKEN"}]}"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("invalid credential integration id"),
            "got: {err:#}"
        );
    }

    #[test]
    fn parse_rejects_a_credential_slot_with_no_env_target() {
        let err = parse(&def_json(
            r#"{"image":"x:1","credentials":[{"name":"some-provider","env":" "}]}"#,
        ))
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("env var it is injected as"),
            "got: {err:#}"
        );
    }

    #[test]
    fn parse_reads_a_required_credential_slot() {
        let def = parse(&def_json(
            r#"{"image":"x:1","credentials":[{"name":"some-provider","env":"SOME_TOKEN","required":true}]}"#,
        ))
        .unwrap();
        assert!(def.spec.credentials[0].required);
    }

    #[test]
    fn parse_surfaces_a_malformed_document_as_a_parse_error() {
        let no_metadata = br#"{"apiVersion":"lns.run/v1","kind":"Sandbox","spec":{"image":"x:1"}}"#;
        let err = parse(no_metadata).unwrap_err();
        assert!(
            format!("{err:#}").contains("parsing sandbox definition"),
            "got: {err:#}"
        );
    }

    #[test]
    fn is_sandbox_definition_detects_the_api_group() {
        assert!(is_sandbox_definition(&def_json(r#"{"image":"x:1"}"#)));
        assert!(!is_sandbox_definition(
            br#"{"apiVersion":"lens.dev/v1alpha1","kind":"Sandbox"}"#
        ));
        assert!(!is_sandbox_definition(b"not json"));
    }
}
