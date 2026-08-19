use crate::spec;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const EMPTY_MEDIA_TYPE: &str = "application/vnd.oci.empty.v1+json";
const MANIFEST_MEDIA_TYPE: &str = "application/vnd.oci.image.manifest.v1+json";

/// The media type of a packed `filesets[].path` layer; a sandbox or mixin artifact carries one per entry it declares (`docs/sandbox-spec.md` §7).
pub const FILESET_LAYER_MEDIA_TYPE: &str = "application/vnd.oci.image.layer.v1.tar+gzip";

/// Maximum aggregate size of one packed fileset layer.
pub const MAX_FILESET_BYTES: u64 = 256 * 1024 * 1024;

/// Maximum number of regular files in one packed fileset.
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

impl BuiltArtifact {
    /// The packed fileset layers, in the order the document declares its `path` entries.
    pub fn fileset_layers(&self) -> impl Iterator<Item = &Blob> {
        self.blobs
            .iter()
            .filter(|blob| blob.media_type == FILESET_LAYER_MEDIA_TYPE)
    }
}

fn sha256_digest(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

/// Assemble one OCI artifact from a validated document and the directories its `path` filesets pack, in declaration order: one layer per entry (`docs/sandbox-spec.md` §6), so the files and the declaration that mounts them share one digest. A document declaring none is config-only.
pub fn build_artifact(doc: &[u8], filesets: &[Vec<FileEntry>]) -> Result<BuiltArtifact> {
    if let Err(problems) = crate::validate::validate(doc) {
        bail!(
            "refusing to build an invalid manifest:\n  - {}",
            problems.join("\n  - ")
        );
    }
    let kind = spec::read_kind(doc)?;
    let declared = crate::sandbox::parse_document(doc)?;
    let declared = crate::merge::path_filesets(&declared.spec).count();
    if declared != filesets.len() {
        bail!(
            "this document declares {declared} path fileset(s) but {} were packed; an artifact carries one layer per entry",
            filesets.len()
        );
    }

    let config_value: Value = serde_json::from_slice(doc).context("parsing manifest")?;
    let config_blob = serde_json::to_vec(&config_value).context("serialising config blob")?;
    let config_digest = sha256_digest(&config_blob);

    let mut blobs = vec![Blob {
        digest: config_digest.clone(),
        media_type: kind.config_media_type(),
        data: config_blob.clone(),
    }];
    let mut layers = Vec::new();
    for entries in filesets {
        let blob = fileset_layer(entries)?;
        layers.push(json!({
            "mediaType": FILESET_LAYER_MEDIA_TYPE,
            "digest": blob.digest,
            "size": blob.data.len(),
        }));
        blobs.push(blob);
    }
    if layers.is_empty() {
        let empty_blob = b"{}".to_vec();
        let empty_digest = sha256_digest(&empty_blob);
        layers.push(json!({
            "mediaType": EMPTY_MEDIA_TYPE,
            "digest": empty_digest,
            "size": empty_blob.len(),
            "data": "e30=",
        }));
        blobs.push(Blob {
            digest: empty_digest,
            media_type: EMPTY_MEDIA_TYPE.to_string(),
            data: empty_blob,
        });
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

/// Pack one fileset directory into a gzipped tar layer. Deterministic — entries sorted, uid/gid/mtime zeroed, each file's mode preserved, no gzip timestamp — so identical directories dedupe at the blob level however many artifacts carry them.
fn fileset_layer(entries: &[FileEntry]) -> Result<Blob> {
    validate_fileset_entries(entries, MAX_FILESET_BYTES, MAX_FILESET_ENTRIES)?;
    let tar = tar_layer(entries).context("packing fileset layer")?;
    validate_fileset_layer_size(tar.len() as u64, MAX_FILESET_BYTES)?;
    let data = gzip(&tar).context("compressing fileset layer")?;
    Ok(Blob {
        digest: sha256_digest(&data),
        media_type: FILESET_LAYER_MEDIA_TYPE.to_string(),
        data,
    })
}

fn gzip(tar: &[u8]) -> Result<Vec<u8>> {
    use std::io::Write;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(tar)?;
    Ok(encoder.finish()?)
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

    /// Every artifact in this module is built from a document plus the directories its `path` filesets pack; most fixtures declare none.
    fn build(doc: &[u8]) -> Result<BuiltArtifact> {
        build_artifact(doc, &[])
    }

    #[test]
    fn a_mixin_publishes_under_its_own_artifact_type() {
        let doc = br#"{"apiVersion":"lns.run/v1","kind":"mixin","name":"postgres-tools","spec":{"tools":["node@22"]}}"#;
        let built = build(doc).expect("a mixin is an artifact, published like any other");
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
        let err = build(doc).unwrap_err();
        assert!(
            format!("{err:#}").contains("a mixin must not declare image"),
            "got: {err:#}"
        );
    }

    #[test]
    fn build_artifact_produces_a_config_only_manifest_with_matching_digests() {
        let built = build(&sandbox()).unwrap();
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
        let built = build(doc).unwrap();
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
        let built = build(&sandbox()).unwrap();
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
            build(&sandbox()).unwrap().manifest_digest,
            build(&sandbox()).unwrap().manifest_digest,
        );
    }

    #[test]
    fn build_artifact_refuses_an_invalid_manifest() {
        let imageless =
            br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"some-sandbox","spec":{}}"#;
        let err = build(imageless).unwrap_err();
        assert!(
            format!("{err:#}").contains("must carry an image"),
            "got: {err:#}"
        );
    }

    #[test]
    fn build_artifact_refuses_an_unknown_kind() {
        let doc = br#"{"apiVersion":"lns.run/v1","kind":"Sorcery","name":"x","spec":{"image":"reg/base:1"}}"#;
        let err = build(doc).unwrap_err();
        assert!(format!("{err:#}").contains("kind"), "got: {err:#}");
    }

    #[test]
    fn build_artifact_refuses_a_document_no_verb_can_run() {
        // Publishing a retired-group document would put an artifact in a registry that `lns run` and `lns inspect` both refuse to read.
        let doc = format!(
            r#"{{"apiVersion":"lens.dev/v1alpha1","kind":"sandbox","name":"some-sandbox","spec":{{"isolation":"microvm","baseImage":"reg/base@sha256:{}"}}}}"#,
            "a".repeat(64)
        );
        let err = build(doc.as_bytes()).unwrap_err();
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

    fn with_path_filesets(mounts: &[&str]) -> Vec<u8> {
        let entries: Vec<String> = mounts
            .iter()
            .enumerate()
            .map(|(i, mount)| format!(r#"{{"path":"./dir{i}","guestPath":"{mount}"}}"#))
            .collect();
        format!(
            r#"{{"apiVersion":"lns.run/v1","kind":"sandbox","name":"some-sandbox","spec":{{"image":"reg/base:1","filesets":[{}]}}}}"#,
            entries.join(",")
        )
        .into_bytes()
    }

    fn unpacked(layer: &Blob) -> std::collections::BTreeMap<String, (u32, Vec<u8>)> {
        let tar = flate2::read::GzDecoder::new(layer.data.as_slice());
        tar::Archive::new(tar)
            .entries()
            .unwrap()
            .map(|entry| {
                let mut entry = entry.unwrap();
                let path = entry.path().unwrap().to_string_lossy().into_owned();
                let mode = entry.header().mode().unwrap();
                let mut data = Vec::new();
                std::io::Read::read_to_end(&mut entry, &mut data).unwrap();
                (path, (mode, data))
            })
            .collect()
    }

    #[test]
    fn a_path_fileset_becomes_a_layer_of_the_artifact_the_document_configures() {
        let built = build_artifact(
            &with_path_filesets(&["/root/.some-agent/skills"]),
            &[vec![
                entry("deep.md", "research"),
                entry("nested/tools.md", "tools"),
            ]],
        )
        .expect("a declared path fileset packs into this artifact");
        assert_eq!(built.artifact_type, "application/vnd.lens.sandbox.v1+json");

        let manifest: Value = serde_json::from_slice(&built.manifest).unwrap();
        assert_eq!(manifest["layers"].as_array().unwrap().len(), 1);
        assert_eq!(manifest["layers"][0]["mediaType"], FILESET_LAYER_MEDIA_TYPE);

        let layer = built
            .fileset_layers()
            .next()
            .expect("the packed layer travels with the artifact");
        assert_eq!(manifest["layers"][0]["digest"], layer.digest);
        assert_eq!(
            manifest["layers"][0]["size"].as_u64().unwrap() as usize,
            layer.data.len(),
            "the descriptor has to address the blob, or a puller asks the registry for bytes nobody uploaded"
        );
        assert_eq!(
            unpacked(layer).keys().collect::<Vec<_>>(),
            ["deep.md", "nested/tools.md"],
            "the files and the declaration that mounts them share one digest (§3.1.11)"
        );
    }

    #[test]
    fn each_path_entry_gets_its_own_layer_in_declaration_order() {
        let built = build_artifact(
            &with_path_filesets(&["/first", "/second"]),
            &[vec![entry("a.md", "first")], vec![entry("b.md", "second")]],
        )
        .unwrap();
        let layers: Vec<Vec<String>> = built
            .fileset_layers()
            .map(|layer| unpacked(layer).keys().cloned().collect())
            .collect();
        assert_eq!(
            layers,
            [["a.md".to_string()], ["b.md".to_string()]],
            "the i-th path entry owns the i-th layer, which is the whole correlation a consumer materializes by (§7)"
        );
    }

    #[test]
    fn a_packed_layer_preserves_each_entrys_mode() {
        let built = build_artifact(
            &with_path_filesets(&["/root/.agent/hooks"]),
            &[vec![
                entry_mode("run.sh", "#!/bin/sh\n", 0o755),
                entry("notes.md", "x"),
            ]],
        )
        .unwrap();
        let modes = unpacked(built.fileset_layers().next().expect("a packed layer"));
        assert_eq!(
            modes["run.sh"].0, 0o755,
            "an executable fileset file must keep its exec bit through push→pull"
        );
        assert_eq!(modes["notes.md"].0, 0o644);
    }

    #[test]
    fn one_directory_packs_to_one_layer_digest_however_it_is_ordered_or_republished() {
        let doc = with_path_filesets(&["/x"]);
        let digest = |entries: Vec<FileEntry>| {
            build_artifact(&doc, &[entries])
                .unwrap()
                .fileset_layers()
                .next()
                .expect("a packed layer")
                .digest
                .clone()
        };
        assert_eq!(
            digest(vec![entry("a", "1"), entry("b", "2")]),
            digest(vec![entry("b", "2"), entry("a", "1")]),
            "identical directories have to dedupe at the blob level across artifacts, so neither entry order nor the clock may reach the digest"
        );
        assert_ne!(
            digest(vec![entry("a", "1"), entry("b", "2")]),
            digest(vec![entry("a", "changed"), entry("b", "2")]),
            "different content must change the digest"
        );
    }

    #[test]
    fn a_document_and_its_packed_directories_have_to_agree_in_number() {
        let err = build_artifact(&with_path_filesets(&["/x", "/y"]), &[vec![entry("a", "1")]])
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("declares 2 path fileset(s) but 1 were packed"),
            "a layer per entry is what makes the i-th entry addressable, so a mismatch has to refuse the build rather than publish an artifact no consumer can correlate; got: {err:#}"
        );
        let err = build(&with_path_filesets(&["/x"])).unwrap_err();
        assert!(
            format!("{err:#}").contains("declares 1 path fileset(s) but 0 were packed"),
            "got: {err:#}"
        );
    }

    #[test]
    fn a_mixin_carries_its_own_packed_filesets() {
        let doc = br#"{"apiVersion":"lns.run/v1","kind":"mixin","name":"skills","spec":{"filesets":[{"path":"./skills","guestPath":"/skills"}]}}"#;
        let built = build_artifact(doc, &[vec![entry("a.md", "shared")]])
            .expect("sharing one directory across sandboxes is publishing a mixin that carries it");
        assert_eq!(built.fileset_layers().count(), 1);
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
