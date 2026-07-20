//! Live per-run CredentialSessions registered here so an in-process store save reaches every running sandbox — the keychain backend emits no file events for the watcher to relay.

use std::sync::{Arc, Mutex, Weak};

use crate::credential_flow::session::CredentialSession;
use crate::credential_flow::store::CredentialStateFile;

static SESSIONS: Mutex<Vec<Weak<CredentialSession>>> = Mutex::new(Vec::new());

pub fn register(session: &Arc<CredentialSession>) {
    let mut sessions = SESSIONS.lock().expect("live sessions mutex poisoned");
    sessions.retain(|w| w.strong_count() > 0);
    sessions.push(Arc::downgrade(session));
}

/// Snapshots the registry before applying so no lock is held while sessions emit policy frames.
pub fn broadcast(state: &CredentialStateFile) {
    let live: Vec<Arc<CredentialSession>> = {
        let mut sessions = SESSIONS.lock().expect("live sessions mutex poisoned");
        sessions.retain(|w| w.strong_count() > 0);
        sessions.iter().filter_map(Weak::upgrade).collect()
    };
    for session in live {
        session.apply_external_state(state.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credential_flow::notification::NoopCredentialNotifier;
    use crate::credential_flow::store::{CredentialEntry, JsonFileCredentialStore};
    use std::time::Duration;

    fn session_with_tempdir() -> (Arc<CredentialSession>, tempfile::TempDir) {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let store = Arc::new(JsonFileCredentialStore::new(dir.path().join("creds.json")));
        let (frame_tx, _frame_rx) = tokio::sync::mpsc::unbounded_channel();
        let session = Arc::new(CredentialSession::new(
            CredentialStateFile::new(),
            Arc::new(NoopCredentialNotifier),
            store,
            frame_tx,
            Duration::from_secs(30),
        ));
        (session, dir)
    }

    fn state_with_entry() -> CredentialStateFile {
        let mut state = CredentialStateFile::new();
        state.insert("some-provider".into(), CredentialEntry::HostDetect);
        state
    }

    #[test]
    #[serial_test::serial(credential_live)]
    fn broadcast_applies_state_to_every_registered_session() {
        let (a, _dir_a) = session_with_tempdir();
        let (b, _dir_b) = session_with_tempdir();
        register(&a);
        register(&b);
        broadcast(&state_with_entry());
        assert!(a.current_state().contains_key("some-provider"));
        assert!(b.current_state().contains_key("some-provider"));
    }

    #[test]
    #[serial_test::serial(credential_live)]
    fn broadcast_skips_sessions_whose_run_has_ended() {
        let (kept, _dir) = session_with_tempdir();
        register(&kept);
        {
            let (dropped, _dir_dropped) = session_with_tempdir();
            register(&dropped);
        }
        broadcast(&state_with_entry());
        assert!(kept.current_state().contains_key("some-provider"));
    }
}
