use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use lns_artifact::build::BuiltArtifact;

/// Parse a YAML or JSON manifest into canonical JSON bytes, or a human message on a parse failure.
pub(super) fn to_json(raw: &[u8]) -> std::result::Result<Vec<u8>, String> {
    let value: serde_json::Value =
        serde_yaml::from_slice(raw).map_err(|e| format!("not valid YAML or JSON: {e}"))?;
    serde_json::to_vec(&value).map_err(|e| format!("re-serialising manifest: {e}"))
}

/// Validate a manifest and print every problem; exit 0 when valid, 1 otherwise.
pub(super) fn check_and_report(raw: &[u8], path: &Path, writer: &mut impl Write) -> Result<i32> {
    let display = path.display();
    let json = match to_json(raw) {
        Ok(json) => json,
        Err(message) => {
            writeln!(writer, "✖ {display}: {message}")?;
            return Ok(1);
        }
    };
    match lns_artifact::validate::validate(&json) {
        Ok(()) => {
            writeln!(writer, "✔ {display}: valid")?;
            Ok(0)
        }
        Err(problems) => {
            writeln!(writer, "✖ {display}: {} problem(s)", problems.len())?;
            for problem in &problems {
                writeln!(writer, "  - {problem}")?;
            }
            Ok(1)
        }
    }
}

/// Build (validate + assemble) a manifest into an OCI artifact, printing the digest. Returns the artifact for a subsequent push, or `None` when the build failed.
pub(super) fn build_and_report(
    raw: &[u8],
    tag: Option<&str>,
    writer: &mut impl Write,
) -> Result<Option<BuiltArtifact>> {
    let json = match to_json(raw) {
        Ok(json) => json,
        Err(message) => {
            writeln!(writer, "✖ {message}")?;
            return Ok(None);
        }
    };
    match lns_artifact::build::build_artifact(&json) {
        Ok(built) => {
            let target = tag.unwrap_or("<untagged>");
            writeln!(
                writer,
                "✔ built {} {target}@{}",
                built.artifact_type, built.manifest_digest
            )?;
            Ok(Some(built))
        }
        Err(e) => {
            writeln!(writer, "✖ build failed: {e:#}")?;
            Ok(None)
        }
    }
}

/// Validate that a `--push` invocation names a target ref, returning it.
pub(super) fn push_target(tag: Option<&str>) -> Result<&str> {
    tag.context("--push requires a target ref: pass -t <ref>")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn sandbox_yaml(base_image: &str) -> String {
        format!(
            "apiVersion: lens.dev/v1alpha1\nkind: Sandbox\nmetadata:\n  name: some-sandbox\nspec:\n  isolation: microvm\n  baseImage: {base_image}\n"
        )
    }

    fn pinned() -> String {
        sandbox_yaml(&format!("reg/base@sha256:{}", "a".repeat(64)))
    }

    fn check(raw: &[u8]) -> (i32, String) {
        let mut out: Vec<u8> = Vec::new();
        let code = check_and_report(raw, &PathBuf::from("bundle.yaml"), &mut out).unwrap();
        (code, String::from_utf8(out).unwrap())
    }

    #[test]
    fn check_passes_a_valid_manifest() {
        let (code, out) = check(pinned().as_bytes());
        assert_eq!(code, 0);
        assert!(out.contains("valid"), "got: {out}");
    }

    #[test]
    fn check_lists_a_schema_problem() {
        let (code, out) = check(sandbox_yaml("reg/base:1").as_bytes());
        assert_eq!(code, 1);
        assert!(out.contains("digest-pinned"), "got: {out}");
    }

    #[test]
    fn check_reports_unparseable_input() {
        let (code, out) = check(b": : not : yaml :");
        assert_eq!(code, 1);
        assert!(out.contains("not valid YAML or JSON"), "got: {out}");
    }

    #[test]
    fn build_reports_the_digest_and_returns_the_artifact() {
        let mut out: Vec<u8> = Vec::new();
        let built = build_and_report(pinned().as_bytes(), Some("reg/some-sandbox:1"), &mut out)
            .unwrap()
            .expect("a valid manifest builds");
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("built"), "got: {text}");
        assert!(text.contains(&built.manifest_digest), "got: {text}");
        assert!(text.contains("reg/some-sandbox:1"), "got: {text}");
    }

    #[test]
    fn build_reports_an_untagged_target() {
        let mut out: Vec<u8> = Vec::new();
        build_and_report(pinned().as_bytes(), None, &mut out)
            .unwrap()
            .expect("builds without a tag");
        assert!(String::from_utf8(out).unwrap().contains("<untagged>"));
    }

    #[test]
    fn build_reports_a_failure_and_returns_none() {
        let mut out: Vec<u8> = Vec::new();
        let built =
            build_and_report(sandbox_yaml("reg/base:1").as_bytes(), None, &mut out).unwrap();
        assert!(built.is_none());
        assert!(String::from_utf8(out).unwrap().contains("build failed"));
    }

    #[test]
    fn build_reports_unparseable_input_and_returns_none() {
        let mut out: Vec<u8> = Vec::new();
        let built = build_and_report(b": : not : yaml :", None, &mut out).unwrap();
        assert!(built.is_none());
        assert!(
            String::from_utf8(out)
                .unwrap()
                .contains("not valid YAML or JSON")
        );
    }

    #[test]
    fn push_target_requires_a_ref() {
        assert_eq!(push_target(Some("reg/x:1")).unwrap(), "reg/x:1");
        assert!(
            format!("{:#}", push_target(None).unwrap_err())
                .contains("--push requires a target ref")
        );
    }
}
