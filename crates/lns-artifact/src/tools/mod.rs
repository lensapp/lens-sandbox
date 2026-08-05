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

/// Whether a name the engine reports for a tool's environment is one we may hand a workload. `PATH` and `HOME` are the workload's own to compose; the loader and shell hooks (`LD_*`, `BASH_ENV`, `ENV`, `IFS`) would reach every process in the workload rather than only the tool's own binaries; and anything outside the POSIX name shape would arrive from a source that is not the engine.
pub fn is_safe_env_name(name: &str) -> bool {
    const WORKLOAD_OWNED: &[&str] = &["PATH", "HOME", "BASH_ENV", "ENV", "IFS"];
    !name.is_empty()
        && !WORKLOAD_OWNED.contains(&name)
        && !name.starts_with("LD_")
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
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

/// Whether a version already names a full release, so `push` keeps it verbatim without the index; counting numeric runs admits the vendor shapes the resolver itself emits (`temurin-21.0.5+11.0.LTS`), and any answer of [`resolve_from_index`] must satisfy it.
pub fn is_exact_version(version: &str) -> bool {
    version != LATEST
        && version
            .split(|c: char| !c.is_ascii_digit())
            .filter(|run| !run.is_empty())
            .count()
            >= 3
}

/// Whether the index publishes this version verbatim today — the best-effort membership half of push verification, never a resolution.
pub fn index_lists_exact(body: &str, version: &str) -> bool {
    body.lines().any(|line| line.trim() == version)
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
    fn a_tool_may_set_its_own_vars_but_never_the_workloads_own() {
        // A tool's env is composed into the workload and into every later `lns exec`; PATH is the bin dirs' job and HOME belongs to the workload, so neither may arrive this way.
        assert!(is_safe_env_name("RUSTUP_HOME"));
        assert!(is_safe_env_name("SOME_TOOL_2"));
        // The loader and shell hooks reach every process in the workload, not just the tool's own binaries.
        for refused in [
            "PATH",
            "HOME",
            "LD_PRELOAD",
            "LD_LIBRARY_PATH",
            "BASH_ENV",
            "ENV",
            "IFS",
            "",
            "2BAD",
            "lowercase",
            "HAS SPACE",
            "A=B",
        ] {
            assert!(!is_safe_env_name(refused), "{refused:?} must be refused");
        }
    }

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
        for exact in [
            "22.11.0",
            "3.12.6",
            "1.0.0.1",
            "temurin-21.0.5+11.0.LTS",
            "v1.2.3",
            "1.22.0-rc1",
            "3.13.0rc1",
        ] {
            assert!(is_exact_version(exact), "{exact}");
        }
        for fuzzy in ["22", "22.11", LATEST, "", "22.x.0", "22..0", "temurin-21"] {
            assert!(!is_exact_version(fuzzy), "{fuzzy}");
        }
    }

    #[test]
    fn every_line_the_real_index_publishes_is_a_usable_stable_pin() {
        // The bodies under index_snapshots/ are the real index (refreshed by bump-mise), so this is the contract's reality check: every published line must be a safe version whose resolution is a fixed point — a pin the resolver answered once re-answers identically on the next push.
        for (tool, body) in [
            ("java", include_str!("index_snapshots/java.txt")),
            ("go", include_str!("index_snapshots/go.txt")),
            ("jq", include_str!("index_snapshots/jq.txt")),
            ("python", include_str!("index_snapshots/python.txt")),
            ("node", include_str!("index_snapshots/node.txt")),
            ("ruby", include_str!("index_snapshots/ruby.txt")),
        ] {
            for line in body.lines().map(str::trim).filter(|line| !line.is_empty()) {
                assert!(is_safe_version(line), "{tool}: {line:?}");
                let answer = resolve_from_index(tool, line, body)
                    .unwrap_or_else(|e| panic!("{tool}: {line:?}: {e:#}"));
                // resolve is pure, so `answer == line` is already the fixed point; only a differing answer needs the second resolution.
                if answer != line {
                    let again = resolve_from_index(tool, &answer, body).unwrap();
                    assert_eq!(again, answer, "{tool}: {line:?}");
                }
            }
        }
    }

    #[test]
    fn the_index_membership_check_is_verbatim() {
        let body = "21.0.2\ntemurin-21.0.5+11.0.LTS\n 22.11.0 \n";
        assert!(index_lists_exact(body, "temurin-21.0.5+11.0.LTS"));
        assert!(index_lists_exact(body, "22.11.0"), "lines are trimmed");
        assert!(!index_lists_exact(body, "21"), "a prefix is not membership");
        assert!(!index_lists_exact(body, "21.0"), "nor a dotted prefix");
        assert!(!index_lists_exact("", "21.0.2"));
    }

    #[test]
    fn build_metadata_numerals_count_toward_exactness_by_decision() {
        // `21+11.0` names one release component yet counts as exact; the accepted consequence is that push keeps it verbatim and provisioning fails loud, whereas narrowing the rule would re-open the vendor-pin drift this predicate exists to prevent.
        assert!(is_exact_version("21+11.0"));
    }

    #[test]
    fn a_resolver_answer_republishes_to_the_same_pin() {
        // jq ships `1.6` as a finished release, so no shape rule can promise every answer is exact; the durable contract is stability — re-resolving a published pin returns that pin, so a re-push with the index up reproduces the artifact, and offline re-push narrows to exact-classified pins.
        let body = "1.5\n1.6\n21.0.2\ntemurin-21.0.4+7\ntemurin-21.0.5+11.0.LTS\n22.9.0\n22.11.0\n";
        for request in ["1.6", "temurin-21", "21", "22", "22.11.0", LATEST] {
            let answer = resolve_from_index("some-tool", request, body).unwrap();
            let again = resolve_from_index("some-tool", &answer, body).unwrap();
            assert_eq!(again, answer, "{request} → {answer}");
        }
        assert!(
            is_exact_version("temurin-21.0.5+11.0.LTS") && !is_exact_version("1.6"),
            "the vendor answer re-pushes offline; the two-run answer needs the index back"
        );
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
