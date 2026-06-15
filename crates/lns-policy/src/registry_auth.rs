//! Registry login credentials live in `~/.lns-registry-auth.json` (0600), separate from workload credentials: these are host-side registry tokens that must never reach a sandboxed workload.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::secret_file::atomic_write_0600;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryCredential {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    pub token: String,
}

pub type RegistryAuthFile = HashMap<String, RegistryCredential>;

pub trait RegistryCredentialStore: Send + Sync {
    fn load(&self) -> io::Result<RegistryAuthFile>;
    fn save(&self, state: &RegistryAuthFile) -> io::Result<()>;
}

/// Falls back to `./.lns-registry-auth.json` when `HOME` is unset rather than panicking.
pub fn default_registry_auth_path() -> PathBuf {
    if let Some(p) = std::env::var_os("LNS_REGISTRY_AUTH_PATH") {
        return PathBuf::from(p);
    }
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".lns-registry-auth.json"))
        .unwrap_or_else(|| PathBuf::from(".lns-registry-auth.json"))
}

pub struct JsonFileRegistryCredentialStore {
    pub path: PathBuf,
}

impl JsonFileRegistryCredentialStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl RegistryCredentialStore for JsonFileRegistryCredentialStore {
    fn load(&self) -> io::Result<RegistryAuthFile> {
        match fs::read_to_string(&self.path) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(HashMap::new()),
            Err(e) => Err(e),
        }
    }

    fn save(&self, state: &RegistryAuthFile) -> io::Result<()> {
        let json = serde_json::to_string_pretty(state)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        atomic_write_0600(&self.path, json.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_state() -> RegistryAuthFile {
        let mut m = RegistryAuthFile::new();
        m.insert(
            "registry.some-registry.example".into(),
            RegistryCredential {
                username: Some("any".into()),
                token: "lns_some_token".into(),
            },
        );
        m
    }

    #[test]
    fn credential_serializes_username_alongside_token() {
        let entry = RegistryCredential {
            username: Some("any".into()),
            token: "lns_tok".into(),
        };
        assert_eq!(
            serde_json::to_value(&entry).unwrap(),
            json!({"username": "any", "token": "lns_tok"})
        );
    }

    #[test]
    fn credential_omits_username_when_absent() {
        let entry = RegistryCredential {
            username: None,
            token: "lns_tok".into(),
        };
        assert_eq!(
            serde_json::to_value(&entry).unwrap(),
            json!({"token": "lns_tok"})
        );
    }

    #[test]
    fn credential_round_trips_through_json() {
        for username in [Some("any".to_string()), None] {
            let entry = RegistryCredential {
                username,
                token: "lns_tok".into(),
            };
            let s = serde_json::to_string(&entry).unwrap();
            let parsed: RegistryCredential = serde_json::from_str(&s).unwrap();
            assert_eq!(entry, parsed);
        }
    }

    #[test]
    fn load_returns_empty_state_when_file_missing() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = JsonFileRegistryCredentialStore::new(dir.path().join("absent.json"));
        assert!(store.load().unwrap().is_empty());
    }

    #[test]
    fn load_returns_invalid_data_for_malformed_json() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("auth.json");
        fs::write(&path, "{ not json").unwrap();
        let store = JsonFileRegistryCredentialStore::new(path);
        assert_eq!(store.load().unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn load_propagates_non_not_found_io_errors() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = JsonFileRegistryCredentialStore::new(dir.path().to_path_buf());
        assert_ne!(store.load().unwrap_err().kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn save_then_load_round_trips_full_state() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = JsonFileRegistryCredentialStore::new(dir.path().join("auth.json"));
        let original = sample_state();
        store.save(&original).unwrap();
        assert_eq!(store.load().unwrap(), original);
    }

    #[test]
    fn save_writes_file_with_mode_0600_so_registry_tokens_are_not_world_readable() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("auth.json");
        let store = JsonFileRegistryCredentialStore::new(path.clone());
        store.save(&sample_state()).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "got 0o{mode:o}, want 0o600");
    }

    #[test]
    fn save_does_not_follow_a_symlink_planted_at_the_tmp_path() {
        let dir = tempfile::TempDir::new().unwrap();
        let attacker_target = dir.path().join("attacker-target");
        let attacker_contents = b"victim-data-must-survive";
        fs::write(&attacker_target, attacker_contents).unwrap();
        let path = dir.path().join("auth.json");
        let mut tmp = path.as_os_str().to_owned();
        tmp.push(".tmp");
        std::os::unix::fs::symlink(&attacker_target, PathBuf::from(tmp)).unwrap();

        let store = JsonFileRegistryCredentialStore::new(path);
        let _ = store.save(&sample_state());

        assert_eq!(
            fs::read(&attacker_target).unwrap(),
            attacker_contents,
            "a symlink at the tmp path must not redirect the credential write"
        );
    }

    #[test]
    fn save_overwrites_existing_file_atomically() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("auth.json");
        let store = JsonFileRegistryCredentialStore::new(path);
        store.save(&sample_state()).unwrap();
        let mut second = RegistryAuthFile::new();
        second.insert(
            "other.example".into(),
            RegistryCredential {
                username: None,
                token: "lns_other".into(),
            },
        );
        store.save(&second).unwrap();
        assert_eq!(store.load().unwrap(), second);
    }

    #[test]
    #[serial_test::serial(env)]
    fn default_registry_auth_path_uses_override_when_set() {
        use crate::test_env::EnvVarGuard;
        let _g1 = EnvVarGuard::set("LNS_REGISTRY_AUTH_PATH", "/tmp/custom-registry-auth.json");
        let _g2 = EnvVarGuard::set("HOME", "/tmp/home-should-be-ignored");
        assert_eq!(
            default_registry_auth_path(),
            PathBuf::from("/tmp/custom-registry-auth.json")
        );
    }

    #[test]
    #[serial_test::serial(env)]
    fn default_registry_auth_path_falls_back_to_home_dotfile() {
        use crate::test_env::EnvVarGuard;
        let _g1 = EnvVarGuard::unset("LNS_REGISTRY_AUTH_PATH");
        let _g2 = EnvVarGuard::set("HOME", "/home/dev");
        assert_eq!(
            default_registry_auth_path(),
            PathBuf::from("/home/dev/.lns-registry-auth.json")
        );
    }

    #[test]
    #[serial_test::serial(env)]
    fn default_registry_auth_path_falls_back_to_cwd_when_home_unset() {
        use crate::test_env::EnvVarGuard;
        let _g1 = EnvVarGuard::unset("LNS_REGISTRY_AUTH_PATH");
        let _g2 = EnvVarGuard::unset("HOME");
        assert_eq!(
            default_registry_auth_path(),
            PathBuf::from(".lns-registry-auth.json")
        );
    }
}
