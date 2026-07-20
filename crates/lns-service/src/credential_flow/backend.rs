//! The credential backend chosen once at startup — OS keychain when reachable, plaintext JSON file otherwise — with saves fanned out to live sessions in-process.

use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use lns_policy::keychain::{BackendKind, StoreSelection};

use crate::credential_flow::live;
use crate::credential_flow::store::{
    CredentialStateFile, CredentialStore, JsonFileCredentialStore, default_credentials_path,
};
use crate::log;

struct ActiveBackend {
    store: Arc<dyn CredentialStore>,
    kind: BackendKind,
    file_path: Option<PathBuf>,
}

static ACTIVE: RwLock<Option<ActiveBackend>> = RwLock::new(None);

#[cfg(test)]
pub(crate) fn reset_for_tests() {
    *ACTIVE.write().expect("credential backend lock poisoned") = None;
}

struct NotifyingStore {
    inner: Arc<dyn CredentialStore>,
    // Serializes save+broadcast pairs so live sessions always converge to the last-saved state, matching the reload-from-store guarantee the file watcher gave.
    fanout: std::sync::Mutex<()>,
}

impl NotifyingStore {
    fn wrapping(inner: Arc<dyn CredentialStore>) -> Self {
        Self {
            inner,
            fanout: std::sync::Mutex::new(()),
        }
    }
}

impl CredentialStore for NotifyingStore {
    fn load(&self) -> std::io::Result<CredentialStateFile> {
        self.inner.load()
    }

    fn save(&self, state: &CredentialStateFile) -> std::io::Result<()> {
        let _ordered = self.fanout.lock().expect("fanout mutex poisoned");
        self.inner.save(state)?;
        live::broadcast(state);
        Ok(())
    }
}

pub fn install(selection: StoreSelection) {
    if let Some(reason) = &selection.fallback_reason {
        let path = selection
            .file_path
            .as_deref()
            .unwrap_or_else(|| std::path::Path::new("~/.lns-credentials.json"))
            .display()
            .to_string();
        log::warn!(
            "no OS keychain reachable ({reason}); credential values will be stored in plaintext at {path}"
        );
    }
    *ACTIVE.write().expect("credential backend lock poisoned") = Some(ActiveBackend {
        store: Arc::new(NotifyingStore::wrapping(selection.store)),
        kind: selection.kind,
        file_path: selection.file_path,
    });
}

/// Uninstalled (unit tests, direct callers) behaves as the pre-keychain default: the JSON file at the default path.
pub fn store() -> Arc<dyn CredentialStore> {
    match ACTIVE
        .read()
        .expect("credential backend lock poisoned")
        .as_ref()
    {
        Some(active) => active.store.clone(),
        None => Arc::new(NotifyingStore::wrapping(Arc::new(
            JsonFileCredentialStore::new(default_credentials_path()),
        ))),
    }
}

/// Load-modify-save through the active backend; the save fans out to live sessions, which is what makes a sign-in take effect without a restart or file event.
pub fn persist_entry(
    id: &str,
    entry: crate::credential_flow::store::CredentialEntry,
) -> std::io::Result<()> {
    let store = store();
    let mut state = store.load()?;
    state.insert(id.to_string(), entry);
    store.save(&state)
}

/// Removes one value decision and fans the change out live; saving only on removal keeps a miss free of writes and broadcasts.
pub fn revoke_entry(id: &str) -> std::io::Result<bool> {
    let store = store();
    let mut state = store.load()?;
    let existed = state.remove(id).is_some();
    if existed {
        store.save(&state)?;
    }
    Ok(existed)
}

pub fn kind() -> Option<BackendKind> {
    ACTIVE
        .read()
        .expect("credential backend lock poisoned")
        .as_ref()
        .map(|active| active.kind)
}

