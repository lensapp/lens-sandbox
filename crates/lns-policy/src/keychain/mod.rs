//! The OS-native keychain holds the whole credential map as one item so secrets never rest on disk; selection falls back to the JSON file when no keychain is reachable.

pub mod real;

use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use crate::credentials::{
    CredentialStateFile, CredentialStore, JsonFileCredentialStore, default_credentials_path,
};

pub trait KeychainBlob: Send + Sync {
    fn read(&self) -> io::Result<Option<String>>;
    fn write(&self, blob: &str) -> io::Result<()>;
}

pub struct KeychainCredentialStore {
    blob: Arc<dyn KeychainBlob>,
}

impl KeychainCredentialStore {
    pub fn new(blob: Arc<dyn KeychainBlob>) -> Self {
        Self { blob }
    }
}

impl CredentialStore for KeychainCredentialStore {
    fn load(&self) -> io::Result<CredentialStateFile> {
        match self.blob.read()? {
            Some(text) => serde_json::from_str(&text)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
            None => Ok(CredentialStateFile::new()),
        }
    }

    fn save(&self, state: &CredentialStateFile) -> io::Result<()> {
        let json = serde_json::to_string(state)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        self.blob.write(&json)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    Keychain,
    File,
}

pub struct StoreSelection {
    pub store: Arc<dyn CredentialStore>,
    pub kind: BackendKind,
    pub file_path: Option<PathBuf>,
    pub fallback_reason: Option<String>,
}

pub fn select_credential_store<F>(blob_source: F) -> StoreSelection
where
    F: FnOnce() -> io::Result<Arc<dyn KeychainBlob>>,
{
    if let Some(p) = std::env::var_os("LNS_CREDENTIALS_PATH") {
        return file_selection(PathBuf::from(p), None);
    }
    let probed = blob_source().and_then(|blob| blob.read().map(|_| blob));
    match probed {
        Ok(blob) => StoreSelection {
            store: Arc::new(KeychainCredentialStore::new(blob)),
            kind: BackendKind::Keychain,
            file_path: None,
            fallback_reason: None,
        },
        Err(e) => file_selection(default_credentials_path(), Some(e.to_string())),
    }
}

fn file_selection(path: PathBuf, fallback_reason: Option<String>) -> StoreSelection {
    StoreSelection {
        store: Arc::new(JsonFileCredentialStore::new(path.clone())),
        kind: BackendKind::File,
        file_path: Some(path),
        fallback_reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::CredentialEntry;
    use std::sync::Mutex;

    struct FakeBlob {
        data: Mutex<Option<String>>,
        fail_read: bool,
        fail_write: bool,
    }

    impl FakeBlob {
        fn empty() -> Self {
            Self::holding(None)
        }

        fn holding(data: Option<&str>) -> Self {
            Self {
                data: Mutex::new(data.map(str::to_string)),
                fail_read: false,
                fail_write: false,
            }
        }
    }

    impl KeychainBlob for FakeBlob {
        fn read(&self) -> io::Result<Option<String>> {
            if self.fail_read {
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionRefused,
                    "no secret service",
                ));
            }
            Ok(self.data.lock().unwrap().clone())
        }

        fn write(&self, blob: &str) -> io::Result<()> {
            if self.fail_write {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, "locked"));
            }
            *self.data.lock().unwrap() = Some(blob.to_string());
            Ok(())
        }
    }

    fn sample_state() -> CredentialStateFile {
        let mut m = CredentialStateFile::new();
        m.insert("some-provider".into(), CredentialEntry::HostDetect);
        m.insert(
            "some-other".into(),
            CredentialEntry::Stored {
                value: "SOME_TOKEN".into(),
            },
        );
        m.insert("some-denied".into(), CredentialEntry::Deny);
        m.insert(
            "some-oauth".into(),
            CredentialEntry::Oauth {
                access_token: "some-access".into(),
                refresh_token: "some-refresh".into(),
                expires_at: 1_900_000_000,
                scopes: vec!["repo".into()],
                account: Some("@some-account".into()),
            },
        );
        m
    }

    #[test]
    fn every_entry_kind_round_trips_through_the_keychain_blob() {
        let blob = Arc::new(FakeBlob::empty());
        let store = KeychainCredentialStore::new(blob);
        let original = sample_state();
        store.save(&original).unwrap();
        assert_eq!(store.load().unwrap(), original);
    }

    #[test]
    fn missing_blob_loads_as_empty_state() {
        let store = KeychainCredentialStore::new(Arc::new(FakeBlob::empty()));
        assert!(store.load().unwrap().is_empty());
    }

    #[test]
    fn corrupt_blob_surfaces_invalid_data_not_empty_state() {
        let store = KeychainCredentialStore::new(Arc::new(FakeBlob::holding(Some("{ not json"))));
        let err = store.load().unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn read_failure_propagates_from_load() {
        let blob = FakeBlob {
            fail_read: true,
            ..FakeBlob::empty()
        };
        let store = KeychainCredentialStore::new(Arc::new(blob));
        let err = store.load().unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::ConnectionRefused);
    }

    #[test]
    fn write_failure_propagates_from_save() {
        let blob = FakeBlob {
            fail_write: true,
            ..FakeBlob::empty()
        };
        let store = KeychainCredentialStore::new(Arc::new(blob));
        let err = store.save(&sample_state()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    #[serial_test::serial(env)]
    fn credentials_path_override_forces_the_file_backend_without_probing() {
        use crate::test_env::EnvVarGuard;
        let _g = EnvVarGuard::set("LNS_CREDENTIALS_PATH", "/tmp/forced-creds.json");
        let selection = select_credential_store(|| -> io::Result<Arc<dyn KeychainBlob>> {
            panic!("the keychain must never be probed under LNS_CREDENTIALS_PATH")
        });
        assert_eq!(selection.kind, BackendKind::File);
        assert_eq!(
            selection.file_path.as_deref(),
            Some(std::path::Path::new("/tmp/forced-creds.json"))
        );
        assert_eq!(selection.fallback_reason, None);
    }

    #[test]
    #[serial_test::serial(env)]
    fn reachable_keychain_is_selected_with_no_file_path() {
        use crate::test_env::EnvVarGuard;
        let _g = EnvVarGuard::unset("LNS_CREDENTIALS_PATH");
        let selection =
            select_credential_store(|| Ok(Arc::new(FakeBlob::empty()) as Arc<dyn KeychainBlob>));
        assert_eq!(selection.kind, BackendKind::Keychain);
        assert_eq!(selection.file_path, None);
        assert_eq!(selection.fallback_reason, None);
    }

    #[test]
    #[serial_test::serial(env)]
    fn selected_keychain_store_round_trips_state() {
        use crate::test_env::EnvVarGuard;
        let _g = EnvVarGuard::unset("LNS_CREDENTIALS_PATH");
        let selection =
            select_credential_store(|| Ok(Arc::new(FakeBlob::empty()) as Arc<dyn KeychainBlob>));
        let original = sample_state();
        selection.store.save(&original).unwrap();
        assert_eq!(selection.store.load().unwrap(), original);
    }

    #[test]
    #[serial_test::serial(env)]
    fn unconstructible_keychain_falls_back_to_the_default_file_with_a_reason() {
        use crate::test_env::EnvVarGuard;
        let _g1 = EnvVarGuard::unset("LNS_CREDENTIALS_PATH");
        let _g2 = EnvVarGuard::set("HOME", "/home/dev");
        let selection = select_credential_store(|| {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "platform store unavailable",
            ))
        });
        assert_eq!(selection.kind, BackendKind::File);
        assert_eq!(
            selection.file_path.as_deref(),
            Some(std::path::Path::new("/home/dev/.lns-credentials.json"))
        );
        let reason = selection.fallback_reason.unwrap();
        assert!(reason.contains("platform store unavailable"), "{reason}");
    }

    #[test]
    #[serial_test::serial(env)]
    fn unreadable_keychain_falls_back_to_the_default_file_with_a_reason() {
        use crate::test_env::EnvVarGuard;
        let _g1 = EnvVarGuard::unset("LNS_CREDENTIALS_PATH");
        let _g2 = EnvVarGuard::set("HOME", "/home/dev");
        let blob = FakeBlob {
            fail_read: true,
            ..FakeBlob::empty()
        };
        let selection =
            select_credential_store(move || Ok(Arc::new(blob) as Arc<dyn KeychainBlob>));
        assert_eq!(selection.kind, BackendKind::File);
        let reason = selection.fallback_reason.unwrap();
        assert!(reason.contains("no secret service"), "{reason}");
    }
}
