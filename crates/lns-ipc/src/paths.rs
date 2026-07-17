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

pub fn build_cache_root() -> Result<PathBuf, CachePathError> {
    Ok(cache_root()?.join("builds"))
}

pub fn short_run_id(id: &str) -> &str {
    id.char_indices().nth(12).map_or(id, |(i, _)| &id[..i])
}

pub fn audit_runs_root() -> Result<PathBuf, CachePathError> {
    Ok(data_root()?.join("runs"))
}

pub fn audit_log_for_run(run_id: &str) -> Result<PathBuf, CachePathError> {
    Ok(audit_runs_root()?.join(run_id).join("audit.jsonl"))
}

pub fn audit_anchor_for_run(run_id: &str) -> Result<PathBuf, CachePathError> {
    Ok(audit_runs_root()?.join(run_id).join("audit.anchor"))
}

pub fn connection_ledger() -> Result<PathBuf, CachePathError> {
    Ok(data_root()?.join("ledger.jsonl"))
}

pub fn connection_ledger_anchor() -> Result<PathBuf, CachePathError> {
    Ok(data_root()?.join("ledger.anchor"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_run_id_truncates_to_twelve_chars_and_passes_shorter_ids_through() {
        assert_eq!(
            short_run_id("1a2b3c4d0000000000000000000000aa"),
            "1a2b3c4d0000"
        );
        assert_eq!(short_run_id("abc"), "abc");
        assert_eq!(short_run_id(""), "");
    }

    #[test]
    fn short_run_id_truncates_on_a_char_boundary_for_tampered_multibyte_ids() {
        assert_eq!(short_run_id("abcdefghijké"), "abcdefghijké");
        assert_eq!(short_run_id("aaaaaaaaaaaéz"), "aaaaaaaaaaaé");
    }

    #[test]
    fn audit_log_lives_under_data_root_so_it_outlives_the_ephemeral_run_dir() {
        let data = data_root().expect("data dir resolves in test env");
        let p = audit_log_for_run("42").expect("audit_log_for_run");
        assert_eq!(p, data.join("runs").join("42").join("audit.jsonl"));
        assert!(
            !p.starts_with(cache_root().expect("cache dir resolves in test env")),
            "the audit trail must outlive ephemeral run dirs, so it cannot live under cache_root: {p:?}"
        );
    }

    #[test]
    fn audit_runs_root_is_the_shared_base_of_every_per_run_log() {
        let root = audit_runs_root().expect("audit_runs_root");
        assert_eq!(root, data_root().expect("data dir").join("runs"));
        assert!(audit_log_for_run("42").expect("log").starts_with(&root));
        assert!(
            audit_anchor_for_run("42")
                .expect("anchor")
                .starts_with(&root)
        );
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
    fn build_cache_root_lives_under_cache_root_not_data_root() {
        let cache = cache_root().expect("cache dir resolves in test env");
        let builds = build_cache_root().expect("build_cache_root");
        assert_eq!(builds, cache.join("builds"));
        assert!(
            !builds.starts_with(data_root().expect("data dir resolves in test env")),
            "the build cache is reconstructible content, so it belongs under cache_root, not data_root"
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

    #[test]
    fn connection_ledger_lives_under_data_root_not_cache_root() {
        let data = data_root().expect("data dir resolves in test env");
        let ledger = connection_ledger().expect("connection_ledger");
        assert_eq!(ledger, data.join("ledger.jsonl"));
        assert!(
            !ledger.starts_with(cache_root().expect("cache dir resolves in test env")),
            "the ledger must outlive ephemeral run dirs, so it cannot live under cache_root: {ledger:?}"
        );
    }

    #[test]
    fn connection_ledger_anchor_is_a_sibling_of_the_ledger() {
        let ledger = connection_ledger().expect("connection_ledger");
        let anchor = connection_ledger_anchor().expect("connection_ledger_anchor");
        assert_eq!(anchor.parent(), ledger.parent());
        assert!(anchor.ends_with("ledger.anchor"));
    }
}
