//! Whether a pulled sandbox may read one of this machine's files lives in `~/.lns-host-path-decisions.json`, per machine and never committed — a `hostPath` makes what a document mounts depend on the machine running it, which is a risk one developer accepts on one computer, not a rule a directory keeps.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HostPathDecision {
    Allow,
    Deny,
}

pub type HostPathDecisionFile = HashMap<String, HostPathDecision>;

pub trait HostPathDecisionStore: Send + Sync {
    fn load(&self) -> io::Result<HostPathDecisionFile>;
    fn save(&self, state: &HostPathDecisionFile) -> io::Result<()>;
}

/// The repository, tag and digest stripped, so a version bump keeps the answer and a different sandbox never inherits it.
pub fn decision_key(reference: &str, host_path: &str) -> String {
    format!("{}|{host_path}", repository_of(reference))
}

/// The reference without its tag or digest — what a decision is keyed on, and what the prompt names so the developer sees which sandbox is asking.
pub fn repository_of(reference: &str) -> &str {
    let reference = match reference.split_once('@') {
        Some((repository, _digest)) => repository,
        None => reference,
    };
    match reference.rsplit_once(':') {
        Some((repository, tag)) if !tag.contains('/') => repository,
        _ => reference,
    }
}

/// Falls back to `./.lns-host-path-decisions.json` when `HOME` is unset rather than panicking.
pub fn default_host_path_decisions_path() -> PathBuf {
    if let Some(p) = std::env::var_os("LNS_HOST_PATH_DECISIONS_PATH") {
        return PathBuf::from(p);
    }
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".lns-host-path-decisions.json"))
        .unwrap_or_else(|| PathBuf::from(".lns-host-path-decisions.json"))
}

pub struct JsonFileHostPathDecisionStore {
    pub path: PathBuf,
}

impl JsonFileHostPathDecisionStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl HostPathDecisionStore for JsonFileHostPathDecisionStore {
    fn load(&self) -> io::Result<HostPathDecisionFile> {
        match fs::read_to_string(&self.path) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(HashMap::new()),
            Err(e) => Err(e),
        }
    }

    fn save(&self, state: &HostPathDecisionFile) -> io::Result<()> {
        let json = serde_json::to_string_pretty(state)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        crate::secure_file::write_json_secret_atomic(&self.path, json.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn decisions_serialize_to_kebab_case() {
        assert_eq!(
            serde_json::to_value(HostPathDecision::Allow).unwrap(),
            json!("allow")
        );
        assert_eq!(
            serde_json::to_value(HostPathDecision::Deny).unwrap(),
            json!("deny")
        );
    }

    #[test]
    fn decisions_round_trip_through_json() {
        for d in [HostPathDecision::Allow, HostPathDecision::Deny] {
            let s = serde_json::to_string(&d).unwrap();
            let parsed: HostPathDecision = serde_json::from_str(&s).unwrap();
            assert_eq!(d, parsed);
        }
    }

    #[test]
    fn unknown_decision_deserializes_as_error() {
        let r: serde_json::Result<HostPathDecision> = serde_json::from_str(r#""ask""#);
        assert!(r.is_err());
    }

    #[test]
    fn a_tag_bump_keeps_the_same_key() {
        assert_eq!(
            decision_key("ghcr.io/team/hermes:1.4.0", "~/.gitconfig"),
            decision_key("ghcr.io/team/hermes:2.0.0", "~/.gitconfig")
        );
    }

    #[test]
    fn a_digest_pin_keeps_the_same_key_as_its_tag() {
        assert_eq!(
            decision_key(
                &format!("ghcr.io/team/hermes@sha256:{}", "a".repeat(64)),
                "~/.gitconfig"
            ),
            decision_key("ghcr.io/team/hermes:1.4.0", "~/.gitconfig")
        );
    }

    #[test]
    fn a_different_repository_is_a_different_key() {
        assert_ne!(
            decision_key("ghcr.io/other/agent:1.0.0", "~/.gitconfig"),
            decision_key("ghcr.io/team/hermes:1.0.0", "~/.gitconfig")
        );
    }

    #[test]
    fn a_different_host_path_is_a_different_key() {
        assert_ne!(
            decision_key("ghcr.io/team/hermes:1.0.0", "~/.gitconfig"),
            decision_key("ghcr.io/team/hermes:1.0.0", "~/.npmrc")
        );
    }

    #[test]
    fn a_registry_port_is_not_read_as_a_tag() {
        assert_eq!(
            decision_key("localhost:5000/team/hermes", "~/.gitconfig"),
            "localhost:5000/team/hermes|~/.gitconfig"
        );
    }

    #[test]
    fn a_reference_with_no_tag_keys_on_itself() {
        assert_eq!(
            decision_key("ghcr.io/team/hermes", "~/.gitconfig"),
            "ghcr.io/team/hermes|~/.gitconfig"
        );
    }

    #[test]
    fn load_returns_empty_state_when_file_missing() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = JsonFileHostPathDecisionStore::new(dir.path().join("never-created.json"));
        assert!(store.load().unwrap().is_empty());
    }

    #[test]
    fn load_returns_invalid_data_for_malformed_json() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("decisions.json");
        fs::write(&path, "{ not json").unwrap();
        let store = JsonFileHostPathDecisionStore::new(path);
        assert_eq!(store.load().unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn load_propagates_non_not_found_io_errors() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = JsonFileHostPathDecisionStore::new(dir.path().to_path_buf());
        assert_ne!(store.load().unwrap_err().kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn save_then_load_round_trips_full_state() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = JsonFileHostPathDecisionStore::new(dir.path().join("decisions.json"));
        let mut original = HostPathDecisionFile::new();
        original.insert(
            decision_key("ghcr.io/team/hermes:1.4.0", "~/.gitconfig"),
            HostPathDecision::Allow,
        );
        original.insert(
            decision_key("ghcr.io/team/hermes:1.4.0", "~/.npmrc"),
            HostPathDecision::Deny,
        );
        store.save(&original).unwrap();
        assert_eq!(store.load().unwrap(), original);
    }

    #[test]
    #[serial_test::serial(env)]
    fn default_path_uses_override_when_set() {
        use crate::test_env::EnvVarGuard;
        let _g1 = EnvVarGuard::set(
            "LNS_HOST_PATH_DECISIONS_PATH",
            "/tmp/custom-host-paths.json",
        );
        let _g2 = EnvVarGuard::set("HOME", "/tmp/home-should-be-ignored");
        assert_eq!(
            default_host_path_decisions_path(),
            PathBuf::from("/tmp/custom-host-paths.json")
        );
    }

    #[test]
    #[serial_test::serial(env)]
    fn default_path_falls_back_to_home_dotfile() {
        use crate::test_env::EnvVarGuard;
        let _g1 = EnvVarGuard::unset("LNS_HOST_PATH_DECISIONS_PATH");
        let _g2 = EnvVarGuard::set("HOME", "/home/dev");
        assert_eq!(
            default_host_path_decisions_path(),
            PathBuf::from("/home/dev/.lns-host-path-decisions.json")
        );
    }

    #[test]
    #[serial_test::serial(env)]
    fn default_path_falls_back_to_cwd_when_home_unset() {
        use crate::test_env::EnvVarGuard;
        let _g1 = EnvVarGuard::unset("LNS_HOST_PATH_DECISIONS_PATH");
        let _g2 = EnvVarGuard::unset("HOME");
        assert_eq!(
            default_host_path_decisions_path(),
            PathBuf::from(".lns-host-path-decisions.json")
        );
    }
}
