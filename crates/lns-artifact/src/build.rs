use crate::spec;
use anyhow::{Context, Result};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const EMPTY_MEDIA_TYPE: &str = "application/vnd.oci.empty.v1+json";
const MANIFEST_MEDIA_TYPE: &str = "application/vnd.oci.image.manifest.v1+json";

/// A content-addressed blob to upload before its referencing manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blob {
    pub digest: String,
    pub media_type: String,
    pub data: Vec<u8>,
}

/// A built, digest-addressed OCI artifact ready to push: the manifest plus every blob it references.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltArtifact {
    pub artifact_type: String,
    pub manifest_media_type: String,
    pub manifest: Vec<u8>,
    pub manifest_digest: String,
    pub blobs: Vec<Blob>,
}

fn sha256_digest(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

/// Assemble a config-only OCI artifact (the standard shape for lens runtime kinds and bundles) from a validated manifest document.
pub fn build_artifact(doc: &[u8]) -> Result<BuiltArtifact> {
    spec::validate_any(doc).context("validating manifest before build")?;
    let kind = spec::read_kind(doc)?;

    let config_value: Value = serde_json::from_slice(doc).context("parsing manifest")?;
    let config_blob = serde_json::to_vec(&config_value).context("serialising config blob")?;
    let config_digest = sha256_digest(&config_blob);

    let empty_blob = b"{}".to_vec();
    let empty_digest = sha256_digest(&empty_blob);

    let manifest = json!({
        "schemaVersion": 2,
        "mediaType": MANIFEST_MEDIA_TYPE,
        "artifactType": kind.artifact_type(),
        "config": {
            "mediaType": kind.config_media_type(),
            "digest": config_digest,
            "size": config_blob.len(),
        },
        "layers": [{
            "mediaType": EMPTY_MEDIA_TYPE,
            "digest": empty_digest,
            "size": empty_blob.len(),
            "data": "e30=",
        }],
    });
    let manifest_bytes = serde_json::to_vec(&manifest).context("serialising manifest")?;
    let manifest_digest = sha256_digest(&manifest_bytes);

    Ok(BuiltArtifact {
        artifact_type: kind.artifact_type(),
        manifest_media_type: MANIFEST_MEDIA_TYPE.to_string(),
        manifest: manifest_bytes,
        manifest_digest,
        blobs: vec![
            Blob {
                digest: config_digest,
                media_type: kind.config_media_type(),
                data: config_blob,
            },
            Blob {
                digest: empty_digest,
                media_type: EMPTY_MEDIA_TYPE.to_string(),
                data: empty_blob,
            },
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox() -> Vec<u8> {
        format!(
            r#"{{"apiVersion":"lens.dev/v1alpha1","kind":"Sandbox","metadata":{{"name":"some-sandbox"}},"spec":{{"isolation":"microvm","baseImage":"reg/base@sha256:{}"}}}}"#,
            "a".repeat(64)
        )
        .into_bytes()
    }

    #[test]
    fn build_artifact_produces_a_config_only_manifest_with_matching_digests() {
        let built = build_artifact(&sandbox()).unwrap();
        assert_eq!(built.artifact_type, "application/vnd.lens.sandbox.v1+json");

        let manifest: Value = serde_json::from_slice(&built.manifest).unwrap();
        assert_eq!(
            manifest["artifactType"],
            "application/vnd.lens.sandbox.v1+json"
        );
        assert_eq!(
            manifest["config"]["mediaType"],
            "application/vnd.lens.sandbox.config.v1+json"
        );

        let config = built
            .blobs
            .iter()
            .find(|b| b.media_type.contains("sandbox"))
            .unwrap();
        assert_eq!(
            manifest["config"]["digest"].as_str().unwrap(),
            config.digest,
            "the manifest's config digest must address the config blob"
        );
        assert_eq!(
            manifest["config"]["size"].as_u64().unwrap() as usize,
            config.data.len()
        );
        assert_eq!(built.manifest_digest, sha256_digest(&built.manifest));
    }

    #[test]
    fn build_artifact_attaches_the_oci_empty_layer() {
        let built = build_artifact(&sandbox()).unwrap();
        let empty = built
            .blobs
            .iter()
            .find(|b| b.media_type == EMPTY_MEDIA_TYPE)
            .unwrap();
        assert_eq!(empty.data, b"{}");
        assert_eq!(
            empty.digest, "sha256:44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a",
            "the empty layer must be the canonical OCI empty descriptor"
        );
    }

    #[test]
    fn build_artifact_is_deterministic() {
        assert_eq!(
            build_artifact(&sandbox()).unwrap().manifest_digest,
            build_artifact(&sandbox()).unwrap().manifest_digest,
        );
    }

    #[test]
    fn build_artifact_refuses_an_invalid_manifest() {
        let floating = br#"{"apiVersion":"lens.dev/v1alpha1","kind":"Sandbox","metadata":{"name":"some-sandbox"},"spec":{"isolation":"microvm","baseImage":"reg/base:1"}}"#;
        let err = build_artifact(floating).unwrap_err();
        assert!(format!("{err:#}").contains("digest-pinned"), "got: {err:#}");
    }

    #[test]
    fn build_artifact_refuses_an_unknown_kind() {
        let doc = br#"{"apiVersion":"lens.dev/v1alpha1","kind":"Sorcery","metadata":{"name":"x"},"spec":{}}"#;
        let err = build_artifact(doc).unwrap_err();
        assert!(
            format!("{err:#}").contains("unknown artifact kind"),
            "got: {err:#}"
        );
    }

    #[test]
    fn build_artifact_builds_a_bundle_manifest() {
        let bundle = br#"{"apiVersion":"lens.dev/v1alpha1","kind":"AgentSystem","metadata":{"name":"some-bundle"},"spec":{"components":{"sandbox":{"ref":"reg/base:1"}}}}"#;
        let built = build_artifact(bundle).unwrap();
        assert_eq!(built.artifact_type, "application/vnd.lens.bundle.v1+json");
    }
}
