use std::path::{Path, PathBuf};
use std::sync::Arc;

use notify::{Event, EventKind, RecursiveMode, Watcher};

use crate::approval_flow::session::ApprovalSession;
use lns_policy::Policy;

pub struct PolicyWatcher {
    _watcher: notify::RecommendedWatcher,
}

impl PolicyWatcher {
    pub fn spawn(path: PathBuf, session: Arc<ApprovalSession>) -> notify::Result<Self> {
        let parent = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));

        let mut watcher = notify::recommended_watcher(event_handler(path.clone(), session))?;
        watcher.watch(&parent, RecursiveMode::NonRecursive)?;
        Ok(Self { _watcher: watcher })
    }
}

fn event_handler(
    target: PathBuf,
    session: Arc<ApprovalSession>,
) -> impl Fn(notify::Result<Event>) + Send + 'static {
    move |res| handle_event(res, &target, session.as_ref())
}

fn handle_event(res: notify::Result<Event>, target: &Path, session: &ApprovalSession) {
    let Ok(event) = res else {
        return;
    };
    if !is_policy_change(&event, target) {
        return;
    }
    if let Ok(p) = Policy::load_or_default(target) {
        session.apply_external_policy(p);
    }
}

fn is_policy_change(event: &Event, target: &Path) -> bool {
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
    use lns_policy::{Policy, RouteRule};
    use notify::event::{CreateKind, ModifyKind, RemoveKind};

    fn evt(kind: EventKind, path: &Path) -> Event {
        Event {
            kind,
            paths: vec![path.to_path_buf()],
            attrs: notify::event::EventAttributes::default(),
        }
    }

    #[test]
    fn is_policy_change_accepts_modify_on_target_path() {
        let target = Path::new("/tmp/lns-policy.yaml");
        assert!(is_policy_change(
            &evt(EventKind::Modify(ModifyKind::Any), target),
            target
        ));
    }

    #[test]
    fn is_policy_change_accepts_create_on_target_path() {
        let target = Path::new("/tmp/lns-policy.yaml");
        assert!(is_policy_change(
            &evt(EventKind::Create(CreateKind::Any), target),
            target
        ));
    }

    #[test]
    fn is_policy_change_accepts_remove_on_target_path() {
        let target = Path::new("/tmp/lns-policy.yaml");
        assert!(is_policy_change(
            &evt(EventKind::Remove(RemoveKind::Any), target),
            target
        ));
    }

    #[test]
    fn is_policy_change_rejects_event_on_sibling_path() {
        let target = Path::new("/tmp/lns-policy.yaml");
        let sibling = Path::new("/tmp/something-else.yaml");
        assert!(!is_policy_change(
            &evt(EventKind::Modify(ModifyKind::Any), sibling),
            target
        ));
    }

    #[test]
    fn is_policy_change_rejects_uninteresting_event_kinds() {
        let target = Path::new("/tmp/lns-policy.yaml");
        assert!(!is_policy_change(
            &evt(EventKind::Access(notify::event::AccessKind::Any), target),
            target
        ));
        assert!(!is_policy_change(&evt(EventKind::Any, target), target));
        assert!(!is_policy_change(&evt(EventKind::Other, target), target));
    }

    #[test]
    fn spawn_returns_ok_for_existing_parent_dir() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("lns-policy.yaml");
        Policy::default().save_atomic(&path).unwrap();

        let notifier = Arc::new(crate::approval_flow::session::tests::RecordingNotifier::default());
        let store = Arc::new(crate::approval_flow::session::tests::CapturingStore::default());
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let session = Arc::new(ApprovalSession::new(
            Policy::default(),
            Policy::default(),
            notifier,
            store,
            tx,
            std::time::Duration::from_secs(30),
        ));

        let _w = PolicyWatcher::spawn(path, session).unwrap();
    }

    #[test]
    fn spawn_with_path_that_has_no_parent_uses_cwd() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("lns-policy.yaml");
        Policy::default().save_atomic(&path).unwrap();

        let notifier = Arc::new(crate::approval_flow::session::tests::RecordingNotifier::default());
        let store = Arc::new(crate::approval_flow::session::tests::CapturingStore::default());
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let session = Arc::new(ApprovalSession::new(
            Policy::default(),
            Policy::default(),
            notifier,
            store,
            tx,
            std::time::Duration::from_secs(30),
        ));
        let _w = PolicyWatcher::spawn(path, session).unwrap();
    }

    fn make_session() -> (
        Arc<ApprovalSession>,
        tokio::sync::mpsc::UnboundedReceiver<crate::approval_flow::protocol::HostFrame>,
    ) {
        let notifier = Arc::new(crate::approval_flow::session::tests::RecordingNotifier::default());
        let store = Arc::new(crate::approval_flow::session::tests::CapturingStore::default());
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let session = Arc::new(ApprovalSession::new(
            Policy::default(),
            Policy::default(),
            notifier,
            store,
            tx,
            std::time::Duration::from_secs(30),
        ));
        (session, rx)
    }

    #[test]
    fn event_handler_closure_reloads_policy_on_matching_event() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("lns-policy.yaml");
        let mut updated = Policy::default();
        updated.add_rule(RouteRule::allow_host("api.linear.app"));
        updated.save_atomic(&path).unwrap();

        let (session, mut rx) = make_session();
        let handler = event_handler(path.clone(), session.clone());
        handler(Ok(evt(EventKind::Modify(ModifyKind::Any), &path)));

        assert_eq!(session.current_policy().network.egress.http.len(), 1);
        assert!(rx.try_recv().is_ok(), "expected a Policy hot-swap frame");
    }

    #[test]
    fn handle_event_for_matching_modify_reloads_and_applies_policy() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("lns-policy.yaml");
        let mut updated = Policy::default();
        updated.add_rule(RouteRule::allow_host("api.linear.app"));
        updated.save_atomic(&path).unwrap();

        let (session, mut rx) = make_session();

        handle_event(
            Ok(evt(EventKind::Modify(ModifyKind::Any), &path)),
            &path,
            session.as_ref(),
        );

        let cur = session.current_policy();
        assert_eq!(cur.network.egress.http.len(), 1);
        assert!(rx.try_recv().is_ok(), "expected a Policy hot-swap frame");
    }

    #[test]
    fn handle_event_for_unrelated_path_is_a_noop() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = dir.path().join("lns-policy.yaml");
        let sibling = dir.path().join("other.yaml");
        Policy::default().save_atomic(&target).unwrap();

        let (session, mut rx) = make_session();

        handle_event(
            Ok(evt(EventKind::Modify(ModifyKind::Any), &sibling)),
            &target,
            session.as_ref(),
        );

        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn handle_event_with_notify_error_is_a_noop() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("lns-policy.yaml");
        let (session, mut rx) = make_session();

        handle_event(
            Err(notify::Error::generic("simulated")),
            &path,
            session.as_ref(),
        );

        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn handle_event_with_unreadable_policy_file_is_a_noop() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("never-created.yaml");
        let (session, mut rx) = make_session();

        handle_event(
            Ok(evt(EventKind::Modify(ModifyKind::Any), &path)),
            &path,
            session.as_ref(),
        );

        assert!(rx.try_recv().is_ok());
        assert_eq!(session.current_policy(), Policy::default());
    }

    #[test]
    fn handle_event_with_malformed_policy_file_is_a_noop() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("lns-policy.yaml");
        std::fs::write(&path, "this is: not\n  - a: valid\npolicy: [").unwrap();
        let (session, mut rx) = make_session();

        handle_event(
            Ok(evt(EventKind::Modify(ModifyKind::Any), &path)),
            &path,
            session.as_ref(),
        );

        assert!(rx.try_recv().is_err());
    }
}
