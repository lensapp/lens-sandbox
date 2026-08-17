use crate::spec;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const EMPTY_MEDIA_TYPE: &str = "application/vnd.oci.empty.v1+json";
const MANIFEST_MEDIA_TYPE: &str = "application/vnd.oci.image.manifest.v1+json";
const LAYER_MEDIA_TYPE: &str = "application/vnd.oci.image.layer.v1.tar";

/// Maximum aggregate size of one fileset's tar content layer.
pub const MAX_FILESET_BYTES: u64 = 256 * 1024 * 1024;

/// Maximum number of regular files in one fileset.
pub const MAX_FILESET_ENTRIES: usize = 100_000;

/// One file to pack into a fileset layer: a path relative to the mount root, its bytes, and its permission bits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub path: String,
    pub data: Vec<u8>,
    pub mode: u32,
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

/// Assemble an OCI artifact from a validated manifest document, packing one tar layer per `spec.filesets` `path` entry (§7) so the files and the declaration that mounts them share one digest; `path_filesets` supplies those directories' contents in declaration order.
pub fn build_artifact(doc: &[u8], path_filesets: &[Vec<FileEntry>]) -> Result<BuiltArtifact> {
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

    let declared = declared_path_filesets(doc)?;
    if declared != path_filesets.len() {
        bail!(
            "the document declares {declared} path filesets but {} were packed",
            path_filesets.len()
        );
    }

    let mut blobs = vec![Blob {
        digest: config_digest.clone(),
        media_type: kind.config_media_type(),
        data: config_blob.clone(),
    }];
    let mut layers = Vec::new();
    for entries in path_filesets {
        let layer = fileset_layer(entries)?;
        layers.push(json!({
            "mediaType": LAYER_MEDIA_TYPE,
            "digest": layer.digest,
            "size": layer.data.len(),
        }));
        blobs.push(layer);
    }
    if layers.is_empty() {
        let empty = Blob {
            digest: sha256_digest(b"{}"),
            media_type: EMPTY_MEDIA_TYPE.to_string(),
            data: b"{}".to_vec(),
        };
        layers.push(json!({
            "mediaType": EMPTY_MEDIA_TYPE,
            "digest": empty.digest,
            "size": empty.data.len(),
            "data": "e30=",
        }));
        blobs.push(empty);
    }

    let manifest = json!({
        "schemaVersion": 2,
        "mediaType": MANIFEST_MEDIA_TYPE,
        "artifactType": kind.artifact_type(),
        "config": {
            "mediaType": kind.config_media_type(),
            "digest": config_digest,
            "size": config_blob.len(),
        },
        "layers": layers,
    });
    let manifest_bytes = serde_json::to_vec(&manifest).context("serialising manifest")?;
    let manifest_digest = sha256_digest(&manifest_bytes);

    Ok(BuiltArtifact {
        artifact_type: kind.artifact_type(),
        manifest_media_type: MANIFEST_MEDIA_TYPE.to_string(),
        manifest: manifest_bytes,
        manifest_digest,
        blobs,
    })
}

/// How many `spec.filesets` entries name a directory this publish packs, so the caller's layer list can be checked against the document rather than trusted.
fn declared_path_filesets(doc: &[u8]) -> Result<usize> {
    let def = crate::sandbox::parse_document(doc)?;
    Ok(def
        .spec
        .filesets
        .iter()
        .filter(|fileset| fileset.path.is_some())
        .count())
}

/// Pack one fileset's files into a deterministic tar layer (entries sorted, uid/gid/mtime zeroed, each file's mode preserved) so identical inputs yield identical digests.
fn fileset_layer(entries: &[FileEntry]) -> Result<Blob> {
    validate_fileset_entries(entries, MAX_FILESET_BYTES, MAX_FILESET_ENTRIES)?;
    let data = tar_layer(entries).context("packing fileset layer")?;
    validate_fileset_layer_size(data.len() as u64, MAX_FILESET_BYTES)?;
    Ok(Blob {
        digest: sha256_digest(&data),
        media_type: LAYER_MEDIA_TYPE.to_string(),
        data,
    })
}

