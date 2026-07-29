pub mod registry;

use anyhow::{Result, bail};

/// The one version keyword the spec accepts: it re-resolves on every run rather than pinning.
pub const LATEST: &str = "latest";

/// Whether a version is safe to use as a path component and to interpolate into the provisioner's shell driver. An allowlist, not a denylist: anything a shell reads specially must never parse in the first place.
pub fn is_safe_version(version: &str) -> bool {
    !version.is_empty()
        && version != "."
        && version != ".."
        && version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+'))
}

/// Whether a tool's bin directory is safe to append to its own root: `.` for the tree root, otherwise plain segments. It becomes a `PATH` entry for the workload and for every later `lns exec`, so it is checked wherever it arrives from — the driver's output and a manifest read back off disk alike.
pub fn is_safe_bin_path(bin_path: &str) -> bool {
    bin_path == "." || bin_path.split('/').all(is_safe_version)
}

/// A version that has passed [`is_safe_version`], so no source of one — index, driver output, or a file on disk a later process could edit — can reach a path component or the shell driver unchecked.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(try_from = "String", into = "String")]
pub struct SafeVersion(String);

impl SafeVersion {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for SafeVersion {
    type Error = anyhow::Error;

    fn try_from(version: String) -> Result<Self> {
        if !is_safe_version(&version) {
            bail!("{version:?} is not a usable version");
        }
        Ok(Self(version))
    }
}

impl std::str::FromStr for SafeVersion {
    type Err = anyhow::Error;

    fn from_str(version: &str) -> Result<Self> {
        Self::try_from(version.to_string())
    }
}

impl From<SafeVersion> for String {
    fn from(version: SafeVersion) -> String {
        version.0
    }
}

impl std::fmt::Display for SafeVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<std::path::Path> for SafeVersion {
    fn as_ref(&self) -> &std::path::Path {
        std::path::Path::new(&self.0)
    }
}

/// A version already names every component of a release, so nothing is left for the index to resolve and its cache key is fully determined by the request. `push` uses it to skip the index; the run path uses it to address the cache without a record — they are two halves of one contract and must not drift.
pub fn is_exact_version(version: &str) -> bool {
    version != LATEST
        && version.split('.').count() >= 3
        && version
            .split('.')
            .all(|part| !part.is_empty() && part.chars().next().is_some_and(char::is_numeric))
}

const VERSION_INDEX_URL: &str = "https://mise-versions.jdx.dev";

/// The public index a fuzzy version resolves against; the override is what the e2e suite points at a local fixture.
pub fn version_index_url(name: &str) -> String {
    let base =
        std::env::var("LNS_TOOL_INDEX_URL").unwrap_or_else(|_| VERSION_INDEX_URL.to_string());
    format!("{}/{name}", base.trim_end_matches('/'))
}

/// A portable declared tool: `name@version`, where version may be fuzzy (`22`) or `latest`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolRef {
    pub name: String,
    pub version: String,
}

impl std::fmt::Display for ToolRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.name, self.version)
    }
}

pub fn parse(entry: &str) -> Result<ToolRef> {
    if entry.is_empty() {
        bail!(
            "invalid spec.tools entry {entry:?}: expected the \"name@version\" shape (like \"node@22\" or \"python@3.12\")"
        );
    }
    let name_half = entry.split('@').next().unwrap_or_default();
    if name_half.contains(':') {
        bail!(
            "spec.tools entry {entry:?} uses an engine backend prefix; the spec carries portable tool names only — declare \"node@22\""
        );
    }
    let Some((name, version)) = entry.split_once('@') else {
        bail!(
            "spec.tools entry {entry:?} has no version; declare an explicit version such as \"{entry}@22\" or \"{entry}@latest\""
        );
    };
    if name.is_empty() || version.is_empty() || !is_safe_version(version) {
        bail!(
            "invalid spec.tools entry {entry:?}: expected the \"name@version\" shape (like \"node@22\" or \"python@3.12\")"
        );
    }
    if !crate::spec::is_valid_name(name) {
        bail!("invalid tool name in spec.tools entry {entry:?}");
    }
    Ok(ToolRef {
        name: name.to_string(),
        version: version.to_string(),
    })
}

