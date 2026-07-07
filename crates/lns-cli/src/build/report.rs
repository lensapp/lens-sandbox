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
    let mut problems = lns_artifact::validate::validate(&json)
        .err()
        .unwrap_or_default();
    problems.extend(policy_network_problem(&json));
    if problems.is_empty() {
        writeln!(writer, "✔ {display}: valid")?;
        Ok(0)
    } else {
        writeln!(writer, "✖ {display}: {} problem(s)", problems.len())?;
        for problem in &problems {
            writeln!(writer, "  - {problem}")?;
        }
        Ok(1)
    }
}

/// A Policy manifest's network rules aren't modelled by the shared schema validator, so check them here at build (fail-closed before push) against the runtime policy type. Returns a problem only for a Policy whose `spec.network` is present but malformed.
fn policy_network_problem(json: &[u8]) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(json).ok()?;
    if value.get("kind").and_then(|k| k.as_str()) != Some("Policy") {
        return None;
    }
    let network = value.get("spec").and_then(|spec| spec.get("network"))?;
    serde_json::from_value::<lns_policy::NetworkPolicy>(network.clone())
        .err()
        .map(|e| format!("malformed policy network: {e}"))
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
    if let Some(problem) = policy_network_problem(&json) {
        writeln!(writer, "✖ {problem}")?;
        return Ok(None);
    }
    match lns_artifact::build::build_artifact(&json) {
        Ok(built) => {
            let pins = match bundle_pins(&json) {
                Ok(pins) => pins,
                Err(message) => {
                    writeln!(writer, "✖ {message}")?;
                    return Ok(None);
                }
            };
            let target = tag.unwrap_or("<untagged>");
            writeln!(
                writer,
                "✔ built {} {target}@{}",
                built.artifact_type, built.manifest_digest
            )?;
            for pin in &pins {
                writeln!(writer, "  pinned {} → {}", pin.name, pin.digest)?;
            }
            Ok(Some(built))
        }
        Err(e) => {
            writeln!(writer, "✖ build failed: {e:#}")?;
            Ok(None)
        }
    }
}

/// For an AgentSystem bundle, the digest each component is pinned to; empty for a leaf artifact. `Err` names a component left on a floating tag.
fn bundle_pins(json: &[u8]) -> std::result::Result<Vec<lns_artifact::build::ComponentPin>, String> {
    match lns_artifact::spec::read_kind(json) {
        Ok(lns_artifact::spec::Kind::AgentSystem) => {
            lns_artifact::build::bundle_component_pins(json).map_err(|e| format!("{e:#}"))
        }
        _ => Ok(Vec::new()),
    }
}

/// Pack a directory's collected files into a FileSet artifact mounting at `mount_path`, printing the digest. Returns the artifact for a subsequent push, or `None` when packing failed.
pub(super) fn build_fileset_and_report(
    name: &str,
    mount_path: &str,
    entries: &[lns_artifact::build::FileEntry],
    tag: Option<&str>,
    writer: &mut impl Write,
) -> Result<Option<BuiltArtifact>> {
    match lns_artifact::build::build_fileset(name, mount_path, entries) {
        Ok(built) => {
            let target = tag.unwrap_or("<untagged>");
            writeln!(
                writer,
                "✔ built FileSet {target}@{} mounting {mount_path}",
                built.manifest_digest
            )?;
            Ok(Some(built))
        }
        Err(e) => {
            writeln!(writer, "✖ build failed: {e:#}")?;
            Ok(None)
        }
    }
}

