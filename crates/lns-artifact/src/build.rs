use crate::spec;
use anyhow::{Context, Result, bail};
use lns_policy::connectors::Connector;
use lns_policy::providers::is_self_identifying;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const EMPTY_MEDIA_TYPE: &str = "application/vnd.oci.empty.v1+json";
const MANIFEST_MEDIA_TYPE: &str = "application/vnd.oci.image.manifest.v1+json";
const LAYER_MEDIA_TYPE: &str = "application/vnd.oci.image.layer.v1.tar";

/// Maximum aggregate size of a FileSet artifact's tar content layer.
pub const MAX_FILESET_BYTES: u64 = 256 * 1024 * 1024;

/// Maximum number of regular files in a FileSet artifact.
pub const MAX_FILESET_ENTRIES: usize = 100_000;

/// One file to pack into a FileSet layer: a path relative to the mount root, its bytes, and its permission bits.
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

/// Pack a directory's files into a FileSet artifact mounting at `mount_path`; the tar layer is deterministic (entries sorted, uid/gid/mtime zeroed, each file's mode preserved) so identical inputs yield identical digests.
pub fn build_fileset(name: &str, mount_path: &str, entries: &[FileEntry]) -> Result<BuiltArtifact> {
    spec::validate_mount_path(mount_path).context("fileset mount path")?;
    validate_fileset_entries(entries, MAX_FILESET_BYTES, MAX_FILESET_ENTRIES)?;
    let layer = tar_layer(entries).context("packing fileset layer")?;
    validate_fileset_layer_size(layer.len() as u64, MAX_FILESET_BYTES)?;
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

/// Build a connector definition into a config-only OCI artifact, taking `id` as `metadata.name`; refuses anything a public registry artifact must not carry — an `oauth.clientSecret`, or an empty or unresolved `oauth.clientId`.
pub fn build_connector(connector: &Connector) -> Result<BuiltArtifact> {
    if connector
        .oauth
        .as_ref()
        .is_some_and(|oauth| oauth.client_secret.is_some())
    {
        bail!(
            "refusing to publish connector {:?}: it carries oauth.clientSecret, which a registry artifact would embed in the clear",
            connector.id
        );
    }
    if let Some(client_id) = connector
        .oauth
        .as_ref()
        .and_then(|oauth| oauth.client_id.as_deref())
        && (client_id.is_empty() || client_id.contains("${"))
    {
        bail!(
            "refusing to publish connector {:?}: oauth.clientId {client_id:?} is empty or still an unresolved ${{...}} reference; set the LNS_OAUTH_CLIENT_ID_* variable when publishing, or drop the clientId line to publish a token-paste connector",
            connector.id
        );
    }
    for placeholder in [
        connector
            .credential
            .as_ref()
            .map(|c| c.placeholder.as_str()),
        connector.oauth.as_ref().map(|o| o.placeholder.as_str()),
    ]
    .into_iter()
    .flatten()
    {
        if !is_self_identifying(placeholder) {
            bail!(
                "refusing to publish connector {:?}: placeholder {placeholder:?} is not self-identifying (must contain \"placeholder\" or \"lns\"), so a real credential could be embedded in the registry artifact",
                connector.id
            );
        }
    }
    connector.validate().map_err(|e| anyhow::anyhow!(e))?;
    let mut spec_value = serde_json::to_value(connector).context("serialising connector")?;
    if let Some(obj) = spec_value.as_object_mut() {
        obj.remove("id");
    }
    let envelope = json!({
        "apiVersion": spec::API_VERSION,
        "kind": spec::Kind::Connector.as_str(),
        "metadata": {"name": &connector.id},
        "spec": spec_value,
    });
    let envelope_bytes = serde_json::to_vec(&envelope).context("serialising connector envelope")?;
    build_artifact(&envelope_bytes)
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
            r#"{{"apiVersion":"lens.dev/v1alpha1","kind":"Sandbox","metadata":{{"name":"some-sandbox"}},"spec":{{"isolation":"microvm","baseImage":"reg/base@sha256:{}"}}}}"#,
            "a".repeat(64)
        )
        .into_bytes()
    }

    fn credential_connector() -> Connector {
        serde_json::from_value(json!({
            "id": "some-provider",
            "authKind": "credential",
            "routes": [{"match": "api.some-provider.example"}],
            "credential": {
                "envVar": "SOME_TOKEN",
                "placeholder": "lns-placeholder",
                "injections": [{"kind": "bearer_header", "domain": "api.some-provider.example"}]
            }
        }))
        .unwrap()
    }

    #[test]
    fn build_connector_produces_a_config_only_connector_artifact_that_round_trips() {
        let connector = credential_connector();
        let built = build_connector(&connector).unwrap();
        assert_eq!(
            built.artifact_type,
            "application/vnd.lens.connector.v1+json"
        );

        let manifest: Value = serde_json::from_slice(&built.manifest).unwrap();
        assert_eq!(manifest["layers"][0]["mediaType"], EMPTY_MEDIA_TYPE);

        let config = built
            .blobs
            .iter()
            .find(|b| b.media_type == spec::Kind::Connector.config_media_type())
            .unwrap();
        let round_trip = spec::parse_connector(&config.data).unwrap();
        assert_eq!(
            round_trip, connector,
            "the id must survive as metadata.name and the spec must round-trip"
        );
    }

    #[test]
    fn build_connector_refuses_an_embedded_client_secret() {
        let connector: Connector = serde_json::from_value(json!({
            "id": "some-oauth",
            "authKind": "oauth",
            "oauth": {
                "clientId": "some-client",
                "clientSecret": "some-secret",
                "deviceAuthorizationEndpoint": "https://api.some-oauth.example/device",
                "tokenEndpoint": "https://api.some-oauth.example/token",
                "envVar": "SOME_OAUTH_TOKEN",
                "placeholder": "lns-placeholder"
            }
        }))
        .unwrap();
        let err = build_connector(&connector).unwrap_err();
        assert!(
            format!("{err:#}").contains("clientSecret"),
            "publishing a secret-bearing connector would embed the secret in the registry: {err:#}"
        );
    }

    #[test]
    fn build_connector_refuses_an_unresolved_client_id_env_reference() {
        let connector: Connector = serde_json::from_value(json!({
            "id": "some-oauth",
            "authKind": "oauth",
            "oauth": {
                "clientId": "${LNS_OAUTH_CLIENT_ID_SOME}",
                "deviceAuthorizationEndpoint": "https://api.some-oauth.example/device",
                "tokenEndpoint": "https://api.some-oauth.example/token",
                "envVar": "SOME_OAUTH_TOKEN",
                "placeholder": "lns-placeholder"
            }
        }))
        .unwrap();
        let err = build_connector(&connector).unwrap_err();
        assert!(
            format!("{err:#}").contains("unresolved"),
            "an unresolved ${{...}} client id would be pushed verbatim and fail every puller's sign-in: {err:#}"
        );
    }

    #[test]
    fn build_connector_refuses_an_empty_client_id() {
        let connector: Connector = serde_json::from_value(json!({
            "id": "some-oauth",
            "authKind": "oauth",
            "oauth": {
                "clientId": "",
                "deviceAuthorizationEndpoint": "https://api.some-oauth.example/device",
                "tokenEndpoint": "https://api.some-oauth.example/token",
                "envVar": "SOME_OAUTH_TOKEN",
                "placeholder": "lns-placeholder"
            }
        }))
        .unwrap();
        let err = build_connector(&connector).unwrap_err();
        assert!(
            format!("{err:#}").contains("empty"),
            "a set-but-empty client id env var must fail loudly, not silently publish clientId \"\": {err:#}"
        );
    }

    #[test]
    fn build_connector_refuses_an_invalid_connector() {
        let connector: Connector = serde_json::from_value(json!({
            "id": "some-oauth",
            "authKind": "oauth"
        }))
        .unwrap();
        let err = build_connector(&connector).unwrap_err();
        assert!(
            format!("{err:#}").contains("no `oauth:` block"),
            "got: {err:#}"
        );
    }

    #[test]
    fn build_connector_builds_a_valid_oauth_connector() {
        let connector: Connector = serde_json::from_value(json!({
            "id": "some-oauth",
            "authKind": "oauth",
            "routes": [{"match": "api.some-oauth.example"}],
            "oauth": {
                "clientId": "some-client",
                "deviceAuthorizationEndpoint": "https://api.some-oauth.example/device",
                "tokenEndpoint": "https://api.some-oauth.example/token",
                "envVar": "SOME_OAUTH_TOKEN",
                "placeholder": "lns-placeholder",
                "injections": [{"kind": "bearer_header", "domain": "api.some-oauth.example"}]
            }
        }))
        .unwrap();
        let built = build_connector(&connector).unwrap();
        let config = built
            .blobs
            .iter()
            .find(|b| b.media_type == spec::Kind::Connector.config_media_type())
            .unwrap();
        assert_eq!(spec::parse_connector(&config.data).unwrap(), connector);
    }

    #[test]
    fn build_connector_refuses_a_non_self_identifying_placeholder() {
        let connector: Connector = serde_json::from_value(json!({
            "id": "some-provider",
            "authKind": "credential",
            "credential": {
                "envVar": "SOME_TOKEN",
                "placeholder": "ghx-realtokenlookingvalue",
                "injections": []
            }
        }))
        .unwrap();
        let err = build_connector(&connector).unwrap_err();
        assert!(
            format!("{err:#}").contains("self-identifying"),
            "a real-token placeholder would be embedded in the registry artifact and must be refused: {err:#}"
        );
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
        entry_mode(path, data, 0o644)
    }

    fn entry_mode(path: &str, data: &str, mode: u32) -> FileEntry {
        FileEntry {
            path: path.into(),
            data: data.as_bytes().to_vec(),
            mode,
        }
    }

    #[test]
    fn build_fileset_preserves_each_entrys_mode_in_the_tar_layer() {
        let built = build_fileset(
            "hooks",
            "/root/.agent/hooks",
            &[
                entry_mode("run.sh", "#!/bin/sh\n", 0o755),
                entry("notes.md", "x"),
            ],
        )
        .unwrap();
        let layer = built
            .blobs
            .iter()
            .find(|b| b.media_type == LAYER_MEDIA_TYPE)
            .expect("a tar layer blob");
        let modes: std::collections::BTreeMap<String, u32> =
            tar::Archive::new(layer.data.as_slice())
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
