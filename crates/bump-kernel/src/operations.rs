use anyhow::{Context, Result, bail, ensure};
use clap::ValueEnum;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use toml_edit::{DocumentMut, Item, Table, value};

pub(crate) const KERNELS_TOML: &str = "crates/lns-service/kernels.toml";

#[derive(Clone, Debug, ValueEnum)]
pub(crate) enum Variant {
    Mainline,
    Confidential,
    Tdx,
    Sev,
    Snp,
    Dragonball,
}

impl Variant {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            Variant::Mainline => "mainline",
            Variant::Confidential => "confidential",
            Variant::Tdx => "tdx",
            Variant::Sev => "sev",
            Variant::Snp => "snp",
            Variant::Dragonball => "dragonball",
        }
    }
}

#[derive(Clone, Debug, ValueEnum)]
pub(crate) enum CommitType {
    Feat,
    Fix,
    Chore,
}

impl CommitType {
    pub(crate) fn as_str(&self) -> &'static str {
        match self {
            CommitType::Feat => "feat",
            CommitType::Fix => "fix",
            CommitType::Chore => "chore",
        }
    }

    pub(crate) fn version_bump_hint(&self) -> &'static str {
        match self {
            CommitType::Feat => "minor (e.g., 0.1.x -> 0.2.0)",
            CommitType::Fix => "patch (e.g., 0.1.x -> 0.1.x+1)",
            CommitType::Chore => "none until next feat/fix lands",
        }
    }
}

#[derive(Deserialize, Debug)]
pub(crate) struct ComputeResult {
    pub(crate) arch: String,
    pub(crate) kernel_filename: String,
    pub(crate) published_version: String,
    pub(crate) sha256: String,
}

#[derive(Deserialize)]
struct ShowManifest {
    current: ShowCurrent,
}

#[derive(Deserialize)]
struct ShowCurrent {
    kata_version: Option<String>,
    kernel_filename: Option<String>,
    kernel_variant: Option<String>,
    published_version: Option<String>,
    sha256: BTreeMap<String, String>,
    #[serde(default)]
    kata_bundle_sha256: BTreeMap<String, String>,
}

pub(crate) fn render_show(raw: &str, arch: Option<&str>) -> Result<String> {
    let m: ShowManifest = toml::from_str(raw).context("parsing kernels.toml")?;
    let mut out = String::new();
    out.push_str(&format!(
        "kata_version={}\n",
        m.current.kata_version.unwrap_or_default()
    ));
    out.push_str(&format!(
        "kernel_filename={}\n",
        m.current.kernel_filename.unwrap_or_default()
    ));
    out.push_str(&format!(
        "kernel_variant={}\n",
        m.current
            .kernel_variant
            .unwrap_or_else(|| "mainline".into())
    ));
    out.push_str(&format!(
        "published_version={}\n",
        m.current.published_version.unwrap_or_default()
    ));
    for (kata_arch, sha) in &m.current.kata_bundle_sha256 {
        out.push_str(&format!("kata_bundle_sha256_{kata_arch}={sha}\n"));
    }
    if let Some(a) = arch {
        let sha = m.current.sha256.get(a).cloned().unwrap_or_default();
        if sha.is_empty() {
            bail!(
                "kernels.toml [current.sha256].{a} missing or empty (declared arches: {:?})",
                m.current.sha256.keys().collect::<Vec<_>>(),
            );
        }
        out.push_str(&format!("expected_sha={sha}\n"));
    }
    Ok(out)
}

pub(crate) fn load_compute_results(dir: &Path) -> Result<Vec<ComputeResult>> {
    let entries = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .collect::<std::io::Result<Vec<_>>>()
        .with_context(|| format!("iterating {}", dir.display()))?;
    let mut out = Vec::new();
    for entry in entries {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let r: ComputeResult =
            serde_json::from_str(&raw).with_context(|| format!("parsing {}", path.display()))?;
        out.push(r);
    }
    out.sort_by(|a, b| a.arch.cmp(&b.arch));
    ensure!(
        !out.is_empty(),
        "no per-arch results found under {}/*.json — \
         did the publish-kernel compute matrix run?",
        dir.display()
    );
    Ok(out)
}

