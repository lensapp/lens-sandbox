use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, bail};
use lns_artifact::build::BuiltArtifact;

use super::author::Fs;
use crate::connector::LocalBoxFuture;

/// Builds a sandbox definition into an OCI artifact and uploads it, returning the pushed manifest digest; the real impl reuses the `lns login` credential, a fake drives the push scenarios offline.
pub trait Producer {
    fn build_and_push<'a>(
        &'a self,
        doc: &'a [u8],
        reference: &'a str,
    ) -> LocalBoxFuture<'a, Result<String>>;

    fn push_prebuilt<'a>(
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
}

/// What a declared entry was published as, so the publisher sees the version they shipped rather than having to read it back out of the registry.
#[derive(Debug)]
pub struct PinnedTool {
    pub declared: String,
    pub published: String,
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
        let exact = resolver
            .resolve(&tool)
            .await
            .with_context(|| format!("resolving {declared} for publishing"))?;
        let published = format!("{}@{exact}", tool.name);
        pinned.push(PinnedTool {
            declared: declared.to_string(),
            published: published.clone(),
        });
        *entry = serde_json::Value::String(published);
    }
    let doc = serde_json::to_vec(&value).context("serializing the tool-pinned definition")?;
    Ok((doc, pinned))
}

#[derive(Debug)]
pub struct PackedFileset {
    pub built: BuiltArtifact,
    pub reference: String,
}

/// Pack every path fileset into a FileSet artifact addressed by digest in the target repository, returning the definition rewritten to carry only digest-pinned refs; a pre-declared ref must already be digest-pinned.
pub fn pack_path_filesets<F: Fs + ?Sized>(
    fs: &F,
    cwd: &Path,
    doc: &[u8],
    reference: &str,
) -> Result<(Vec<u8>, Vec<PackedFileset>)> {
    let def = lns_artifact::sandbox::parse(doc)
        .map_err(|e| anyhow::anyhow!("refusing to push an invalid sandbox: {e:#}"))?;
    if def.spec.filesets.is_empty() {
        return Ok((doc.to_vec(), Vec::new()));
    }
    let target: oci_client::Reference = reference
        .parse()
        .with_context(|| format!("invalid target ref {reference}"))?;
    let mut value: serde_json::Value =
        serde_json::from_slice(doc).context("re-reading the definition for fileset pinning")?;
    let entries = value["spec"]["filesets"]
        .as_array_mut()
        .context("spec.filesets is not an array")?;
    let mut packed = Vec::new();
    for (index, fileset) in def.spec.filesets.iter().enumerate() {
        if let Some(path) = &fileset.path {
            let files = super::fileset::walk(fs, &cwd.join(path))
                .with_context(|| format!("fileset {path}"))?;
            let built = lns_artifact::build::build_fileset(
                &fileset_name(path),
                &fileset.mount_path,
                &files,
            )?;
            let pinned = format!(
                "{}/{}@{}",
                target.registry(),
                target.repository(),
                built.manifest_digest
            );
            entries[index] = serde_json::json!({
                "ref": pinned,
                "mountPath": fileset.mount_path,
                "owner": owner_str(fileset.owner),
            });
            packed.push(PackedFileset {
                built,
                reference: pinned,
            });
        } else if let Some(declared) = &fileset.reference
            && !lns_artifact::spec::is_digest_pinned_image(declared)
        {
            bail!(
                "fileset ref {declared} is not digest-pinned; a published sandbox pins every fileset by digest"
            );
        }
    }
    let rewritten = serde_json::to_vec(&value).context("serializing the pinned definition")?;
    Ok((rewritten, packed))
}

fn owner_str(owner: lns_artifact::sandbox::FilesetOwner) -> &'static str {
    match owner {
        lns_artifact::sandbox::FilesetOwner::Workload => "workload",
        lns_artifact::sandbox::FilesetOwner::Root => "root",
    }
}

fn fileset_name(path: &str) -> String {
    let base = Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let sanitized: String = base
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let sanitized = sanitized.trim_matches('-');
    let mut name = if sanitized.is_empty() {
        "fileset".to_string()
    } else {
        sanitized.to_string()
    };
    name.truncate(63);
    name.trim_end_matches('-').to_string()
}

