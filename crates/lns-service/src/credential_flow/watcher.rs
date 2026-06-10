//! Edits to the credentials file hot-swap running state (S10), and deletion doubles as revocation, re-arming the prompt on next use.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use notify::{Event, EventKind, RecursiveMode, Watcher};

use crate::credential_flow::session::CredentialSession;
use crate::credential_flow::store::{CredentialStore, JsonFileCredentialStore};

/// Dropping this unregisters the OS subscription, so callers must keep it alive for the watch duration.
pub struct CredentialWatcher {
    _watcher: notify::RecommendedWatcher,
}

impl CredentialWatcher {
    /// Watches the parent directory, not the file, so the write-tmp-rename save pattern still surfaces as events on the credentials path.
    pub fn spawn(path: PathBuf, session: Arc<CredentialSession>) -> notify::Result<Self> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));

        let mut watcher = notify::recommended_watcher(event_handler(path, session))?;
        watcher.watch(&parent, RecursiveMode::NonRecursive)?;
        Ok(Self { _watcher: watcher })
    }
}

fn event_handler(
    target: PathBuf,
    session: Arc<CredentialSession>,
) -> impl FnMut(notify::Result<Event>) + Send + 'static {
    move |res| handle_event(res, &target, session.as_ref())
}

/// Extracted from the watcher closure so synthetic events can pin its behaviour, since real fs-event timing is unreliable in tests.
fn handle_event(res: notify::Result<Event>, target: &Path, session: &CredentialSession) {
    let Ok(event) = res else {
        return;
    };
    if !is_credentials_change(&event, target) {
        return;
    }
    let store = JsonFileCredentialStore::new(target.to_path_buf());
    if let Ok(state) = store.load() {
        session.apply_external_state(state);
    }
}

