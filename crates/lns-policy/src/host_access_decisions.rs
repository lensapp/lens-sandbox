//! Host-access verdicts live in `~/.lns-host-access-decisions.json`, per-machine and never committed — a decline is a standing refusal to expose a host capability, which is a per-machine risk judgement rather than a shareable rule.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HostAccessVerdict {
    Declined,
}

pub type HostAccessDecisionFile = HashMap<String, HostAccessVerdict>;

pub trait HostAccessDecisionStore: Send + Sync {
    fn load(&self) -> io::Result<HostAccessDecisionFile>;
    fn save(&self, state: &HostAccessDecisionFile) -> io::Result<()>;
}

/// Falls back to `./.lns-host-access-decisions.json` when `HOME` is unset rather than panicking.
pub fn default_host_access_decisions_path() -> PathBuf {
    if let Some(p) = std::env::var_os("LNS_HOST_ACCESS_DECISIONS_PATH") {
        return PathBuf::from(p);
    }
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".lns-host-access-decisions.json"))
        .unwrap_or_else(|| PathBuf::from(".lns-host-access-decisions.json"))
}

pub struct JsonFileHostAccessDecisionStore {
    pub path: PathBuf,
}

impl JsonFileHostAccessDecisionStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl HostAccessDecisionStore for JsonFileHostAccessDecisionStore {
    fn load(&self) -> io::Result<HostAccessDecisionFile> {
        match fs::read_to_string(&self.path) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(HashMap::new()),
            Err(e) => Err(e),
        }
    }

    fn save(&self, state: &HostAccessDecisionFile) -> io::Result<()> {
        let json = serde_json::to_string_pretty(state)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        crate::secure_file::write_json_secret_atomic(&self.path, json.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn state() -> HostAccessDecisionFile {
        let mut m = HostAccessDecisionFile::new();
        m.insert("some-access".into(), HostAccessVerdict::Declined);
        m
    }

    #[test]
    fn a_verdict_serializes_to_kebab_case() {
        assert_eq!(
            serde_json::to_value(HostAccessVerdict::Declined).unwrap(),
            json!("declined")
        );
    }

    #[test]
    fn an_unknown_verdict_is_an_error_rather_than_a_silent_default() {
        let r: serde_json::Result<HostAccessVerdict> = serde_json::from_str(r#""maybe""#);
        assert!(r.is_err());
    }

    #[test]
    fn load_returns_empty_state_when_the_file_is_missing() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = JsonFileHostAccessDecisionStore::new(dir.path().join("never-created.json"));
        assert!(store.load().unwrap().is_empty());
    }

    #[test]
    fn load_returns_invalid_data_for_malformed_json() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("decisions.json");
        fs::write(&path, "{ not json").unwrap();
        let store = JsonFileHostAccessDecisionStore::new(path);
        assert_eq!(store.load().unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn load_propagates_non_not_found_io_errors() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = JsonFileHostAccessDecisionStore::new(dir.path().to_path_buf());
        assert_ne!(store.load().unwrap_err().kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn save_then_load_round_trips_and_overwrites() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = JsonFileHostAccessDecisionStore::new(dir.path().join("decisions.json"));
        store.save(&state()).unwrap();
        assert_eq!(store.load().unwrap(), state());
        store.save(&HostAccessDecisionFile::new()).unwrap();
        assert!(store.load().unwrap().is_empty());
    }

    #[test]
    #[serial_test::serial(env)]
    fn default_path_uses_the_override_then_home_then_the_working_directory() {
        use crate::test_env::EnvVarGuard;
        {
            let _g1 = EnvVarGuard::set("LNS_HOST_ACCESS_DECISIONS_PATH", "/tmp/custom.json");
            let _g2 = EnvVarGuard::set("HOME", "/tmp/ignored");
            assert_eq!(
                default_host_access_decisions_path(),
                PathBuf::from("/tmp/custom.json")
            );
        }
        {
            let _g1 = EnvVarGuard::unset("LNS_HOST_ACCESS_DECISIONS_PATH");
            let _g2 = EnvVarGuard::set("HOME", "/home/dev");
            assert_eq!(
                default_host_access_decisions_path(),
                PathBuf::from("/home/dev/.lns-host-access-decisions.json")
            );
        }
        {
            let _g1 = EnvVarGuard::unset("LNS_HOST_ACCESS_DECISIONS_PATH");
            let _g2 = EnvVarGuard::unset("HOME");
            assert_eq!(
                default_host_access_decisions_path(),
                PathBuf::from(".lns-host-access-decisions.json")
            );
        }
    }
}
