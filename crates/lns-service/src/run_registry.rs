use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use lns_ipc::RunStatus;
#[cfg(target_os = "macos")]
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

#[cfg(target_os = "macos")]
use crate::vm::VsockConnector;
#[cfg(target_os = "macos")]
use crate::vm::session_client::SessionInput;

static NEXT_RUN_ID: AtomicU32 = AtomicU32::new(1);

static ACTIVE: Mutex<Option<HashMap<u32, RunHandle>>> = Mutex::new(None);

pub struct RunHandle {
    pub cancel_tx: oneshot::Sender<i32>,
    pub task: JoinHandle<()>,
    #[cfg(target_os = "macos")]
    pub input_tx: Option<mpsc::Sender<SessionInput>>,
    #[cfg(target_os = "macos")]
    pub connector: Option<std::sync::Arc<VsockConnector>>,
    pub image: String,
    pub command: String,
    pub started: String,
    pub status: std::sync::Mutex<RunStatus>,
}

pub fn allocate_run_id() -> u32 {
    NEXT_RUN_ID.fetch_add(1, Ordering::Relaxed)
}

pub fn init_from_existing_runs(cache_root: &Path) {
    let next = max_run_id_in(&cache_root.join("runs")).saturating_add(1);
    NEXT_RUN_ID.store(next, Ordering::Relaxed);
}

fn max_run_id_in(runs_dir: &Path) -> u32 {
    std::fs::read_dir(runs_dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|e| e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()))
        .max()
        .unwrap_or(0)
}

pub fn register(run_id: u32, handle: RunHandle) {
    let mut g = ACTIVE.lock().expect("ACTIVE poisoned");
    g.get_or_insert_with(HashMap::new).insert(run_id, handle);
}

pub fn deregister(run_id: u32) {
    let mut g = ACTIVE.lock().expect("ACTIVE poisoned");
    if let Some(m) = g.as_mut() {
        m.remove(&run_id);
    }
}

#[cfg(target_os = "macos")]
pub fn input_sender(run_id: u32) -> Option<mpsc::Sender<SessionInput>> {
    let g = ACTIVE.lock().expect("ACTIVE poisoned");
    g.as_ref()
        .and_then(|m| m.get(&run_id))
        .and_then(|h| h.input_tx.clone())
}

#[cfg(target_os = "macos")]
pub fn connector(run_id: u32) -> Option<std::sync::Arc<VsockConnector>> {
    let g = ACTIVE.lock().expect("ACTIVE poisoned");
    g.as_ref()
        .and_then(|m| m.get(&run_id))
        .and_then(|h| h.connector.clone())
}

#[cfg(target_os = "macos")]
pub fn set_connector(run_id: u32, connector: std::sync::Arc<VsockConnector>) {
    let mut g = ACTIVE.lock().expect("ACTIVE poisoned");
    if let Some(h) = g.as_mut().and_then(|m| m.get_mut(&run_id)) {
        h.connector = Some(connector);
    }
}

#[cfg(target_os = "macos")]
pub fn set_exit_code(run_id: u32, code: i32) {
    let g = ACTIVE.lock().expect("ACTIVE poisoned");
    if let Some(h) = g.as_ref().and_then(|m| m.get(&run_id))
        && let Ok(mut s) = h.status.lock()
    {
        *s = RunStatus::Exited { code };
    }
}

pub fn snapshot() -> Vec<lns_ipc::RunSummary> {
    let g = ACTIVE.lock().expect("ACTIVE poisoned");
    snapshot_from(g.as_ref())
}

fn snapshot_from(map: Option<&HashMap<u32, RunHandle>>) -> Vec<lns_ipc::RunSummary> {
    let Some(map) = map else {
        return Vec::new();
    };
    map.iter()
        .map(|(id, h)| {
            let status = h.status.lock().map(|s| *s).unwrap_or(RunStatus::Running);
            lns_ipc::RunSummary {
                id: *id,
                image: h.image.clone(),
                command: h.command.clone(),
                status,
                started: h.started.clone(),
            }
        })
        .collect()
}

