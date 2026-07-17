use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, bail};
use lns_artifact::build::BuiltArtifact;

use super::author::Fs;
use crate::integration::LocalBoxFuture;

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
            });
            packed.push(PackedFileset {
                built,
                reference: pinned,
            });
        } else if let Some(declared) = &fileset.reference
            && !declared.contains("@sha256:")
        {
            bail!(
                "fileset ref {declared} is not digest-pinned; a published sandbox pins every fileset by digest"
            );
        }
    }
    let rewritten = serde_json::to_vec(&value).context("serializing the pinned definition")?;
    Ok((rewritten, packed))
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
pub async fn push<F, P, W>(
    fs: &F,
    cwd: &Path,
    producer: &P,
    doc: &[u8],
    reference: &str,
    out: &mut W,
) -> Result<i32>
where
    F: Fs + ?Sized,
    P: Producer + ?Sized,
    W: Write,
{
    let (doc, packed) = pack_path_filesets(fs, cwd, doc, reference)?;
    for fileset in &packed {
        producer
            .push_prebuilt(&fileset.built, &fileset.reference)
            .await?;
        writeln!(out, "pushed fileset {}", fileset.reference)?;
    }
    let digest = producer.build_and_push(&doc, reference).await?;
    writeln!(out, "built and pushed {reference}@{digest}")?;
    Ok(0)
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
    }

    #[tokio::test]
    async fn push_refuses_a_floating_declared_fileset_ref() {
        let producer = FakeProducer::err("must not reach the producer");
        let mut out = Vec::new();
        let err = push(
            &fs_with_skills(),
            cwd(),
            &producer,
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
        let doc = br#"{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{"name":"hermes"},"spec":{"image":"x:1","filesets":[{"ref":"registry.example.test/team/skills@sha256:abc","mountPath":"/s"}]}}"#;
        let (rewritten, packed) =
            pack_path_filesets(&fs_with_skills(), cwd(), doc, "ghcr.io/team/hermes:1.4.0").unwrap();
        assert!(packed.is_empty());
        let value: serde_json::Value = serde_json::from_slice(&rewritten).unwrap();
        assert_eq!(
            value["spec"]["filesets"][0]["ref"],
            "registry.example.test/team/skills@sha256:abc"
        );
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
