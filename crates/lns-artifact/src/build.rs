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

/// Assemble a config-only OCI artifact (the standard shape for lens runtime kinds and bundles) from a validated manifest document.
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

/// The digest a bundle component is pinned to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentPin {
    pub name: String,
    pub digest: String,
}

fn pinned_digest(reference: &spec::ArtifactRef) -> Option<String> {
    if let Some(digest) = &reference.digest {
        return Some(digest.clone());
    }
    reference
        .reference
        .split_once('@')
        .map(|(_, digest)| digest.to_string())
        .filter(|d| d.starts_with("sha256:"))
}

fn declared_components(components: &spec::BundleComponents) -> Vec<(String, &spec::ArtifactRef)> {
    let mut declared: Vec<(String, &spec::ArtifactRef)> = Vec::new();
    if let Some(sandbox) = &components.sandbox {
        declared.push(("sandbox".to_string(), sandbox));
    }
    let indexed = [
        ("agent", &components.agents),
        ("fileset", &components.filesets),
        ("policy", &components.policies),
    ];
    for (kind, refs) in indexed {
        for (i, reference) in refs.iter().enumerate() {
            declared.push((format!("{kind}-{i}"), reference));
        }
    }
    declared
}

/// Verify every component of an AgentSystem bundle is pinned to a digest, returning the pins; refuses a component left on a floating tag so a published bundle can never drift.
pub fn bundle_component_pins(doc: &[u8]) -> Result<Vec<ComponentPin>> {
    let bundle = spec::parse_bundle(doc).context("reading bundle components")?;
    let mut pins = Vec::new();
    for (name, reference) in declared_components(&bundle.spec.components) {
        let Some(digest) = pinned_digest(reference) else {
            bail!(
                "component {name} ({}) is left on a floating tag; pin it to a digest",
                reference.reference
            );
        };
        pins.push(ComponentPin { name, digest });
    }
    Ok(pins)
}

/// The refs of every bundle component still on a floating tag, in declaration order and de-duplicated — the set a producer resolves to pin.
pub fn unpinned_component_refs(doc: &[u8]) -> Result<Vec<String>> {
    let bundle = spec::parse_bundle(doc).context("reading bundle components")?;
    let mut seen = std::collections::BTreeSet::new();
    let mut refs = Vec::new();
    for (_, reference) in declared_components(&bundle.spec.components) {
        if pinned_digest(reference).is_none() && seen.insert(reference.reference.clone()) {
            refs.push(reference.reference.clone());
        }
    }
    Ok(refs)
}

fn pin_component_value(
    component: &mut Value,
    digests: &std::collections::BTreeMap<String, String>,
) {
    let Some(object) = component.as_object_mut() else {
        return;
    };
    if object.contains_key("digest") {
        return;
    }
    let Some(reference) = object
        .get("ref")
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return;
    };
    if let Some(digest) = digests.get(&reference) {
        object.insert("digest".to_string(), Value::String(digest.clone()));
    }
}

