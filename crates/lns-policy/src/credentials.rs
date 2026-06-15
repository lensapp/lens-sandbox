//! Credential rules live in `~/.lns-credentials.json`, not `lns-policy.yaml`, to keep the shareable policy file free of per-machine state.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::secret_file::atomic_write_0600;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum CredentialEntry {
    HostDetect,
    Stored {
        value: String,
    },
    Deny,
    /// A device-flow grant: the access token armed at the boundary, the refresh token to renew it, and the access token's wall-clock expiry (unix seconds).
    Oauth {
        access_token: String,
        refresh_token: String,
        expires_at: u64,
    },
}

pub type CredentialStateFile = HashMap<String, CredentialEntry>;

pub trait CredentialStore: Send + Sync {
    fn load(&self) -> io::Result<CredentialStateFile>;
    fn save(&self, state: &CredentialStateFile) -> io::Result<()>;
}

/// Falls back to `./.lns-credentials.json` when `HOME` is unset rather than panicking.
pub fn default_credentials_path() -> PathBuf {
    if let Some(p) = std::env::var_os("LNS_CREDENTIALS_PATH") {
        return PathBuf::from(p);
    }
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".lns-credentials.json"))
        .unwrap_or_else(|| PathBuf::from(".lns-credentials.json"))
}

pub struct JsonFileCredentialStore {
    pub path: PathBuf,
}