/// `lns push <ref>`: validate the sandbox definition, pack and upload its path filesets, then build and upload the pinned definition as a sandbox artifact in one step. The caller reads `./lns.yaml` into `doc`.
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
    lns_artifact::validate::refuse_unprovisionable_tools(doc).map_err(|problem| {
        anyhow::anyhow!("refusing to push a sandbox no consumer can start: {problem}")
    })?;
    let (doc, packed) = pack_path_filesets(fs, cwd, doc, reference)?;
    let (doc, pinned_tools) = pin_declared_tools(resolver, &doc).await?;
    for fileset in &packed {
        producer
            .push_prebuilt(&fileset.built, &fileset.reference)
            .await?;
        writeln!(out, "pushed fileset {}", fileset.reference)?;
    }
    for tool in &pinned_tools {
        writeln!(out, "pinned {} → {}", tool.declared, tool.published)?;
    }
    let digest = producer.build_and_push(&doc, reference).await?;
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
    let (doc, packed) = pack_path_filesets(fs, cwd, doc, reference)?;
    for fileset in &packed {
        let bytes = blob_bytes(&fileset.built);
        writeln!(out, "would push fileset {} ({bytes})", fileset.reference)?;
    }
    let declared_tools = serde_json::from_slice::<serde_json::Value>(&doc)
        .ok()
        .and_then(|value| value["spec"]["tools"].as_array().map(Vec::len))
        .unwrap_or(0);
    if declared_tools > 0 {
        writeln!(
            out,
            "note: {declared_tools} tool version(s) resolve at push time; the published digest may differ from this preview"
        )?;
    }
    let built = lns_artifact::build::build_artifact(&doc)?;
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
        prebuilt: RefCell<Vec<String>>,
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
            _reference: &'a str,
        ) -> LocalBoxFuture<'a, Result<String>> {
            self.docs.borrow_mut().push(doc.to_vec());
            let outcome = self
                .outcome
                .clone()
                .expect("outcome set")
                .map_err(|message| anyhow::anyhow!(message));
            Box::pin(async move { outcome })
        }

        fn push_prebuilt<'a>(
            &'a self,
            _built: &'a BuiltArtifact,
            reference: &'a str,
        ) -> LocalBoxFuture<'a, Result<()>> {
            self.prebuilt.borrow_mut().push(reference.to_string());
            Box::pin(async move { Ok(()) })
        }
    }

    use crate::sandbox::test_support::MapFs;

    struct FakeResolver {
        versions: std::collections::HashMap<String, String>,
    }

    impl FakeResolver {
        fn with(entries: &[(&str, &str)]) -> Self {
            Self {
                versions: entries
                    .iter()
                    .map(|(spec, exact)| (spec.to_string(), exact.to_string()))
                    .collect(),
            }
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
    }

    /// An empty index: any resolution attempt fails, so a passing push proves the resolver was never consulted.
    fn unconsultable() -> FakeResolver {
        FakeResolver::with(&[])
    }

    fn fs_with_skills() -> MapFs {
        MapFs::with(&[("/work/skills/prompts.md", "p")])
    }

    const VALID: &[u8] =
        br#"{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{"name":"hermes"},"spec":{"image":"ghcr.io/team/base:1"}}"#;

    const WITH_PATH_FILESET: &[u8] = br#"{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{"name":"hermes"},"spec":{"image":"ghcr.io/team/base:1","filesets":[{"path":"./skills","mountPath":"/root/.agent/skills"}]}}"#;

    fn cwd() -> &'static Path {
        Path::new("/work")
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
        assert!(producer.prebuilt.borrow().is_empty());
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
            br#"{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{"name":"hermes"},"spec":{}}"#,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("invalid sandbox"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn push_packs_a_path_fileset_and_pins_it_into_the_published_config() {
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
        let prebuilt = producer.prebuilt.borrow();
        assert_eq!(prebuilt.len(), 1);
        let fileset_ref = &prebuilt[0];
        assert!(
            fileset_ref.starts_with("ghcr.io/team/hermes@sha256:"),
            "got: {fileset_ref}"
        );
        let docs = producer.docs.borrow();
        let published: serde_json::Value = serde_json::from_slice(&docs[0]).unwrap();
        let entry = &published["spec"]["filesets"][0];
        assert!(entry.get("path").is_none(), "got: {entry}");
        assert_eq!(entry["ref"], serde_json::Value::String(prebuilt[0].clone()));
        assert_eq!(entry["mountPath"], "/root/.agent/skills");
        assert_eq!(entry["owner"], "workload");
    }

    #[test]
    fn pack_preserves_an_explicit_root_owner_on_the_pinned_entry() {
        let doc = br#"{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{"name":"hermes"},"spec":{"image":"x:1","filesets":[{"path":"./skills","mountPath":"/opt/skills","owner":"root"}]}}"#;
        let (rewritten, packed) =
            pack_path_filesets(&fs_with_skills(), cwd(), doc, "ghcr.io/team/hermes:1.4.0").unwrap();
        assert_eq!(packed.len(), 1);
        let value: serde_json::Value = serde_json::from_slice(&rewritten).unwrap();
        assert_eq!(value["spec"]["filesets"][0]["owner"], "root");
    }

    #[tokio::test]
    async fn push_refuses_a_floating_declared_fileset_ref() {
        let producer = FakeProducer::err("must not reach the producer");
        let mut out = Vec::new();
        let err = push(
            &fs_with_skills(),
            cwd(),
            &producer,
            &unconsultable(),
            br#"{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{"name":"hermes"},"spec":{"image":"x:1","filesets":[{"ref":"registry.example.test/team/skills:latest","mountPath":"/s"}]}}"#,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("not digest-pinned"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn push_keeps_a_digest_pinned_declared_ref_verbatim() {
        let pinned = format!(
            "registry.example.test/team/skills@sha256:{}",
            "a".repeat(64)
        );
        let doc = format!(
            r#"{{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{{"name":"hermes"}},"spec":{{"image":"x:1","filesets":[{{"ref":"{pinned}","mountPath":"/s"}}]}}}}"#
        );
        let (rewritten, packed) = pack_path_filesets(
            &fs_with_skills(),
            cwd(),
            doc.as_bytes(),
            "ghcr.io/team/hermes:1.4.0",
        )
        .unwrap();
        assert!(packed.is_empty());
        let value: serde_json::Value = serde_json::from_slice(&rewritten).unwrap();
        assert_eq!(value["spec"]["filesets"][0]["ref"], pinned);
    }

    #[tokio::test]
    async fn push_refuses_a_declared_fileset_ref_with_a_malformed_digest() {
        let doc = br#"{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{"name":"hermes"},"spec":{"image":"x:1","filesets":[{"ref":"registry.example.test/team/skills@sha256:abc","mountPath":"/s"}]}}"#;
        let err = pack_path_filesets(&fs_with_skills(), cwd(), doc, "ghcr.io/team/hermes:1.4.0")
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("not digest-pinned"),
            "a truncated sha256 must not pass as pinned: {err:#}"
        );
    }

    const WITH_TOOLS: &[u8] = br#"{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{"name":"hermes"},"spec":{"image":"ghcr.io/team/base:1","tools":["node@22","python@latest"]}}"#;

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
        let doc = br#"{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{"name":"hermes"},"spec":{"image":"ghcr.io/team/base:1","tools":["prettier@3"]}}"#;
        let mut out = Vec::new();
        let err = push(
            &fs_with_skills(),
            cwd(),
            &producer,
            &unconsultable(),
            doc,
            "ghcr.io/team/hermes:1.4.0",
            &mut out,
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("no consumer can start")
                && format!("{err:#}").contains("bring it via spec.image"),
            "got: {err:#}"
        );
        assert!(
            producer.docs.borrow().is_empty(),
            "nothing is uploaded and the index is never consulted"
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

    #[tokio::test]
    async fn pin_declared_tools_leaves_an_empty_tool_list_untouched() {
        let doc = br#"{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{"name":"hermes"},"spec":{"image":"x:1","tools":[]}}"#;
        let (pinned, reported) = pin_declared_tools(&unconsultable(), doc).await.unwrap();
        assert_eq!(pinned, doc.to_vec());
        assert!(reported.is_empty());
    }

    #[tokio::test]
    async fn pin_declared_tools_refuses_a_non_string_entry() {
        let doc = br#"{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{"name":"hermes"},"spec":{"image":"x:1","tools":[42]}}"#;
        let err = pin_declared_tools(&unconsultable(), doc).await.unwrap_err();
        assert!(format!("{err:#}").contains("not a string"), "got: {err:#}");
    }

    #[test]
    fn fileset_name_sanitizes_to_a_dns_label() {
        assert_eq!(fileset_name("./skills"), "skills");
        assert_eq!(fileset_name("./My_Skills.v2"), "my-skills-v2");
        assert_eq!(fileset_name("./---"), "fileset");
        assert_eq!(
            fileset_name(&format!("./{}", "a".repeat(80))),
            "a".repeat(63)
        );
    }
}
