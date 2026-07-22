//! Credential rules live in `~/.lns-credentials.json`, not `lns-policy.yaml`, to keep the shareable policy file free of per-machine state.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum CredentialEntry {
    HostDetect,
    Stored {
        value: String,
    },
    Deny,
    /// A device-flow grant: the access token armed at the boundary, the refresh token to renew it, the access token's wall-clock expiry (unix seconds), and the scopes granted and account resolved at sign-in.
    Oauth {
        access_token: String,
        refresh_token: String,
        expires_at: u64,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        scopes: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        account: Option<String>,
    },
}

pub type CredentialStateFile = HashMap<String, CredentialEntry>;

impl std::fmt::Debug for CredentialEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CredentialEntry::HostDetect => f.write_str("HostDetect"),
            CredentialEntry::Stored { .. } => write!(f, "Stored {{ value: <redacted> }}"),
            CredentialEntry::Deny => f.write_str("Deny"),
            CredentialEntry::Oauth { expires_at, .. } => f
                .debug_struct("Oauth")
                .field("access_token", &"<redacted>")
                .field("refresh_token", &"<redacted>")
                .field("expires_at", expires_at)
                .finish(),
        }
    }
}

pub trait CredentialStore: Send + Sync {
    fn load(&self) -> io::Result<CredentialStateFile>;
    fn save(&self, state: &CredentialStateFile) -> io::Result<()>;
}

/// True when the id's entry arms injection without a prompt: a non-empty stored value or oauth access token; absent, host-detect, and deny all read as unbound.
pub fn has_armed_entry(state: &CredentialStateFile, id: &str) -> bool {
    match state.get(id) {
        Some(CredentialEntry::Oauth { access_token, .. }) => !access_token.is_empty(),
        Some(CredentialEntry::Stored { value }) => !value.is_empty(),
        _ => false,
    }
}

pub const CREDENTIAL_SCHEMA_VERSION: u64 = 1;

fn invalid_data<E: Into<Box<dyn std::error::Error + Send + Sync>>>(e: E) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e)
}

/// Renders the state in the versioned envelope every backend (file and keychain) persists.
pub fn encode_state(state: &CredentialStateFile) -> io::Result<String> {
    #[derive(Serialize)]
    struct Envelope<'a> {
        version: u64,
        entries: &'a CredentialStateFile,
    }
    serde_json::to_string_pretty(&Envelope {
        version: CREDENTIAL_SCHEMA_VERSION,
        entries: state,
    })
    .map_err(invalid_data)
}

