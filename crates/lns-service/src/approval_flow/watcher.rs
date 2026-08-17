use std::path::{Path, PathBuf};
use std::sync::Arc;

use notify::{Event, EventKind, RecursiveMode, Watcher};

use crate::approval_flow::session::ApprovalSession;

pub struct PolicyWatcher {
    _watcher: notify::RecommendedWatcher,
}

impl PolicyWatcher {
    /// Watches both files a run's effective policy is read from: the developer's decisions, and the sidecar the machine keeps its connections in — a `lns connector disconnect` touches only the second, and a live run has to see it.
    pub fn spawn(
        policy_path: PathBuf,
        grants_path: PathBuf,
        session: Arc<ApprovalSession>,
    ) -> notify::Result<Self> {
        let mut watcher = notify::recommended_watcher(event_handler(
            policy_path.clone(),
            grants_path.clone(),
            session,
        ))?;
        let mut watched: Vec<PathBuf> = Vec::new();
        for target in [&policy_path, &grants_path] {
            let parent = parent_of(target);
            if watched.contains(&parent) {
                continue;
            }
            watcher.watch(&parent, RecursiveMode::NonRecursive)?;
            watched.push(parent);
        }
        Ok(Self { _watcher: watcher })
    }
}

fn parent_of(path: &Path) -> PathBuf {
    path.parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn event_handler(
    policy_path: PathBuf,
    grants_path: PathBuf,
    session: Arc<ApprovalSession>,
) -> impl Fn(notify::Result<Event>) + Send + 'static {
    move |res| handle_event(res, &policy_path, &grants_path, session.as_ref())
}

fn handle_event(
    res: notify::Result<Event>,
    policy_path: &Path,
    grants_path: &Path,
    session: &ApprovalSession,
) {
    let Ok(event) = res else {
        return;
    };
    if !is_policy_change(&event, policy_path) && !is_policy_change(&event, grants_path) {
        return;
    }
    if let Ok(p) = crate::supervisor::adapter::reload_with_connections(policy_path, grants_path) {
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
        let target = Path::new("/tmp/lns-local-mixin.yaml");
        assert!(is_policy_change(
            &evt(EventKind::Modify(ModifyKind::Any), target),
            target
        ));
    }

    #[test]
    fn is_policy_change_accepts_create_on_target_path() {
        let target = Path::new("/tmp/lns-local-mixin.yaml");
        assert!(is_policy_change(
            &evt(EventKind::Create(CreateKind::Any), target),
            target
        ));
    }

    #[test]
    fn is_policy_change_accepts_remove_on_target_path() {
        let target = Path::new("/tmp/lns-local-mixin.yaml");
        assert!(is_policy_change(
            &evt(EventKind::Remove(RemoveKind::Any), target),
            target
        ));
    }

    #[test]
    fn is_policy_change_rejects_event_on_sibling_path() {
        let target = Path::new("/tmp/lns-local-mixin.yaml");
        let sibling = Path::new("/tmp/something-else.yaml");
        assert!(!is_policy_change(
            &evt(EventKind::Modify(ModifyKind::Any), sibling),
            target
        ));
    }

    #[test]
    fn is_policy_change_rejects_uninteresting_event_kinds() {
        let target = Path::new("/tmp/lns-local-mixin.yaml");
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
        let path = dir.path().join("lns-local-mixin.yaml");
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

        let _w = PolicyWatcher::spawn(path, dir.path().join("grants.json"), session).unwrap();
    }

    #[test]
    fn spawn_with_path_that_has_no_parent_uses_cwd() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("lns-local-mixin.yaml");
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
        let _w = PolicyWatcher::spawn(path, dir.path().join("grants.json"), session).unwrap();
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
        let path = dir.path().join("lns-local-mixin.yaml");
        let mut updated = Policy::default();
        updated.add_rule(RouteRule::allow_host("api.linear.app"));
        updated.save_atomic(&path).unwrap();

        let (session, mut rx) = make_session();
        let handler = event_handler(
            path.clone(),
            dir.path().join("grants.json"),
            session.clone(),
        );
        handler(Ok(evt(EventKind::Modify(ModifyKind::Any), &path)));

        assert_eq!(session.current_policy().network.egress.http.len(), 1);
        assert!(rx.try_recv().is_ok(), "expected a Policy hot-swap frame");
    }

    #[test]
    fn handle_event_for_matching_modify_reloads_and_applies_policy() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("lns-local-mixin.yaml");
        let mut updated = Policy::default();
        updated.add_rule(RouteRule::allow_host("api.linear.app"));
        updated.save_atomic(&path).unwrap();

        let (session, mut rx) = make_session();

        handle_event(
            Ok(evt(EventKind::Modify(ModifyKind::Any), &path)),
            &path,
            &dir.path().join("grants.json"),
            session.as_ref(),
        );

        let cur = session.current_policy();
        assert_eq!(cur.network.egress.http.len(), 1);
        assert!(rx.try_recv().is_ok(), "expected a Policy hot-swap frame");
    }

    #[test]
    fn a_change_to_the_sidecar_reaches_the_run() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("lns-local-mixin.yaml");
        Policy::default().save_atomic(&path).unwrap();
        let grants = dir.path().join("grants.json");
        let store = lns_policy::grants::JsonFileGrantStore::new(grants.clone());
        let mut file = lns_policy::grants::WorkloadGrantFile::default();
        file.connect(&lns_policy::grants::project_key(&path), "some-provider");
        lns_policy::grants::GrantStore::save(&store, &file).unwrap();

        let (session, mut rx) = make_session();
        handle_event(
            Ok(evt(EventKind::Modify(ModifyKind::Any), &grants)),
            &path,
            &grants,
            session.as_ref(),
        );

        assert_eq!(
            session.current_policy().connectors,
            ["some-provider"],
            "`lns connector disconnect` touches only the sidecar now, so a run that watched the decisions file alone would keep an armed credential the developer just revoked"
        );
        assert!(rx.try_recv().is_ok(), "expected a Policy hot-swap frame");
    }

    #[test]
    fn handle_event_for_unrelated_path_is_a_noop() {
        let dir = tempfile::TempDir::new().unwrap();
        let target = dir.path().join("lns-local-mixin.yaml");
        let sibling = dir.path().join("other.yaml");
        Policy::default().save_atomic(&target).unwrap();

        let (session, mut rx) = make_session();

        handle_event(
            Ok(evt(EventKind::Modify(ModifyKind::Any), &sibling)),
            &target,
            &dir.path().join("grants.json"),
            session.as_ref(),
        );

        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn handle_event_with_notify_error_is_a_noop() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("lns-local-mixin.yaml");
        let (session, mut rx) = make_session();

        handle_event(
            Err(notify::Error::generic("simulated")),
            &path,
            &dir.path().join("grants.json"),
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
            &dir.path().join("grants.json"),
            session.as_ref(),
        );

        assert!(rx.try_recv().is_ok());
        assert_eq!(session.current_policy(), Policy::default());
    }

    #[test]
    fn handle_event_with_malformed_policy_file_is_a_noop() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("lns-local-mixin.yaml");
        std::fs::write(&path, "this is: not\n  - a: valid\npolicy: [").unwrap();
        let (session, mut rx) = make_session();

        handle_event(
            Ok(evt(EventKind::Modify(ModifyKind::Any), &path)),
            &path,
            &dir.path().join("grants.json"),
            session.as_ref(),
        );

        assert!(rx.try_recv().is_err());
    }
}
