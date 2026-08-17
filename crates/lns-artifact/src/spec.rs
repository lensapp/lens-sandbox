use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Sandbox,
    Mixin,
}

const ALL_KINDS: [Kind; 2] = [Kind::Sandbox, Kind::Mixin];

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Sandbox => "sandbox",
            Kind::Mixin => "mixin",
        }
    }

    pub fn artifact_type(self) -> String {
        format!("application/vnd.lens.{}.v1+json", self.as_str())
    }

    pub fn config_media_type(self) -> String {
        format!("application/vnd.lens.{}.config.v1+json", self.as_str())
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Quantity {
    Int(i64),
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Resources {
    #[serde(default)]
    pub cpu: Option<Quantity>,
    #[serde(default)]
    pub memory: Option<Quantity>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Port {
    #[serde(default)]
    pub host: Option<i64>,
    pub container: i64,
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
    fn a_kind_is_spelled_the_way_the_specification_writes_it() {
        for (kind, spelling) in [(Kind::Sandbox, "sandbox"), (Kind::Mixin, "mixin")] {
            assert_eq!(
                kind.as_str(),
                spelling,
                "docs/sandbox-spec.md writes every kind in lower case, and a document is refused when its kind does not match exactly"
            );
            assert_eq!(
                Kind::from_kind_str(spelling),
                Some(kind),
                "a document spelled the way the specification writes it has to parse"
            );
        }
    }

    #[test]
    fn every_kind_carries_the_media_types_the_specification_names() {
        for (kind, name, artifact_type, config_media_type) in [
            (
                Kind::Sandbox,
                "sandbox",
                "application/vnd.lens.sandbox.v1+json",
                "application/vnd.lens.sandbox.config.v1+json",
            ),
            (
                Kind::Mixin,
                "mixin",
                "application/vnd.lens.mixin.v1+json",
                "application/vnd.lens.mixin.config.v1+json",
            ),
        ] {
            assert_eq!(kind.as_str(), name);
            assert_eq!(kind.artifact_type(), artifact_type);
            assert_eq!(kind.config_media_type(), config_media_type);
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
    fn read_kind_rejects_a_malformed_document_and_an_unknown_kind() {
        assert!(
            format!("{:#}", read_kind(b"not json").unwrap_err()).contains("parsing artifact kind")
        );
        assert!(
            format!("{:#}", read_kind(br#"{"kind":"fileset"}"#).unwrap_err())
                .contains("unknown artifact kind"),
            "a fileset is not addressable on its own (docs/sandbox-spec.md §7), so a document declaring one has no reader"
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

    #[test]
    fn a_kind_the_specification_retired_is_addressable_by_nothing() {
        for retired in ["fileset", "policy", "integration"] {
            assert_eq!(
                Kind::from_kind_str(retired),
                None,
                "docs/sandbox-spec.md §7: nothing but a kit is addressable on its own"
            );
            assert_eq!(
                Kind::from_artifact_type(&format!("application/vnd.lens.{retired}.v1+json")),
                None
            );
            assert_eq!(
                Kind::from_config_media_type(&format!(
                    "application/vnd.lens.{retired}.config.v1+json"
                )),
                None
            );
        }
    }
}