/// Pick the exact version a fuzzy request pins from an ascending newline-separated version index; `latest` takes the newest stable line, `22` the newest `22.*` line.
pub fn resolve_from_index(name: &str, version: &str, index_body: &str) -> Result<String> {
    let stable = |line: &str| !line.chars().any(|c| c.is_ascii_alphabetic());
    let lines: Vec<&str> = index_body
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    if lines.is_empty() {
        bail!(
            "tool {name:?} is unknown to the version index; check the name against mise's registry"
        );
    }
    // The stable filter exists so a letter-free request never lands on a prerelease line. A request that already carries a token (`temurin-21`) is asking for exactly those lines, so applying it there matches nothing at all.
    let skip_prereleases = version == LATEST || !version.chars().any(|c| c.is_ascii_alphabetic());
    let resolved = if version == LATEST {
        lines.iter().copied().rfind(|line| stable(line))
    } else {
        let prefix = format!("{version}.");
        lines.iter().copied().rfind(|line| {
            *line == version || (line.starts_with(&prefix) && (!skip_prereleases || stable(line)))
        })
    };
    match resolved {
        Some(exact) if is_safe_version(exact) => Ok(exact.to_string()),
        Some(unusable) => bail!(
            "the version index answered {unusable:?} for {name}, which is not a usable version"
        ),
        None => bail!(
            "no published version of {name} matches {version:?}; newest in the index is {}",
            lines.last().unwrap_or(&"")
        ),
    }
}

