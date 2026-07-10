use std::io::Write;

use anyhow::Result;

use crate::integration::LocalBoxFuture;

/// Builds a sandbox definition into an OCI artifact and uploads it, returning the pushed manifest digest; the real impl reuses the `lns login` credential, a fake drives the push scenarios offline.
pub trait Producer {
    fn build_and_push<'a>(
        &'a self,
        doc: &'a [u8],
        reference: &'a str,
    ) -> LocalBoxFuture<'a, Result<String>>;
}

/// `lns push <ref>`: validate the sandbox definition, then build and upload it as a sandbox artifact in one step. The caller reads `./lns.yaml` into `doc`.
pub async fn push<P, W>(producer: &P, doc: &[u8], reference: &str, out: &mut W) -> Result<i32>
where
    P: Producer + ?Sized,
    W: Write,
{
    lns_artifact::sandbox::parse(doc)
        .map_err(|e| anyhow::anyhow!("refusing to push an invalid sandbox: {e:#}"))?;
    let digest = producer.build_and_push(doc, reference).await?;
    writeln!(out, "built and pushed {reference}@{digest}")?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    const VALID: &[u8] =
        br#"{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{"name":"hermes"},"spec":{"image":"ghcr.io/team/base:1"}}"#;

    #[tokio::test]
    async fn push_builds_then_reports_the_pushed_reference() {
        let producer = FakeProducer(Ok(format!("sha256:{}", "a".repeat(64))));
        let mut out = Vec::new();
        let code = push(&producer, VALID, "ghcr.io/team/hermes:1.4.0", &mut out)
            .await
            .unwrap();
        assert_eq!(code, 0);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("built"), "got: {text}");
        assert!(text.contains("ghcr.io/team/hermes:1.4.0"), "got: {text}");
    }

    #[tokio::test]
    async fn push_surfaces_a_producer_failure_naming_the_host() {
        let producer = FakeProducer(Err("credential for ghcr.io lacks push scope".to_string()));
        let mut out = Vec::new();
        let err = push(&producer, VALID, "ghcr.io/team/hermes:1.4.0", &mut out)
            .await
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("push scope"), "got: {msg}");
        assert!(msg.contains("ghcr.io"), "got: {msg}");
    }

    #[tokio::test]
    async fn push_refuses_an_invalid_sandbox_before_uploading() {
        let producer = FakeProducer(Err("must not reach the producer".to_string()));
        let mut out = Vec::new();
        let err = push(
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
}
