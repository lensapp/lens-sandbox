use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

/// The operating system every image lns builds and every guest it boots declares.
pub const OS: &str = "linux";

/// The media type of the index a push publishes and a pull selects from.
pub const INDEX_MEDIA_TYPE: &str = "application/vnd.oci.image.index.v1+json";

/// The architectures lns builds for, so a push knows which per-architecture tags to look under and a refusal knows which hosts could add one.
pub const ARCHITECTURES: [&str; 2] = ["amd64", "arm64"];

/// The annotation an entry carries when a host Docker daemon built it rather than a build guest, so the digest a consumer verifies covers the record (`docs/sandbox-spec.md` §6.2).
pub const BUILT_OUTSIDE_THE_GATE_ANNOTATION: &str = "run.lns.built-outside-the-gate";

/// What every line about such an image says, so an approver never has to ask which engine ran the build (`docs/sandbox-spec.md` §3.1.1).
pub const BUILT_OUTSIDE_THE_GATE: &str = "built outside the gate by the host Docker daemon";

/// One architecture's image, as the index addresses it (`docs/sandbox-spec.md` §6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexEntry {
    pub digest: String,
    pub size: u64,
    pub media_type: String,
    pub os: String,
    pub architecture: String,
    /// True when a host Docker daemon built this architecture, so the document's egress decided nothing about what it fetched (§3.1.1).
    pub built_outside_the_gate: bool,
}

impl IndexEntry {
    /// How a line about this entry names the platform it holds.
    pub fn platform(&self) -> String {
        format!("{}/{}", self.os, self.architecture)
    }
}

/// The index bytes a push uploads, and the digest the published document names them by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssembledIndex {
    pub bytes: Vec<u8>,
    pub digest: String,
}

/// An index as a registry served it: the digest a document pins it by, taken over the bytes that arrived, and the entries those bytes hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldIndex {
    pub digest: String,
    pub entries: Vec<IndexEntry>,
}

/// What one manifest document is, read from the bytes a registry served: an index a pull selects an entry from, or a plain manifest, which is nothing an index holds.
pub fn held(bytes: &[u8]) -> Result<Option<HeldIndex>> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).context("reading the manifest a registry served")?;
    if !value["manifests"].is_array() {
        return Ok(None);
    }
    Ok(Some(HeldIndex {
        digest: digest_of(bytes),
        entries: parse(bytes)?,
    }))
}

