use std::io::Write;
use std::path::Path;

use anyhow::Result;

use crate::integration::LocalBoxFuture;
use crate::sandbox::author::{Fs, load_definition_json};

/// Builds a sandbox definition into an OCI artifact and uploads it, returning the pushed manifest digest; the real impl reuses the `lns login` credential, a fake drives the push scenarios offline.
pub trait Producer {
    fn build_and_push<'a>(
        &'a self,
        doc: &'a [u8],
        reference: &'a str,
    ) -> LocalBoxFuture<'a, Result<String>>;
}

/// `lns push <ref>`: validate ./lns.yaml, then build and upload it as a sandbox artifact in one step.
pub async fn push<P, F, W>(
    producer: &P,
    fs: &F,
    cwd: &Path,
    reference: &str,
    out: &mut W,
) -> Result<i32>
where
    P: Producer + ?Sized,
    F: Fs,
    W: Write,
{
    let doc = load_definition_json(fs, cwd)?;
    lns_artifact::sandbox::parse(&doc)
        .map_err(|e| anyhow::anyhow!("refusing to push an invalid sandbox: {e:#}"))?;
    let digest = producer.build_and_push(&doc, reference).await?;
    writeln!(out, "built and pushed {reference}@{digest}")?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::path::PathBuf;

    struct FakeFs {
        files: RefCell<HashMap<PathBuf, String>>,
    }

    impl FakeFs {
        fn with(contents: &str) -> Self {
            let files = RefCell::new(HashMap::new());
            files
                .borrow_mut()
                .insert(PathBuf::from("/work/lns.yaml"), contents.to_string());
            Self { files }
        }
        fn empty() -> Self {
            Self {
                files: RefCell::new(HashMap::new()),
            }
        }
    }

    impl Fs for FakeFs {
        fn read_to_string(&self, path: &Path) -> std::io::Result<String> {
            self.files
                .borrow()
                .get(path)
                .cloned()
                .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no such file"))
        }
        fn write(&self, path: &Path, contents: &str) -> std::io::Result<()> {
            self.files
                .borrow_mut()
                .insert(path.to_path_buf(), contents.to_string());
            Ok(())
        }
        fn exists(&self, path: &Path) -> bool {
            self.files.borrow().contains_key(path)
        }
    }

    struct FakeProducer(Result<String, String>);

    impl Producer for FakeProducer {
        fn build_and_push<'a>(
            &'a self,
            _doc: &'a [u8],
            _reference: &'a str,
        ) -> LocalBoxFuture<'a, Result<String>> {
            let outcome = self.0.clone().map_err(|message| anyhow::anyhow!(message));
            Box::pin(async move { outcome })
        }
    }

    const VALID: &str = "apiVersion: lns.run/v1\nkind: Sandbox\nmetadata:\n  name: hermes\nspec:\n  image: ghcr.io/team/base:1\n";

    fn cwd() -> &'static Path {
        Path::new("/work")
    }

    #[tokio::test]
    async fn push_builds_then_reports_the_pushed_reference() {
        let fs = FakeFs::with(VALID);
        let producer = FakeProducer(Ok(format!("sha256:{}", "a".repeat(64))));
        let mut out = Vec::new();
        let code = push(&producer, &fs, cwd(), "ghcr.io/team/hermes:1.4.0", &mut out)
            .await
            .unwrap();
        assert_eq!(code, 0);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("built"), "got: {text}");
        assert!(text.contains("ghcr.io/team/hermes:1.4.0"), "got: {text}");
    }

    #[tokio::test]
    async fn push_surfaces_a_producer_failure_naming_the_host() {
        let fs = FakeFs::with(VALID);
        let producer = FakeProducer(Err("credential for ghcr.io lacks push scope".to_string()));
        let mut out = Vec::new();
        let err = push(&producer, &fs, cwd(), "ghcr.io/team/hermes:1.4.0", &mut out)
            .await
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("push scope"), "got: {msg}");
        assert!(msg.contains("ghcr.io"), "got: {msg}");
    }

    #[tokio::test]
    async fn push_refuses_when_there_is_no_definition() {
        let fs = FakeFs::empty();
        let producer = FakeProducer(Ok("sha256:unused".to_string()));
        let mut out = Vec::new();
        let err = push(&producer, &fs, cwd(), "ghcr.io/team/hermes:1.4.0", &mut out)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("lns init"), "got: {err:#}");
    }

    #[tokio::test]
    async fn push_refuses_an_invalid_sandbox_before_uploading() {
        let fs = FakeFs::with(
            "apiVersion: lns.run/v1\nkind: Sandbox\nmetadata:\n  name: hermes\nspec: {}\n",
        );
        let producer = FakeProducer(Err("must not reach the producer".to_string()));
        let mut out = Vec::new();
        let err = push(&producer, &fs, cwd(), "ghcr.io/team/hermes:1.4.0", &mut out)
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("invalid sandbox"),
            "got: {err:#}"
        );
    }
}