fn is_credentials_change(event: &Event, target: &Path) -> bool {
    if !matches!(
        event.kind,
        EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)
    ) {
        return false;
    }
    event.paths.iter().any(|p| p == target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credential_flow::session::{
        CredentialNotifier, CredentialPendingPrompt, SignInPrompt,
    };
    use crate::credential_flow::store::{CredentialEntry, CredentialStateFile};
    use notify::event::{CreateKind, ModifyKind, RemoveKind};
    use std::time::Duration;
    use tokio::sync::mpsc;

    fn evt(kind: EventKind, path: &Path) -> Event {
        Event {
            kind,
            paths: vec![path.to_path_buf()],
            attrs: notify::event::EventAttributes::default(),
        }
    }

    #[test]
    fn is_credentials_change_accepts_modify_on_target_path() {
        let target = Path::new("/tmp/lns-credentials.json");
        assert!(is_credentials_change(
            &evt(EventKind::Modify(ModifyKind::Any), target),
            target
        ));
    }

    #[test]
    fn is_credentials_change_accepts_create_on_target_path() {
        let target = Path::new("/tmp/lns-credentials.json");
        assert!(is_credentials_change(
            &evt(EventKind::Create(CreateKind::Any), target),
            target
        ));
    }

    #[test]
    fn is_credentials_change_accepts_remove_on_target_path() {
        let target = Path::new("/tmp/lns-credentials.json");
        assert!(is_credentials_change(
            &evt(EventKind::Remove(RemoveKind::Any), target),
            target
        ));
    }

    #[test]
    fn is_credentials_change_rejects_event_on_sibling_path() {
        let target = Path::new("/tmp/lns-credentials.json");
        let sibling = Path::new("/tmp/other.json");
        assert!(!is_credentials_change(
            &evt(EventKind::Modify(ModifyKind::Any), sibling),
            target
        ));
    }

    #[test]
    fn is_credentials_change_rejects_uninteresting_event_kinds() {
        let target = Path::new("/tmp/lns-credentials.json");
        assert!(!is_credentials_change(
            &evt(EventKind::Access(notify::event::AccessKind::Any), target),
            target
        ));
        assert!(!is_credentials_change(&evt(EventKind::Any, target), target));
        assert!(!is_credentials_change(
            &evt(EventKind::Other, target),
            target
        ));
    }

    #[derive(Default)]
    struct NoopNotifier;
    impl CredentialNotifier for NoopNotifier {
        fn present(&self, _: &CredentialPendingPrompt) {}
        fn dismiss(&self, _: &str) {}
        fn inform(&self, _: &str) {}
        fn clear_informs(&self) {}
    }

    #[test]
    fn noop_notifier_methods_are_safe_to_call() {
        // The apply_external_state path never touches the notifier, so the helper's trait surface is pinned directly.
        let n = NoopNotifier;
        n.present(&CredentialPendingPrompt {
            id: "x".into(),
            credential_id: "some-provider".into(),
            action: "use".into(),
            oauth_display_name: None,
            token_fallback: None,
        });
        let (cancel_tx, _cancel_rx) = tokio::sync::oneshot::channel();
        n.present_sign_in(
            &SignInPrompt {
                credential_id: "some-provider".into(),
                display_name: "GitHub".into(),
                user_code: "WXYZ-1234".into(),
                verification_uri: "https://example.com/device".into(),
                token_fallback: None,
            },
            cancel_tx,
        );
        n.dismiss_sign_in("some-provider");
        n.dismiss("x");
        n.inform("anything");
        n.clear_informs();
    }

    fn make_session() -> Arc<CredentialSession> {
        let notifier = Arc::new(NoopNotifier);
        let dir = tempfile::TempDir::new().expect("tempdir");
        let store = Arc::new(JsonFileCredentialStore::new(dir.path().join("creds.json")));
        // Leak the tempdir so it outlives the session for the test's duration.
        Box::leak(Box::new(dir));
        let (tx, _rx) = mpsc::unbounded_channel();
        Arc::new(CredentialSession::new(
            CredentialStateFile::new(),
            notifier,
            store,
            tx,
            Duration::from_secs(30),
        ))
    }

    #[test]
    fn spawn_returns_ok_for_existing_parent_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("creds.json");
        JsonFileCredentialStore::new(path.clone())
            .save(&CredentialStateFile::new())
            .unwrap();
        let _w = CredentialWatcher::spawn(path, make_session()).unwrap();
    }

    #[test]
    fn spawn_with_path_that_has_no_parent_uses_cwd() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("creds.json");
        JsonFileCredentialStore::new(path.clone())
            .save(&CredentialStateFile::new())
            .unwrap();
        let _w = CredentialWatcher::spawn(path, make_session()).unwrap();
    }

    #[test]
    fn handle_event_for_matching_modify_reloads_and_applies_state() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("creds.json");
        let mut updated = CredentialStateFile::new();
        updated.insert("some-provider".into(), CredentialEntry::HostDetect);
        JsonFileCredentialStore::new(path.clone())
            .save(&updated)
            .unwrap();

        let session = make_session();
        handle_event(
            Ok(evt(EventKind::Modify(ModifyKind::Any), &path)),
            &path,
            session.as_ref(),
        );

        assert_eq!(
            session.current_state().get("some-provider"),
            Some(&CredentialEntry::HostDetect)
        );
    }

    #[test]
    fn event_handler_forwards_events_to_handle_event() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("creds.json");
        let mut updated = CredentialStateFile::new();
        updated.insert("some-provider".into(), CredentialEntry::HostDetect);
        JsonFileCredentialStore::new(path.clone())
            .save(&updated)
            .unwrap();

        let session = make_session();
        let mut handler = event_handler(path.clone(), session.clone());
        handler(Ok(evt(EventKind::Modify(ModifyKind::Any), &path)));

        assert_eq!(
            session.current_state().get("some-provider"),
            Some(&CredentialEntry::HostDetect)
        );
    }

    #[test]
    fn handle_event_for_unrelated_path_is_a_noop() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = dir.path().join("creds.json");
        let sibling = dir.path().join("other.json");
        let mut state = CredentialStateFile::new();
        state.insert("some-provider".into(), CredentialEntry::HostDetect);
        JsonFileCredentialStore::new(target.clone())
            .save(&state)
            .unwrap();

        let session = make_session();
        handle_event(
            Ok(evt(EventKind::Modify(ModifyKind::Any), &sibling)),
            &target,
            session.as_ref(),
        );

        assert!(session.current_state().is_empty());
    }

    #[test]
    fn handle_event_with_notify_error_is_a_noop() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("creds.json");
        let session = make_session();
        handle_event(
            Err(notify::Error::generic("simulated")),
            &path,
            session.as_ref(),
        );
        assert!(session.current_state().is_empty());
    }

    #[test]
    fn handle_event_with_unreadable_credentials_file_reverts_to_empty_state() {
        // A missing file loads as empty state, which is the revocation path: deleting the file clears every rule.
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("never-created.json");
        let session = make_session();
        let mut prior = CredentialStateFile::new();
        prior.insert("some-provider".into(), CredentialEntry::HostDetect);
        session.apply_external_state(prior);

        handle_event(
            Ok(evt(EventKind::Remove(RemoveKind::Any), &path)),
            &path,
            session.as_ref(),
        );

        assert!(
            session.current_state().is_empty(),
            "deleting the credentials file must revoke every rule"
        );
    }

    #[test]
    fn handle_event_with_malformed_credentials_file_is_a_noop() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("creds.json");
        std::fs::write(&path, "{ this is not json").unwrap();
        let session = make_session();
        // Malformed input must not clobber prior rules: silent revocation on a typo would be terrible UX.
        let mut prior = CredentialStateFile::new();
        prior.insert("some-provider".into(), CredentialEntry::HostDetect);
        session.apply_external_state(prior.clone());

        handle_event(
            Ok(evt(EventKind::Modify(ModifyKind::Any), &path)),
            &path,
            session.as_ref(),
        );

        assert_eq!(session.current_state(), prior);
    }
}
