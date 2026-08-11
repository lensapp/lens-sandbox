use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::collections::BTreeMap;

pub const API_VERSION: &str = "lens.dev/v1alpha1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Sandbox,
    FileSet,
}

const ALL_KINDS: [Kind; 2] = [Kind::Sandbox, Kind::FileSet];

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Sandbox => "Sandbox",
            Kind::FileSet => "FileSet",
        }
    }

    pub fn family(self) -> &'static str {
        match self {
            Kind::Sandbox => "sandbox",
            Kind::FileSet => "fileset",
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
#[serde(deny_unknown_fields)]
pub struct Metadata {
    pub name: String,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mount {
    pub path: String,
    #[serde(rename = "readOnly", default)]
    pub read_only: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum Quantity {
    Int(i64),
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Resources {
    #[serde(default)]
    pub cpu: Option<Quantity>,
    #[serde(default)]
    pub memory: Option<Quantity>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialSlot {
    pub name: String,
    pub env: String,
    #[serde(default)]
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Port {
    #[serde(default)]
    pub host: Option<i64>,
    pub container: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileSet {
    pub metadata: Metadata,
    pub mount: Mount,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Doc {
    #[serde(rename = "apiVersion", default)]
    api_version: String,
    #[serde(default)]
    kind: String,
    metadata: Metadata,
    #[serde(default)]
    mount: Option<Mount>,
    // Every FileSet artifact ever published carries `spec: {}`, so the strict decoder has to know the key even though nothing reads it.
    #[serde(rename = "spec", default)]
    _spec: serde::de::IgnoredAny,
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
    Ok(doc)
}

pub fn validate_mount_path(path: &str) -> Result<()> {
    if path.is_empty() {
        bail!("mount path must not be empty");
    }
    if path.chars().any(char::is_control) {
        bail!("mount path {path:?} must not contain control characters");
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

/// True only for a `…@sha256:<64 lowercase hex>` reference; the strict pin check `lns push` shares so a malformed digest is rejected at publish, not pull.
pub fn is_digest_pinned_image(reference: &str) -> bool {
    match reference.rsplit_once('@') {
        Some((_, digest)) => is_sha256_digest(digest),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_maps_family_and_media_types() {
        assert_eq!(Kind::Sandbox.family(), "sandbox");
        assert_eq!(Kind::Sandbox.as_str(), "Sandbox");
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
    fn parse_doc_rejects_wrong_api_version_kind_and_name() {
        let bad_api = br#"{"apiVersion":"v0","kind":"FileSet","metadata":{"name":"skills"},"mount":{"path":"/skills"},"spec":{}}"#;
        assert!(format!("{:#}", parse_fileset(bad_api).unwrap_err()).contains("apiVersion"));
        let bad_kind = br#"{"apiVersion":"lens.dev/v1alpha1","kind":"Sandbox","metadata":{"name":"skills"},"mount":{"path":"/skills"},"spec":{}}"#;
        assert!(format!("{:#}", parse_fileset(bad_kind).unwrap_err()).contains("expected kind"));
        let bad_name = br#"{"apiVersion":"lens.dev/v1alpha1","kind":"FileSet","metadata":{"name":"-bad"},"mount":{"path":"/skills"},"spec":{}}"#;
        assert!(format!("{:#}", parse_fileset(bad_name).unwrap_err()).contains("metadata.name"));
    }

    #[test]
    fn parse_doc_rejects_malformed_json() {
        assert!(
            format!("{:#}", parse_fileset(b"not json").unwrap_err()).contains("parsing artifact")
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
    fn validate_mount_path_rejects_control_characters() {
        assert!(
            validate_mount_path("/.lens\n/etc").is_err(),
            "a newline in a mount path is line-injected into the newline-delimited /.lens/fileset-owned chown manifest and must be refused at the validation chokepoint"
        );
        assert!(
            validate_mount_path("/ok\u{7f}/x").is_err(),
            "any control character in a mount path must be refused"
        );
    }

    #[test]
    fn from_kind_str_round_trips_and_rejects_unknown() {
        for kind in ALL_KINDS {
            assert_eq!(Kind::from_kind_str(kind.as_str()), Some(kind));
        }
        assert_eq!(Kind::from_kind_str("Sorcery"), None);
    }
}