pub(crate) fn back_fill_manifest(current: &str, results: &[ComputeResult]) -> Result<String> {
    let filenames: std::collections::BTreeSet<&str> =
        results.iter().map(|r| r.kernel_filename.as_str()).collect();
    let pubvers: std::collections::BTreeSet<&str> = results
        .iter()
        .map(|r| r.published_version.as_str())
        .collect();
    ensure!(
        filenames.len() == 1,
        "per-arch kernel_filename disagreement: {:?}",
        filenames
    );
    ensure!(
        pubvers.len() == 1,
        "per-arch published_version disagreement: {:?}",
        pubvers
    );

    let kfile = filenames.into_iter().next().unwrap();
    let pver = pubvers.into_iter().next().unwrap();

    let mut doc: DocumentMut = current.parse().context("parsing kernels.toml")?;
    let cur = doc
        .get_mut("current")
        .and_then(|v| v.as_table_mut())
        .context("kernels.toml has no [current] table")?;
    cur["kernel_filename"] = value(kfile);
    cur["published_version"] = value(pver);
    let shas = cur
        .get_mut("sha256")
        .and_then(|v| v.as_table_mut())
        .context("kernels.toml has no [current.sha256] table")?;
    for r in results {
        shas[r.arch.as_str()] = value(r.sha256.as_str());
    }
    Ok(doc.to_string())
}

pub(crate) fn bump_manifest(
    current: &str,
    kata_version: &str,
    variant: &str,
    bundle_shas: &BTreeMap<String, String>,
) -> Result<String> {
    ensure!(
        !bundle_shas.is_empty(),
        "no kata_bundle_sha256 values to pin — bump-kernel computes these from the downloaded Kata bundles"
    );

    let mut doc: DocumentMut = current
        .parse()
        .context("parsing kernels.toml as a toml_edit document")?;

    let current_tbl = doc
        .get_mut("current")
        .and_then(|v| v.as_table_mut())
        .context("kernels.toml has no [current] table")?;

    current_tbl["kata_version"] = value(kata_version);
    current_tbl["kernel_variant"] = value(variant);
    current_tbl["kernel_filename"] = value("pending");
    current_tbl["published_version"] = value("pending");

    let shas = current_tbl
        .get_mut("sha256")
        .and_then(|v| v.as_table_mut())
        .context("kernels.toml has no [current.sha256] table")?;
    let arches: Vec<String> = shas.iter().map(|(k, _)| k.to_string()).collect();
    ensure!(
        !arches.is_empty(),
        "[current.sha256] is empty — declare at least one arch (e.g. aarch64, x86_64)"
    );
    for arch in arches {
        shas[&arch] = value("");
    }

    let mut bundle_tbl = Table::new();
    bundle_tbl.set_implicit(false);
    for (kata_arch, sha) in bundle_shas {
        bundle_tbl[kata_arch] = value(sha.as_str());
    }
    current_tbl["kata_bundle_sha256"] = Item::Table(bundle_tbl);

    Ok(doc.to_string())
}