pub fn cancel(run_id: u32) -> bool {
    let removed = {
        let mut g = ACTIVE.lock().expect("ACTIVE poisoned");
        g.as_mut().and_then(|m| m.remove(&run_id))
    };
    let Some(handle) = removed else {
        return false;
    };
    let _ = handle.cancel_tx.send(130);
    handle.task.abort();
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_handle() -> (RunHandle, oneshot::Receiver<i32>) {
        let (cancel_tx, cancel_rx) = oneshot::channel::<i32>();
        let task = tokio::spawn(std::future::pending::<()>());
        (
            RunHandle {
                cancel_tx,
                task,
                #[cfg(target_os = "macos")]
                input_tx: None,
                #[cfg(target_os = "macos")]
                connector: None,
                image: String::new(),
                command: String::new(),
                started: String::new(),
                status: std::sync::Mutex::new(RunStatus::Running),
            },
            cancel_rx,
        )
    }

    #[tokio::test]
    async fn allocate_run_id_returns_distinct_values() {
        let a = allocate_run_id();
        let b = allocate_run_id();
        assert_ne!(a, b);
    }

    #[tokio::test]
    async fn cancel_unknown_run_returns_false() {
        let id = allocate_run_id() + 1_000_000;
        assert!(!cancel(id));
    }

    #[tokio::test]
    async fn cancel_fires_oneshot_with_130() {
        let id = allocate_run_id();
        let (handle, cancel_rx) = make_handle();
        register(id, handle);

        assert!(cancel(id));

        assert_eq!(cancel_rx.await.expect("cancel oneshot delivered"), 130);
    }

    #[tokio::test]
    async fn cancel_twice_only_fires_once() {
        let id = allocate_run_id();
        let (handle, _rx) = make_handle();
        register(id, handle);

        assert!(cancel(id));
        assert!(!cancel(id), "second cancel of the same id must be a no-op");
    }

    #[tokio::test]
    async fn deregister_clears_registry() {
        let id = allocate_run_id();
        let (handle, _rx) = make_handle();
        register(id, handle);
        deregister(id);
        assert!(!cancel(id));
    }

    #[test]
    fn max_run_id_in_missing_dir_returns_zero() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(max_run_id_in(&d.path().join("does-not-exist")), 0);
    }

    #[test]
    fn max_run_id_in_empty_dir_returns_zero() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(max_run_id_in(d.path()), 0);
    }

    #[test]
    fn max_run_id_in_picks_highest_numeric_skipping_garbage() {
        let d = tempfile::tempdir().unwrap();
        for name in ["1", "2", "17", "abc", "3-foo", ".hidden"] {
            std::fs::create_dir(d.path().join(name)).unwrap();
        }
        std::fs::write(d.path().join("99"), b"").unwrap();
        assert_eq!(max_run_id_in(d.path()), 99);
    }

    #[test]
    #[serial_test::serial(next_run_id)]
    fn init_from_existing_runs_advances_next_id_past_highest_existing() {
        let d = tempfile::tempdir().unwrap();
        let runs = d.path().join("runs");
        std::fs::create_dir(&runs).unwrap();
        for name in ["3", "7", "42"] {
            std::fs::create_dir(runs.join(name)).unwrap();
        }
        init_from_existing_runs(d.path());
        let next = allocate_run_id();
        assert_eq!(next, 43, "expected id 43 (max existing + 1), got {next}");
    }

    #[test]
    #[serial_test::serial(next_run_id)]
    fn init_from_existing_runs_handles_missing_runs_dir() {
        let d = tempfile::tempdir().unwrap();
        init_from_existing_runs(d.path());
        let next = allocate_run_id();
        assert_eq!(next, 1, "expected id 1 (no prior runs), got {next}");
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn input_sender_returns_none_for_unknown_id() {
        let id = allocate_run_id() + 2_000_000;
        assert!(input_sender(id).is_none());
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn input_sender_returns_clone_when_handle_has_one() {
        let id = allocate_run_id();
        let (mut handle, _rx) = make_handle();
        let (tx, _input_rx) = mpsc::channel::<SessionInput>(1);
        handle.input_tx = Some(tx);
        register(id, handle);

        let cloned = input_sender(id);
        assert!(cloned.is_some());

        deregister(id);
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn connector_returns_none_for_unknown_id() {
        let id = allocate_run_id() + 2_000_001;
        assert!(connector(id).is_none());
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn connector_returns_none_when_handle_has_no_connector() {
        let id = allocate_run_id();
        let (handle, _rx) = make_handle();
        register(id, handle);

        assert!(connector(id).is_none());

        deregister(id);
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn set_connector_populates_handle_field() {
        let id = allocate_run_id();
        let (handle, _rx) = make_handle();
        register(id, handle);

        let conn = std::sync::Arc::new(crate::vm::VsockConnector::new_for_testing());
        set_connector(id, conn);

        assert!(connector(id).is_some());

        deregister(id);
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn set_connector_is_noop_when_run_not_registered() {
        let id = allocate_run_id() + 2_000_002;
        let conn = std::sync::Arc::new(crate::vm::VsockConnector::new_for_testing());
        set_connector(id, conn);

        assert!(connector(id).is_none());
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn set_exit_code_flips_status_to_exited() {
        let id = allocate_run_id();
        let (handle, _rx) = make_handle();
        register(id, handle);

        set_exit_code(id, 42);

        let summary = snapshot().into_iter().find(|s| s.id == id).unwrap();
        assert_eq!(summary.status, RunStatus::Exited { code: 42 });

        deregister(id);
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn set_exit_code_is_noop_when_run_not_registered() {
        let id = allocate_run_id() + 2_000_003;
        set_exit_code(id, 17);
        assert!(snapshot().iter().all(|s| s.id != id));
    }

    #[test]
    fn snapshot_from_none_returns_empty_vec() {
        assert!(snapshot_from(None).is_empty());
    }

    #[tokio::test]
    async fn snapshot_lists_registered_handles_with_running_status() {
        let id = allocate_run_id();
        let (mut handle, _rx) = make_handle();
        handle.image = "alpine:latest".to_string();
        handle.command = "sleep 1".to_string();
        handle.started = "2026-05-21 10:00:00".to_string();
        register(id, handle);

        let row = snapshot()
            .into_iter()
            .find(|s| s.id == id)
            .expect("registered handle must appear in snapshot");
        assert_eq!(row.image, "alpine:latest");
        assert_eq!(row.command, "sleep 1");
        assert_eq!(row.started, "2026-05-21 10:00:00");
        assert_eq!(row.status, RunStatus::Running);

        deregister(id);
    }
}
