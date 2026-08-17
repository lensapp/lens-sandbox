use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, bail};
use lns_artifact::build::{BuiltArtifact, FileEntry};

use super::author::Fs;
use crate::connector::LocalBoxFuture;

/// Builds a sandbox definition and its packed filesets into one OCI artifact and uploads it, returning the pushed manifest digest; the real impl reuses the `lns login` credential, a fake drives the push scenarios offline.
pub trait Producer {
    fn build_and_push<'a>(
        &'a self,
        doc: &'a [u8],
        path_filesets: &'a [Vec<FileEntry>],
        reference: &'a str,
    ) -> LocalBoxFuture<'a, Result<String>>;
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

/// Read every `path` fileset's directory in declaration order, so publish can pack one layer per entry into the artifact the document configures (§7); the document is validated first, because a fileset that no consumer could resolve must not reach the registry.
pub fn read_path_filesets<F: Fs + ?Sized>(
    fs: &F,
    cwd: &Path,
    doc: &[u8],
) -> Result<Vec<Vec<FileEntry>>> {
    let def = lns_artifact::sandbox::parse_document(doc)
        .map_err(|e| anyhow::anyhow!("refusing to push an invalid document: {e:#}"))?;
    refuse_unpinned_mixins(&def)?;
    def.spec
        .filesets
        .iter()
        .filter_map(|fileset| fileset.path.as_ref())
        .map(|path| {
            super::fileset::walk(fs, &cwd.join(path)).with_context(|| format!("fileset {path}"))
        })
        .collect()
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

/// `lns push <ref>`: validate the sandbox definition, pack its path filesets, then build and upload the pinned definition as one sandbox artifact. The caller reads `./lns.yaml` into `doc`.
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
    let path_filesets = read_path_filesets(fs, cwd, doc)?;
    let (doc, pinned_tools) = pin_declared_tools(resolver, doc).await?;
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
    let digest = producer
        .build_and_push(&doc, &path_filesets, reference)
        .await?;
    writeln!(out, "built and pushed {reference}@{digest}")?;
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
    let path_filesets = read_path_filesets(fs, cwd, doc)?;
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
    let built = lns_artifact::build::build_artifact(doc, &path_filesets)?;
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
        outcome: Option<Result<String, String>>,
        docs: RefCell<Vec<Vec<u8>>>,
        packed: RefCell<Vec<Vec<Vec<FileEntry>>>>,
    }

    impl FakeProducer {
        fn ok(digest: &str) -> Self {
            Self {
                outcome: Some(Ok(digest.to_string())),
                ..Default::default()
            }
        }
        fn err(message: &str) -> Self {
            Self {
                outcome: Some(Err(message.to_string())),
                ..Default::default()
            }
        }
    }

    impl Producer for FakeProducer {
        fn build_and_push<'a>(
            &'a self,
            doc: &'a [u8],
            path_filesets: &'a [Vec<FileEntry>],
            _reference: &'a str,
        ) -> LocalBoxFuture<'a, Result<String>> {
            self.docs.borrow_mut().push(doc.to_vec());
            self.packed.borrow_mut().push(path_filesets.to_vec());
            let outcome = self
                .outcome
                .clone()
                .expect("outcome set")
                .map_err(|message| anyhow::anyhow!(message));
            Box::pin(async move { outcome })
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

    const WITH_TOOLS: &[u8] = br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"ghcr.io/team/base:1","tools":["node@22","python@latest"]}}"#;

    const WITH_PATH_FILESET: &[u8] = br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"ghcr.io/team/base:1","filesets":[{"path":"./skills","mountPath":"/root/.agent/skills"}]}}"#;

    fn cwd() -> &'static Path {
        Path::new("/work")
    }

    fn unsupported_backend_doc(with_fileset: bool) -> (String, Vec<u8>) {
        let tool = lns_artifact::tools::registry::backends()
            .find(|(_, backend)| !lns_artifact::tools::registry::is_supported_backend(backend))
            .map(|(name, _)| name.to_string())
            .expect("the snapshot carries at least one unsupported-backend entry");
        let fileset = if with_fileset {
            r#","filesets":[{"path":"./skills","mountPath":"/root/.agent/skills"}]"#
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
        let producer = FakeProducer::ok(&format!("sha256:{}", "a".repeat(64)));
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
        assert!(
            producer.packed.borrow()[0].is_empty(),
            "a document with no path fileset packs no layer"
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
    async fn push_packs_a_path_fileset_into_the_sandbox_artifact_and_publishes_the_entry_as_written()
     {
        let producer = FakeProducer::ok(&format!("sha256:{}", "a".repeat(64)));
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
        let packed = producer.packed.borrow();
        assert_eq!(packed[0].len(), 1, "one packed layer per path entry");
        assert_eq!(packed[0][0][0].path, "prompts.md");
        let docs = producer.docs.borrow();
        let published: serde_json::Value = serde_json::from_slice(&docs[0]).unwrap();
        let entry = &published["spec"]["filesets"][0];
        assert_eq!(
            entry["path"], "./skills",
            "§6: the entry keeps its path, because the content now lives in this artifact: {entry}"
        );
        assert_eq!(entry["mountPath"], "/root/.agent/skills");
        assert!(
            String::from_utf8(out).unwrap().find("fileset").is_none(),
            "a path fileset is no longer a thing of its own to push"
        );
    }

    #[test]
    fn reading_path_filesets_walks_each_declared_directory_in_order() {
        let doc = br#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"x:1","filesets":[{"inline":{"a.md":"x"},"mountPath":"/notes"},{"path":"./skills","mountPath":"/opt/skills","owner":"root"}]}}"#;
        let packed = read_path_filesets(&fs_with_skills(), cwd(), doc).unwrap();
        assert_eq!(
            packed.len(),
            1,
            "an inline entry ships in the config blob, so it claims no layer"
        );
        assert_eq!(packed[0][0].path, "prompts.md");
    }

    #[tokio::test]
    async fn push_publishes_a_mixin_rather_than_refusing_a_kind_it_does_not_run() {
        let producer = FakeProducer::ok(&format!("sha256:{}", "b".repeat(64)));
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
        .expect("a mixin is a kit, published like any other");
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

    #[tokio::test]
    async fn push_pins_resolved_tool_versions_into_the_published_config() {
        let producer = FakeProducer::ok(&format!("sha256:{}", "a".repeat(64)));
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
        let docs = producer.docs.borrow();
        let published: serde_json::Value = serde_json::from_slice(&docs[0]).unwrap();
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
        let producer = FakeProducer::ok(&format!("sha256:{}", "a".repeat(64)));
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
            producer.docs.borrow().is_empty(),
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
        let producer = FakeProducer::ok(&format!("sha256:{}", "a".repeat(64)));
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
        let docs = producer.docs.borrow();
        let published: serde_json::Value = serde_json::from_slice(&docs[0]).unwrap();
        assert_eq!(
            published["spec"]["tools"],
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
        let producer = FakeProducer::ok(&format!("sha256:{}", "a".repeat(64)));
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
        let docs = producer.docs.borrow();
        let published: serde_json::Value = serde_json::from_slice(&docs[0]).unwrap();
        assert_eq!(
            published["spec"]["tools"],
            serde_json::json!(["java@temurin-9.9.9+9"])
        );
    }

    #[tokio::test]
    async fn a_confirmed_exact_pin_publishes_silently() {
        let producer = FakeProducer::ok(&format!("sha256:{}", "a".repeat(64)));
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
        let producer = FakeProducer::ok(&format!("sha256:{}", "a".repeat(64)));
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
        let docs = producer.docs.borrow();
        let published: serde_json::Value = serde_json::from_slice(&docs[0]).unwrap();
        assert_eq!(
            published["spec"]["tools"],
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
        assert!(producer.docs.borrow().is_empty(), "nothing must upload");
    }

    #[tokio::test]
    async fn push_without_tools_never_consults_the_resolver() {
        let producer = FakeProducer::ok(&format!("sha256:{}", "a".repeat(64)));
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
}