/// Turn a directory name into a valid FileSet `metadata.name` (lowercase alphanumerics and `-`, trimmed, ≤63 chars); falls back to `fileset` when nothing usable remains.
pub(super) fn sanitize_name(raw: &str) -> String {
    let mapped: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .take(63)
        .collect();
    let trimmed = mapped.trim_matches('-');
    if trimmed.is_empty() {
        "fileset".to_string()
    } else {
        trimmed.to_string()
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

    fn policy_yaml(default_verdict: &str) -> String {
        format!(
            "apiVersion: lens.dev/v1alpha1\nkind: Policy\nmetadata:\n  name: some-policy\nspec:\n  network:\n    defaultVerdict: {default_verdict}\n"
        )
    }

    #[test]
    fn check_flags_a_malformed_policy_network_the_shared_validator_misses() {
        let (code, out) = check(policy_yaml("maybe").as_bytes());
        assert_eq!(code, 1);
        assert!(out.contains("malformed policy network"), "got: {out}");
    }

    #[test]
    fn check_passes_a_well_formed_policy_network() {
        let (code, out) = check(policy_yaml("ask").as_bytes());
        assert_eq!(code, 0, "got: {out}");
        assert!(out.contains("valid"), "got: {out}");
    }

    #[test]
    fn build_refuses_a_malformed_policy_network_before_push() {
        let mut out: Vec<u8> = Vec::new();
        let built = build_and_report(policy_yaml("maybe").as_bytes(), None, &mut out).unwrap();
        assert!(built.is_none(), "a malformed policy must not build");
        assert!(
            String::from_utf8(out)
                .unwrap()
                .contains("malformed policy network"),
            "must be refused at build, before push"
        );
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

    fn entry(path: &str, data: &str) -> lns_artifact::build::FileEntry {
        lns_artifact::build::FileEntry {
            path: path.into(),
            data: data.as_bytes().to_vec(),
        }
    }

    #[test]
    fn fileset_build_reports_the_mount_digest_and_returns_the_artifact() {
        let mut out: Vec<u8> = Vec::new();
        let built = build_fileset_and_report(
            "skills",
            "/root/.some-agent/skills",
            &[entry("deep.md", "research")],
            Some("reg/skills:1"),
            &mut out,
        )
        .unwrap()
        .expect("a directory packs into a fileset");
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("FileSet"), "got: {text}");
        assert!(text.contains("/root/.some-agent/skills"), "got: {text}");
        assert!(text.contains(&built.manifest_digest), "got: {text}");
    }

    #[test]
    fn fileset_build_reports_a_bad_mount_and_returns_none() {
        let mut out: Vec<u8> = Vec::new();
        let built = build_fileset_and_report(
            "skills",
            "relative/path",
            &[entry("a", "1")],
            None,
            &mut out,
        )
        .unwrap();
        assert!(built.is_none());
        assert!(String::from_utf8(out).unwrap().contains("build failed"));
    }

    fn bundle_yaml(components: &str) -> String {
        format!(
            "apiVersion: lens.dev/v1alpha1\nkind: AgentSystem\nmetadata:\n  name: some-bundle\nspec:\n  components:\n{components}"
        )
    }

    #[test]
    fn build_reports_the_digests_a_bundle_pins_its_components_to() {
        let pinned = format!("sha256:{}", "a".repeat(64));
        let yaml = bundle_yaml(&format!(
            "    sandbox:\n      ref: reg/base\n      digest: {pinned}\n"
        ));
        let mut out: Vec<u8> = Vec::new();
        build_and_report(yaml.as_bytes(), Some("reg/some-bundle:1"), &mut out)
            .unwrap()
            .expect("a pinned bundle builds");
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("pinned sandbox"), "got: {text}");
        assert!(text.contains(&pinned), "got: {text}");
    }

    #[test]
    fn build_refuses_a_bundle_component_left_on_a_floating_tag() {
        let yaml = bundle_yaml("    sandbox:\n      ref: reg/base:1\n");
        let mut out: Vec<u8> = Vec::new();
        let built = build_and_report(yaml.as_bytes(), Some("reg/some-bundle:1"), &mut out).unwrap();
        assert!(
            built.is_none(),
            "a floating component must refuse the build"
        );
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("floating tag"), "got: {text}");
        assert!(!text.contains("✔ built"), "must not report a build: {text}");
    }

    #[test]
    fn sanitize_name_lowercases_maps_and_trims_into_a_valid_metadata_name() {
        assert_eq!(sanitize_name("deep-research"), "deep-research");
        assert_eq!(sanitize_name("tmp.vDm6RD"), "tmp-vdm6rd");
        assert_eq!(sanitize_name("_weird_.name_"), "weird--name");
        assert_eq!(sanitize_name("...---"), "fileset");
        assert_eq!(sanitize_name(&"a".repeat(80)).len(), 63);
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