/// Rewrite an AgentSystem bundle's config so every component whose ref appears in `digests` is pinned to it; a non-bundle doc and already-pinned components are returned unchanged.
pub fn pin_bundle_components(
    doc: &[u8],
    digests: &std::collections::BTreeMap<String, String>,
) -> Result<Vec<u8>> {
    let mut value: Value =
        serde_json::from_slice(doc).context("parsing bundle for component pinning")?;
    let Some(components) = value
        .pointer_mut("/spec/components")
        .and_then(Value::as_object_mut)
    else {
        return serde_json::to_vec(&value).context("serialising bundle");
    };
    for component in components.values_mut() {
        match component {
            Value::Array(entries) => entries
                .iter_mut()
                .for_each(|entry| pin_component_value(entry, digests)),
            _ => pin_component_value(component, digests),
        }
    }
    serde_json::to_vec(&value).context("serialising pinned bundle")
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

    #[test]
    fn build_artifact_refuses_a_manifest_carrying_a_real_secret() {
        let agent = format!(
            r#"{{"apiVersion":"lens.dev/v1alpha1","kind":"Agent","metadata":{{"name":"a"}},"spec":{{"command":"agent","env":{{"GH_TOKEN":"ghp_{}"}}}}}}"#,
            "a".repeat(36)
        );
        let err = build_artifact(agent.as_bytes()).unwrap_err();
        assert!(
            format!("{err:#}").contains("secret"),
            "a real secret must be refused before the artifact is assembled: {err:#}"
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

    fn bundle_json(components: &str) -> Vec<u8> {
        format!(
            r#"{{"apiVersion":"lens.dev/v1alpha1","kind":"AgentSystem","metadata":{{"name":"some-bundle"}},"spec":{{"components":{components}}}}}"#
        )
        .into_bytes()
    }

    #[test]
    fn bundle_component_pins_reports_a_digest_for_each_pinned_component() {
        let pinned = format!("sha256:{}", "a".repeat(64));
        let doc = bundle_json(&format!(
            r#"{{"sandbox":{{"ref":"reg/base","digest":"{pinned}"}},"agents":[{{"ref":"reg/agent@{pinned}"}}],"filesets":[{{"ref":"reg/skills@{pinned}"}}],"policies":[{{"ref":"reg/policy","digest":"{pinned}"}}]}}"#
        ));
        let pins = bundle_component_pins(&doc).unwrap();
        assert_eq!(pins.len(), 4);
        assert!(pins.iter().all(|p| p.digest == pinned));
        let names: Vec<&str> = pins.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["sandbox", "agent-0", "fileset-0", "policy-0"]);
    }

    #[test]
    fn bundle_component_pins_refuses_a_component_left_on_a_floating_tag() {
        let doc = bundle_json(r#"{"sandbox":{"ref":"reg/base:1"}}"#);
        let err = bundle_component_pins(&doc).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("floating tag"), "got: {msg}");
        assert!(msg.contains("reg/base:1"), "got: {msg}");
    }

    #[test]
    fn unpinned_component_refs_lists_only_the_floating_ones_across_every_kind() {
        let pinned = format!("sha256:{}", "a".repeat(64));
        let doc = bundle_json(&format!(
            r#"{{"sandbox":{{"ref":"reg/base:1"}},"agents":[{{"ref":"reg/agent","digest":"{pinned}"}},{{"ref":"reg/other:2"}}],"filesets":[{{"ref":"reg/skills@{pinned}"}}],"policies":[{{"ref":"reg/policy:3"}}]}}"#
        ));
        let refs = unpinned_component_refs(&doc).unwrap();
        assert_eq!(refs, ["reg/base:1", "reg/other:2", "reg/policy:3"]);
    }

    #[test]
    fn unpinned_component_refs_de_duplicates_a_ref_declared_twice() {
        let doc = bundle_json(r#"{"agents":[{"ref":"reg/agent:1"},{"ref":"reg/agent:1"}]}"#);
        assert_eq!(unpinned_component_refs(&doc).unwrap(), ["reg/agent:1"]);
    }

    #[test]
    fn pin_bundle_components_pins_the_resolved_refs_and_leaves_others_untouched() {
        let pinned = format!("sha256:{}", "a".repeat(64));
        let resolved = format!("sha256:{}", "b".repeat(64));
        let doc = bundle_json(&format!(
            r#"{{"sandbox":{{"ref":"reg/base:1"}},"agents":[{{"ref":"reg/agent","digest":"{pinned}"}}],"filesets":[{{"ref":"reg/skills:3"}}]}}"#
        ));
        let digests = std::collections::BTreeMap::from([
            ("reg/base:1".to_string(), resolved.clone()),
            ("reg/skills:3".to_string(), resolved.clone()),
        ]);
        let out = pin_bundle_components(&doc, &digests).unwrap();
        let value: Value = serde_json::from_slice(&out).unwrap();
        let components = &value["spec"]["components"];
        assert_eq!(components["sandbox"]["digest"], resolved);
        assert_eq!(components["filesets"][0]["digest"], resolved);
        assert_eq!(
            components["agents"][0]["digest"], pinned,
            "an already-pinned component keeps its digest"
        );
    }

    #[test]
    fn pin_bundle_components_ignores_a_non_object_or_ref_less_component_entry() {
        let doc = bundle_json(r#"{"sandbox":42,"agents":[{"name":"no-ref"}]}"#);
        let out = pin_bundle_components(&doc, &std::collections::BTreeMap::new()).unwrap();
        let value: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(value["spec"]["components"]["sandbox"], 42);
        assert!(
            value["spec"]["components"]["agents"][0]
                .get("digest")
                .is_none()
        );
    }

    #[test]
    fn pin_bundle_components_returns_a_doc_without_components_unchanged() {
        let doc = br#"{"apiVersion":"lens.dev/v1alpha1","kind":"Sandbox","metadata":{"name":"s"},"spec":{"isolation":"microvm"}}"#;
        let out = pin_bundle_components(doc, &std::collections::BTreeMap::new()).unwrap();
        let value: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(value["kind"], "Sandbox");
    }
}
