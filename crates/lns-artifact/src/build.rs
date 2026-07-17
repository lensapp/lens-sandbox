use crate::spec;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const EMPTY_MEDIA_TYPE: &str = "application/vnd.oci.empty.v1+json";
const MANIFEST_MEDIA_TYPE: &str = "application/vnd.oci.image.manifest.v1+json";
const LAYER_MEDIA_TYPE: &str = "application/vnd.oci.image.layer.v1.tar";

/// One file to pack into a FileSet layer: a path relative to the mount root and its bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub path: String,
    pub data: Vec<u8>,
}

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

/// Assemble a config-only OCI artifact (the standard shape for lens runtime kinds) from a validated manifest document.
pub fn build_artifact(doc: &[u8]) -> Result<BuiltArtifact> {
    if let Err(problems) = crate::validate::validate(doc) {
        bail!(
            "refusing to build an invalid manifest:\n  - {}",
            problems.join("\n  - ")
        );
    }
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

/// Pack a directory's files into a FileSet artifact mounting at `mount_path`; the tar layer is deterministic (entries sorted, metadata zeroed) so identical inputs yield identical digests.
pub fn build_fileset(name: &str, mount_path: &str, entries: &[FileEntry]) -> Result<BuiltArtifact> {
    spec::validate_mount_path(mount_path).context("fileset mount path")?;
    let layer = tar_layer(entries).context("packing fileset layer")?;
    let layer_digest = sha256_digest(&layer);

    let config_doc = json!({
        "apiVersion": "lens.dev/v1alpha1",
        "kind": "FileSet",
        "metadata": {"name": name},
        "mount": {"path": mount_path},
        "spec": {},
    });
    let config_blob = serde_json::to_vec(&config_doc).context("serialising fileset config")?;
    spec::parse_fileset(&config_blob).context("validating the built fileset config")?;
    let config_digest = sha256_digest(&config_blob);
    let kind = spec::Kind::FileSet;

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
            "mediaType": LAYER_MEDIA_TYPE,
            "digest": layer_digest,
            "size": layer.len(),
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
                digest: layer_digest,
                media_type: LAYER_MEDIA_TYPE.to_string(),
                data: layer,
            },
        ],
    })
}

fn tar_layer(entries: &[FileEntry]) -> Result<Vec<u8>> {
    let mut sorted: Vec<&FileEntry> = entries.iter().collect();
    sorted.sort_by(|a, b| a.path.cmp(&b.path));
    let mut builder = tar::Builder::new(Vec::new());
    for entry in sorted {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(entry.data.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(0);
        header.set_uid(0);
        header.set_gid(0);
        header.set_cksum();
        builder
            .append_data(&mut header, &entry.path, entry.data.as_slice())
            .with_context(|| format!("adding {} to the fileset layer", entry.path))?;
    }
    builder.into_inner().context("finalising the fileset layer")
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
    fn build_artifact_preserves_declarative_workdir_and_mounts() {
        let doc = br#"{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{"name":"some-sandbox"},"spec":{"image":"registry.example.test/runtime:1","workdir":"/workspace","volumes":[{"type":"bind","source":".","target":"/workspace"},{"type":"volume","source":"some-cache","target":"/home/node/.cache","readOnly":true}]}}"#;
        let built = build_artifact(doc).unwrap();
        let config = built
            .blobs
            .iter()
            .find(|blob| blob.media_type.contains("sandbox.config"))
            .expect("the sandbox config blob must be present");
        let value: Value = serde_json::from_slice(&config.data).unwrap();
        assert_eq!(value["spec"]["workdir"], "/workspace");
        assert_eq!(value["spec"]["volumes"][0]["source"], ".");
        assert_eq!(value["spec"]["volumes"][1]["readOnly"], true);
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

    fn entry(path: &str, data: &str) -> FileEntry {
        FileEntry {
            path: path.into(),
            data: data.as_bytes().to_vec(),
        }
    }

    #[test]
    fn build_fileset_produces_a_fileset_artifact_with_a_tar_layer() {
        let built = build_fileset(
            "skills",
            "/root/.some-agent/skills",
            &[
                entry("deep.md", "research"),
                entry("nested/tools.md", "tools"),
            ],
        )
        .unwrap();
        assert_eq!(built.artifact_type, "application/vnd.lens.fileset.v1+json");

        let manifest: Value = serde_json::from_slice(&built.manifest).unwrap();
        assert_eq!(manifest["layers"][0]["mediaType"], LAYER_MEDIA_TYPE);

        let config = built
            .blobs
            .iter()
            .find(|b| b.media_type.contains("fileset"))
            .expect("a fileset config blob");
        let fileset = spec::parse_fileset(&config.data).expect("config parses as a fileset");
        assert_eq!(fileset.mount.path, "/root/.some-agent/skills");

        let layer = built
            .blobs
            .iter()
            .find(|b| b.media_type == LAYER_MEDIA_TYPE)
            .expect("a tar layer blob");
        let mut names: Vec<String> = tar::Archive::new(layer.data.as_slice())
            .entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, vec!["deep.md", "nested/tools.md"]);
    }

    #[test]
    fn build_fileset_is_deterministic_regardless_of_entry_order() {
        let a = build_fileset("s", "/x", &[entry("a", "1"), entry("b", "2")]).unwrap();
        let b = build_fileset("s", "/x", &[entry("b", "2"), entry("a", "1")]).unwrap();
        assert_eq!(
            a.manifest_digest, b.manifest_digest,
            "entry order must not change the packed digest"
        );

        let different =
            build_fileset("s", "/x", &[entry("a", "changed"), entry("b", "2")]).unwrap();
        assert_ne!(
            a.manifest_digest, different.manifest_digest,
            "different content must change the digest"
        );
    }

    #[test]
    fn build_fileset_rejects_a_traversing_mount_path() {
        let err = build_fileset("s", "/root/../etc", &[entry("a", "1")]).unwrap_err();
        assert!(format!("{err:#}").contains("`..` segment"), "got: {err:#}");
    }
}
