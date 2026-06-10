use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub fn merged_run_env(env_files: &[PathBuf], env_flags: &[String]) -> Result<Vec<String>> {
    let mut entries: Vec<(String, String)> = Vec::new();
    for path in env_files {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("reading --env-file {}", path.display()))?;
        parse_env_file(path, &content, &mut entries)?;
    }
    for kv in env_flags {
        if let Some((key, value)) = kv.split_once('=') {
            upsert(&mut entries, key, value);
        }
    }
    Ok(entries
        .into_iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect())
}

fn parse_env_file(path: &Path, content: &str, entries: &mut Vec<(String, String)>) -> Result<()> {
    for (idx, raw) in content.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let lineno = idx + 1;
        let Some((key, value)) = line.split_once('=') else {
            anyhow::bail!(
                "{}:{lineno}: `{line}` has no value — host env passthrough is not supported; \
                 write KEY=VALUE",
                path.display(),
            );
        };
        if key.is_empty() {
            anyhow::bail!(
                "{}:{lineno}: empty variable name in `{line}`; write KEY=VALUE",
                path.display(),
            );
        }
        upsert(entries, key, value);
    }
    Ok(())
}

fn upsert(entries: &mut Vec<(String, String)>, key: &str, value: &str) {
    match entries.iter_mut().find(|(k, _)| k == key) {
        Some(slot) => slot.1 = value.to_string(),
        None => entries.push((key.to_string(), value.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_file(dir: &tempfile::TempDir, name: &str, content: &str) -> PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, content).expect("write env file");
        path
    }

    #[test]
    fn flags_alone_merge_without_any_file() {
        let env = merged_run_env(&[], &["A=1".into(), "B=2".into()]).unwrap();
        assert_eq!(env, ["A=1", "B=2"]);
    }

    #[test]
    fn a_value_keeps_embedded_equals_signs() {
        let dir = tempfile::TempDir::new().unwrap();
        let f = env_file(&dir, "a.env", "DSN=user=admin;pw=x\n");
        let env = merged_run_env(&[f], &[]).unwrap();
        assert_eq!(env, ["DSN=user=admin;pw=x"]);
    }

    #[test]
    fn an_empty_value_is_preserved() {
        let dir = tempfile::TempDir::new().unwrap();
        let f = env_file(&dir, "a.env", "FEATURE_X=\n");
        let env = merged_run_env(&[f], &[]).unwrap();
        assert_eq!(env, ["FEATURE_X="]);
    }

    #[test]
    fn crlf_line_endings_do_not_leak_into_values() {
        let dir = tempfile::TempDir::new().unwrap();
        let f = env_file(&dir, "a.env", "PORT=3003\r\nHOST=db\r\n");
        let env = merged_run_env(&[f], &[]).unwrap();
        assert_eq!(env, ["PORT=3003", "HOST=db"]);
    }

    #[test]
    fn a_duplicate_key_in_one_file_keeps_the_last_value() {
        let dir = tempfile::TempDir::new().unwrap();
        let f = env_file(&dir, "a.env", "A=first\nA=last\n");
        let env = merged_run_env(&[f], &[]).unwrap();
        assert_eq!(env, ["A=last"]);
    }

    #[test]
    fn a_bare_key_fails_with_file_and_line() {
        let dir = tempfile::TempDir::new().unwrap();
        let f = env_file(&dir, "a.env", "A=1\nHOME\n");
        let err = format!("{:#}", merged_run_env(&[f], &[]).unwrap_err());
        assert!(err.contains("a.env:2:"), "got: {err}");
        assert!(err.contains("KEY=VALUE"), "got: {err}");
        assert!(err.contains("passthrough"), "got: {err}");
    }

    #[test]
    fn an_empty_key_fails_with_file_and_line() {
        let dir = tempfile::TempDir::new().unwrap();
        let f = env_file(&dir, "a.env", "=oops\n");
        let err = format!("{:#}", merged_run_env(&[f], &[]).unwrap_err());
        assert!(err.contains("a.env:1:"), "got: {err}");
        assert!(err.contains("empty variable name"), "got: {err}");
    }

    #[test]
    fn an_unreadable_file_fails_naming_the_path() {
        let dir = tempfile::TempDir::new().unwrap();
        let missing = dir.path().join("nope.env");
        let err = format!("{:#}", merged_run_env(&[missing], &[]).unwrap_err());
        assert!(err.contains("nope.env"), "got: {err}");
        assert!(err.contains("--env-file"), "got: {err}");
    }
}
