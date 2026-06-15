//! Host-bind KEEP/DROP decisions live in `~/.lns-host-bind-decisions.json`, per-machine and never committed — a KEEP exposes a real secret to the workload, which is a per-machine risk acceptance, not a shareable rule.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SecretDisposition {
    Keep,
    Drop,
}

pub type HostBindDecisionFile = HashMap<String, SecretDisposition>;

pub trait HostBindDecisionStore: Send + Sync {
    fn load(&self) -> io::Result<HostBindDecisionFile>;
    fn save(&self, state: &HostBindDecisionFile) -> io::Result<()>;
}

/// Falls back to `./.lns-host-bind-decisions.json` when `HOME` is unset rather than panicking.
pub fn default_host_bind_decisions_path() -> PathBuf {
    if let Some(p) = std::env::var_os("LNS_HOST_BIND_DECISIONS_PATH") {
        return PathBuf::from(p);
    }
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".lns-host-bind-decisions.json"))
        .unwrap_or_else(|| PathBuf::from(".lns-host-bind-decisions.json"))
}

pub struct JsonFileHostBindDecisionStore {
    pub path: PathBuf,
}

impl JsonFileHostBindDecisionStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl HostBindDecisionStore for JsonFileHostBindDecisionStore {
    fn load(&self) -> io::Result<HostBindDecisionFile> {
        match fs::read_to_string(&self.path) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(HashMap::new()),
            Err(e) => Err(e),
        }
    }

    fn save(&self, state: &HostBindDecisionFile) -> io::Result<()> {
        let json = serde_json::to_string_pretty(state)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        crate::secure_file::write_json_secret_atomic(&self.path, json.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_state() -> HostBindDecisionFile {
        let mut m = HostBindDecisionFile::new();
        m.insert("/Users/me/proj/.env".into(), SecretDisposition::Keep);
        m.insert("/Users/me/proj/.npmrc".into(), SecretDisposition::Drop);
        m
    }

    #[test]
    fn dispositions_serialize_to_kebab_case() {
        assert_eq!(
            serde_json::to_value(SecretDisposition::Keep).unwrap(),
            json!("keep")
        );
        assert_eq!(
            serde_json::to_value(SecretDisposition::Drop).unwrap(),
            json!("drop")
        );
    }

    #[test]
    fn dispositions_round_trip_through_json() {
        for d in [SecretDisposition::Keep, SecretDisposition::Drop] {
            let s = serde_json::to_string(&d).unwrap();
            let parsed: SecretDisposition = serde_json::from_str(&s).unwrap();
            assert_eq!(d, parsed);
        }
    }

    #[test]
    fn unknown_disposition_deserializes_as_error() {
        let r: serde_json::Result<SecretDisposition> = serde_json::from_str(r#""mask""#);
        assert!(r.is_err());
    }

    #[test]
    fn load_returns_empty_state_when_file_missing() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = JsonFileHostBindDecisionStore::new(dir.path().join("never-created.json"));
        assert!(store.load().unwrap().is_empty());
    }

    #[test]
    fn load_returns_invalid_data_for_malformed_json() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("decisions.json");
        fs::write(&path, "{ not json").unwrap();
        let store = JsonFileHostBindDecisionStore::new(path);
        assert_eq!(store.load().unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn load_propagates_non_not_found_io_errors() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = JsonFileHostBindDecisionStore::new(dir.path().to_path_buf());
        assert_ne!(store.load().unwrap_err().kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn save_then_load_round_trips_full_state() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = JsonFileHostBindDecisionStore::new(dir.path().join("decisions.json"));
        let original = sample_state();
        store.save(&original).unwrap();
        assert_eq!(store.load().unwrap(), original);
    }

    #[test]
    fn save_overwrites_existing_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = JsonFileHostBindDecisionStore::new(dir.path().join("decisions.json"));
        store.save(&sample_state()).unwrap();
        let mut second = HostBindDecisionFile::new();
        second.insert("/Users/me/proj/.env".into(), SecretDisposition::Drop);
        store.save(&second).unwrap();
        assert_eq!(store.load().unwrap(), second);
    }

    #[test]
    #[serial_test::serial(env)]
    fn default_path_uses_override_when_set() {
        use crate::test_env::EnvVarGuard;
        let _g1 = EnvVarGuard::set("LNS_HOST_BIND_DECISIONS_PATH", "/tmp/custom-decisions.json");
        let _g2 = EnvVarGuard::set("HOME", "/tmp/home-should-be-ignored");
        assert_eq!(
            default_host_bind_decisions_path(),
            PathBuf::from("/tmp/custom-decisions.json")
        );
    }

    #[test]
    #[serial_test::serial(env)]
    fn default_path_falls_back_to_home_dotfile() {
        use crate::test_env::EnvVarGuard;
        let _g1 = EnvVarGuard::unset("LNS_HOST_BIND_DECISIONS_PATH");
        let _g2 = EnvVarGuard::set("HOME", "/home/dev");
        assert_eq!(
            default_host_bind_decisions_path(),
            PathBuf::from("/home/dev/.lns-host-bind-decisions.json")
        );
    }

    #[test]
    #[serial_test::serial(env)]
    fn default_path_falls_back_to_cwd_when_home_unset() {
        use crate::test_env::EnvVarGuard;
        let _g1 = EnvVarGuard::unset("LNS_HOST_BIND_DECISIONS_PATH");
        let _g2 = EnvVarGuard::unset("HOME");
        assert_eq!(
            default_host_bind_decisions_path(),
            PathBuf::from(".lns-host-bind-decisions.json")
        );
    }
}