pub(crate) fn find_workspace_root_from(start: &Path) -> Result<PathBuf> {
    let mut dir = start.to_path_buf();
    loop {
        let cargo = dir.join("Cargo.toml");
        if cargo.is_file() {
            let s = std::fs::read_to_string(&cargo)?;
            if s.contains("[workspace]") {
                return Ok(dir);
            }
        }
        match dir.parent() {
            Some(p) => dir = p.to_path_buf(),
            None => bail!(
                "no Cargo.toml with [workspace] in cwd or any parent — \
                 are you running this from inside a lens-sandbox checkout?"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variant_as_str_covers_every_arm() {
        for (v, expected) in [
            (Variant::Mainline, "mainline"),
            (Variant::Confidential, "confidential"),
            (Variant::Tdx, "tdx"),
            (Variant::Sev, "sev"),
            (Variant::Snp, "snp"),
            (Variant::Dragonball, "dragonball"),
        ] {
            assert_eq!(v.as_str(), expected);
        }
    }

    #[test]
    fn commit_type_as_str_covers_every_arm() {
        assert_eq!(CommitType::Feat.as_str(), "feat");
        assert_eq!(CommitType::Fix.as_str(), "fix");
        assert_eq!(CommitType::Chore.as_str(), "chore");
    }

    #[test]
    fn commit_type_version_bump_hint_describes_release_please_behavior() {
        assert!(CommitType::Feat.version_bump_hint().contains("minor"));
        assert!(CommitType::Fix.version_bump_hint().contains("patch"));
        assert!(
            CommitType::Chore
                .version_bump_hint()
                .contains("none until next")
        );
    }

    const SAMPLE_MANIFEST: &str = "\
# Top comment that should survive editing.
[current]
kata_version    = \"3.30.0\"
kernel_filename = \"vmlinuz-6.12.6-141\"
kernel_variant  = \"mainline\"
published_version = \"6.12.6-141\"

[current.sha256]
aarch64 = \"aaaa1111aaaa1111aaaa1111aaaa1111aaaa1111aaaa1111aaaa1111aaaa1111\"
x86_64  = \"bbbb2222bbbb2222bbbb2222bbbb2222bbbb2222bbbb2222bbbb2222bbbb2222\"
";

    fn parsed(manifest: &str) -> toml::Value {
        toml::from_str(manifest).unwrap()
    }

    fn sample_bundle_shas() -> BTreeMap<String, String> {
        BTreeMap::from([
            ("arm64".to_string(), "c".repeat(64)),
            ("amd64".to_string(), "d".repeat(64)),
        ])
    }

    #[test]
    fn bump_clears_pending_fields_and_shas() {
        let out =
            bump_manifest(SAMPLE_MANIFEST, "3.31.0", "mainline", &sample_bundle_shas()).unwrap();
        let v = parsed(&out);
        let current = &v["current"];
        assert_eq!(current["kata_version"].as_str(), Some("3.31.0"));
        assert_eq!(current["kernel_filename"].as_str(), Some("pending"));
        assert_eq!(current["published_version"].as_str(), Some("pending"));
        let shas = current["sha256"].as_table().unwrap();
        for v in shas.values() {
            assert_eq!(v.as_str(), Some(""));
        }
    }

    #[test]
    fn bump_pins_kata_bundle_shas_creating_the_table_when_absent() {
        let out =
            bump_manifest(SAMPLE_MANIFEST, "3.31.0", "mainline", &sample_bundle_shas()).unwrap();
        let v = parsed(&out);
        let bundle = v["current"]["kata_bundle_sha256"].as_table().unwrap();
        assert_eq!(bundle["arm64"].as_str(), Some("c".repeat(64).as_str()));
        assert_eq!(bundle["amd64"].as_str(), Some("d".repeat(64).as_str()));
    }

    #[test]
    fn bump_overwrites_a_preexisting_bundle_sha_table() {
        let with_stale = format!(
            "{SAMPLE_MANIFEST}\n[current.kata_bundle_sha256]\narm64 = \"{stale}\"\namd64 = \"{stale}\"\n",
            stale = "0".repeat(64),
        );
        let out = bump_manifest(&with_stale, "3.31.0", "mainline", &sample_bundle_shas()).unwrap();
        let v = parsed(&out);
        let bundle = v["current"]["kata_bundle_sha256"].as_table().unwrap();
        assert_eq!(bundle["arm64"].as_str(), Some("c".repeat(64).as_str()));
        assert_eq!(bundle["amd64"].as_str(), Some("d".repeat(64).as_str()));
    }

    #[test]
    fn bump_preserves_comments() {
        let out =
            bump_manifest(SAMPLE_MANIFEST, "3.31.0", "mainline", &sample_bundle_shas()).unwrap();
        assert!(out.starts_with("# Top comment that should survive editing."));
    }

    #[test]
    fn bump_can_switch_variant() {
        let out = bump_manifest(SAMPLE_MANIFEST, "3.31.0", "tdx", &sample_bundle_shas()).unwrap();
        let v = parsed(&out);
        assert_eq!(v["current"]["kernel_variant"].as_str(), Some("tdx"));
    }

    #[test]
    fn bump_fails_on_missing_current_table() {
        let err = bump_manifest(
            "[other]\nfoo=1\n",
            "3.31.0",
            "mainline",
            &sample_bundle_shas(),
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("[current]"));
    }

    #[test]
    fn bump_fails_on_missing_sha256_table() {
        let err = bump_manifest(
            "[current]\nkata_version=\"3.30.0\"\nkernel_filename=\"x\"\npublished_version=\"y\"\n",
            "3.31.0",
            "mainline",
            &sample_bundle_shas(),
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("[current.sha256]"));
    }

    #[test]
    fn bump_fails_on_empty_sha256_table() {
        let manifest = "\
[current]
kata_version=\"3.30.0\"
kernel_filename=\"vmlinuz\"
kernel_variant=\"mainline\"
published_version=\"v\"

[current.sha256]
";
        let err = bump_manifest(manifest, "3.31.0", "mainline", &sample_bundle_shas()).unwrap_err();
        assert!(format!("{err:#}").contains("empty"));
    }

    #[test]
    fn bump_fails_when_no_bundle_shas_provided() {
        let err =
            bump_manifest(SAMPLE_MANIFEST, "3.31.0", "mainline", &BTreeMap::new()).unwrap_err();
        assert!(format!("{err:#}").contains("kata_bundle_sha256"));
    }

    #[test]
    fn show_emits_provenance_fields_no_arch() {
        let out = render_show(SAMPLE_MANIFEST, None).unwrap();
        assert!(out.contains("kata_version=3.30.0"));
        assert!(out.contains("kernel_filename=vmlinuz-6.12.6-141"));
        assert!(out.contains("kernel_variant=mainline"));
        assert!(out.contains("published_version=6.12.6-141"));
        assert!(!out.contains("expected_sha="));
    }

    #[test]
    fn show_emits_expected_sha_for_arch() {
        let out = render_show(SAMPLE_MANIFEST, Some("aarch64")).unwrap();
        assert!(out.contains(
            "expected_sha=aaaa1111aaaa1111aaaa1111aaaa1111aaaa1111aaaa1111aaaa1111aaaa1111"
        ));
    }

    #[test]
    fn show_errors_on_unknown_arch() {
        let err = render_show(SAMPLE_MANIFEST, Some("riscv64")).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("missing or empty"));
        assert!(msg.contains("declared arches"));
    }

    #[test]
    fn show_defaults_kernel_variant_to_mainline_when_absent() {
        let no_variant = "\
[current]
kata_version=\"3.30.0\"
kernel_filename=\"vmlinuz\"
published_version=\"v\"

[current.sha256]
aarch64=\"a\"
";
        let out = render_show(no_variant, None).unwrap();
        assert!(out.contains("kernel_variant=mainline"));
    }

    #[test]
    fn show_rejects_unparseable_toml() {
        let err = render_show("this is not toml at all {{{", None).unwrap_err();
        assert!(format!("{err:#}").contains("parsing kernels.toml"));
    }

    #[test]
    fn show_emits_kata_bundle_shas_keyed_by_kata_arch_when_present() {
        let bumped =
            bump_manifest(SAMPLE_MANIFEST, "3.31.0", "mainline", &sample_bundle_shas()).unwrap();
        let out = render_show(&bumped, None).unwrap();
        assert!(out.contains(&format!("kata_bundle_sha256_arm64={}", "c".repeat(64))));
        assert!(out.contains(&format!("kata_bundle_sha256_amd64={}", "d".repeat(64))));
    }

    #[test]
    fn show_omits_kata_bundle_shas_when_the_table_is_absent() {
        let out = render_show(SAMPLE_MANIFEST, None).unwrap();
        assert!(!out.contains("kata_bundle_sha256_"));
    }

    fn r(arch: &str, sha: &str) -> ComputeResult {
        ComputeResult {
            arch: arch.into(),
            kernel_filename: "vmlinuz-6.20.5-200".into(),
            published_version: "6.20.5-200".into(),
            sha256: sha.into(),
        }
    }

    #[test]
    fn back_fill_writes_per_arch_shas_and_filename() {
        let results = vec![r("aarch64", &"a".repeat(64)), r("x86_64", &"b".repeat(64))];
        let out = back_fill_manifest(SAMPLE_MANIFEST, &results).unwrap();
        let v = parsed(&out);
        let current = &v["current"];
        assert_eq!(
            current["kernel_filename"].as_str(),
            Some("vmlinuz-6.20.5-200")
        );
        assert_eq!(current["published_version"].as_str(), Some("6.20.5-200"));
        let shas = current["sha256"].as_table().unwrap();
        assert_eq!(shas["aarch64"].as_str(), Some("a".repeat(64).as_str()));
        assert_eq!(shas["x86_64"].as_str(), Some("b".repeat(64).as_str()));
    }

    #[test]
    fn back_fill_preserves_top_comment() {
        let results = vec![r("aarch64", &"a".repeat(64)), r("x86_64", &"b".repeat(64))];
        let out = back_fill_manifest(SAMPLE_MANIFEST, &results).unwrap();
        assert!(out.starts_with("# Top comment that should survive editing."));
    }

    #[test]
    fn back_fill_rejects_per_arch_filename_disagreement() {
        let mut r1 = r("aarch64", &"a".repeat(64));
        r1.kernel_filename = "vmlinuz-1.0.0-1".into();
        let mut r2 = r("x86_64", &"b".repeat(64));
        r2.kernel_filename = "vmlinuz-2.0.0-2".into();
        let err = back_fill_manifest(SAMPLE_MANIFEST, &[r1, r2]).unwrap_err();
        assert!(format!("{err:#}").contains("kernel_filename disagreement"));
    }

    #[test]
    fn back_fill_rejects_per_arch_pubver_disagreement() {
        let mut r1 = r("aarch64", &"a".repeat(64));
        r1.published_version = "1.0.0-1".into();
        let mut r2 = r("x86_64", &"b".repeat(64));
        r2.published_version = "2.0.0-2".into();
        let err = back_fill_manifest(SAMPLE_MANIFEST, &[r1, r2]).unwrap_err();
        assert!(format!("{err:#}").contains("published_version disagreement"));
    }

    #[test]
    fn back_fill_rejects_unparseable_toml() {
        let results = vec![r("aarch64", &"a".repeat(64))];
        let err = back_fill_manifest("not toml {", &results).unwrap_err();
        assert!(format!("{err:#}").contains("parsing kernels.toml"));
    }

    #[test]
    fn back_fill_errors_when_manifest_has_no_current_table() {
        let results = vec![r("aarch64", &"a".repeat(64))];
        let err = back_fill_manifest("[other]\n", &results).unwrap_err();
        assert!(format!("{err:#}").contains("[current]"));
    }

    #[test]
    fn back_fill_errors_when_manifest_has_no_sha256_table() {
        let results = vec![r("aarch64", &"a".repeat(64))];
        let m = "\
[current]
kata_version=\"3.30.0\"
kernel_filename=\"x\"
kernel_variant=\"mainline\"
published_version=\"y\"
";
        let err = back_fill_manifest(m, &results).unwrap_err();
        assert!(format!("{err:#}").contains("[current.sha256]"));
    }

    #[test]
    fn load_compute_results_reads_json_files_from_dir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("aarch64.json"),
            r#"{"arch":"aarch64","kernel_filename":"vmlinuz-6.20.5-200","published_version":"6.20.5-200","sha256":"aaaa1111aaaa1111aaaa1111aaaa1111aaaa1111aaaa1111aaaa1111aaaa1111"}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("x86_64.json"),
            r#"{"arch":"x86_64","kernel_filename":"vmlinuz-6.20.5-200","published_version":"6.20.5-200","sha256":"bbbb2222bbbb2222bbbb2222bbbb2222bbbb2222bbbb2222bbbb2222bbbb2222"}"#,
        )
        .unwrap();
        std::fs::write(dir.path().join("notes.txt"), "ignore me").unwrap();
        let results = load_compute_results(dir.path()).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].arch, "aarch64");
        assert_eq!(results[1].arch, "x86_64");
    }

    #[test]
    fn load_compute_results_errors_on_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let err = load_compute_results(dir.path()).unwrap_err();
        assert!(format!("{err:#}").contains("no per-arch results"));
    }

    #[test]
    fn load_compute_results_propagates_parse_failure_with_path_context() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("aarch64.json"), "not actually json").unwrap();
        let err = load_compute_results(dir.path()).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("parsing"));
        assert!(msg.contains("aarch64.json"));
    }

    #[test]
    fn load_compute_results_errors_on_missing_dir() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("does-not-exist");
        let err = load_compute_results(&missing).unwrap_err();
        assert!(format!("{err:#}").contains("reading"));
    }

    #[test]
    fn find_workspace_root_locates_dir_with_workspace_marker_in_cargo_toml() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("Cargo.toml"),
            "[workspace]\nmembers = [\"foo\"]\n",
        )
        .unwrap();
        let nested = dir.path().join("foo");
        std::fs::create_dir(&nested).unwrap();
        std::fs::write(
            nested.join("Cargo.toml"),
            "[package]\nname = \"foo\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        let resolved = find_workspace_root_from(&nested).unwrap();
        assert_eq!(
            resolved.canonicalize().unwrap(),
            dir.path().canonicalize().unwrap()
        );
    }

    #[test]
    fn find_workspace_root_bails_when_no_workspace_marker_anywhere() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("nested/deep");
        std::fs::create_dir_all(&nested).unwrap();
        let err = find_workspace_root_from(&nested).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("workspace"));
        assert!(msg.contains("lens-sandbox"));
    }

    #[test]
    fn kernels_toml_const_points_at_lns_service_crate() {
        assert_eq!(KERNELS_TOML, "crates/lns-service/kernels.toml");
    }
}
