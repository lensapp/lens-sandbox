use std::ffi::OsString;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum LnsHomeError {
    #[error(
        "could not determine your home directory; set LNS_HOME to the absolute directory lns should keep its data in"
    )]
    NoHome,
    #[error(
        "LNS_HOME must be a non-empty absolute path, got {0:?}; lns keeps everything in one directory and will not scatter it relative to the working directory"
    )]
    NotAbsolute(String),
}

/// Everything lns keeps for you lives in one directory, so there is one thing to back up and one thing `lns uninstall --purge` removes.
pub fn lns_home() -> Result<PathBuf, LnsHomeError> {
    lns_home_with(|key| std::env::var_os(key), dirs::home_dir())
}

pub fn lns_home_with(
    env: impl Fn(&str) -> Option<OsString>,
    home: Option<PathBuf>,
) -> Result<PathBuf, LnsHomeError> {
    match env("LNS_HOME") {
        Some(overridden) => {
            let path = PathBuf::from(&overridden);
            if path.as_os_str().is_empty() || !path.is_absolute() {
                return Err(LnsHomeError::NotAbsolute(
                    overridden.to_string_lossy().into_owned(),
                ));
            }
            Ok(path)
        }
        None => home
            .map(|home| home.join(".lns"))
            .ok_or(LnsHomeError::NoHome),
    }
}

/// These paths also feed secret writes, so no resolvable home is a refusal, never a project-relative fallback.
fn per_machine_path(
    home: Result<PathBuf, LnsHomeError>,
    name: &str,
) -> Result<PathBuf, LnsHomeError> {
    Ok(home?.join(name))
}

fn in_lns_home(name: &str) -> Result<PathBuf, LnsHomeError> {
    per_machine_path(lns_home(), name)
}

pub fn build_cache_root() -> Result<PathBuf, LnsHomeError> {
    Ok(lns_home()?.join("builds"))
}

pub fn short_run_id(id: &str) -> &str {
    id.char_indices().nth(12).map_or(id, |(i, _)| &id[..i])
}

/// A sandbox's chain outlives the sandbox, so it cannot live in `runs/`, which is what removing one deletes.
pub fn audit_runs_root() -> Result<PathBuf, LnsHomeError> {
    Ok(lns_home()?.join("audit"))
}

pub fn audit_log_for_run(run_id: &str) -> Result<PathBuf, LnsHomeError> {
    Ok(audit_runs_root()?.join(run_id).join("audit.jsonl"))
}

pub fn audit_anchor_for_run(run_id: &str) -> Result<PathBuf, LnsHomeError> {
    Ok(audit_runs_root()?.join(run_id).join("audit.anchor"))
}

pub fn connection_ledger() -> Result<PathBuf, LnsHomeError> {
    Ok(lns_home()?.join("ledger.jsonl"))
}

pub fn connection_ledger_anchor() -> Result<PathBuf, LnsHomeError> {
    Ok(lns_home()?.join("ledger.anchor"))
}

pub fn config_path() -> Result<PathBuf, LnsHomeError> {
    Ok(lns_home()?.join("config.yaml"))
}

pub fn registry_auth_path() -> Result<PathBuf, LnsHomeError> {
    in_lns_home("registry-auth.json")
}

pub fn host_path_decisions_path() -> Result<PathBuf, LnsHomeError> {
    in_lns_home("host-path-decisions.json")
}

pub fn host_bind_decisions_path() -> Result<PathBuf, LnsHomeError> {
    in_lns_home("host-bind-decisions.json")
}

/// One directory per installed connector, holding its document verbatim and the digest it came from (sandbox-spec §7.1).
pub fn connectors_root() -> Result<PathBuf, LnsHomeError> {
    in_lns_home("connectors")
}

/// A project's one answer per connector — granted or declined — because `forget` clears either (sandbox-spec §8.4).
pub fn connector_grants_path() -> Result<PathBuf, LnsHomeError> {
    in_lns_home("connector-grants.json")
}