/// Parse every entry, refusing duplicate tool names — `node@22` and `node@latest` contradict.
pub fn parse_all(entries: &[String]) -> Result<Vec<ToolRef>> {
    let mut refs = Vec::with_capacity(entries.len());
    for entry in entries {
        let tool = parse(entry)?;
        if refs.iter().any(|seen: &ToolRef| seen.name == tool.name) {
            bail!(
                "duplicate tool {:?} in spec.tools: declare one version per tool",
                tool.name
            );
        }
        refs.push(tool);
    }
    Ok(refs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[serial_test::serial(env)]
    fn the_version_index_url_names_the_tool_and_honors_the_override() {
        // SAFETY: #[serial(env)] is the whole-process lock for env mutation, and this test owns this key.
        unsafe { std::env::remove_var("LNS_TOOL_INDEX_URL") };
        assert_eq!(
            version_index_url("node"),
            "https://mise-versions.jdx.dev/node"
        );
        // SAFETY: as above.
        unsafe { std::env::set_var("LNS_TOOL_INDEX_URL", "http://localhost:9/") };
        assert_eq!(version_index_url("node"), "http://localhost:9/node");
        // SAFETY: as above; leaves the key as this test found it.
        unsafe { std::env::remove_var("LNS_TOOL_INDEX_URL") };
    }

    #[test]
    fn an_exact_version_is_the_pin_both_sides_recognize() {
        // push skips the index on these and the run path addresses the cache with them, so the two halves must agree.
        for exact in ["22.11.0", "3.12.6", "1.0.0.1"] {
            assert!(is_exact_version(exact), "{exact}");
        }
        for fuzzy in ["22", "22.11", LATEST, "", "22.x.0", "22..0"] {
            assert!(!is_exact_version(fuzzy), "{fuzzy}");
        }
    }

    #[test]
    fn parse_reads_name_and_version() {
        let tool = parse("node@22").unwrap();
        assert_eq!(tool.name, "node");
        assert_eq!(tool.version, "22");
        assert_eq!(tool.to_string(), "node@22");
    }

    #[test]
    fn parse_accepts_latest() {
        assert_eq!(parse("node@latest").unwrap().version, "latest");
    }

    #[test]
    fn parse_requires_an_explicit_version() {
        let err = parse("node").unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains(r#""node""#)
                && msg.contains(r#"explicit version such as "node@22" or "node@latest""#),
            "got: {msg}"
        );
    }

    #[test]
    fn parse_rejects_the_malformed_shapes() {
        for entry in ["node@", "@22", "", "node@1@2", "node@1 2"] {
            let err = parse(entry).unwrap_err();
            let msg = format!("{err:#}");
            assert!(
                msg.contains(&format!("{entry:?}")) && msg.contains(r#""name@version""#),
                "entry {entry:?}: got: {msg}"
            );
        }
    }

    #[test]
    fn parse_rejects_a_version_carrying_shell_metacharacters() {
        for entry in [
            "node@1';id;'",
            "node@$(id)",
            "node@`id`",
            "node@1|id",
            "node@../../escape",
            "node@1&id",
        ] {
            let err = parse(entry).unwrap_err();
            assert!(
                format!("{err:#}").contains(&format!("{entry:?}")),
                "entry {entry:?} must not reach the provisioner driver"
            );
        }
    }

    #[test]
    fn parse_accepts_the_version_shapes_upstream_publishes() {
        for entry in [
            "node@22",
            "python@3.12.6",
            "go@1.22.0-rc1",
            // The spelling upstream actually publishes; an underscore matches nothing in the index.
            "java@temurin-21",
        ] {
            parse(entry).unwrap_or_else(|e| panic!("entry {entry:?}: {e:#}"));
        }
    }

    #[test]
    fn parse_rejects_an_invalid_tool_name() {
        let err = parse("No_de@22").unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("invalid tool name") && msg.contains(r#""No_de@22""#),
            "got: {msg}"
        );
    }

    #[test]
    fn parse_rejects_engine_backend_prefixes() {
        for entry in [
            "aqua:node@22",
            "ubi:owner/repo@1",
            "npm:some-tool@3",
            "npm:some-tool",
        ] {
            let err = parse(entry).unwrap_err();
            let msg = format!("{err:#}");
            assert!(
                msg.contains("engine backend prefix") && msg.contains(&format!("{entry:?}")),
                "entry {entry:?}: got: {msg}"
            );
        }
    }

    #[test]
    fn parse_all_rejects_a_duplicate_tool_name() {
        let entries = vec!["node@22".to_string(), "node@latest".to_string()];
        let err = parse_all(&entries).unwrap_err();
        assert!(
            format!("{err:#}").contains(r#"duplicate tool "node""#),
            "got: {err:#}"
        );
    }

    #[test]
    fn parse_all_keeps_declaration_order() {
        let entries = vec!["node@22".to_string(), "python@3.12".to_string()];
        let refs = parse_all(&entries).unwrap();
        assert_eq!(refs[0].to_string(), "node@22");
        assert_eq!(refs[1].to_string(), "python@3.12");
    }

    #[test]
    fn resolve_from_index_picks_the_newest_dot_boundary_match() {
        let body = "20.1.0\n22.9.0\n22.11.0\n220.1.0\n23.0.0\n";
        assert_eq!(resolve_from_index("node", "22", body).unwrap(), "22.11.0");
    }

    #[test]
    fn resolve_from_index_latest_skips_prerelease_lines() {
        let body = "3.11.9\n3.12.6\n3.13.0rc1\n";
        assert_eq!(
            resolve_from_index("python", "latest", body).unwrap(),
            "3.12.6"
        );
    }

    #[test]
    fn resolve_from_index_accepts_an_exact_version_verbatim() {
        let body = "22.9.0\n22.11.0\n";
        assert_eq!(
            resolve_from_index("node", "22.11.0", body).unwrap(),
            "22.11.0"
        );
    }

    #[test]
    fn a_vendor_prefixed_request_resolves_against_the_lines_that_carry_that_vendor() {
        // mise ships java as `temurin-21.0.5+11.0.LTS`; treating every letter as a prerelease left such a request publishable-never, while the offline gate accepted it.
        let body = "21.0.2\ntemurin-21.0.4+7\ntemurin-21.0.5+11.0.LTS\ntemurin-26.0.1+8\n";
        assert_eq!(
            resolve_from_index("java", "temurin-21", body).unwrap(),
            "temurin-21.0.5+11.0.LTS"
        );
        assert_eq!(
            resolve_from_index("java", LATEST, body).unwrap(),
            "21.0.2",
            "a letter-free request still refuses vendor and prerelease lines"
        );
        assert_eq!(resolve_from_index("java", "21", body).unwrap(), "21.0.2");
    }

    #[test]
    fn an_index_answer_that_could_escape_the_cache_tree_is_refused() {
        // The body is third-party and `latest` only skips alphabetic lines, so a traversal answer passes that filter.
        for body in ["../..\n", "22./../..\n", "..\n", "22/11\n"] {
            let err = resolve_from_index("node", LATEST, body).unwrap_err();
            assert!(
                format!("{err:#}").contains("not a usable version"),
                "body {body:?}: got: {err:#}"
            );
        }
        assert_eq!(
            resolve_from_index("node", LATEST, "22.11.0\n").unwrap(),
            "22.11.0"
        );
    }

    #[test]
    fn resolve_from_index_treats_an_empty_index_as_unknown() {
        let err = resolve_from_index("nodde", "22", "\n").unwrap_err();
        assert!(
            format!("{err:#}").contains(r#"tool "nodde" is unknown to the version index"#),
            "got: {err:#}"
        );
    }

    #[test]
    fn resolve_from_index_names_the_newest_when_nothing_matches() {
        let err = resolve_from_index("node", "99", "22.9.0\n23.0.0\n").unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains(r#"no published version of node matches "99""#) && msg.contains("23.0.0"),
            "got: {msg}"
        );
    }
}
