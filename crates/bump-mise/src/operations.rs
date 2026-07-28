use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};

/// mise's published minisign key (checked into the mise repo as minisign.pub); every SHASUMS256.txt must verify against it before a pin lands.
pub const MISE_MINISIGN_PUBKEY: &str = "RWTC3g8W3z4RZK3V3qv7fa1QY4JEWyBtqIHW+85QlJpZc5yG+uNYNBSZ";

/// Stage every payload before moving any live file: the failure that actually happens is a full disk on the larger write, and a bumped engine pin against last release's snapshot is the one state worse than no bump.
pub fn write_all_atomically(files: &[(&std::path::Path, Vec<u8>)]) -> Result<()> {
    let mut staged = Vec::with_capacity(files.len());
    for (path, bytes) in files {
        let tmp = path.with_extension("bump-tmp");
        if let Err(e) = std::fs::write(&tmp, bytes) {
            for (tmp, _) in &staged {
                let _ = std::fs::remove_file(tmp);
            }
            return Err(e).with_context(|| format!("writing {}", tmp.display()));
        }
        staged.push((tmp, *path));
    }
    for (tmp, path) in &staged {
        if let Err(e) = std::fs::rename(tmp, path) {
            for (tmp, _) in &staged {
                let _ = std::fs::remove_file(tmp);
            }
            return Err(e).with_context(|| format!("replacing {}", path.display()));
        }
    }
    Ok(())
}

/// The engine pin as `show` reports it, so a hand-edited manifest missing the section is named rather than indexed into.
pub fn engine_summary(manifest: &str) -> Result<(String, String)> {
    let table: toml::Table = manifest.parse().context("parsing mise.toml")?;
    let engine = table
        .get("engine")
        .context("mise.toml carries no [engine] section")?;
    let version = engine
        .get("version")
        .and_then(toml::Value::as_str)
        .context("mise.toml carries no engine version")?;
    let shas = engine
        .get("sha256")
        .context("mise.toml carries no engine sha256 pins")?;
    Ok((version.to_string(), shas.to_string()))
}

fn is_download_only(backend: &str) -> bool {
    let kind = backend.split(':').next().unwrap_or(backend);
    lns_artifact::tools::registry::SUPPORTED_BACKEND_KINDS.contains(&kind)
}

/// A tool the guest cannot run is not provisionable, and letting it validate and push only to burn a provisioner boot before mise refuses the OS is worse than refusing it by name.
fn runs_on_linux(values: Option<&toml::Value>) -> bool {
    let Some(array) = values.and_then(toml::Value::as_array) else {
        return true;
    };
    array
        .iter()
        .filter_map(toml::Value::as_str)
        .any(|value| value == "linux" || value.starts_with("linux-"))
}

pub fn verify_shasums_signature(shasums: &[u8], minisig: &str) -> Result<()> {
    let key = minisign_verify::PublicKey::from_base64(MISE_MINISIGN_PUBKEY)
        .context("parsing the pinned mise minisign key")?;
    let signature =
        minisign_verify::Signature::decode(minisig).context("parsing SHASUMS256.txt.minisig")?;
    key.verify(shasums, &signature, false)
        .context("SHASUMS256.txt does not verify against mise's minisign key")
}

/// The sha256 of the bare static-musl binary for each arch, as published in SHASUMS256.txt.
pub fn musl_binary_shas(shasums: &str, version: &str) -> Result<BTreeMap<String, String>> {
    let mut shas = BTreeMap::new();
    for (release_arch, manifest_arch) in [("arm64", "aarch64"), ("x64", "x86_64")] {
        let suffix = format!("mise-v{version}-linux-{release_arch}-musl");
        let sha = shasums
            .lines()
            .find_map(|line| {
                let (sha, name) = line.split_once("  ")?;
                name.trim()
                    .trim_start_matches("./")
                    .eq(&suffix)
                    .then(|| sha.to_string())
            })
            .with_context(|| format!("SHASUMS256.txt has no entry for {suffix}"))?;
        shas.insert(manifest_arch.to_string(), sha);
    }
    Ok(shas)
}