/// A bare map is the pre-versioning format and still loads; a version above the current one refuses to load rather than misread (or later clobber) a newer build's data.
pub fn decode_state(text: &str) -> io::Result<CredentialStateFile> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(invalid_data)?;
    let Some(version) = value.get("version") else {
        return serde_json::from_value(value).map_err(invalid_data);
    };
    match version.as_u64() {
        Some(v) if v <= CREDENTIAL_SCHEMA_VERSION => {
            let entries = value
                .get("entries")
                .cloned()
                .ok_or_else(|| invalid_data("credential store envelope carries no entries"))?;
            serde_json::from_value(entries).map_err(invalid_data)
        }
        Some(newer) => Err(invalid_data(format!(
            "credential store schema v{newer} is newer than this build understands (v{CREDENTIAL_SCHEMA_VERSION}); update lns or run `lns integration revoke --all` to reset"
        ))),
        None => Err(invalid_data("credential store version is not a number")),
    }
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
            Ok(text) => decode_state(&text),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(HashMap::new()),
            Err(e) => Err(e),
        }
    }

    fn save(&self, state: &CredentialStateFile) -> io::Result<()> {
        let json = encode_state(state)?;
        crate::secure_file::write_json_secret_atomic(&self.path, json.as_bytes())
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
    fn debug_redacts_secret_values_for_every_variant() {
        let stored = format!(
            "{:?}",
            CredentialEntry::Stored {
                value: "sk-secret".into()
            }
        );
        assert!(!stored.contains("sk-secret"), "{stored}");
        assert!(stored.contains("redacted"), "{stored}");

        let oauth = format!(
            "{:?}",
            CredentialEntry::Oauth {
                access_token: "gho_secret".into(),
                refresh_token: "ghr_secret".into(),
                expires_at: 123,
                scopes: vec!["repo".into()],
                account: Some("@hchen".into()),
            }
        );
        assert!(
            !oauth.contains("gho_secret") && !oauth.contains("ghr_secret"),
            "{oauth}"
        );
        assert!(
            oauth.contains("123"),
            "a non-secret expiry stays visible: {oauth}"
        );

        assert_eq!(format!("{:?}", CredentialEntry::HostDetect), "HostDetect");
        assert_eq!(format!("{:?}", CredentialEntry::Deny), "Deny");
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
            scopes: vec!["repo".into(), "read:org".into()],
            account: Some("@hchen".into()),
        };
        let v = serde_json::to_value(&entry).unwrap();
        assert_eq!(
            v,
            json!({
                "kind": "oauth",
                "access_token": "gho_access",
                "refresh_token": "ghr_refresh",
                "expires_at": 1_900_000_000u64,
                "scopes": ["repo", "read:org"],
                "account": "@hchen"
            })
        );
    }

    #[test]
    fn oauth_omits_empty_scopes_and_absent_account() {
        let entry = CredentialEntry::Oauth {
            access_token: "a".into(),
            refresh_token: "r".into(),
            expires_at: 1,
            scopes: vec![],
            account: None,
        };
        let v = serde_json::to_value(&entry).unwrap();
        assert_eq!(v.get("scopes"), None, "empty scopes are skipped");
        assert_eq!(v.get("account"), None, "an absent account is skipped");
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
                scopes: vec!["repo".into()],
                account: Some("@hchen".into()),
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
    fn save_persists_the_versioned_envelope() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("creds.json");
        let store = JsonFileCredentialStore::new(path.clone());
        store.save(&sample_state()).unwrap();
        let raw: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(raw["version"], CREDENTIAL_SCHEMA_VERSION);
        assert_eq!(
            raw["entries"]["some-provider"],
            json!({"kind": "host-detect"})
        );
    }

    #[test]
    fn legacy_bare_map_still_loads_and_upgrades_on_the_next_save() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("creds.json");
        fs::write(&path, r#"{"some-provider": {"kind": "host-detect"}}"#).unwrap();
        let store = JsonFileCredentialStore::new(path.clone());
        let state = store.load().unwrap();
        assert_eq!(
            state.get("some-provider"),
            Some(&CredentialEntry::HostDetect)
        );
        store.save(&state).unwrap();
        let raw: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(raw["version"], CREDENTIAL_SCHEMA_VERSION);
    }

    #[test]
    fn a_newer_schema_version_refuses_to_load_and_says_why() {
        let err = decode_state(r#"{"version": 2, "entries": {}}"#).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        let msg = err.to_string();
        assert!(msg.contains("v2") && msg.contains("newer"), "{msg}");
    }

    #[test]
    fn an_envelope_without_entries_is_invalid_data() {
        let err = decode_state(r#"{"version": 1}"#).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn an_envelope_with_malformed_entries_is_invalid_data() {
        let err =
            decode_state(r#"{"version": 1, "entries": {"x": {"kind": "nope"}}}"#).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn a_non_numeric_version_is_invalid_data() {
        let err = decode_state(r#"{"version": "one", "entries": {}}"#).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(err.to_string().contains("not a number"), "{err}");
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
    fn has_armed_entry_arms_only_a_nonempty_stored_value_or_oauth_token() {
        let mut state = CredentialStateFile::new();
        state.insert(
            "stored".into(),
            CredentialEntry::Stored {
                value: "some-secret".into(),
            },
        );
        state.insert(
            "oauth".into(),
            CredentialEntry::Oauth {
                access_token: "some-access".into(),
                refresh_token: "some-refresh".into(),
                expires_at: 9999,
                scopes: vec![],
                account: None,
            },
        );
        state.insert(
            "empty-stored".into(),
            CredentialEntry::Stored {
                value: String::new(),
            },
        );
        state.insert(
            "expired-grant".into(),
            CredentialEntry::Oauth {
                access_token: String::new(),
                refresh_token: String::new(),
                expires_at: 0,
                scopes: vec![],
                account: None,
            },
        );
        state.insert("host".into(), CredentialEntry::HostDetect);
        state.insert("denied".into(), CredentialEntry::Deny);

        assert!(has_armed_entry(&state, "stored"));
        assert!(has_armed_entry(&state, "oauth"));
        assert!(!has_armed_entry(&state, "empty-stored"));
        assert!(!has_armed_entry(&state, "expired-grant"));
        assert!(
            !has_armed_entry(&state, "host"),
            "host-detect binds at first use, not upfront"
        );
        assert!(!has_armed_entry(&state, "denied"));
        assert!(!has_armed_entry(&state, "absent"));
    }
}