impl JsonFileCredentialStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl CredentialStore for JsonFileCredentialStore {
    fn load(&self) -> io::Result<CredentialStateFile> {
        match fs::read_to_string(&self.path) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(HashMap::new()),
            Err(e) => Err(e),
        }
    }

    fn save(&self, state: &CredentialStateFile) -> io::Result<()> {
        let json = serde_json::to_string_pretty(state)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        atomic_write_0600(&self.path, json.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sample_state() -> CredentialStateFile {
        let mut m = CredentialStateFile::new();
        m.insert("some-provider".into(), CredentialEntry::HostDetect);
        m.insert(
            "openai".into(),
            CredentialEntry::Stored {
                value: "sk-real-token".into(),
            },
        );
        m.insert("linear".into(), CredentialEntry::Deny);
        m
    }

    #[test]
    fn host_detect_serializes_to_kebab_case_kind_only() {
        let entry = CredentialEntry::HostDetect;
        let v = serde_json::to_value(&entry).unwrap();
        assert_eq!(v, json!({"kind": "host-detect"}));
    }

    #[test]
    fn stored_serializes_with_value_alongside_kind() {
        let entry = CredentialEntry::Stored {
            value: "sk-real".into(),
        };
        let v = serde_json::to_value(&entry).unwrap();
        assert_eq!(v, json!({"kind": "stored", "value": "sk-real"}));
    }

    #[test]
    fn deny_serializes_to_kebab_case_kind_only() {
        let entry = CredentialEntry::Deny;
        let v = serde_json::to_value(&entry).unwrap();
        assert_eq!(v, json!({"kind": "deny"}));
    }

    #[test]
    fn oauth_serializes_with_token_set_alongside_kind() {
        let entry = CredentialEntry::Oauth {
            access_token: "gho_access".into(),
            refresh_token: "ghr_refresh".into(),
            expires_at: 1_900_000_000,
        };
        let v = serde_json::to_value(&entry).unwrap();
        assert_eq!(
            v,
            json!({
                "kind": "oauth",
                "access_token": "gho_access",
                "refresh_token": "ghr_refresh",
                "expires_at": 1_900_000_000u64
            })
        );
    }

    #[test]
    fn each_variant_round_trips_through_json() {
        for entry in [
            CredentialEntry::HostDetect,
            CredentialEntry::Stored { value: "x".into() },
            CredentialEntry::Deny,
            CredentialEntry::Oauth {
                access_token: "a".into(),
                refresh_token: "r".into(),
                expires_at: 42,
            },
        ] {
            let s = serde_json::to_string(&entry).unwrap();
            let parsed: CredentialEntry = serde_json::from_str(&s).unwrap();
            assert_eq!(entry, parsed);
        }
    }

    #[test]
    fn unknown_kind_deserializes_as_error() {
        let raw = r#"{"kind": "host-resolve"}"#;
        let r: serde_json::Result<CredentialEntry> = serde_json::from_str(raw);
        assert!(r.is_err());
    }

    #[test]
    fn load_returns_empty_state_when_file_missing() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = JsonFileCredentialStore::new(dir.path().join("never-created.json"));
        let state = store.load().unwrap();
        assert!(state.is_empty());
    }

    #[test]
    fn load_returns_invalid_data_for_malformed_json() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("creds.json");
        fs::write(&path, "{ this is not json").unwrap();
        let store = JsonFileCredentialStore::new(path);
        let err = store.load().unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn load_propagates_non_not_found_io_errors() {
        // Pointing the store at a directory yields a non-NotFound IO error (IsADirectory/Other by platform) that must not be swallowed as empty state.
        let dir = tempfile::TempDir::new().unwrap();
        let store = JsonFileCredentialStore::new(dir.path().to_path_buf());
        let err = store.load().unwrap_err();
        assert_ne!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn save_then_load_round_trips_full_state() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = JsonFileCredentialStore::new(dir.path().join("creds.json"));
        let original = sample_state();
        store.save(&original).unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(original, loaded);
    }

    #[test]
    fn save_creates_parent_directory_when_missing() {
        let dir = tempfile::TempDir::new().unwrap();
        let nested = dir.path().join("a/b/c/creds.json");
        let store = JsonFileCredentialStore::new(nested.clone());
        store.save(&sample_state()).unwrap();
        assert!(nested.exists());
    }

    #[test]
    fn save_uses_tmp_rename_pattern_leaving_no_stale_tmp() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("creds.json");
        let store = JsonFileCredentialStore::new(path.clone());
        store.save(&sample_state()).unwrap();
        let tmp = path.with_extension("json.tmp");
        assert!(!tmp.exists(), "tmp must be renamed away, not left in place");
    }

    #[test]
    fn save_writes_file_with_mode_0600_so_real_credentials_are_not_world_readable() {
        // Must be 0600 regardless of umask (default 022 would yield 0644, readable by other local users) since a stored entry holds a plaintext credential.
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("creds.json");
        let store = JsonFileCredentialStore::new(path.clone());
        store.save(&sample_state()).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "got 0o{mode:o}, want 0o600");
    }

    #[test]
    fn save_with_pre_existing_loose_perm_tmp_writes_credentials_at_mode_0600() {
        // Before the fix: a leftover `creds.json.tmp` with default-umask 0o644 was reused by `create(true).truncate(true)` (which ignores `.mode()` on an existing file), so the rename produced a world-readable credentials file.
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("creds.json");
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, b"stale leftover from a prior crash").unwrap();
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o644)).unwrap();

        let store = JsonFileCredentialStore::new(path.clone());
        store.save(&sample_state()).unwrap();

        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "stale tmp must not carry its 0o644 perms through the rename, got 0o{mode:o}"
        );
    }

    #[test]
    fn save_propagates_non_not_found_error_when_clearing_leftover_tmp() {
        // If the tmp path is occupied by a directory (e.g. a stray `creds.json.tmp/` planted by another process), `fs::remove_file` returns a non-NotFound error and `save` must surface it rather than fall through and clobber.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("creds.json");
        let tmp = path.with_extension("json.tmp");
        fs::create_dir(&tmp).unwrap();

        let store = JsonFileCredentialStore::new(path.clone());
        let err = store.save(&sample_state()).unwrap_err();

        assert_ne!(
            err.kind(),
            io::ErrorKind::NotFound,
            "must propagate a non-NotFound error, got {err:?}"
        );
        assert!(
            !path.exists(),
            "save must not produce the target file when the tmp clear failed"
        );
    }

    #[test]
    fn save_does_not_follow_a_symlink_planted_at_the_tmp_path() {
        // Before the fix: open(tmp) followed a symlink and truncated/wrote the credential JSON into whatever the symlink pointed at (attacker_target).
        let dir = tempfile::TempDir::new().unwrap();
        let attacker_target = dir.path().join("attacker-target");
        let attacker_contents = b"victim-data-must-survive";
        fs::write(&attacker_target, attacker_contents).unwrap();
        let path = dir.path().join("creds.json");
        let tmp = path.with_extension("json.tmp");
        std::os::unix::fs::symlink(&attacker_target, &tmp).unwrap();

        let store = JsonFileCredentialStore::new(path);
        let _ = store.save(&sample_state());

        let after = fs::read(&attacker_target).unwrap();
        assert_eq!(
            after, attacker_contents,
            "a symlink at the tmp path must not redirect the credential write"
        );
    }

    #[test]
    fn save_overwrites_existing_file_atomically() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("creds.json");
        let store = JsonFileCredentialStore::new(path);
        store.save(&sample_state()).unwrap();
        let mut second = CredentialStateFile::new();
        second.insert("some-provider".into(), CredentialEntry::Deny);
        store.save(&second).unwrap();
        let loaded = store.load().unwrap();
        assert_eq!(loaded, second);
    }

    #[test]
    #[serial_test::serial(env)]
    fn default_credentials_path_uses_override_when_set() {
        use crate::test_env::EnvVarGuard;
        let _g1 = EnvVarGuard::set("LNS_CREDENTIALS_PATH", "/tmp/custom-creds.json");
        let _g2 = EnvVarGuard::set("HOME", "/tmp/home-should-be-ignored");
        assert_eq!(
            default_credentials_path(),
            PathBuf::from("/tmp/custom-creds.json")
        );
    }

    #[test]
    #[serial_test::serial(env)]
    fn default_credentials_path_falls_back_to_home_dotfile() {
        use crate::test_env::EnvVarGuard;
        let _g1 = EnvVarGuard::unset("LNS_CREDENTIALS_PATH");
        let _g2 = EnvVarGuard::set("HOME", "/home/dev");
        assert_eq!(
            default_credentials_path(),
            PathBuf::from("/home/dev/.lns-credentials.json")
        );
    }

    #[test]
    #[serial_test::serial(env)]
    fn default_credentials_path_falls_back_to_cwd_when_home_unset() {
        use crate::test_env::EnvVarGuard;
        let _g1 = EnvVarGuard::unset("LNS_CREDENTIALS_PATH");
        let _g2 = EnvVarGuard::unset("HOME");
        assert_eq!(
            default_credentials_path(),
            PathBuf::from(".lns-credentials.json")
        );
    }

    #[test]
    fn save_with_bare_filename_path_uses_cwd_without_panic() {
        // Uses a tempdir rather than `set_current_dir`, which would race with sibling tests.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("creds.json");
        let store = JsonFileCredentialStore::new(path.clone());
        store.save(&sample_state()).unwrap();
        store.save(&sample_state()).unwrap();
        assert!(path.exists());
    }
}