fn digest_of(bytes: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

/// The tag one architecture's image manifest publishes under, beside the artifact that names it: only that architecture's push writes it, so the index stays derivable from the tags after any race (§6).
pub fn architecture_tag(artifact_tag: &str, os: &str, architecture: &str) -> String {
    format!("{artifact_tag}-image-{os}-{architecture}")
}

/// The tag the assembled index publishes under, so the manifests it names stay reachable from a tag rather than only from a digest a garbage collector cannot see.
pub fn index_tag(artifact_tag: &str) -> String {
    format!("{artifact_tag}-image")
}

/// The tag part of a push reference, which is what every image tag beside it is derived from; a reference that names none publishes under `latest`, as a registry reads it.
pub fn tag_of(reference: &str) -> &str {
    let without_digest = reference
        .split_once('@')
        .map_or(reference, |(head, _)| head);
    match without_digest.rfind(':') {
        Some(colon) if !without_digest[colon..].contains('/') => &without_digest[colon + 1..],
        _ => "latest",
    }
}

/// Add this architecture's entry to what the repository already holds, replacing the entry of the same platform and keeping every other, in a fixed order so two pushes of the same set assemble the same bytes.
pub fn with_entry(held: &[IndexEntry], entry: IndexEntry) -> Vec<IndexEntry> {
    let mut entries: Vec<IndexEntry> = held
        .iter()
        .filter(|held| held.os != entry.os || held.architecture != entry.architecture)
        .cloned()
        .collect();
    entries.push(entry);
    in_index_order(entries)
}

/// The one order an index is assembled and read in, so the same set of entries is the same bytes and a push that adds nothing names the index the last push named.
pub fn in_index_order(mut entries: Vec<IndexEntry>) -> Vec<IndexEntry> {
    entries.sort_by(|a, b| (&a.os, &a.architecture).cmp(&(&b.os, &b.architecture)));
    entries
}

/// The index document over these entries, with the digest a document names it by.
pub fn assemble(entries: &[IndexEntry]) -> Result<AssembledIndex> {
    if entries.is_empty() {
        bail!("an image index names at least one architecture, and this one names none");
    }
    let manifests: Vec<serde_json::Value> = entries
        .iter()
        .map(|entry| {
            let mut descriptor = serde_json::json!({
                "mediaType": entry.media_type,
                "digest": entry.digest,
                "size": entry.size,
                "platform": { "architecture": entry.architecture, "os": entry.os },
            });
            if entry.built_outside_the_gate {
                descriptor["annotations"] =
                    serde_json::json!({ BUILT_OUTSIDE_THE_GATE_ANNOTATION: "true" });
            }
            descriptor
        })
        .collect();
    let index = serde_json::json!({
        "schemaVersion": 2,
        "mediaType": INDEX_MEDIA_TYPE,
        "manifests": manifests,
    });
    let bytes = serde_json::to_vec(&index).context("serializing the image index")?;
    let digest = digest_of(&bytes);
    Ok(AssembledIndex { bytes, digest })
}

/// The entries of an index document as the registry served it; an entry naming no platform is not one a pull can select, so it is left out.
pub fn parse(bytes: &[u8]) -> Result<Vec<IndexEntry>> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).context("reading the image index")?;
    let manifests = value["manifests"]
        .as_array()
        .context("the image index names no manifests")?;
    Ok(manifests
        .iter()
        .filter_map(|manifest| {
            Some(IndexEntry {
                digest: manifest["digest"].as_str()?.to_string(),
                size: manifest["size"].as_u64().unwrap_or_default(),
                media_type: manifest["mediaType"].as_str()?.to_string(),
                os: manifest["platform"]["os"].as_str()?.to_string(),
                architecture: manifest["platform"]["architecture"].as_str()?.to_string(),
                built_outside_the_gate: manifest["annotations"][BUILT_OUTSIDE_THE_GATE_ANNOTATION]
                    == "true",
            })
        })
        .collect())
}

/// Which entry this host boots from: the one whose platform is the host's, named by digest, so a run pulls exactly the manifest the index pinned.
pub fn select<'a>(
    entries: &'a [IndexEntry],
    os: &str,
    architecture: &str,
) -> Option<&'a IndexEntry> {
    entries
        .iter()
        .find(|entry| entry.os == os && entry.architecture == architecture)
}

/// The platforms an index holds, in the order it holds them, as every line that lists them spells them.
pub fn platforms(entries: &[IndexEntry]) -> Vec<String> {
    entries.iter().map(IndexEntry::platform).collect()
}