/// Holds the values an authentication returned, so this is the one connector store that is a secret.
pub fn connector_values_path() -> Result<PathBuf, LnsHomeError> {
    in_lns_home("connector-values.json")
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
    fn an_empty_lns_home_override_is_rejected_not_a_cwd_relative_root() {
        let err = lns_home_with(
            |key| (key == "LNS_HOME").then(|| OsString::from("")),
            Some(PathBuf::from("/home/dev")),
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("absolute"),
            "an empty override must be refused with the way out named: {err}"
        );
    }

    #[test]
    fn a_relative_lns_home_override_is_rejected_before_it_can_scatter_data() {
        for bad in ["relative/dir", ".", "./here"] {
            let err = lns_home_with(
                |key| (key == "LNS_HOME").then(|| OsString::from(bad)),
                Some(PathBuf::from("/home/dev")),
            )
            .unwrap_err();
            assert!(
                err.to_string().contains("absolute"),
                "{bad:?} must be refused: {err}"
            );
        }
    }

    #[test]
    fn a_per_machine_file_with_no_home_is_an_error_not_a_project_relative_write() {
        let err = per_machine_path(Err(LnsHomeError::NoHome), "registry-auth.json").unwrap_err();
        assert!(
            err.to_string().contains("LNS_HOME"),
            "these paths feed secret writes, so nowhere to keep them is a refusal: {err}"
        );
        assert_eq!(
            per_machine_path(Ok(PathBuf::from("/home/dev/.lns")), "registry-auth.json").unwrap(),
            PathBuf::from("/home/dev/.lns/registry-auth.json")
        );
    }

    #[test]
    fn lns_home_is_a_dot_directory_beside_the_rest_of_your_home() {
        let resolved = lns_home_with(|_| None, Some(PathBuf::from("/home/dev"))).unwrap();
        assert_eq!(resolved, PathBuf::from("/home/dev/.lns"));
    }

    #[test]
    fn lns_home_redirects_the_whole_directory_at_once() {
        let resolved = lns_home_with(
            |key| (key == "LNS_HOME").then(|| OsString::from("/tmp/elsewhere")),
            Some(PathBuf::from("/home/dev")),
        )
        .unwrap();
        assert_eq!(
            resolved,
            PathBuf::from("/tmp/elsewhere"),
            "the override names the root itself, not a directory to put .lns inside"
        );
    }

    #[test]
    fn with_no_home_and_no_override_there_is_nowhere_to_keep_anything() {
        let err = lns_home_with(|_| None, None).unwrap_err();
        assert!(
            err.to_string().contains("LNS_HOME"),
            "the answer must name the way out: {err}"
        );
    }

    #[test]
    fn every_path_lns_keeps_for_you_is_inside_the_one_directory() {
        let home = lns_home().expect("a home resolves in the test env");
        for path in [
            build_cache_root().unwrap(),
            audit_runs_root().unwrap(),
            audit_log_for_run("42").unwrap(),
            audit_anchor_for_run("42").unwrap(),
            connection_ledger().unwrap(),
            connection_ledger_anchor().unwrap(),
            config_path().unwrap(),
            registry_auth_path().unwrap(),
            host_path_decisions_path().unwrap(),
            host_bind_decisions_path().unwrap(),
            connectors_root().unwrap(),
            connector_grants_path().unwrap(),
            connector_values_path().unwrap(),
        ] {
            assert!(
                path.starts_with(&home),
                "one directory, one thing to back up: {path:?} is outside {home:?}"
            );
        }
    }

    #[test]
    fn the_three_connector_stores_are_three_distinct_paths() {
        // Aliasing two of these would make one store's writes clobber the other's, and the values store is the one holding real tokens.
        let paths = [
            connectors_root().unwrap(),
            connector_grants_path().unwrap(),
            connector_values_path().unwrap(),
        ];
        let distinct: std::collections::BTreeSet<_> = paths.iter().collect();
        assert_eq!(
            distinct.len(),
            paths.len(),
            "each connector store needs its own file: {paths:?}"
        );
    }

    #[test]
    fn a_sandboxs_chain_is_not_inside_the_directory_removing_it_deletes() {
        // §3.6: removing a sandbox does not remove what it did.
        let scratch = lns_home().unwrap().join("runs").join("42");
        assert!(
            !audit_log_for_run("42").unwrap().starts_with(&scratch),
            "the chain must outlive the run directory that `lns sandbox rm` deletes"
        );
    }

    #[test]
    fn the_audit_anchor_is_a_sibling_of_the_log_it_anchors() {
        let log = audit_log_for_run("42").expect("audit_log_for_run");
        let anchor = audit_anchor_for_run("42").expect("audit_anchor_for_run");
        assert_eq!(anchor.parent(), log.parent());
        assert!(anchor.ends_with("audit/42/audit.anchor"));
        let ledger = connection_ledger().expect("connection_ledger");
        assert_eq!(
            connection_ledger_anchor().unwrap().parent(),
            ledger.parent()
        );
    }
}
