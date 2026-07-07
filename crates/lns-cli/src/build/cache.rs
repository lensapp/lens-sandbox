use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use lns_artifact::build::{Blob, BuiltArtifact};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// What `lns push <ref>` needs to find a cached build: the ref and the manifest blob addressing it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RefRecord {
    pub reference: String,
    pub manifest_digest: String,
}

/// One file to write into the build cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CacheFile {
    pub path: PathBuf,
    pub bytes: Vec<u8>,
}

fn digest_hex(digest: &str) -> Result<&str> {
    let hex = digest
        .strip_prefix("sha256:")
        .with_context(|| format!("build-cache digest {digest} is not sha256-prefixed"))?;
    if hex.len() == 64
        && hex
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        Ok(hex)
    } else {
        bail!("build-cache digest {digest} is not a lowercase 64-hex sha256")
    }
}

pub(crate) fn blob_path(root: &Path, digest: &str) -> Result<PathBuf> {
    Ok(root.join(digest_hex(digest)?))
}

pub(crate) fn ref_record_path(root: &Path, reference: &str) -> PathBuf {
    let key = hex::encode(Sha256::digest(reference.as_bytes()));
    root.join(format!("{key}.json"))
}

/// The full set of files that persist `built` under `reference`: every blob and the manifest content-addressed, plus the ref record pointing at the manifest.
pub(crate) fn plan_writes(
    root: &Path,
    reference: &str,
    built: &BuiltArtifact,
) -> Result<Vec<CacheFile>> {
    let mut files = Vec::new();
    for blob in &built.blobs {
        files.push(CacheFile {
            path: blob_path(root, &blob.digest)?,
            bytes: blob.data.clone(),
        });
    }
    files.push(CacheFile {
        path: blob_path(root, &built.manifest_digest)?,
        bytes: built.manifest.clone(),
    });
    let record = RefRecord {
        reference: reference.to_string(),
        manifest_digest: built.manifest_digest.clone(),
    };
    files.push(CacheFile {
        path: ref_record_path(root, reference),
        bytes: serde_json::to_vec(&record).context("serialising build-cache ref record")?,
    });
    Ok(files)
}

fn descriptor(value: &Value) -> Result<(String, String)> {
    let media_type = value
        .get("mediaType")
        .and_then(Value::as_str)
        .context("build-cache manifest descriptor missing mediaType")?;
    let digest = value
        .get("digest")
        .and_then(Value::as_str)
        .context("build-cache manifest descriptor missing digest")?;
    Ok((media_type.to_string(), digest.to_string()))
}

/// Rebuild the pushable artifact from a cached manifest, loading each referenced blob via `load`.
pub(crate) fn reconstruct(
    manifest_bytes: &[u8],
    manifest_digest: &str,
    mut load: impl FnMut(&str) -> Result<Vec<u8>>,
) -> Result<BuiltArtifact> {
    let manifest: Value =
        serde_json::from_slice(manifest_bytes).context("parsing cached manifest")?;
    let manifest_media_type = manifest
        .get("mediaType")
        .and_then(Value::as_str)
        .context("cached manifest missing mediaType")?
        .to_string();
    let artifact_type = manifest
        .get("artifactType")
        .and_then(Value::as_str)
        .context("cached manifest missing artifactType")?
        .to_string();
    let layers = manifest
        .get("layers")
        .and_then(Value::as_array)
        .context("cached manifest missing layers")?;
    let mut descriptors = vec![descriptor(
        manifest
            .get("config")
            .context("cached manifest missing config")?,
    )?];
    for layer in layers {
        descriptors.push(descriptor(layer)?);
    }
    let mut blobs = Vec::with_capacity(descriptors.len());
    for (media_type, digest) in descriptors {
        let data = load(&digest).with_context(|| format!("loading cached blob {digest}"))?;
        blobs.push(Blob {
            digest,
            media_type,
            data,
        });
    }
    Ok(BuiltArtifact {
        artifact_type,
        manifest_media_type,
        manifest: manifest_bytes.to_vec(),
        manifest_digest: manifest_digest.to_string(),
        blobs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn built() -> BuiltArtifact {
        let sandbox = format!(
            "{{\"apiVersion\":\"lens.dev/v1alpha1\",\"kind\":\"Sandbox\",\"metadata\":{{\"name\":\"s\"}},\"spec\":{{\"isolation\":\"microvm\",\"baseImage\":\"reg/base@sha256:{}\"}}}}",
            "a".repeat(64)
        );
        lns_artifact::build::build_artifact(sandbox.as_bytes()).unwrap()
    }

    #[test]
    fn digest_hex_rejects_a_non_sha256_or_malformed_digest() {
        assert!(digest_hex("md5:abc").is_err());
        assert!(digest_hex("sha256:XYZ").is_err());
        assert!(digest_hex(&format!("sha256:{}", "a".repeat(64))).is_ok());
    }

    #[test]
    fn plan_writes_stores_every_blob_the_manifest_and_the_ref_record() {
        let built = built();
        let root = Path::new("/cache/builds");
        let files = plan_writes(root, "reg/some-sandbox:1", &built).unwrap();
        assert_eq!(files.len(), built.blobs.len() + 2);
        let manifest_file = files
            .iter()
            .find(|f| f.path == blob_path(root, &built.manifest_digest).unwrap())
            .expect("manifest blob is written");
        assert_eq!(manifest_file.bytes, built.manifest);
        let record_file = files
            .iter()
            .find(|f| f.path == ref_record_path(root, "reg/some-sandbox:1"))
            .expect("ref record is written");
        let record: RefRecord = serde_json::from_slice(&record_file.bytes).unwrap();
        assert_eq!(record.manifest_digest, built.manifest_digest);
        assert_eq!(record.reference, "reg/some-sandbox:1");
    }

    #[test]
    fn reconstruct_round_trips_a_planned_build() {
        let built = built();
        let root = Path::new("/cache/builds");
        let files = plan_writes(root, "reg/some-sandbox:1", &built).unwrap();
        let store: BTreeMap<PathBuf, Vec<u8>> =
            files.into_iter().map(|f| (f.path, f.bytes)).collect();
        let restored = reconstruct(&built.manifest, &built.manifest_digest, |digest| {
            store
                .get(&blob_path(root, digest).unwrap())
                .cloned()
                .context("blob absent")
        })
        .unwrap();
        assert_eq!(restored, built);
    }

    #[test]
    fn reconstruct_surfaces_a_missing_blob() {
        let built = built();
        let err =
            reconstruct(&built.manifest, &built.manifest_digest, |_| bail!("gone")).unwrap_err();
        assert!(
            format!("{err:#}").contains("loading cached blob"),
            "{err:#}"
        );
    }

    #[test]
    fn reconstruct_rejects_a_manifest_missing_its_config() {
        let manifest = br#"{"mediaType":"m","artifactType":"a","layers":[]}"#;
        let err = reconstruct(manifest, "sha256:x", |_| Ok(Vec::new())).unwrap_err();
        assert!(format!("{err:#}").contains("missing config"), "{err:#}");
    }
}