/// A host nobody built for is told what the index does hold and what would add its own architecture — the build always happens on the machine that pushes, so the command names that host (§6).
pub fn refuse_missing_architecture(
    reference: &str,
    entries: &[IndexEntry],
    os: &str,
    architecture: &str,
) -> String {
    format!(
        "the image index {reference} holds {}, and this host is {os}/{architecture}; \
         nobody has built this document for {os}/{architecture} — run `lns push` for it on an {architecture} host to add it",
        platforms(entries).join(", "),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(architecture: &str, digest: &str) -> IndexEntry {
        IndexEntry {
            digest: digest.to_string(),
            size: 512,
            media_type: "application/vnd.oci.image.manifest.v1+json".to_string(),
            os: OS.to_string(),
            architecture: architecture.to_string(),
            built_outside_the_gate: false,
        }
    }

    fn outside_the_gate(architecture: &str, digest: &str) -> IndexEntry {
        IndexEntry {
            built_outside_the_gate: true,
            ..entry(architecture, digest)
        }
    }

    #[test]
    fn an_image_the_host_daemon_built_is_annotated_on_the_entry_the_index_holds() {
        let assembled =
            assemble(&[outside_the_gate("arm64", "sha256:aa")]).expect("assembling the index");
        let value: serde_json::Value =
            serde_json::from_slice(&assembled.bytes).expect("the index is json");
        assert_eq!(
            value["manifests"][0]["annotations"][BUILT_OUTSIDE_THE_GATE_ANNOTATION], "true",
            "the record rides on the entry, so the digest a consumer verifies covers it"
        );
    }

    #[test]
    fn an_image_a_build_guest_built_carries_no_annotation_at_all() {
        let assembled = assemble(&[entry("arm64", "sha256:aa")]).expect("assembling the index");
        let value: serde_json::Value =
            serde_json::from_slice(&assembled.bytes).expect("the index is json");
        assert_eq!(
            value["manifests"][0]["annotations"],
            serde_json::Value::Null,
            "a gated build says nothing rather than saying false"
        );
    }

    #[test]
    fn what_the_index_records_about_the_gate_reads_back_per_architecture() {
        let entries = vec![
            outside_the_gate("amd64", "sha256:bb"),
            entry("arm64", "sha256:aa"),
        ];
        let assembled = assemble(&entries).expect("assembling the index");
        assert_eq!(
            parse(&assembled.bytes).expect("parsing"),
            entries,
            "one host may build through its daemon while another builds in a guest"
        );
    }

    #[test]
    fn an_entry_annotated_with_anything_but_true_is_not_one_built_outside_the_gate() {
        let entries = parse(
            br#"{"manifests":[
                {"mediaType":"application/vnd.oci.image.manifest.v1+json","digest":"sha256:aa","size":1,"platform":{"os":"linux","architecture":"arm64"},"annotations":{"run.lns.built-outside-the-gate":"false"}}
            ]}"#,
        )
        .expect("parsing");
        assert!(!entries[0].built_outside_the_gate);
    }

    #[test]
    fn a_push_from_a_second_architecture_adds_an_entry_and_keeps_the_first() {
        let held = vec![entry("arm64", "sha256:aa")];
        let entries = with_entry(&held, entry("amd64", "sha256:bb"));
        assert_eq!(
            entries,
            vec![entry("amd64", "sha256:bb"), entry("arm64", "sha256:aa")],
            "a second architecture adds to the index rather than replacing what it holds"
        );
    }

    #[test]
    fn a_second_push_from_one_architecture_replaces_only_its_own_entry() {
        let held = vec![entry("arm64", "sha256:aa"), entry("amd64", "sha256:bb")];
        let entries = with_entry(&held, entry("arm64", "sha256:cc"));
        assert_eq!(
            entries,
            vec![entry("amd64", "sha256:bb"), entry("arm64", "sha256:cc")],
            "an architecture owns its own entry and nobody else's"
        );
    }

    #[test]
    fn the_same_set_of_entries_assembles_to_the_same_digest_whatever_order_it_arrives_in() {
        let one = with_entry(&[entry("arm64", "sha256:aa")], entry("amd64", "sha256:bb"));
        let other = with_entry(&[entry("amd64", "sha256:bb")], entry("arm64", "sha256:aa"));
        assert_eq!(
            assemble(&one).expect("assembling").digest,
            assemble(&other).expect("assembling").digest,
            "a push that adds nothing must name the index the last push named"
        );
    }

    #[test]
    fn an_assembled_index_reads_back_as_the_entries_it_was_assembled_from() {
        let entries = vec![entry("amd64", "sha256:bb"), entry("arm64", "sha256:aa")];
        let assembled = assemble(&entries).expect("assembling");
        assert_eq!(parse(&assembled.bytes).expect("parsing"), entries);
        let value: serde_json::Value =
            serde_json::from_slice(&assembled.bytes).expect("the index is json");
        assert_eq!(value["mediaType"], INDEX_MEDIA_TYPE);
        assert_eq!(value["schemaVersion"], 2);
    }

    #[test]
    fn an_index_a_registry_served_is_read_back_with_the_digest_a_document_pins_it_by() {
        let entries = vec![entry("amd64", "sha256:bb"), entry("arm64", "sha256:aa")];
        let assembled = assemble(&entries).expect("assembling");
        let held = held(&assembled.bytes)
            .expect("reading the index")
            .expect("an index is what these bytes are");
        assert_eq!(
            held.digest, assembled.digest,
            "the pin a document names is taken over the bytes that arrived, not over a header"
        );
        assert_eq!(held.entries, entries);
    }

    #[test]
    fn a_plain_manifest_is_no_index_and_holds_no_entry() {
        let manifest = br#"{"schemaVersion":2,"mediaType":"application/vnd.oci.image.manifest.v1+json","config":{"digest":"sha256:cc"},"layers":[]}"#;
        assert_eq!(
            held(manifest).expect("reading the manifest"),
            None,
            "a pull of a plain manifest has no index to verify a pin against"
        );
        assert!(held(b"not json").is_err());
    }

    #[test]
    fn an_index_over_no_entry_is_refused_rather_than_published_empty() {
        let err = assemble(&[]).unwrap_err();
        assert!(format!("{err:#}").contains("names none"), "{err:#}");
    }

    #[test]
    fn an_index_document_that_is_not_one_is_refused_by_the_reader() {
        assert!(parse(b"not json").is_err());
        assert!(parse(br#"{"schemaVersion":2}"#).is_err());
    }

    #[test]
    fn an_entry_naming_no_platform_is_not_one_a_pull_could_select() {
        let entries = parse(
            br#"{"manifests":[
                {"mediaType":"application/vnd.oci.image.manifest.v1+json","digest":"sha256:aa","size":1,"platform":{"os":"linux","architecture":"arm64"}},
                {"mediaType":"application/vnd.oci.image.manifest.v1+json","digest":"sha256:cc","size":1,"annotations":{"vnd.docker.reference.type":"attestation-manifest"}}
            ]}"#,
        )
        .expect("parsing");
        assert_eq!(platforms(&entries), vec!["linux/arm64"]);
    }

    #[test]
    fn a_pull_takes_the_entry_for_this_host_by_digest() {
        let entries = vec![entry("amd64", "sha256:bb"), entry("arm64", "sha256:aa")];
        assert_eq!(
            select(&entries, OS, "arm64").map(|entry| entry.digest.as_str()),
            Some("sha256:aa"),
            "the host boots the manifest its own architecture published"
        );
        assert_eq!(select(&entries, OS, "riscv64"), None);
        assert_eq!(
            select(&entries, "windows", "amd64"),
            None,
            "an entry for another operating system is not this host's"
        );
    }

    #[test]
    fn a_host_nobody_built_for_is_told_what_is_held_and_what_would_add_its_own() {
        let refusal = refuse_missing_architecture(
            "ghcr.io/team/hermes@sha256:ee",
            &[entry("arm64", "sha256:aa")],
            OS,
            "amd64",
        );
        assert!(refusal.contains("linux/arm64"), "{refusal}");
        assert!(refusal.contains("linux/amd64"), "{refusal}");
        assert!(
            refusal.contains("lns push") && refusal.contains("amd64 host"),
            "the build happens where the push runs, so the refusal names that host: {refusal}"
        );
    }

    #[test]
    fn each_architecture_publishes_under_a_tag_only_its_own_pushes_write() {
        assert_eq!(
            architecture_tag("1.4.0", OS, "arm64"),
            "1.4.0-image-linux-arm64"
        );
        assert_eq!(index_tag("1.4.0"), "1.4.0-image");
    }

    #[test]
    fn the_tags_beside_an_artifact_are_derived_from_the_tag_it_publishes_under() {
        assert_eq!(tag_of("ghcr.io/team/hermes:1.4.0"), "1.4.0");
        assert_eq!(tag_of("localhost:5000/team/hermes:1.4.0"), "1.4.0");
        assert_eq!(
            tag_of("ghcr.io/team/hermes"),
            "latest",
            "a reference that names no tag publishes under the one a registry reads it as"
        );
        assert_eq!(tag_of("localhost:5000/team/hermes"), "latest");
        assert_eq!(tag_of("ghcr.io/team/hermes@sha256:aa"), "latest");
    }
}
