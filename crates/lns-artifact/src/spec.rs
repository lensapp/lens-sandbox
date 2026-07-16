use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::BTreeMap;

pub const API_VERSION: &str = "lens.dev/v1alpha1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Runtime,
    Application,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Sandbox,
    FileSet,
    Policy,
    Integration,
}

const ALL_KINDS: [Kind; 4] = [
    Kind::Sandbox,
    Kind::FileSet,
    Kind::Policy,
    Kind::Integration,
];

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Sandbox => "Sandbox",
            Kind::FileSet => "FileSet",
            Kind::Policy => "Policy",
            Kind::Integration => "Integration",
        }
    }

    pub fn family(self) -> &'static str {
        match self {
            Kind::Sandbox => "sandbox",
            Kind::FileSet => "fileset",
            Kind::Policy => "policy",
            Kind::Integration => "integration",
        }
    }

    pub fn class(self) -> Class {
        match self {
            Kind::FileSet => Class::Application,
            _ => Class::Runtime,
        }
    }

    pub fn artifact_type(self) -> String {
        format!("application/vnd.lens.{}.v1+json", self.family())
    }

    pub fn config_media_type(self) -> String {
        format!("application/vnd.lens.{}.config.v1+json", self.family())
    }

    pub fn from_artifact_type(media_type: &str) -> Option<Kind> {
        ALL_KINDS
            .into_iter()
            .find(|k| k.artifact_type() == media_type)
    }

    pub fn from_config_media_type(media_type: &str) -> Option<Kind> {
        ALL_KINDS
            .into_iter()
            .find(|k| k.config_media_type() == media_type)
    }

    pub fn from_kind_str(kind: &str) -> Option<Kind> {
        ALL_KINDS.into_iter().find(|k| k.as_str() == kind)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ArtifactRef {
    #[serde(rename = "ref")]
    pub reference: String,
    #[serde(default)]
    pub digest: Option<String>,
}

impl ArtifactRef {
    pub fn validate(&self) -> Result<()> {
        if self.reference.is_empty() {
            bail!("artifact ref must not be empty");
        }
        if let Some(digest) = &self.digest
            && !is_sha256_digest(digest)
        {
            bail!("artifact ref digest {digest} is not a sha256 digest");
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Metadata {
    pub name: String,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Mount {
    pub path: String,
    #[serde(rename = "readOnly", default)]
    pub read_only: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Isolation {
    Microvm,
    Container,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum Quantity {
    Int(i64),
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
pub struct Resources {
    #[serde(default)]
    pub cpu: Option<Quantity>,
    #[serde(default)]
    pub memory: Option<Quantity>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SandboxSpec {
    #[serde(rename = "baseImage", default)]
    pub base_image: Option<String>,
    pub isolation: Isolation,
    #[serde(default)]
    pub resources: Option<Resources>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(rename = "supervisorVersion", default)]
    pub supervisor_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CredentialSlot {
    pub name: String,
    pub env: String,
    #[serde(default)]
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Port {
    #[serde(default)]
    pub host: Option<i64>,
    pub container: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sandbox {
    pub metadata: Metadata,
    pub spec: SandboxSpec,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
pub struct PolicySpec {
    #[serde(default)]
    pub integrations: Vec<ArtifactRef>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSet {
    pub metadata: Metadata,
    pub mount: Mount,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    pub metadata: Metadata,
    pub spec: PolicySpec,
}

#[derive(Deserialize)]
struct Doc {
    #[serde(rename = "apiVersion", default)]
    api_version: String,
    #[serde(default)]
    kind: String,
    metadata: Metadata,
    #[serde(default)]
    mount: Option<Mount>,
    spec: serde_json::Value,
}

fn parse_doc(config_json: &[u8], expected: Kind) -> Result<Doc> {
    let doc: Doc = serde_json::from_slice(config_json).context("parsing artifact config")?;
    if doc.api_version != API_VERSION {
        bail!(
            "unexpected apiVersion {:?}; expected {API_VERSION}",
            doc.api_version
        );
    }
    if doc.kind != expected.as_str() {
        bail!(
            "expected kind {} but config declares {:?}",
            expected.as_str(),
            doc.kind
        );
    }
    if !is_valid_name(&doc.metadata.name) {
        bail!("invalid metadata.name {:?}", doc.metadata.name);
    }
    if expected.class() == Class::Runtime && doc.mount.is_some() {
        bail!(
            "{} is a runtime-layer artifact and must not carry a mount",
            expected.as_str()
        );
    }
    Ok(doc)
}

pub fn parse_sandbox(config_json: &[u8]) -> Result<Sandbox> {
    let doc = parse_doc(config_json, Kind::Sandbox)?;
    let spec: SandboxSpec = serde_json::from_value(doc.spec).context("parsing sandbox spec")?;
    if spec.isolation != Isolation::Microvm {
        bail!("sandbox isolation must be microvm; lns runs workloads only inside a microVM");
    }
    let Some(image) = &spec.base_image else {
        bail!("microvm sandbox must carry a baseImage; the workload rootfs lives on the sandbox");
    };
    if !is_digest_pinned_image(image) {
        bail!("sandbox baseImage {image} must be digest-pinned (…@sha256:<64 hex>)");
    }
    Ok(Sandbox {
        metadata: doc.metadata,
        spec,
    })
}

pub fn validate_mount_path(path: &str) -> Result<()> {
    if path.is_empty() {
        bail!("mount path must not be empty");
    }
    if !path.starts_with('/') {
        bail!("mount path {path} must be absolute (start with `/`)");
    }
    if path.split('/').any(|segment| segment == "..") {
        bail!("mount path {path} must not contain a `..` segment");
    }
    Ok(())
}

pub fn parse_fileset(config_json: &[u8]) -> Result<FileSet> {
    let doc = parse_doc(config_json, Kind::FileSet)?;
    let Some(mount) = doc.mount else {
        bail!("FileSet is an application-layer artifact and requires a mount");
    };
    validate_mount_path(&mount.path)?;
    Ok(FileSet {
        metadata: doc.metadata,
        mount,
    })
}

pub fn parse_policy(config_json: &[u8]) -> Result<Policy> {
    let doc = parse_doc(config_json, Kind::Policy)?;
    let spec: PolicySpec = serde_json::from_value(doc.spec).context("parsing policy spec")?;
    for integration in &spec.integrations {
        integration.validate()?;
    }
    Ok(Policy {
        metadata: doc.metadata,
        spec,
    })
}

pub fn validate_envelope(config_json: &[u8], expected: Kind) -> Result<()> {
    parse_doc(config_json, expected)?;
    Ok(())
}

#[derive(Deserialize)]
struct KindOnly {
    #[serde(default)]
    kind: String,
}

/// The kind a document declares, mapped to `Kind` (errors on an unknown or unparseable kind).
pub fn read_kind(config_json: &[u8]) -> Result<Kind> {
    let KindOnly { kind } = serde_json::from_slice(config_json).context("parsing artifact kind")?;
    Kind::from_kind_str(&kind).ok_or_else(|| anyhow::anyhow!("unknown artifact kind {kind:?}"))
}

/// Run the schema + cross-field guards for whatever kind the document declares.
pub fn validate_any(config_json: &[u8]) -> Result<()> {
    match read_kind(config_json)? {
        Kind::Sandbox => {
            parse_sandbox(config_json)?;
        }
        Kind::FileSet => {
            parse_fileset(config_json)?;
        }
        Kind::Policy => {
            parse_policy(config_json)?;
        }
        Kind::Integration => {
            validate_envelope(config_json, Kind::Integration)?;
        }
    }
    Ok(())
}

pub(crate) fn is_valid_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    if bytes.is_empty() || bytes.len() > 63 {
        return false;
    }
    let is_alnum = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    if !is_alnum(bytes[0]) || !is_alnum(bytes[bytes.len() - 1]) {
        return false;
    }
    bytes.iter().all(|&b| is_alnum(b) || b == b'-')
}

fn is_sha256_digest(value: &str) -> bool {
    match value.strip_prefix("sha256:") {
        Some(hex) => {
            hex.len() == 64
                && hex
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        }
        None => false,
    }
}

fn is_digest_pinned_image(reference: &str) -> bool {
    match reference.rsplit_once('@') {
        Some((_, digest)) => is_sha256_digest(digest),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_maps_family_class_and_media_types() {
        assert_eq!(Kind::Sandbox.family(), "sandbox");
        assert_eq!(Kind::Sandbox.as_str(), "Sandbox");
        assert_eq!(Kind::Sandbox.class(), Class::Runtime);
        assert_eq!(Kind::FileSet.class(), Class::Application);
        assert_eq!(
            Kind::Sandbox.artifact_type(),
            "application/vnd.lens.sandbox.v1+json"
        );
        assert_eq!(
            Kind::Sandbox.config_media_type(),
            "application/vnd.lens.sandbox.config.v1+json"
        );
        for kind in ALL_KINDS {
            let _ = kind.as_str();
            let _ = kind.family();
        }
    }

    #[test]
    fn kind_round_trips_through_its_media_types() {
        for kind in ALL_KINDS {
            assert_eq!(Kind::from_artifact_type(&kind.artifact_type()), Some(kind));
            assert_eq!(
                Kind::from_config_media_type(&kind.config_media_type()),
                Some(kind)
            );
        }
        assert_eq!(Kind::from_artifact_type("application/vnd.unknown"), None);
        assert_eq!(
            Kind::from_config_media_type("application/vnd.unknown"),
            None
        );
    }

    #[test]
    fn artifact_ref_validate_accepts_bare_ref_and_pinned_digest() {
        ArtifactRef {
            reference: "reg/x:1".into(),
            digest: None,
        }
        .validate()
        .unwrap();
        ArtifactRef {
            reference: "reg/x:1".into(),
            digest: Some(format!("sha256:{}", "a".repeat(64))),
        }
        .validate()
        .unwrap();
    }

    #[test]
    fn artifact_ref_validate_rejects_empty_ref_and_bad_digest() {
        let err = ArtifactRef {
            reference: String::new(),
            digest: None,
        }
        .validate()
        .unwrap_err();
        assert!(format!("{err:#}").contains("must not be empty"));
        let err = ArtifactRef {
            reference: "reg/x:1".into(),
            digest: Some("sha256:nothex".into()),
        }
        .validate()
        .unwrap_err();
        assert!(format!("{err:#}").contains("not a sha256 digest"));
    }

    #[test]
    fn is_valid_name_matches_the_dns_label_pattern() {
        assert!(is_valid_name("a"));
        assert!(is_valid_name("some-agent"));
        assert!(is_valid_name("a1"));
        assert!(!is_valid_name(""));
        assert!(!is_valid_name(&"a".repeat(64)));
        assert!(!is_valid_name("-a"));
        assert!(!is_valid_name("a-"));
        assert!(!is_valid_name("Ab"));
        assert!(!is_valid_name("a_b"));
    }

    #[test]
    fn digest_helpers_reject_malformed_values() {
        assert!(is_sha256_digest(&format!("sha256:{}", "0".repeat(64))));
        assert!(!is_sha256_digest("sha256:short"));
        assert!(!is_sha256_digest("md5:whatever"));
        assert!(is_digest_pinned_image(&format!(
            "reg/base@sha256:{}",
            "f".repeat(64)
        )));
        assert!(!is_digest_pinned_image("reg/base:1"));
    }

    #[test]
    fn parse_doc_rejects_wrong_api_version_kind_name_and_mount() {
        let bad_api = br#"{"apiVersion":"v0","kind":"Policy","metadata":{"name":"x"},"spec":{}}"#;
        assert!(format!("{:#}", parse_policy(bad_api).unwrap_err()).contains("apiVersion"));
        let bad_kind = br#"{"apiVersion":"lens.dev/v1alpha1","kind":"Sandbox","metadata":{"name":"x"},"spec":{}}"#;
        assert!(format!("{:#}", parse_policy(bad_kind).unwrap_err()).contains("expected kind"));
        let bad_name = br#"{"apiVersion":"lens.dev/v1alpha1","kind":"Policy","metadata":{"name":"-bad"},"spec":{}}"#;
        assert!(format!("{:#}", parse_policy(bad_name).unwrap_err()).contains("metadata.name"));
        let runtime_mount = br#"{"apiVersion":"lens.dev/v1alpha1","kind":"Policy","metadata":{"name":"x"},"mount":{"path":"/x"},"spec":{}}"#;
        assert!(
            format!("{:#}", parse_policy(runtime_mount).unwrap_err())
                .contains("must not carry a mount")
        );
    }

    #[test]
    fn parse_doc_rejects_malformed_json() {
        assert!(
            format!("{:#}", parse_policy(b"not json").unwrap_err()).contains("parsing artifact")
        );
    }

    #[test]
    fn parse_sandbox_accepts_a_digest_pinned_base_image() {
        let json = format!(
            r#"{{"apiVersion":"lens.dev/v1alpha1","kind":"Sandbox","metadata":{{"name":"some-sandbox"}},"spec":{{"isolation":"microvm","baseImage":"reg/base@sha256:{}","resources":{{"cpu":2,"memory":"1Gi"}}}}}}"#,
            "a".repeat(64)
        );
        let sandbox = parse_sandbox(json.as_bytes()).unwrap();
        assert_eq!(sandbox.spec.isolation, Isolation::Microvm);
        assert!(sandbox.spec.base_image.is_some());
        assert_eq!(sandbox.spec.resources.unwrap().cpu, Some(Quantity::Int(2)));
    }

    #[test]
    fn parse_sandbox_rejects_a_floating_base_image() {
        let json = br#"{"apiVersion":"lens.dev/v1alpha1","kind":"Sandbox","metadata":{"name":"some-sandbox"},"spec":{"isolation":"microvm","baseImage":"reg/base:1"}}"#;
        let err = parse_sandbox(json).unwrap_err();
        assert!(format!("{err:#}").contains("digest-pinned"));
    }

    #[test]
    fn parse_sandbox_rejects_a_non_microvm_isolation() {
        let json = format!(
            r#"{{"apiVersion":"lens.dev/v1alpha1","kind":"Sandbox","metadata":{{"name":"some-sandbox"}},"spec":{{"isolation":"container","baseImage":"reg/base@sha256:{}"}}}}"#,
            "a".repeat(64)
        );
        let err = parse_sandbox(json.as_bytes()).unwrap_err();
        assert!(
            format!("{err:#}").contains("isolation must be microvm"),
            "lns is microVM-only, so a container sandbox must be refused: {err:#}"
        );
    }

    #[test]
    fn parse_sandbox_requires_a_base_image() {
        let json = br#"{"apiVersion":"lens.dev/v1alpha1","kind":"Sandbox","metadata":{"name":"some-sandbox"},"spec":{"isolation":"microvm"}}"#;
        let err = parse_sandbox(json).unwrap_err();
        assert!(
            format!("{err:#}").contains("must carry a baseImage"),
            "a rootless microvm sandbox has nothing to boot; got: {err:#}"
        );
    }

    #[test]
    fn parse_fileset_requires_a_mount() {
        let with_mount = br#"{"apiVersion":"lens.dev/v1alpha1","kind":"FileSet","metadata":{"name":"skills"},"mount":{"path":"/root/.some-agent/skills","readOnly":true},"spec":{}}"#;
        let fileset = parse_fileset(with_mount).unwrap();
        assert_eq!(fileset.mount.path, "/root/.some-agent/skills");
        assert_eq!(fileset.mount.read_only, Some(true));

        let no_mount = br#"{"apiVersion":"lens.dev/v1alpha1","kind":"FileSet","metadata":{"name":"skills"},"spec":{}}"#;
        let err = parse_fileset(no_mount).unwrap_err();
        assert!(format!("{err:#}").contains("requires a mount"));
    }

    #[test]
    fn parse_fileset_rejects_a_traversing_or_relative_mount_path() {
        let escaping = br#"{"apiVersion":"lens.dev/v1alpha1","kind":"FileSet","metadata":{"name":"skills"},"mount":{"path":"/root/../../etc"},"spec":{}}"#;
        let err = parse_fileset(escaping).unwrap_err();
        assert!(
            format!("{err:#}").contains("`..` segment"),
            "a fileset must not mount a traversing path: {err:#}"
        );
        let relative = br#"{"apiVersion":"lens.dev/v1alpha1","kind":"FileSet","metadata":{"name":"skills"},"mount":{"path":"root/skills"},"spec":{}}"#;
        let err = parse_fileset(relative).unwrap_err();
        assert!(
            format!("{err:#}").contains("must be absolute"),
            "got: {err:#}"
        );
    }

    #[test]
    fn validate_mount_path_rejects_empty_relative_and_traversing_paths() {
        assert!(validate_mount_path("/root/.some-agent/skills").is_ok());
        assert!(
            format!("{:#}", validate_mount_path("").unwrap_err()).contains("must not be empty")
        );
        assert!(
            format!("{:#}", validate_mount_path("relative").unwrap_err())
                .contains("must be absolute")
        );
        assert!(
            format!("{:#}", validate_mount_path("/a/../b").unwrap_err()).contains("`..` segment")
        );
    }

    #[test]
    fn parse_policy_surfaces_the_integration_refs_it_declares() {
        let json = br#"{"apiVersion":"lens.dev/v1alpha1","kind":"Policy","metadata":{"name":"some-policy"},"spec":{"network":{"defaultVerdict":"ask"},"integrations":[{"ref":"reg/some-integration:1"}]}}"#;
        let policy = parse_policy(json).unwrap();
        assert_eq!(policy.metadata.name, "some-policy");
        assert_eq!(policy.spec.integrations.len(), 1);
        assert_eq!(
            policy.spec.integrations[0].reference,
            "reg/some-integration:1"
        );
    }

    #[test]
    fn parse_policy_without_integrations_is_allowed() {
        let json = br#"{"apiVersion":"lens.dev/v1alpha1","kind":"Policy","metadata":{"name":"some-policy"},"spec":{"network":{"defaultVerdict":"ask"}}}"#;
        let policy = parse_policy(json).unwrap();
        assert!(policy.spec.integrations.is_empty());
    }

    #[test]
    fn parse_policy_rejects_an_integration_ref_with_a_bad_digest() {
        let json = br#"{"apiVersion":"lens.dev/v1alpha1","kind":"Policy","metadata":{"name":"some-policy"},"spec":{"integrations":[{"ref":"reg/some-integration:1","digest":"sha256:bad"}]}}"#;
        let err = parse_policy(json).unwrap_err();
        assert!(format!("{err:#}").contains("not a sha256 digest"));
    }

    #[test]
    fn parse_policy_rejects_a_mislabeled_envelope() {
        let json = br#"{"apiVersion":"lens.dev/v1alpha1","kind":"Sandbox","metadata":{"name":"x"},"spec":{}}"#;
        let err = parse_policy(json).unwrap_err();
        assert!(format!("{err:#}").contains("expected kind"), "got: {err:#}");
    }

    #[test]
    fn from_kind_str_round_trips_and_rejects_unknown() {
        for kind in ALL_KINDS {
            assert_eq!(Kind::from_kind_str(kind.as_str()), Some(kind));
        }
        assert_eq!(Kind::from_kind_str("Sorcery"), None);
    }

    #[test]
    fn validate_any_dispatches_to_each_kind_parser() {
        let base = format!("reg/base@sha256:{}", "a".repeat(64));
        let sandbox = format!(
            r#"{{"apiVersion":"lens.dev/v1alpha1","kind":"Sandbox","metadata":{{"name":"some-sandbox"}},"spec":{{"isolation":"microvm","baseImage":"{base}"}}}}"#
        );
        validate_any(sandbox.as_bytes()).unwrap();
        validate_any(br#"{"apiVersion":"lens.dev/v1alpha1","kind":"FileSet","metadata":{"name":"skills"},"mount":{"path":"/root/.some-agent/skills"},"spec":{}}"#).unwrap();
        validate_any(br#"{"apiVersion":"lens.dev/v1alpha1","kind":"Policy","metadata":{"name":"some-policy"},"spec":{}}"#).unwrap();
        validate_any(br#"{"apiVersion":"lens.dev/v1alpha1","kind":"Integration","metadata":{"name":"some-integration"},"spec":{"authKind":"credential"}}"#).unwrap();
    }

    #[test]
    fn validate_any_rejects_an_unknown_kind_and_unparseable_json() {
        let unknown = br#"{"apiVersion":"lens.dev/v1alpha1","kind":"Sorcery","metadata":{"name":"x"},"spec":{}}"#;
        assert!(
            format!("{:#}", validate_any(unknown).unwrap_err()).contains("unknown artifact kind")
        );
        assert!(
            format!("{:#}", validate_any(b"nope").unwrap_err()).contains("parsing artifact kind")
        );
    }
}