/// None means no file to watch — the keychain backend has no external-edit channel.
pub fn file_watch_path() -> Option<PathBuf> {
    match ACTIVE
        .read()
        .expect("credential backend lock poisoned")
        .as_ref()
    {
        Some(active) => active.file_path.clone(),
        None => Some(default_credentials_path()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lns_policy::keychain::{KeychainBlob, KeychainCredentialStore};
    use std::io;
    use std::sync::Mutex;

    struct FakeBlob {
        data: Mutex<Option<String>>,
    }

    impl KeychainBlob for FakeBlob {
        fn read(&self) -> io::Result<Option<String>> {
            Ok(self.data.lock().unwrap().clone())
        }

        fn write(&self, blob: &str) -> io::Result<()> {
            *self.data.lock().unwrap() = Some(blob.to_string());
            Ok(())
        }
    }

    fn keychain_selection() -> StoreSelection {
        StoreSelection {
            store: Arc::new(KeychainCredentialStore::new(Arc::new(FakeBlob {
                data: Mutex::new(None),
            }))),
            kind: BackendKind::Keychain,
            file_path: None,
            fallback_reason: None,
        }
    }

    fn file_selection(reason: Option<&str>) -> StoreSelection {
        let path = PathBuf::from("/tmp/backend-test-creds.json");
        StoreSelection {
            store: Arc::new(JsonFileCredentialStore::new(path.clone())),
            kind: BackendKind::File,
            file_path: Some(path),
            fallback_reason: reason.map(str::to_string),
        }
    }

    fn captured_output(f: impl FnOnce()) -> String {
        use std::sync::{Arc as StdArc, Mutex as StdMutex};
        #[derive(Clone)]
        struct Buf(StdArc<StdMutex<Vec<u8>>>);
        impl io::Write for Buf {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let buf = Buf(StdArc::new(StdMutex::new(Vec::new())));
        let writer = buf.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .with_max_level(tracing::Level::WARN)
            .finish();
        tracing::subscriber::with_default(subscriber, f);
        io::Write::flush(&mut buf.clone()).expect("Buf::flush is infallible");
        let bytes = buf.0.lock().unwrap().clone();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    #[test]
    #[serial_test::serial(credential_backend)]
    fn installing_a_keychain_selection_exposes_the_kind_and_no_watch_path() {
        install(keychain_selection());
        assert_eq!(kind(), Some(BackendKind::Keychain));
        assert_eq!(file_watch_path(), None);
    }

    #[test]
    #[serial_test::serial(credential_backend)]
    fn installing_a_file_selection_exposes_the_kind_and_its_watch_path() {
        install(file_selection(None));
        assert_eq!(kind(), Some(BackendKind::File));
        assert_eq!(
            file_watch_path(),
            Some(PathBuf::from("/tmp/backend-test-creds.json"))
        );
    }

    #[test]
    #[serial_test::serial(credential_backend)]
    fn install_warns_that_values_rest_in_plaintext_when_falling_back() {
        let output = captured_output(|| install(file_selection(Some("no secret service"))));
        assert!(output.contains("plaintext"), "{output}");
        assert!(output.contains("no secret service"), "{output}");
    }

    #[test]
    #[serial_test::serial(credential_backend)]
    fn install_stays_silent_when_the_keychain_was_selected() {
        let output = captured_output(|| install(keychain_selection()));
        assert!(output.is_empty(), "{output}");
    }

    #[test]
    #[serial_test::serial(credential_backend)]
    fn installed_store_saves_through_the_selected_backend() {
        install(keychain_selection());
        let mut state = CredentialStateFile::new();
        state.insert(
            "some-provider".into(),
            crate::credential_flow::store::CredentialEntry::HostDetect,
        );
        store().save(&state).unwrap();
        assert_eq!(store().load().unwrap(), state);
    }

    #[test]
    #[serial_test::serial(credential_backend)]
    fn persist_entry_adds_to_existing_state_through_the_active_backend() {
        use crate::credential_flow::store::CredentialEntry;
        install(keychain_selection());
        let mut prior = CredentialStateFile::new();
        prior.insert("some-provider".into(), CredentialEntry::HostDetect);
        store().save(&prior).unwrap();
        persist_entry(
            "some-oauth",
            CredentialEntry::Stored {
                value: "some-secret".into(),
            },
        )
        .unwrap();
        let state = store().load().unwrap();
        assert_eq!(
            state.get("some-provider"),
            Some(&CredentialEntry::HostDetect)
        );
        assert!(matches!(
            state.get("some-oauth"),
            Some(CredentialEntry::Stored { .. })
        ));
    }

    #[test]
    #[serial_test::serial(credential_backend)]
    fn uninstalled_backend_reports_no_kind_and_watches_the_default_path() {
        reset_for_tests();
        assert_eq!(kind(), None);
        assert_eq!(file_watch_path(), Some(default_credentials_path()));
    }

    #[test]
    #[serial_test::serial(credential_backend)]
    fn uninstalled_backend_still_hands_out_a_file_store() {
        reset_for_tests();
        let handed_out = store();
        drop(handed_out);
    }
}
