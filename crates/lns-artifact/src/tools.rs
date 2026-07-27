use anyhow::{Result, bail};

/// A portable declared tool: `name@version`, where version may be fuzzy (`22`) or `latest`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolRef {
    pub name: String,
    pub version: String,
}

impl ToolRef {
    pub fn is_latest(&self) -> bool {
        self.version == "latest"
    }
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
    if name.is_empty() || version.is_empty() || !is_valid_version(version) {
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

fn is_valid_version(version: &str) -> bool {
    !version
        .chars()
        .any(|c| c == '@' || c == ':' || c.is_whitespace() || c.is_control())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_reads_name_and_version() {
        let tool = parse("node@22").unwrap();
        assert_eq!(tool.name, "node");
        assert_eq!(tool.version, "22");
        assert!(!tool.is_latest());
        assert_eq!(tool.to_string(), "node@22");
    }

    #[test]
    fn parse_accepts_latest() {
        let tool = parse("node@latest").unwrap();
        assert!(tool.is_latest());
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
            "npm:prettier@3",
            "npm:prettier",
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
}
