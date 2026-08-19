use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, bail};
use lns_artifact::build::BuiltArtifact;

use super::author::Fs;
use crate::connector::LocalBoxFuture;

/// Uploads a built OCI artifact — its config blob, every packed fileset layer, then the manifest; the real impl reuses the `lns login` credential, a fake drives the push scenarios offline.
pub trait Producer {
    fn push_built<'a>(
        &'a self,
        built: &'a BuiltArtifact,
        reference: &'a str,
    ) -> LocalBoxFuture<'a, Result<()>>;
}

/// Resolves a declared tool's (possibly fuzzy) version to the exact version the published artifact pins, by consulting the tool's public version index; a fake scripts it offline.
pub trait ToolResolver {
    fn resolve<'a>(
        &'a self,
        tool: &'a lns_artifact::tools::ToolRef,
    ) -> LocalBoxFuture<'a, Result<String>>;

    fn verify<'a>(
        &'a self,
        tool: &'a lns_artifact::tools::ToolRef,
    ) -> LocalBoxFuture<'a, IndexVerification>;
}

/// What the index said about an already-exact pin: best-effort only, because a required answer would break offline re-push.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IndexVerification {
    Confirmed,
    Absent,
    Unavailable,
}

/// What a declared entry was published as, so the publisher sees the version they shipped rather than having to read it back out of the registry.
#[derive(Debug)]
pub struct PinnedTool {
    pub declared: String,
    pub published: String,
    pub verification: Option<IndexVerification>,
}

/// Rewrite `spec.tools` so every entry carries the exact version the index resolves today — the tool analogue of digest-pinning path filesets at push.
pub async fn pin_declared_tools<R: ToolResolver + ?Sized>(
    resolver: &R,
    doc: &[u8],
) -> Result<(Vec<u8>, Vec<PinnedTool>)> {
    let mut value: serde_json::Value =
        serde_json::from_slice(doc).context("re-reading the definition for tool pinning")?;
    let Some(entries) = value["spec"]["tools"].as_array_mut() else {
        return Ok((doc.to_vec(), Vec::new()));
    };
    if entries.is_empty() {
        return Ok((doc.to_vec(), Vec::new()));
    }
    let mut pinned = Vec::with_capacity(entries.len());
    for entry in entries {
        let declared = entry.as_str().context("spec.tools entry is not a string")?;
        let tool = lns_artifact::tools::parse(declared)?;
        // An entry that already names an exact version is its own pin; re-publishing must not need the index, which may be blocked or may have dropped that version — so its verification is best-effort, never a veto.
        let (published, verification) = if lns_artifact::tools::is_exact_version(&tool.version) {
            (declared.to_string(), Some(resolver.verify(&tool).await))
        } else {
            let exact = resolver
                .resolve(&tool)
                .await
                .with_context(|| format!("resolving {declared} for publishing"))?;
            (format!("{}@{exact}", tool.name), None)
        };
        pinned.push(PinnedTool {
            declared: declared.to_string(),
            published: published.clone(),
            verification,
        });
        *entry = serde_json::Value::String(published);
    }
    let doc = serde_json::to_vec(&value).context("serializing the tool-pinned definition")?;
    Ok((doc, pinned))
}

/// The directory one `filesets[].path` entry ships, read off the author's machine ready to pack.
#[derive(Debug)]
pub struct PackedDirectory {
    pub path: String,
    pub files: Vec<lns_artifact::build::FileEntry>,
}

/// Read every `path` fileset the document declares, in declaration order, so each becomes a layer of the artifact this document configures (`docs/sandbox-spec.md` §6). The entry itself publishes untouched: it keeps its `path`, `guestPath` and `owner`.
pub fn pack_path_filesets<F: Fs + ?Sized>(
    fs: &F,
    cwd: &Path,
    doc: &[u8],
) -> Result<Vec<PackedDirectory>> {
    let def = lns_artifact::sandbox::parse_document(doc)
        .map_err(|e| anyhow::anyhow!("refusing to push an invalid document: {e:#}"))?;
    refuse_unpinned_mixins(&def)?;
    def.spec
        .filesets
        .iter()
        .filter_map(|fileset| fileset.path.as_deref())
        .map(|path| {
            let files = super::fileset::walk(fs, &cwd.join(path))
                .with_context(|| format!("fileset {path}"))?;
            Ok(PackedDirectory {
                path: path.to_string(),
                files,
            })
        })
        .collect()
}

