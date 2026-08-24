use std::ffi::OsString;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Error)]
#[error(
    "could not determine your home directory; set LNS_HOME to the directory lns should keep its data in"
)]
pub struct NoLnsHome;

/// Everything lns keeps for you lives in one directory, so there is one thing to back up and one thing `lns uninstall --purge` removes.
pub fn lns_home() -> Result<PathBuf, NoLnsHome> {
    lns_home_with(|key| std::env::var_os(key), dirs::home_dir())
}

pub fn lns_home_with(
    env: impl Fn(&str) -> Option<OsString>,
    home: Option<PathBuf>,
) -> Result<PathBuf, NoLnsHome> {
    match env("LNS_HOME") {
        Some(overridden) => Ok(PathBuf::from(overridden)),
        None => home.map(|home| home.join(".lns")).ok_or(NoLnsHome),
    }
}

fn in_lns_home(name: &str) -> PathBuf {
    under(lns_home(), name)
}

/// A per-machine file lns keeps for you; with no home to resolve it lands in the working directory rather than failing a command that only wanted to read one that may not exist.
fn under(home: Result<PathBuf, NoLnsHome>, name: &str) -> PathBuf {
    home.unwrap_or_else(|_| PathBuf::from(".lns")).join(name)
}

pub fn build_cache_root() -> Result<PathBuf, NoLnsHome> {
    Ok(lns_home()?.join("builds"))
}

pub fn short_run_id(id: &str) -> &str {
    id.char_indices().nth(12).map_or(id, |(i, _)| &id[..i])
}

pub fn audit_runs_root() -> Result<PathBuf, NoLnsHome> {
    Ok(lns_home()?.join("runs"))
}

pub fn audit_log_for_run(run_id: &str) -> Result<PathBuf, NoLnsHome> {
    Ok(audit_runs_root()?.join(run_id).join("audit.jsonl"))
}

pub fn audit_anchor_for_run(run_id: &str) -> Result<PathBuf, NoLnsHome> {
    Ok(audit_runs_root()?.join(run_id).join("audit.anchor"))
}

pub fn connection_ledger() -> Result<PathBuf, NoLnsHome> {
    Ok(lns_home()?.join("ledger.jsonl"))
}

pub fn connection_ledger_anchor() -> Result<PathBuf, NoLnsHome> {
    Ok(lns_home()?.join("ledger.anchor"))
}

pub fn config_path() -> Result<PathBuf, NoLnsHome> {
    Ok(lns_home()?.join("config.yaml"))
}

pub fn connectors_path() -> PathBuf {
    in_lns_home("connectors.yaml")
}

pub fn credentials_path() -> PathBuf {
    in_lns_home("credentials.json")
}

pub fn registry_auth_path() -> PathBuf {
    in_lns_home("registry-auth.json")
}

pub fn workload_grants_path() -> PathBuf {
    in_lns_home("workload-grants.json")
}

pub fn host_path_decisions_path() -> PathBuf {
    in_lns_home("host-path-decisions.json")
}

pub fn host_bind_decisions_path() -> PathBuf {
    in_lns_home("host-bind-decisions.json")
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
            connectors_path(),
            credentials_path(),
            registry_auth_path(),
            workload_grants_path(),
            host_path_decisions_path(),
            host_bind_decisions_path(),
        ] {
            assert!(
                path.starts_with(&home),
                "one directory, one thing to back up: {path:?} is outside {home:?}"
            );
        }
    }

    #[test]
    fn the_audit_anchor_is_a_sibling_of_the_log_it_anchors() {
        let log = audit_log_for_run("42").expect("audit_log_for_run");
        let anchor = audit_anchor_for_run("42").expect("audit_anchor_for_run");
        assert_eq!(anchor.parent(), log.parent());
        assert!(anchor.ends_with("runs/42/audit.anchor"));
        let ledger = connection_ledger().expect("connection_ledger");
        assert_eq!(
            connection_ledger_anchor().unwrap().parent(),
            ledger.parent()
        );
    }

    #[test]
    fn a_per_machine_file_falls_back_to_the_working_directory_rather_than_failing_a_read() {
        // These callers only ever want to read a file that may not exist, so nowhere to look is an empty answer rather than a broken command.
        assert_eq!(
            under(Err(NoLnsHome), "connectors.yaml"),
            PathBuf::from(".lns/connectors.yaml")
        );
        assert_eq!(
            under(Ok(PathBuf::from("/home/dev/.lns")), "connectors.yaml"),
            PathBuf::from("/home/dev/.lns/connectors.yaml")
        );
    }
}
