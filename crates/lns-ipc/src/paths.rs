use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CachePathError {
    #[error("could not determine cache dir")]
    NoCacheDir,
    #[error("could not determine data dir")]
    NoDataDir,
}

pub fn cache_root() -> Result<PathBuf, CachePathError> {
    dirs::cache_dir()
        .map(|p| p.join("lns"))
        .ok_or(CachePathError::NoCacheDir)
}

pub fn data_root() -> Result<PathBuf, CachePathError> {
    dirs::data_dir()
        .map(|p| p.join("lns"))
        .ok_or(CachePathError::NoDataDir)
}

pub fn audit_log_for_run(run_id: &str) -> Result<PathBuf, CachePathError> {
    Ok(cache_root()?.join("runs").join(run_id).join("audit.jsonl"))
}

pub fn audit_anchor_for_run(run_id: &str) -> Result<PathBuf, CachePathError> {
    Ok(cache_root()?.join("runs").join(run_id).join("audit.anchor"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audit_log_path_appends_runs_runid_filename_under_cache_root() {
        let root = cache_root().expect("cache dir resolves in test env");
        let p = audit_log_for_run("42").expect("audit_log_for_run");
        assert_eq!(p, root.join("runs").join("42").join("audit.jsonl"));
    }

    #[test]
    fn audit_anchor_path_is_a_sibling_of_the_audit_log() {
        let log = audit_log_for_run("42").expect("audit_log_for_run");
        let anchor = audit_anchor_for_run("42").expect("audit_anchor_for_run");
        assert_eq!(anchor.parent(), log.parent());
        assert!(anchor.ends_with("runs/42/audit.anchor"));
    }

    #[test]
    fn cache_root_is_non_empty_absolute_and_ends_with_lns() {
        let root = cache_root().expect("cache dir resolves in test env");
        assert!(
            !root.as_os_str().is_empty(),
            "cache_root must return a non-empty path"
        );
        assert!(
            root.ends_with("lns"),
            "cache_root must end with 'lns', got: {root:?}"
        );
        assert!(
            root.is_absolute(),
            "cache_root must be absolute, got: {root:?}"
        );
    }

    #[test]
    fn data_root_is_absolute_and_ends_with_lns() {
        let root = data_root().expect("data dir resolves in test env");
        assert!(
            root.ends_with("lns"),
            "data_root must end with 'lns', got: {root:?}"
        );
        assert!(
            root.is_absolute(),
            "data_root must be absolute, got: {root:?}"
        );
    }
}
