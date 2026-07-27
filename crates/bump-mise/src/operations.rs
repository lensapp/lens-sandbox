use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};

/// mise's published minisign key (checked into the mise repo as minisign.pub); every SHASUMS256.txt must verify against it before a pin lands.
pub const MISE_MINISIGN_PUBKEY: &str = "RWTC3g8W3z4RZK3V3qv7fa1QY4JEWyBtqIHW+85QlJpZc5yG+uNYNBSZ";

pub const SUPPORTED_BACKEND_PREFIXES: &[&str] =
    &["core:", "aqua:", "ubi:", "github:", "http:", "gitlab:"];

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
    for (stem, contents) in entries {
        let table: toml::Table = contents
            .parse()
            .with_context(|| format!("parsing registry entry {stem}"))?;
        let backends: Vec<String> = table
            .get("backends")
            .and_then(|value| value.as_array())
            .map(|array| {
                array
                    .iter()
                    .filter_map(|backend| match backend {
                        toml::Value::String(full) => Some(full.clone()),
                        toml::Value::Table(t) => {
                            t.get("full").and_then(|v| v.as_str()).map(str::to_string)
                        }
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
            .find(|backend| {
                SUPPORTED_BACKEND_PREFIXES
                    .iter()
                    .any(|prefix| backend.starts_with(prefix))
            })
            .unwrap_or(first)
            .clone();
        let mut names = vec![stem.clone()];
        if let Some(aliases) = table.get("aliases").and_then(|value| value.as_array()) {
            names.extend(
                aliases
                    .iter()
                    .filter_map(|alias| alias.as_str().map(str::to_string)),
            );
        }
        for name in names {
            rows.entry(name).or_insert_with(|| pick.clone());
        }
    }
    if rows.is_empty() {
        bail!("the release carried no registry entries — refusing to write an empty snapshot");
    }
    Ok(rows
        .into_iter()
        .map(|(name, backend)| format!("{name}\t{backend}\n"))
        .collect())
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