/// Build the artifact a push uploads: the document, plus one layer per `path` fileset it declares.
fn build<F: Fs + ?Sized>(
    fs: &F,
    cwd: &Path,
    doc: &[u8],
) -> Result<(BuiltArtifact, Vec<PackedDirectory>)> {
    let packed = pack_path_filesets(fs, cwd, doc)?;
    let layers: Vec<Vec<lns_artifact::build::FileEntry>> =
        packed.iter().map(|dir| dir.files.clone()).collect();
    let built = lns_artifact::build::build_artifact(doc, &layers)?;
    Ok((built, packed))
}

/// What the publisher sees for each directory that became a layer: the entry they wrote, and the digest its content published under.
fn report_packed<W: Write>(
    out: &mut W,
    verb: &str,
    built: &BuiltArtifact,
    packed: &[PackedDirectory],
) -> Result<()> {
    for (dir, layer) in packed.iter().zip(built.fileset_layers()) {
        writeln!(
            out,
            "{verb} fileset {} -> {} ({} bytes)",
            dir.path,
            layer.digest,
            layer.data.len()
        )?;
    }
    Ok(())
}

/// A local directory means nothing on the machine that pulls the published document, and §3.3.1 pins a published `spec.mixins` entry by digest — so publish refuses what authoring accepts, exactly as it does for a fileset `path`.
fn refuse_unpinned_mixins(def: &lns_artifact::sandbox::Definition) -> Result<()> {
    for reference in &def.spec.mixins {
        if !lns_artifact::spec::is_digest_pinned_image(reference) {
            bail!(
                "mixin {reference} is not digest-pinned; a published document pins every mixin by digest so every consumer resolves the same one"
            );
        }
    }
    Ok(())
}

fn refuse_unpushable_tools(doc: &[u8]) -> Result<()> {
    lns_artifact::validate::refuse_unprovisionable_tools(doc).map_err(|problem| {
        anyhow::anyhow!("refusing to push a sandbox no consumer can start: {problem}")
    })
}

/// `lns push <ref>`: validate the document, pack each of its path filesets into a layer of the same artifact, and upload the whole thing in one step. The caller reads `./lns.yaml` into `doc`.
pub async fn push<F, P, R, W>(
    fs: &F,
    cwd: &Path,
    producer: &P,
    resolver: &R,
    doc: &[u8],
    reference: &str,
    out: &mut W,
) -> Result<i32>
where
    F: Fs + ?Sized,
    P: Producer + ?Sized,
    R: ToolResolver + ?Sized,
    W: Write,
{
    refuse_unpushable_tools(doc)?;
    // Packing before resolution reads the directories offline, so a broken fileset refuses the push without consulting the index.
    pack_path_filesets(fs, cwd, doc)?;
    let (doc, pinned_tools) = pin_declared_tools(resolver, doc).await?;
    let (built, packed) = build(fs, cwd, &doc)?;
    report_packed(out, "packed", &built, &packed)?;
    for tool in &pinned_tools {
        if tool.verification == Some(IndexVerification::Absent) {
            writeln!(
                out,
                "warning: the version index does not list {} today; consumers will provision it exactly as declared",
                tool.published
            )?;
        }
        writeln!(out, "pinned {} → {}", tool.declared, tool.published)?;
    }
    producer.push_built(&built, reference).await?;
    writeln!(
        out,
        "built and pushed {reference}@{}",
        built.manifest_digest
    )?;
    Ok(0)
}