fn validate_fileset_entries(
    entries: &[FileEntry],
    max_bytes: u64,
    max_entries: usize,
) -> Result<()> {
    if entries.len() > max_entries {
        bail!("fileset contains more than {max_entries} files");
    }
    let mut total = 0_u64;
    for entry in entries {
        let size = entry.data.len() as u64;
        if size > max_bytes.saturating_sub(total) {
            bail!("fileset content exceeds the {max_bytes}-byte limit");
        }
        total += size;
    }
    Ok(())
}

fn validate_fileset_layer_size(size: u64, max_bytes: u64) -> Result<()> {
    if size > max_bytes {
        bail!("fileset content layer exceeds the {max_bytes}-byte limit");
    }
    Ok(())
}

fn tar_layer(entries: &[FileEntry]) -> Result<Vec<u8>> {
    let mut sorted: Vec<&FileEntry> = entries.iter().collect();
    sorted.sort_by(|a, b| a.path.cmp(&b.path));
    let mut builder = tar::Builder::new(Vec::new());
    for entry in sorted {
        let mut header = tar::Header::new_gnu();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_size(entry.data.len() as u64);
        header.set_mode(entry.mode);
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
            r#"{{"apiVersion":"lns.run/v1","kind":"sandbox","name":"some-sandbox","spec":{{"image":"reg/base@sha256:{}"}}}}"#,
            "a".repeat(64)
        )
        .into_bytes()
    }

    #[test]
    fn a_mixin_publishes_under_its_own_artifact_type() {
        let doc = br#"{"apiVersion":"lns.run/v1","kind":"mixin","name":"postgres-tools","spec":{"tools":["node@22"]}}"#;
        let built = build_artifact(doc, &[]).expect("a mixin is a kit, published like any other");
        assert_eq!(built.artifact_type, "application/vnd.lens.mixin.v1+json");
        let manifest: Value = serde_json::from_slice(&built.manifest).unwrap();
        assert_eq!(
            manifest["config"]["mediaType"], "application/vnd.lens.mixin.config.v1+json",
            "the media type names the kind, so a puller knows what it fetched before reading it"
        );
    }

    #[test]
    fn a_mixin_that_declares_a_launch_block_is_refused_before_it_publishes() {
        let doc = br#"{"apiVersion":"lns.run/v1","kind":"mixin","name":"postgres-tools","spec":{"image":"reg/base:1"}}"#;
        let err = build_artifact(doc, &[]).unwrap_err();
        assert!(
            format!("{err:#}").contains("a mixin must not declare image"),
            "got: {err:#}"
        );
    }

    #[test]
    fn build_artifact_produces_a_config_only_manifest_with_matching_digests() {
        let built = build_artifact(&sandbox(), &[]).unwrap();
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
        let doc = br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"some-sandbox","spec":{"image":"registry.example.test/runtime:1","workdir":"/workspace","volumes":[{"type":"bind","source":".","target":"/workspace"},{"type":"volume","source":"some-cache","target":"/home/node/.cache","readOnly":true}]}}"#;
        let built = build_artifact(doc, &[]).unwrap();
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
        let built = build_artifact(&sandbox(), &[]).unwrap();
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
            build_artifact(&sandbox(), &[]).unwrap().manifest_digest,
            build_artifact(&sandbox(), &[]).unwrap().manifest_digest,
        );
    }

    #[test]
    fn build_artifact_refuses_an_invalid_manifest() {
        let imageless =
            br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"some-sandbox","spec":{}}"#;
        let err = build_artifact(imageless, &[]).unwrap_err();
        assert!(
            format!("{err:#}").contains("must carry an image"),
            "got: {err:#}"
        );
    }

    #[test]
    fn build_artifact_refuses_an_unknown_kind() {
        let doc = br#"{"apiVersion":"lns.run/v1","kind":"Sorcery","name":"x","spec":{"image":"reg/base:1"}}"#;
        let err = build_artifact(doc, &[]).unwrap_err();
        assert!(format!("{err:#}").contains("kind"), "got: {err:#}");
    }

    #[test]
    fn build_artifact_refuses_a_document_no_verb_can_run() {
        // Publishing a retired-group document would put an artifact in a registry that `lns run` and `lns inspect` both refuse to read.
        let doc = format!(
            r#"{{"apiVersion":"lens.dev/v1alpha1","kind":"sandbox","name":"some-sandbox","spec":{{"isolation":"microvm","baseImage":"reg/base@sha256:{}"}}}}"#,
            "a".repeat(64)
        );
        let err = build_artifact(doc.as_bytes(), &[]).unwrap_err();
        assert!(
            format!("{err:#}").contains(crate::sandbox::API_VERSION),
            "got: {err:#}"
        );
    }

    fn entry(path: &str, data: &str) -> FileEntry {
        entry_mode(path, data, 0o644)
    }

    fn entry_mode(path: &str, data: &str, mode: u32) -> FileEntry {
        FileEntry {
            path: path.into(),
            data: data.as_bytes().to_vec(),
            mode,
        }
    }

    fn sandbox_with_path_filesets(paths: &[(&str, &str)]) -> Vec<u8> {
        let entries: Vec<String> = paths
            .iter()
            .map(|(path, mount)| format!(r#"{{"path":"{path}","mountPath":"{mount}"}}"#))
            .collect();
        format!(
            r#"{{"apiVersion":"lns.run/v1","kind":"sandbox","name":"some-sandbox","spec":{{"image":"reg/base@sha256:{}","filesets":[{}]}}}}"#,
            "a".repeat(64),
            entries.join(",")
        )
        .into_bytes()
    }

    fn tar_layers(built: &BuiltArtifact) -> Vec<&Blob> {
        let manifest: Value = serde_json::from_slice(&built.manifest).unwrap();
        manifest["layers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|layer| {
                let digest = layer["digest"].as_str().unwrap();
                built
                    .blobs
                    .iter()
                    .find(|blob| blob.digest == digest)
                    .expect("every layer the manifest names must be uploaded with it")
            })
            .collect()
    }

    fn names(layer: &Blob) -> Vec<String> {
        let mut names: Vec<String> = tar::Archive::new(layer.data.as_slice())
            .entries()
            .unwrap()
            .map(|e| e.unwrap().path().unwrap().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn a_path_fileset_publishes_as_a_layer_of_the_document_that_declares_it() {
        // §3.1.11: a fileset is not a separate artifact, so the files and the declaration that mounts them share one digest.
        let doc = sandbox_with_path_filesets(&[("./skills", "/root/.agent/skills")]);
        let built = build_artifact(
            &doc,
            &[vec![
                entry("deep.md", "research"),
                entry("nested/tools.md", "tools"),
            ]],
        )
        .unwrap();
        assert_eq!(built.artifact_type, "application/vnd.lens.sandbox.v1+json");

        let layers = tar_layers(&built);
        assert_eq!(layers.len(), 1, "one layer per path fileset");
        assert_eq!(layers[0].media_type, LAYER_MEDIA_TYPE);
        assert_eq!(names(layers[0]), vec!["deep.md", "nested/tools.md"]);

        let config = built
            .blobs
            .iter()
            .find(|blob| blob.media_type.contains("sandbox.config"))
            .expect("the sandbox config blob must be present");
        let value: Value = serde_json::from_slice(&config.data).unwrap();
        assert_eq!(
            value["spec"]["filesets"][0]["path"], "./skills",
            "§6: the entry keeps its path; only the content moves into the artifact"
        );
    }

    #[test]
    fn each_path_fileset_gets_its_own_layer_in_declaration_order() {
        let doc = sandbox_with_path_filesets(&[("./skills", "/a"), ("./hooks", "/b")]);
        let built = build_artifact(
            &doc,
            &[vec![entry("skill.md", "s")], vec![entry("hook.sh", "h")]],
        )
        .unwrap();
        let layers = tar_layers(&built);
        assert_eq!(
            layers.iter().map(|l| names(l)).collect::<Vec<_>>(),
            vec![vec!["skill.md"], vec!["hook.sh"]],
            "a consumer matches layer N to the Nth path entry, so order is the contract"
        );
    }

    #[test]
    fn a_mixin_carries_its_own_path_fileset_layer() {
        let doc = br#"{"apiVersion":"lns.run/v1","kind":"mixin","name":"skills","spec":{"filesets":[{"path":"./skills","mountPath":"/root/.agent/skills"}]}}"#;
        let built = build_artifact(doc, &[vec![entry("prompts.md", "x")]]).unwrap();
        assert_eq!(built.artifact_type, "application/vnd.lens.mixin.v1+json");
        assert_eq!(names(tar_layers(&built)[0]), vec!["prompts.md"]);
    }

    #[test]
    fn a_packed_layer_count_that_disagrees_with_the_document_is_refused() {
        let doc = sandbox_with_path_filesets(&[("./skills", "/a"), ("./hooks", "/b")]);
        let err = build_artifact(&doc, &[vec![entry("skill.md", "s")]]).unwrap_err();
        assert!(
            format!("{err:#}").contains("declares 2 path filesets but 1"),
            "a mismatch would silently mount one fileset's files at another's path: {err:#}"
        );
    }

    #[test]
    fn an_inline_fileset_needs_no_layer_of_its_own() {
        let doc = format!(
            r#"{{"apiVersion":"lns.run/v1","kind":"sandbox","name":"some-sandbox","spec":{{"image":"reg/base@sha256:{}","filesets":[{{"inline":{{"a.md":"x"}},"mountPath":"/notes"}}]}}}}"#,
            "a".repeat(64)
        );
        let built = build_artifact(doc.as_bytes(), &[]).unwrap();
        assert_eq!(
            tar_layers(&built)[0].media_type,
            EMPTY_MEDIA_TYPE,
            "inline content already lives in the config blob"
        );
    }

    #[test]
    fn a_packed_layer_preserves_each_entrys_mode() {
        let doc = sandbox_with_path_filesets(&[("./hooks", "/root/.agent/hooks")]);
        let built = build_artifact(
            &doc,
            &[vec![
                entry_mode("run.sh", "#!/bin/sh\n", 0o755),
                entry("notes.md", "x"),
            ]],
        )
        .unwrap();
        let modes: std::collections::BTreeMap<String, u32> =
            tar::Archive::new(tar_layers(&built)[0].data.as_slice())
                .entries()
                .unwrap()
                .map(|e| {
                    let e = e.unwrap();
                    (
                        e.path().unwrap().to_string_lossy().into_owned(),
                        e.header().mode().unwrap(),
                    )
                })
                .collect();
        assert_eq!(
            modes["run.sh"], 0o755,
            "an executable fileset file must keep its exec bit through push→pull"
        );
        assert_eq!(modes["notes.md"], 0o644);
    }

    #[test]
    fn a_packed_layer_is_deterministic_regardless_of_entry_order() {
        let doc = sandbox_with_path_filesets(&[("./skills", "/x")]);
        let a = build_artifact(&doc, &[vec![entry("a", "1"), entry("b", "2")]]).unwrap();
        let b = build_artifact(&doc, &[vec![entry("b", "2"), entry("a", "1")]]).unwrap();
        assert_eq!(
            a.manifest_digest, b.manifest_digest,
            "entry order must not change the packed digest"
        );

        let different =
            build_artifact(&doc, &[vec![entry("a", "changed"), entry("b", "2")]]).unwrap();
        assert_ne!(
            a.manifest_digest, different.manifest_digest,
            "different content must change the digest"
        );
    }

    #[test]
    fn fileset_limits_reject_too_many_entries() {
        let entries = [entry("a", "1"), entry("b", "2")];
        let err = validate_fileset_entries(&entries, 10, 1).unwrap_err();
        assert!(format!("{err:#}").contains("more than 1 files"));
    }

    #[test]
    fn fileset_limits_reject_aggregate_content_size() {
        let entries = [entry("a", "123"), entry("b", "456")];
        let err = validate_fileset_entries(&entries, 5, 2).unwrap_err();
        assert!(format!("{err:#}").contains("5-byte limit"));
    }

    #[test]
    fn fileset_limits_include_tar_framing_in_the_layer_cap() {
        let err = validate_fileset_layer_size(6, 5).unwrap_err();
        assert!(format!("{err:#}").contains("5-byte limit"));
    }
}