/// Rewrite mise.toml's `[engine]` section in place, preserving comments and every other pin.
pub fn bump_engine_pin(
    manifest: &str,
    version: &str,
    shas: &BTreeMap<String, String>,
) -> Result<String> {
    let mut doc: toml_edit::DocumentMut = manifest.parse().context("parsing mise.toml")?;
    doc["engine"]["version"] = toml_edit::value(version);
    for (arch, sha) in shas {
        doc["engine"]["sha256"][arch.as_str()] = toml_edit::value(sha.as_str());
    }
    Ok(doc.to_string())
}

/// Regenerate the registry snapshot from a release's `registry/*.toml` entries: every name (and alias) maps to its first download-only backend, falling back to the literal first so unsupported-backend tools are refused by name.
pub fn render_registry_snapshot(entries: &[(String, String)]) -> Result<String> {
    let mut rows: BTreeMap<String, String> = BTreeMap::new();
    let mut aliased: Vec<(String, String)> = Vec::new();
    for (stem, contents) in entries {
        let table: toml::Table = contents
            .parse()
            .with_context(|| format!("parsing registry entry {stem}"))?;
        if !runs_on_linux(table.get("os")) {
            continue;
        }
        let backends: Vec<String> = table
            .get("backends")
            .and_then(|value| value.as_array())
            .map(|array| {
                array
                    .iter()
                    .filter_map(|backend| match backend {
                        toml::Value::String(full) => Some(full.clone()),
                        toml::Value::Table(t) => runs_on_linux(t.get("platforms"))
                            .then(|| t.get("full").and_then(|v| v.as_str()).map(str::to_string))
                            .flatten(),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let Some(first) = backends.first() else {
            continue;
        };
        let pick = backends
            .iter()
            .find(|backend| is_download_only(backend))
            .unwrap_or(first)
            .clone();
        rows.insert(stem.clone(), pick.clone());
        if let Some(aliases) = table.get("aliases").and_then(|value| value.as_array()) {
            aliased.extend(
                aliases
                    .iter()
                    .filter_map(|alias| alias.as_str())
                    .map(|alias| (alias.to_string(), pick.clone())),
            );
        }
    }
    // A tool always owns its own name: an alias only fills a row no entry claimed, or an alphabetically earlier tool's alias would decide what a later tool's own name installs.
    for (alias, backend) in aliased {
        rows.entry(alias).or_insert(backend);
    }
    if rows.is_empty() {
        bail!("the release carried no registry entries — refusing to write an empty snapshot");
    }
    Ok(rows
        .into_iter()
        .map(|(name, backend)| format!("{name}\t{backend}\n"))
        .collect())
}

/// The `registry/*.toml` entries of a release source tarball, as (stem, contents) pairs for snapshot rendering.
pub fn registry_entries_from_tarball(tarball: &[u8]) -> Result<Vec<(String, String)>> {
    use std::io::Read;
    let mut entries = Vec::new();
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(tarball));
    for entry in archive.entries().context("reading the source tarball")? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let mut components = path.components();
        components.next();
        let rest: std::path::PathBuf = components.collect();
        if rest.parent() != Some(std::path::Path::new("registry"))
            || rest.extension().and_then(|e| e.to_str()) != Some("toml")
        {
            continue;
        }
        let Some(stem) = rest.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let mut contents = String::new();
        entry.read_to_string(&mut contents)?;
        entries.push((stem.to_string(), contents));
    }
    Ok(entries)
}

pub fn shasums_url(version: &str) -> String {
    format!("https://github.com/jdx/mise/releases/download/v{version}/SHASUMS256.txt")
}

pub fn source_tarball_url(version: &str) -> String {
    format!("https://github.com/jdx/mise/archive/refs/tags/v{version}.tar.gz")
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANIFEST: &str = r#"# header comment survives
[engine]
version = "2026.7.14"

[engine.sha256]
aarch64 = "aaaa"
x86_64 = "bbbb"

[provisioner_rootfs]
gnu = "docker.io/library/debian@sha256:cccc"
musl = "docker.io/library/alpine@sha256:dddd"
"#;

    #[test]
    fn musl_binary_shas_reads_both_arches_from_the_shasums() {
        let shasums = "1111  ./mise-v2026.8.0-linux-arm64-musl\n2222  ./mise-v2026.8.0-linux-x64-musl\n3333  ./mise-v2026.8.0-linux-arm64-musl.tar.gz\n";
        let shas = musl_binary_shas(shasums, "2026.8.0").unwrap();
        assert_eq!(shas["aarch64"], "1111");
        assert_eq!(shas["x86_64"], "2222");
    }

    #[test]
    fn a_shasums_missing_an_arch_is_refused_naming_the_asset() {
        let err =
            musl_binary_shas("1111  ./mise-v2026.8.0-linux-arm64-musl\n", "2026.8.0").unwrap_err();
        assert!(
            format!("{err:#}").contains("mise-v2026.8.0-linux-x64-musl"),
            "got: {err:#}"
        );
    }

    #[test]
    fn bump_engine_pin_rewrites_only_the_engine_section_and_keeps_comments() {
        let shas = BTreeMap::from([
            ("aarch64".to_string(), "1111".to_string()),
            ("x86_64".to_string(), "2222".to_string()),
        ]);
        let bumped = bump_engine_pin(MANIFEST, "2026.8.0", &shas).unwrap();
        assert!(bumped.contains("# header comment survives"));
        assert!(bumped.contains(r#"version = "2026.8.0""#));
        assert!(bumped.contains(r#"aarch64 = "1111""#));
        assert!(bumped.contains("docker.io/library/alpine@sha256:dddd"));
    }

    #[test]
    fn the_snapshot_prefers_download_backends_and_carries_aliases() {
        let entries = vec![
            (
                "github-cli".to_string(),
                "backends = [\"aqua:cli/cli\"]\naliases = [\"gh\"]\n".to_string(),
            ),
            (
                "some-plugin".to_string(),
                "backends = [\"vfox:owner/plugin\"]\n".to_string(),
            ),
            (
                "mixed".to_string(),
                "backends = [\"npm:mixed\", \"aqua:owner/mixed\"]\n".to_string(),
            ),
            (
                "empty".to_string(),
                "description = \"nothing\"\n".to_string(),
            ),
        ];
        let snapshot = render_registry_snapshot(&entries).unwrap();
        assert!(snapshot.contains("github-cli\taqua:cli/cli\n"));
        assert!(snapshot.contains("gh\taqua:cli/cli\n"));
        assert!(snapshot.contains("some-plugin\tvfox:owner/plugin\n"));
        assert!(snapshot.contains("mixed\taqua:owner/mixed\n"));
        assert!(!snapshot.contains("empty"));
    }

    #[test]
    fn an_empty_registry_refuses_the_snapshot() {
        assert!(render_registry_snapshot(&[]).is_err());
    }

    #[test]
    fn a_tools_own_name_beats_an_alias_another_tool_claims_for_it() {
        // Entries arrive in tar order, so an alphabetically earlier tool's alias would otherwise take the row a later tool's own entry needs — and `node@22` would silently provision bun.
        let entries = vec![
            (
                "bun".to_string(),
                "backends = [\"aqua:oven-sh/bun\"]\naliases = [\"node\"]\n".to_string(),
            ),
            (
                "node".to_string(),
                "backends = [\"core:node\"]\n".to_string(),
            ),
        ];
        let snapshot = render_registry_snapshot(&entries).unwrap();
        assert!(snapshot.contains("node\tcore:node\n"), "got: {snapshot}");
        assert!(
            snapshot.contains("bun\taqua:oven-sh/bun\n"),
            "got: {snapshot}"
        );
    }

    #[test]
    fn an_alias_still_lands_when_no_tool_owns_that_name() {
        let entries = vec![(
            "github-cli".to_string(),
            "backends = [\"aqua:cli/cli\"]\naliases = [\"gh\"]\n".to_string(),
        )];
        let snapshot = render_registry_snapshot(&entries).unwrap();
        assert!(snapshot.contains("gh\taqua:cli/cli\n"), "got: {snapshot}");
    }

    #[test]
    fn the_engine_summary_names_a_manifest_missing_its_section_instead_of_panicking() {
        let err = engine_summary("[companion]\nalpine = \"3.21\"\n").unwrap_err();
        assert!(format!("{err:#}").contains("engine"), "got: {err:#}");
        let (version, shas) = engine_summary(
            "[engine]\nversion = \"2026.7.14\"\n[engine.sha256]\naarch64 = \"ab\"\n",
        )
        .unwrap();
        assert_eq!(version, "2026.7.14");
        assert!(shas.contains("ab"), "got: {shas}");
    }

    #[test]
    fn a_bump_that_cannot_stage_every_file_moves_none_of_them() {
        // The failure that actually happens is a full disk while writing the larger snapshot; a bumped engine pin against last release's snapshot is the one state worse than no bump.
        let dir = tempfile::TempDir::new().unwrap();
        let manifest = dir.path().join("mise.toml");
        let snapshot = dir.path().join("registry.snapshot");
        std::fs::write(&manifest, b"old-manifest").unwrap();
        std::fs::write(&snapshot, b"old-snapshot").unwrap();
        // A directory where the second temp file has to go makes staging it fail without touching the first.
        std::fs::create_dir(snapshot.with_extension("bump-tmp")).unwrap();

        let err = write_all_atomically(&[
            (manifest.as_path(), b"new-manifest".to_vec()),
            (snapshot.as_path(), b"new-snapshot".to_vec()),
        ])
        .unwrap_err();

        assert!(format!("{err:#}").contains("registry"), "got: {err:#}");
        assert_eq!(std::fs::read(&manifest).unwrap(), b"old-manifest");
        assert_eq!(std::fs::read(&snapshot).unwrap(), b"old-snapshot");
        assert!(
            !manifest.with_extension("bump-tmp").exists(),
            "a failed bump leaves no temp to puzzle over"
        );
    }

    #[test]
    fn a_bump_that_stages_every_file_replaces_them_all() {
        let dir = tempfile::TempDir::new().unwrap();
        let manifest = dir.path().join("mise.toml");
        let snapshot = dir.path().join("registry.snapshot");
        std::fs::write(&manifest, b"old-manifest").unwrap();
        std::fs::write(&snapshot, b"old-snapshot").unwrap();

        write_all_atomically(&[
            (manifest.as_path(), b"new-manifest".to_vec()),
            (snapshot.as_path(), b"new-snapshot".to_vec()),
        ])
        .unwrap();

        assert_eq!(std::fs::read(&manifest).unwrap(), b"new-manifest");
        assert_eq!(std::fs::read(&snapshot).unwrap(), b"new-snapshot");
        assert!(!manifest.with_extension("bump-tmp").exists());
        assert!(!snapshot.with_extension("bump-tmp").exists());
    }

    #[test]
    fn the_tarball_walk_extracts_only_registry_entries() {
        use flate2::Compression;
        use flate2::write::GzEncoder;
        use std::io::Write as _;
        let mut builder = tar::Builder::new(Vec::new());
        for (path, body) in [
            (
                "mise-1.0.0/registry/node.toml",
                "backends = [\"core:node\"]\n",
            ),
            ("mise-1.0.0/registry/README.md", "not toml"),
            ("mise-1.0.0/src/main.rs", "fn main() {}"),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_path(path).unwrap();
            header.set_size(body.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append(&header, body.as_bytes()).unwrap();
        }
        let tar = builder.into_inner().unwrap();
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(&tar).unwrap();
        let tarball = encoder.finish().unwrap();

        let entries = registry_entries_from_tarball(&tarball).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, "node");
        assert!(entries[0].1.contains("core:node"));
    }

    #[test]
    fn the_pinned_release_fixtures_verify_against_the_minisign_key() {
        verify_shasums_signature(
            include_bytes!("../fixtures/SHASUMS256.txt"),
            include_str!("../fixtures/SHASUMS256.txt.minisig"),
        )
        .unwrap();
    }

    #[test]
    fn a_tampered_shasums_is_refused_by_the_signature() {
        let mut tampered = include_bytes!("../fixtures/SHASUMS256.txt").to_vec();
        tampered[0] ^= 0xff;
        let err = verify_shasums_signature(
            &tampered,
            include_str!("../fixtures/SHASUMS256.txt.minisig"),
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("does not verify"),
            "got: {err:#}"
        );
    }

    #[test]
    fn table_form_backends_and_non_string_entries_are_handled() {
        let entries = vec![(
            "tabular".to_string(),
            "backends = [{ full = \"aqua:owner/tabular\" }, 42]\n".to_string(),
        )];
        let snapshot = render_registry_snapshot(&entries).unwrap();
        assert!(snapshot.contains("tabular\taqua:owner/tabular\n"));
    }

    #[test]
    fn a_rename_that_cannot_land_leaves_the_previous_pin_and_no_temp() {
        // The operator's only recovery signal is `git status`, so a half-finished bump must not leave a stray file to puzzle over.
        let dir = tempfile::TempDir::new().unwrap();
        let occupied = dir.path().join("snapshot");
        std::fs::create_dir(&occupied).unwrap();
        std::fs::write(occupied.join("keep"), b"in the way").unwrap();

        let err = write_all_atomically(&[(occupied.as_path(), b"new".to_vec())]).unwrap_err();

        assert!(
            format!("{err:#}").contains("snapshot"),
            "the error names the file: {err:#}"
        );
        assert!(occupied.join("keep").exists(), "the old state survives");
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name != "snapshot")
            .collect();
        assert!(leftovers.is_empty(), "temp left behind: {leftovers:?}");
    }

    #[test]
    fn a_tool_the_guest_cannot_run_is_left_out_of_the_snapshot() {
        // Otherwise it validates, pushes, and burns a provisioner boot before mise refuses the OS.
        let entries = vec![
            (
                "xcodegen".to_string(),
                "os = [\"macos\"]\nbackends = [\"aqua:owner/xcodegen\"]\n".to_string(),
            ),
            (
                "portable".to_string(),
                "os = [\"linux\", \"macos\"]\nbackends = [\"aqua:owner/portable\"]\n".to_string(),
            ),
        ];
        let snapshot = render_registry_snapshot(&entries).unwrap();
        assert!(!snapshot.contains("xcodegen"), "got: {snapshot}");
        assert!(snapshot.contains("portable\taqua:owner/portable\n"));
    }

    #[test]
    fn a_backend_that_does_not_build_for_linux_is_not_the_recorded_pick() {
        // imagemagick ships its aqua package for windows only; recording it would gate and audit against a backend the guest never uses.
        let entries = vec![(
            "imagemagick".to_string(),
            "backends = [{ full = \"aqua:owner/im\", platforms = [\"windows-x64\"] }, \"conda:imagemagick\"]\n"
                .to_string(),
        )];
        let snapshot = render_registry_snapshot(&entries).unwrap();
        assert!(
            snapshot.contains("imagemagick\tconda:imagemagick\n"),
            "the unusable backend is skipped and the tool is refused by name instead: {snapshot}"
        );
    }

    #[test]
    fn a_non_utf8_registry_stem_is_skipped() {
        use flate2::Compression;
        use flate2::write::GzEncoder;
        use std::io::Write as _;
        let mut builder = tar::Builder::new(Vec::new());
        let mut weird = tar::Header::new_gnu();
        weird.set_path("a").unwrap();
        {
            let name = weird.as_gnu_mut().unwrap();
            let raw = b"m-1/registry/\xff.toml";
            name.name[..raw.len()].copy_from_slice(raw);
            name.name[raw.len()] = 0;
        }
        weird.set_size(0);
        weird.set_mode(0o644);
        weird.set_cksum();
        builder.append(&weird, std::io::empty()).unwrap();
        let tar = builder.into_inner().unwrap();
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(&tar).unwrap();
        let tarball = encoder.finish().unwrap();

        assert!(registry_entries_from_tarball(&tarball).unwrap().is_empty());
    }

    #[test]
    fn a_bad_signature_is_refused() {
        let err = verify_shasums_signature(b"payload", "not a signature").unwrap_err();
        assert!(
            format!("{err:#}").contains("SHASUMS256.txt.minisig"),
            "got: {err:#}"
        );
    }

    #[test]
    fn the_release_urls_name_the_tag() {
        assert_eq!(
            shasums_url("2026.8.0"),
            "https://github.com/jdx/mise/releases/download/v2026.8.0/SHASUMS256.txt"
        );
        assert!(source_tarball_url("2026.8.0").ends_with("tags/v2026.8.0.tar.gz"));
    }
}