/// `lns push --dry-run <ref>`: everything a push validates, packs, and builds — offline, printing the digests that would publish; nothing is uploaded.
pub fn push_dry_run<F, W>(
    fs: &F,
    cwd: &Path,
    doc: &[u8],
    reference: &str,
    out: &mut W,
) -> Result<i32>
where
    F: Fs + ?Sized,
    W: Write,
{
    refuse_unpushable_tools(doc)?;
    let (built, packed) = build(fs, cwd, doc)?;
    report_packed(out, "would pack", &built, &packed)?;
    // Only a fuzzy entry can change the published bytes; an exact pin is short-circuited, so counting it would promise a difference that cannot happen.
    let unresolved_tools = serde_json::from_slice::<serde_json::Value>(doc)
        .ok()
        .and_then(|value| value["spec"]["tools"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|entry| {
            entry
                .as_str()
                .and_then(|entry| lns_artifact::tools::parse(entry).ok())
        })
        .filter(|tool| !lns_artifact::tools::is_exact_version(&tool.version))
        .count();
    if unresolved_tools > 0 {
        writeln!(
            out,
            "note: {unresolved_tools} tool version(s) resolve at push time; the published digest may differ from this preview"
        )?;
    }
    let bytes = blob_bytes(&built);
    writeln!(
        out,
        "would push {reference}@{} ({bytes})",
        built.manifest_digest
    )?;
    writeln!(out, "dry run — built and validated; nothing uploaded")?;
    Ok(0)
}

fn blob_bytes(built: &BuiltArtifact) -> String {
    let bytes: usize = built.blobs.iter().map(|blob| blob.data.len()).sum();
    format!("{bytes} bytes")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Default)]
    struct FakeProducer {
        failure: Option<String>,
        uploaded: RefCell<Vec<BuiltArtifact>>,
    }

    impl FakeProducer {
        fn ok() -> Self {
            Self::default()
        }

        fn err(message: &str) -> Self {
            Self {
                failure: Some(message.to_string()),
                ..Default::default()
            }
        }

        /// The document the registry received, which is what a consumer will read.
        fn published(&self) -> serde_json::Value {
            let uploaded = self.uploaded.borrow();
            let built = uploaded.first().expect("an artifact was uploaded");
            let config = built
                .blobs
                .iter()
                .find(|blob| blob.media_type.ends_with(".config.v1+json"))
                .expect("the config blob travels with the artifact");
            serde_json::from_slice(&config.data).expect("the config blob is json")
        }

        fn packed_layers(&self) -> usize {
            self.uploaded
                .borrow()
                .first()
                .map(|built| built.fileset_layers().count())
                .unwrap_or_default()
        }
    }

    impl Producer for FakeProducer {
        fn push_built<'a>(
            &'a self,
            built: &'a BuiltArtifact,
            _reference: &'a str,
        ) -> LocalBoxFuture<'a, Result<()>> {
            self.uploaded.borrow_mut().push(built.clone());
            let failure = self.failure.clone();
            Box::pin(async move {
                match failure {
                    Some(message) => Err(anyhow::anyhow!(message)),
                    None => Ok(()),
                }
            })
        }
    }

    use crate::sandbox::test_support::MapFs;

    struct FakeResolver {
        versions: std::collections::HashMap<String, String>,
        verifications: std::collections::HashMap<String, IndexVerification>,
    }

    impl FakeResolver {
        fn with(entries: &[(&str, &str)]) -> Self {
            Self {
                versions: entries
                    .iter()
                    .map(|(spec, exact)| (spec.to_string(), exact.to_string()))
                    .collect(),
                verifications: std::collections::HashMap::new(),
            }
        }

        fn verifying(mut self, spec: &str, verification: IndexVerification) -> Self {
            self.verifications.insert(spec.to_string(), verification);
            self
        }
    }

    impl ToolResolver for FakeResolver {
        fn resolve<'a>(
            &'a self,
            tool: &'a lns_artifact::tools::ToolRef,
        ) -> LocalBoxFuture<'a, Result<String>> {
            let outcome = self
                .versions
                .get(&tool.to_string())
                .cloned()
                .ok_or_else(|| {
                    anyhow::anyhow!("tool {:?} is unknown to the version index", tool.name)
                });
            Box::pin(async move { outcome })
        }

        fn verify<'a>(
            &'a self,
            tool: &'a lns_artifact::tools::ToolRef,
        ) -> LocalBoxFuture<'a, IndexVerification> {
            let verification = self
                .verifications
                .get(&tool.to_string())
                .copied()
                .unwrap_or(IndexVerification::Unavailable);
            Box::pin(async move { verification })
        }
    }

    /// An empty index: any resolution attempt fails, so a passing push proves the resolver was never consulted.
    fn unconsultable() -> FakeResolver {
        FakeResolver::with(&[])
    }

    fn fs_with_skills() -> MapFs {
        MapFs::with(&[("/work/skills/prompts.md", "p")])
    }

    const VALID: &[u8] =
        br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"ghcr.io/team/base:1"}}"#;

    const WITH_PATH_FILESET: &[u8] = br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"ghcr.io/team/base:1","filesets":[{"path":"./skills","guestPath":"/root/.agent/skills"}]}}"#;

    fn cwd() -> &'static Path {
        Path::new("/work")
    }

    fn unsupported_backend_doc(with_fileset: bool) -> (String, Vec<u8>) {
        let tool = lns_artifact::tools::registry::backends()
            .find(|(_, backend)| !lns_artifact::tools::registry::is_supported_backend(backend))
            .map(|(name, _)| name.to_string())
            .expect("the snapshot carries at least one unsupported-backend entry");
        let fileset = if with_fileset {
            r#","filesets":[{"path":"./skills","guestPath":"/root/.agent/skills"}]"#
        } else {
            ""
        };
        let doc = format!(
            r#"{{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{{"image":"ghcr.io/team/base:1"{fileset},"tools":["{tool}@1"]}}}}"#
        )
        .into_bytes();
        (tool, doc)
    }

    #[tokio::test]
    async fn push_builds_then_reports_the_pushed_reference() {
        let producer = FakeProducer::ok();
        let mut out = Vec::new();
        let code = push(
            &fs_with_skills(),
            cwd(),
            &producer,
            &unconsultable(),
            VALID,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap();
        assert_eq!(code, 0);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("built"), "got: {text}");
        assert!(text.contains("ghcr.io/team/hermes:1.4.0"), "got: {text}");
        assert_eq!(
            producer.packed_layers(),
            0,
            "a document declaring no path fileset publishes config-only"
        );
    }

    #[tokio::test]
    async fn push_surfaces_a_producer_failure_naming_the_host() {
        let producer = FakeProducer::err("credential for ghcr.io lacks push scope");
        let mut out = Vec::new();
        let err = push(
            &fs_with_skills(),
            cwd(),
            &producer,
            &unconsultable(),
            VALID,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("push scope"), "got: {msg}");
        assert!(msg.contains("ghcr.io"), "got: {msg}");
    }

    #[tokio::test]
    async fn push_refuses_an_invalid_sandbox_before_uploading() {
        let producer = FakeProducer::err("must not reach the producer");
        let mut out = Vec::new();
        let err = push(
            &fs_with_skills(),
            cwd(),
            &producer,
            &unconsultable(),
            br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{}}"#,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("invalid document"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn push_packs_a_path_fileset_into_a_layer_of_the_same_artifact() {
        let producer = FakeProducer::ok();
        let mut out = Vec::new();
        let code = push(
            &fs_with_skills(),
            cwd(),
            &producer,
            &unconsultable(),
            WITH_PATH_FILESET,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap();
        assert_eq!(code, 0);
        assert_eq!(
            producer.uploaded.borrow().len(),
            1,
            "a fileset is not a separate artifact, so one push is one upload"
        );
        assert_eq!(producer.packed_layers(), 1);

        let entry = &producer.published()["spec"]["filesets"][0];
        assert_eq!(
            entry["path"], "./skills",
            "the published entry keeps its path; the content is now part of this artifact's digest (docs/sandbox-spec.md §6)"
        );
        assert_eq!(entry["guestPath"], "/root/.agent/skills");
        assert!(
            entry.get("ref").is_none(),
            "a fileset is not a separate artifact, so nothing in the published entry names one: {entry}"
        );

        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("packed fileset ./skills -> sha256:"),
            "the publisher sees the digest their directory shipped under: {text}"
        );
    }

    #[test]
    fn packing_reads_each_declared_directory_in_declaration_order() {
        let fs = MapFs::with(&[
            ("/work/skills/prompts.md", "p"),
            ("/work/hooks/run.sh", "h"),
        ]);
        let doc = br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"x:1","filesets":[{"path":"./skills","guestPath":"/opt/skills","owner":"root"},{"inline":{"a.md":"x"},"guestPath":"/notes"},{"path":"./hooks","guestPath":"/opt/hooks"}]}}"#;
        let packed = pack_path_filesets(&fs, cwd(), doc).unwrap();
        assert_eq!(
            packed
                .iter()
                .map(|dir| dir.path.as_str())
                .collect::<Vec<_>>(),
            ["./skills", "./hooks"],
            "the i-th path entry owns the i-th layer, so an inline entry in between must not consume a layer"
        );
    }

    #[tokio::test]
    async fn push_keeps_an_explicit_root_owner_on_the_published_entry() {
        let producer = FakeProducer::ok();
        let doc = br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"x:1","filesets":[{"path":"./skills","guestPath":"/opt/skills","owner":"root"}]}}"#;
        let mut out = Vec::new();
        push(
            &fs_with_skills(),
            cwd(),
            &producer,
            &unconsultable(),
            doc,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap();
        assert_eq!(
            producer.published()["spec"]["filesets"][0]["owner"],
            "root",
            "owner pins inputs the workload must not touch, so publishing must not drop it"
        );
    }

    #[tokio::test]
    async fn push_publishes_a_mixin_rather_than_refusing_a_kind_it_does_not_run() {
        let producer = FakeProducer::ok();
        let mut out = Vec::new();
        let code = push(
            &fs_with_skills(),
            cwd(),
            &producer,
            &unconsultable(),
            br#"{"apiVersion":"lns.run/v1","kind":"mixin","name":"postgres-tools","spec":{"env":{"MODE":"research"}}}"#,
            "ghcr.io/acme/postgres-tools:1.4.0",
            &mut out,
        )
        .await
        .expect("a mixin is an artifact, published like any other");
        assert_eq!(code, 0);
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("ghcr.io/acme/postgres-tools:1.4.0"),
            "got: {text}"
        );
    }

    #[tokio::test]
    async fn push_refuses_a_mixin_reference_a_consumer_could_not_resolve() {
        let producer = FakeProducer::err("must not reach the producer");
        let mut out = Vec::new();
        let err = push(
            &fs_with_skills(),
            cwd(),
            &producer,
            &unconsultable(),
            br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"x:1","mixins":["./mixins/postgres-tools/"]}}"#,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("mixin ./mixins/postgres-tools/ is not digest-pinned"),
            "a directory beside the author's file means nothing to a consumer, so publishing one would ship a reference nobody else can resolve; got: {err:#}"
        );
    }

    const WITH_TOOLS: &[u8] = br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"ghcr.io/team/base:1","tools":["node@22","python@latest"]}}"#;

    #[tokio::test]
    async fn push_pins_resolved_tool_versions_into_the_published_config() {
        let producer = FakeProducer::ok();
        let resolver = FakeResolver::with(&[("node@22", "22.11.0"), ("python@latest", "3.12.6")]);
        let mut out = Vec::new();
        let code = push(
            &fs_with_skills(),
            cwd(),
            &producer,
            &resolver,
            WITH_TOOLS,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap();
        assert_eq!(code, 0);
        let published = producer.published();
        assert_eq!(
            published["spec"]["tools"],
            serde_json::json!(["node@22.11.0", "python@3.12.6"])
        );
        assert_eq!(published["spec"]["image"], "ghcr.io/team/base:1");
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("pinned node@22 → node@22.11.0")
                && text.contains("pinned python@latest → python@3.12.6"),
            "the publisher sees the versions they shipped: {text}"
        );
    }

    #[tokio::test]
    async fn push_refuses_a_tool_no_consumer_could_provision() {
        let producer = FakeProducer::ok();
        let (unsupported, doc) = unsupported_backend_doc(false);
        let mut out = Vec::new();
        let err = push(
            &fs_with_skills(),
            cwd(),
            &producer,
            &unconsultable(),
            &doc,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("no consumer can start")
                && format!("{err:#}").contains("bring it via spec.image")
                && format!("{err:#}").contains(&unsupported),
            "got: {err:#}"
        );
        assert!(
            producer.uploaded.borrow().is_empty(),
            "nothing is uploaded and the index is never consulted"
        );
    }

    #[test]
    fn push_dry_run_refuses_a_tool_no_consumer_could_provision() {
        let (unsupported, doc) = unsupported_backend_doc(true);
        let mut out = Vec::new();
        let err = push_dry_run(
            &fs_with_skills(),
            cwd(),
            &doc,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .unwrap_err();

        let message = format!("{err:#}");
        assert!(
            message.contains("no consumer can start")
                && message.contains("bring it via spec.image")
                && message.contains(&unsupported),
            "got: {message}"
        );
        assert!(
            out.is_empty(),
            "a refused dry run must not print a successful push preview"
        );
    }

    #[tokio::test]
    async fn an_already_exact_pin_publishes_when_the_index_is_unavailable() {
        // Re-publishing must not fail because the index is blocked or has dropped that version.
        let producer = FakeProducer::ok();
        let doc = br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"ghcr.io/team/base:1","tools":["node@22.11.0"]}}"#;
        let mut out = Vec::new();
        push(
            &fs_with_skills(),
            cwd(),
            &producer,
            &unconsultable(),
            doc,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap();
        assert_eq!(
            producer.published()["spec"]["tools"],
            serde_json::json!(["node@22.11.0"])
        );
        assert!(
            String::from_utf8(out)
                .unwrap()
                .contains("pinned node@22.11.0"),
            "the entry is still disclosed as published"
        );
    }

    #[tokio::test]
    async fn an_exact_pin_the_index_no_longer_lists_warns_and_still_publishes() {
        // Best-effort verification: the publisher hears about a likely typo, but the index has no veto — it also drops old valid versions.
        let producer = FakeProducer::ok();
        let resolver =
            FakeResolver::with(&[]).verifying("java@temurin-9.9.9+9", IndexVerification::Absent);
        let doc = br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"ghcr.io/team/base:1","tools":["java@temurin-9.9.9+9"]}}"#;
        let mut out = Vec::new();
        let code = push(
            &fs_with_skills(),
            cwd(),
            &producer,
            &resolver,
            doc,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap();
        assert_eq!(code, 0, "the warning never blocks the push");
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("warning") && text.contains("java@temurin-9.9.9+9"),
            "got: {text}"
        );
        assert_eq!(
            producer.published()["spec"]["tools"],
            serde_json::json!(["java@temurin-9.9.9+9"])
        );
    }

    #[tokio::test]
    async fn a_confirmed_exact_pin_publishes_silently() {
        let producer = FakeProducer::ok();
        let resolver = FakeResolver::with(&[])
            .verifying("java@temurin-21.0.5+11.0.LTS", IndexVerification::Confirmed);
        let doc = br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"ghcr.io/team/base:1","tools":["java@temurin-21.0.5+11.0.LTS"]}}"#;
        let mut out = Vec::new();
        push(
            &fs_with_skills(),
            cwd(),
            &producer,
            &resolver,
            doc,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(!text.contains("warning"), "got: {text}");
    }

    #[tokio::test]
    async fn a_vendor_exact_pin_publishes_when_the_index_is_unavailable() {
        // The resolver itself emits vendor versions, so a re-push of its own output must not need the index back.
        let producer = FakeProducer::ok();
        let doc = br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"ghcr.io/team/base:1","tools":["java@temurin-21.0.5+11.0.LTS"]}}"#;
        let mut out = Vec::new();
        push(
            &fs_with_skills(),
            cwd(),
            &producer,
            &unconsultable(),
            doc,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap();
        assert_eq!(
            producer.published()["spec"]["tools"],
            serde_json::json!(["java@temurin-21.0.5+11.0.LTS"])
        );
        assert!(
            !String::from_utf8_lossy(&out).contains("warning"),
            "an unanswerable index is not the publisher's problem"
        );
    }

    #[tokio::test]
    async fn push_refuses_when_the_index_lacks_the_tool() {
        let producer = FakeProducer::err("must not reach the producer");
        let resolver = FakeResolver::with(&[]);
        let mut out = Vec::new();
        let err = push(
            &fs_with_skills(),
            cwd(),
            &producer,
            &resolver,
            WITH_TOOLS,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("resolving node@22") && msg.contains("unknown to the version index"),
            "got: {msg}"
        );
        assert!(producer.uploaded.borrow().is_empty(), "nothing must upload");
    }

    #[tokio::test]
    async fn push_without_tools_never_consults_the_resolver() {
        let producer = FakeProducer::ok();
        let mut out = Vec::new();
        push(
            &fs_with_skills(),
            cwd(),
            &producer,
            &unconsultable(),
            VALID,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap();
    }

    #[test]
    fn push_dry_run_notes_that_tool_versions_resolve_at_push_time() {
        let mut out = Vec::new();
        push_dry_run(
            &fs_with_skills(),
            cwd(),
            WITH_TOOLS,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("note: 2 tool version(s) resolve at push time"),
            "got: {text}"
        );
        let mut quiet = Vec::new();
        push_dry_run(
            &fs_with_skills(),
            cwd(),
            VALID,
            "ghcr.io/team/hermes:1.4.0",
            &mut quiet,
        )
        .unwrap();
        let text = String::from_utf8(quiet).unwrap();
        assert!(!text.contains("note:"), "got: {text}");
    }

    #[test]
    fn a_dry_run_of_already_pinned_tools_promises_the_digest_it_previewed() {
        // push short-circuits exact pins without consulting the index, so the real push produces the same bytes — warning otherwise defeats the point of an offline preview.
        let doc = br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"ghcr.io/team/base:1","tools":["node@22.11.0","python@3.12.6","java@temurin-21.0.5+11.0.LTS"]}}"#;
        let mut out = Vec::new();
        push_dry_run(
            &fs_with_skills(),
            cwd(),
            doc,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(!text.contains("may differ"), "got: {text}");

        let mixed = br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"ghcr.io/team/base:1","tools":["node@22.11.0","jq@latest"]}}"#;
        let mut mixed_out = Vec::new();
        push_dry_run(
            &fs_with_skills(),
            cwd(),
            mixed,
            "ghcr.io/team/hermes:1.4.0",
            &mut mixed_out,
        )
        .unwrap();
        let text = String::from_utf8(mixed_out).unwrap();
        assert!(
            text.contains("note: 1 tool version(s) resolve at push time"),
            "the one fuzzy entry is still called out: {text}"
        );
    }

    #[tokio::test]
    async fn pin_declared_tools_leaves_an_empty_tool_list_untouched() {
        let doc = br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"x:1","tools":[]}}"#;
        let (pinned, reported) = pin_declared_tools(&unconsultable(), doc).await.unwrap();
        assert_eq!(pinned, doc.to_vec());
        assert!(reported.is_empty());
    }

    #[tokio::test]
    async fn pin_declared_tools_refuses_a_non_string_entry() {
        let doc = br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"x:1","tools":[42]}}"#;
        let err = pin_declared_tools(&unconsultable(), doc).await.unwrap_err();
        assert!(format!("{err:#}").contains("not a string"), "got: {err:#}");
    }

    #[test]
    fn a_dry_run_previews_the_layer_digest_each_directory_would_publish_under() {
        let mut out = Vec::new();
        push_dry_run(
            &fs_with_skills(),
            cwd(),
            WITH_PATH_FILESET,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("would pack fileset ./skills -> sha256:"),
            "a dry run says what would publish, and the layer digest is part of what the manifest would carry: {text}"
        );
        assert!(text.contains("nothing uploaded"), "got: {text}");
    }
}
