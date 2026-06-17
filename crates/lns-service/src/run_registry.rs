use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use lns_ipc::{RunStatus, validate_run_name};

use crate::run_name;
#[cfg(target_os = "macos")]
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

#[cfg(target_os = "macos")]
use crate::vm::GuestTransport;
#[cfg(target_os = "macos")]
use crate::vm::session_client::SessionInput;

static NEXT_RUN_ID: AtomicU32 = AtomicU32::new(1);

static NEXT_NAME_SEQ: AtomicUsize = AtomicUsize::new(0);

static ACTIVE: Mutex<Option<HashMap<u32, RunHandle>>> = Mutex::new(None);

pub struct RunHandle {
    pub cancel_tx: oneshot::Sender<i32>,
    pub task: JoinHandle<()>,
    #[cfg(target_os = "macos")]
    pub input_tx: Option<mpsc::Sender<SessionInput>>,
    #[cfg(target_os = "macos")]
    pub connector: Option<std::sync::Arc<dyn GuestTransport>>,
    pub name: String,
    pub image: String,
    pub command: String,
    pub started: String,
    pub status: std::sync::Mutex<RunStatus>,
    pub logs: std::sync::Arc<crate::run_log::RunLogBuffer>,
    pub config: lns_ipc::RunConfig,
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

pub fn register_named(
    run_id: u32,
    requested: Option<String>,
    handle: RunHandle,
) -> Result<String, String> {
    let mut next_name = || run_name::name_for_index(NEXT_NAME_SEQ.fetch_add(1, Ordering::Relaxed));
    let mut g = ACTIVE.lock().expect("ACTIVE poisoned");
    let map = g.get_or_insert_with(HashMap::new);
    register_named_in(map, run_id, requested, handle, &mut next_name)
}

fn register_named_in(
    map: &mut HashMap<u32, RunHandle>,
    run_id: u32,
    requested: Option<String>,
    mut handle: RunHandle,
    next_name: &mut dyn FnMut() -> String,
) -> Result<String, String> {
    let name = match requested {
        Some(requested) => {
            check_name_available(Some(&*map), &requested)?;
            requested
        }
        None => unique_auto_name(map, next_name),
    };
    handle.name = name.clone();
    map.insert(run_id, handle);
    Ok(name)
}

pub fn ensure_name_available(name: &str) -> Result<(), String> {
    let g = ACTIVE.lock().expect("ACTIVE poisoned");
    check_name_available(g.as_ref(), name)
}

fn check_name_available(map: Option<&HashMap<u32, RunHandle>>, name: &str) -> Result<(), String> {
    validate_run_name(name)?;
    match map.and_then(|m| name_holder(m, name)) {
        Some(holder) => Err(format!("name {name:?} already in use by run #{holder}")),
        None => Ok(()),
    }
}

fn unique_auto_name(
    map: &HashMap<u32, RunHandle>,
    next_name: &mut dyn FnMut() -> String,
) -> String {
    for _ in 0..run_name::pool_size() {
        let candidate = next_name();
        if name_holder(map, &candidate).is_none() {
            return candidate;
        }
    }
    let base = next_name();
    (0..=map.len() as u32)
        .map(|n| format!("{base}_{n}"))
        .find(|name| name_holder(map, name).is_none())
        .expect("more distinct suffixes than names in use")
}

fn name_holder(map: &HashMap<u32, RunHandle>, name: &str) -> Option<u32> {
    map.iter().find(|(_, h)| h.name == name).map(|(id, _)| *id)
}

pub fn resolve(handle: &str) -> Result<u32, String> {
    let g = ACTIVE.lock().expect("ACTIVE poisoned");
    resolve_in(g.as_ref(), handle)
}

fn resolve_in(map: Option<&HashMap<u32, RunHandle>>, handle: &str) -> Result<u32, String> {
    if let Ok(id) = handle.parse::<u32>() {
        return Ok(id);
    }
    map.and_then(|m| name_holder(m, handle))
        .ok_or_else(|| format!("no such run: {handle}"))
}

pub fn rename(handle: &str, new_name: &str) -> Result<(), String> {
    let mut g = ACTIVE.lock().expect("ACTIVE poisoned");
    rename_in(g.as_mut(), handle, new_name)
}

fn rename_in(
    map: Option<&mut HashMap<u32, RunHandle>>,
    handle: &str,
    new_name: &str,
) -> Result<(), String> {
    validate_run_name(new_name)?;
    let map = map.ok_or_else(|| format!("no such run: {handle}"))?;
    let target = resolve_in(Some(&*map), handle)?;
    if let Some(holder) = name_holder(map, new_name)
        && holder != target
    {
        return Err(format!("name {new_name:?} already in use by run #{holder}"));
    }
    match map.get_mut(&target) {
        Some(h) => {
            h.name = new_name.to_string();
            Ok(())
        }
        None => Err(format!("no such run: {handle}")),
    }
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

pub fn log_buffer(run_id: u32) -> Option<std::sync::Arc<crate::run_log::RunLogBuffer>> {
    let g = ACTIVE.lock().expect("ACTIVE poisoned");
    g.as_ref()
        .and_then(|m| m.get(&run_id))
        .map(|h| h.logs.clone())
}

#[cfg(target_os = "macos")]
pub fn connector(run_id: u32) -> Option<std::sync::Arc<dyn GuestTransport>> {
    let g = ACTIVE.lock().expect("ACTIVE poisoned");
    g.as_ref()
        .and_then(|m| m.get(&run_id))
        .and_then(|h| h.connector.clone())
}

#[cfg(target_os = "macos")]
pub fn set_connector(run_id: u32, connector: std::sync::Arc<dyn GuestTransport>) {
    let mut g = ACTIVE.lock().expect("ACTIVE poisoned");
    if let Some(h) = g.as_mut().and_then(|m| m.get_mut(&run_id)) {
        h.connector = Some(connector);
    }
}

pub fn set_exit_code(run_id: u32, code: i32) {
    let g = ACTIVE.lock().expect("ACTIVE poisoned");
    if let Some(h) = g.as_ref().and_then(|m| m.get(&run_id))
        && let Ok(mut s) = h.status.lock()
    {
        *s = RunStatus::Exited { code };
    }
}

pub fn mark_exited_from_log(run_id: u32) {
    let g = ACTIVE.lock().expect("ACTIVE poisoned");
    if let Some(h) = g.as_ref().and_then(|m| m.get(&run_id))
        && let Some(code) = h.logs.exit()
        && let Ok(mut s) = h.status.lock()
    {
        *s = RunStatus::Exited { code };
    }
}

pub fn status(run_id: u32) -> Option<RunStatus> {
    let g = ACTIVE.lock().expect("ACTIVE poisoned");
    g.as_ref()
        .and_then(|m| m.get(&run_id))
        .and_then(|h| h.status.lock().ok().map(|s| *s))
}

pub fn snapshot() -> Vec<lns_ipc::RunSummary> {
    let g = ACTIVE.lock().expect("ACTIVE poisoned");
    snapshot_from(g.as_ref())
}

fn snapshot_from(map: Option<&HashMap<u32, RunHandle>>) -> Vec<lns_ipc::RunSummary> {
    let Some(map) = map else {
        return Vec::new();
    };
    map.iter().map(|(id, h)| summary_of(*id, h)).collect()
}

fn summary_of(id: u32, h: &RunHandle) -> lns_ipc::RunSummary {
    let status = h.status.lock().map(|s| *s).unwrap_or(RunStatus::Running);
    lns_ipc::RunSummary {
        id,
        name: h.name.clone(),
        image: h.image.clone(),
        command: h.command.clone(),
        status,
        started: h.started.clone(),
    }
}

pub fn inspect(run_id: u32) -> Option<lns_ipc::RunDetails> {
    let g = ACTIVE.lock().expect("ACTIVE poisoned");
    g.as_ref()
        .and_then(|m| m.get(&run_id))
        .map(|h| lns_ipc::RunDetails {
            summary: summary_of(run_id, h),
            config: h.config.clone(),
        })
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoveOutcome {
    Removed,
    Running,
    NotFound,
}

pub fn remove_if_exited(run_id: u32) -> RemoveOutcome {
    let mut g = ACTIVE.lock().expect("ACTIVE poisoned");
    remove_if_exited_from(g.as_mut(), run_id)
}

fn remove_if_exited_from(map: Option<&mut HashMap<u32, RunHandle>>, run_id: u32) -> RemoveOutcome {
    let Some(map) = map else {
        return RemoveOutcome::NotFound;
    };
    match map.get(&run_id) {
        None => RemoveOutcome::NotFound,
        Some(h) if is_exited(h) => {
            map.remove(&run_id);
            RemoveOutcome::Removed
        }
        Some(_) => RemoveOutcome::Running,
    }
}

pub fn prune_exited() -> Vec<u32> {
    let mut g = ACTIVE.lock().expect("ACTIVE poisoned");
    prune_exited_from(g.as_mut())
}

fn prune_exited_from(map: Option<&mut HashMap<u32, RunHandle>>) -> Vec<u32> {
    let Some(map) = map else {
        return Vec::new();
    };
    let exited: Vec<u32> = map
        .iter()
        .filter(|(_, h)| is_exited(h))
        .map(|(id, _)| *id)
        .collect();
    for id in &exited {
        map.remove(id);
    }
    exited
}

fn is_exited(h: &RunHandle) -> bool {
    matches!(h.status.lock().map(|s| *s), Ok(RunStatus::Exited { .. }))
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
                name: String::new(),
                image: String::new(),
                command: String::new(),
                started: String::new(),
                status: std::sync::Mutex::new(RunStatus::Running),
                logs: std::sync::Arc::new(crate::run_log::RunLogBuffer::default()),
                config: lns_ipc::RunConfig::default(),
            },
            cancel_rx,
        )
    }

    fn gen_amber() -> String {
        "amber_otter".to_string()
    }

    async fn named_handle(map: &mut HashMap<u32, RunHandle>, id: u32, name: &str) {
        let (mut h, _rx) = make_handle();
        h.name = name.to_string();
        map.insert(id, h);
    }

    #[tokio::test]
    async fn register_named_in_keeps_an_explicit_valid_name() {
        let mut map = HashMap::new();
        let (h, _rx) = make_handle();
        let name = register_named_in(
            &mut map,
            1,
            Some("reviewer".into()),
            h,
            &mut (gen_amber as fn() -> String),
        )
        .unwrap();
        assert_eq!(name, "reviewer");
        assert_eq!(map.get(&1).unwrap().name, "reviewer");
    }

    #[tokio::test]
    async fn register_named_in_rejects_a_name_held_by_a_listed_run() {
        let mut map = HashMap::new();
        named_handle(&mut map, 7, "reviewer").await;
        let (h, _rx) = make_handle();
        let err = register_named_in(
            &mut map,
            8,
            Some("reviewer".into()),
            h,
            &mut (gen_amber as fn() -> String),
        )
        .unwrap_err();
        assert!(err.contains("already in use by run #7"), "got: {err}");
    }

    #[tokio::test]
    async fn register_named_in_rejects_an_invalid_explicit_name() {
        let mut map = HashMap::new();
        let (h, _rx) = make_handle();
        register_named_in(
            &mut map,
            1,
            Some("7".into()),
            h,
            &mut (gen_amber as fn() -> String),
        )
        .unwrap_err();
    }

    #[tokio::test]
    async fn register_named_in_auto_generates_when_no_name_is_requested() {
        let mut map = HashMap::new();
        let (h, _rx) = make_handle();
        let name =
            register_named_in(&mut map, 1, None, h, &mut (gen_amber as fn() -> String)).unwrap();
        assert_eq!(name, "amber_otter");
    }

    #[tokio::test]
    async fn register_named_in_regenerates_until_the_auto_name_is_unique() {
        let mut map = HashMap::new();
        named_handle(&mut map, 7, "amber_otter").await;
        let (h, _rx) = make_handle();
        let mut seq = ["amber_otter", "amber_otter", "bold_falcon"].into_iter();
        let mut next_name = || seq.next().unwrap().to_string();
        let name = register_named_in(&mut map, 8, None, h, &mut next_name).unwrap();
        assert_eq!(name, "bold_falcon");
    }

    #[tokio::test]
    async fn register_named_in_suffixes_a_name_when_the_pretty_pool_is_exhausted() {
        let mut map = HashMap::new();
        named_handle(&mut map, 7, "amber_otter").await;
        let (h, _rx) = make_handle();
        let mut next_name = || "amber_otter".to_string();
        let name = register_named_in(&mut map, 8, None, h, &mut next_name).unwrap();
        assert_eq!(name, "amber_otter_0");
    }

    #[tokio::test]
    #[serial_test::serial(global_runs)]
    async fn ensure_name_available_rejects_taken_or_invalid_names_and_accepts_free_ones() {
        let id = allocate_run_id();
        let (h, _rx) = make_handle();
        register_named(id, Some(format!("rev-{id}")), h).unwrap();
        assert!(ensure_name_available(&format!("rev-{id}")).is_err());
        assert!(ensure_name_available("7").is_err());
        assert!(ensure_name_available(&format!("free-{id}")).is_ok());
        deregister(id);
    }

    #[tokio::test]
    async fn resolve_in_passes_numeric_ids_through_and_looks_up_names() {
        let mut map = HashMap::new();
        named_handle(&mut map, 3, "reviewer").await;
        assert_eq!(resolve_in(Some(&map), "3"), Ok(3));
        assert_eq!(resolve_in(Some(&map), "reviewer"), Ok(3));
        // Numeric handles pass through unchecked; the downstream id-op is the existence gate.
        assert_eq!(resolve_in(Some(&map), "9"), Ok(9));
        assert_eq!(resolve_in(None, "5"), Ok(5));
        assert!(
            resolve_in(Some(&map), "ghost")
                .unwrap_err()
                .contains("no such run: ghost")
        );
        assert!(
            resolve_in(None, "reviewer")
                .unwrap_err()
                .contains("no such run")
        );
    }

    #[tokio::test]
    async fn rename_in_changes_the_name_in_place() {
        let mut map = HashMap::new();
        named_handle(&mut map, 3, "reviewer").await;
        rename_in(Some(&mut map), "reviewer", "auditor").unwrap();
        assert_eq!(map.get(&3).unwrap().name, "auditor");
        assert_eq!(resolve_in(Some(&map), "auditor"), Ok(3));
        assert!(
            resolve_in(Some(&map), "reviewer")
                .unwrap_err()
                .contains("no such run")
        );
    }

    #[tokio::test]
    async fn rename_in_to_a_held_name_is_refused() {
        let mut map = HashMap::new();
        named_handle(&mut map, 3, "reviewer").await;
        named_handle(&mut map, 4, "auditor").await;
        let err = rename_in(Some(&mut map), "auditor", "reviewer").unwrap_err();
        assert!(err.contains("already in use by run #3"), "got: {err}");
    }

    #[tokio::test]
    async fn rename_in_to_the_runs_own_name_is_a_noop_success() {
        let mut map = HashMap::new();
        named_handle(&mut map, 3, "reviewer").await;
        rename_in(Some(&mut map), "reviewer", "reviewer").unwrap();
        assert_eq!(map.get(&3).unwrap().name, "reviewer");
    }

    #[tokio::test]
    async fn rename_in_rejects_an_invalid_new_name() {
        let mut map = HashMap::new();
        named_handle(&mut map, 3, "reviewer").await;
        rename_in(Some(&mut map), "reviewer", "7").unwrap_err();
    }

    #[tokio::test]
    async fn rename_in_reports_no_such_run_for_unknown_name_id_or_empty_registry() {
        let mut map = HashMap::new();
        named_handle(&mut map, 3, "reviewer").await;
        let err = rename_in(Some(&mut map), "ghost", "auditor").unwrap_err();
        assert!(err.contains("no such run: ghost"), "got: {err}");
        let err = rename_in(Some(&mut map), "999", "auditor").unwrap_err();
        assert!(err.contains("no such run: 999"), "got: {err}");
        let err = rename_in(None, "ghost", "auditor").unwrap_err();
        assert!(err.contains("no such run: ghost"), "got: {err}");
    }

    #[tokio::test]
    #[serial_test::serial(global_runs)]
    async fn register_named_assigns_an_auto_name_and_resolves_by_name_and_id() {
        let id = allocate_run_id();
        let (h, _rx) = make_handle();
        let name = register_named(id, None, h).unwrap();
        assert!(!name.is_empty());
        assert_eq!(resolve(&name), Ok(id));
        assert_eq!(resolve(&id.to_string()), Ok(id));
        deregister(id);
    }

    #[tokio::test]
    #[serial_test::serial(global_runs)]
    async fn rename_via_the_global_registry_updates_the_name() {
        let id = allocate_run_id();
        let (h, _rx) = make_handle();
        register_named(id, Some(format!("rev-{id}")), h).unwrap();
        rename(&format!("rev-{id}"), &format!("aud-{id}")).unwrap();
        assert_eq!(resolve(&format!("aud-{id}")), Ok(id));
        deregister(id);
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

    #[tokio::test]
    #[serial_test::serial(global_runs)]
    async fn set_exit_code_flips_status_to_exited() {
        let id = allocate_run_id();
        let (handle, _rx) = make_handle();
        register(id, handle);

        set_exit_code(id, 42);

        let summary = snapshot().into_iter().find(|s| s.id == id).unwrap();
        assert_eq!(summary.status, RunStatus::Exited { code: 42 });

        deregister(id);
    }

    #[tokio::test]
    async fn set_exit_code_is_noop_when_run_not_registered() {
        let id = allocate_run_id() + 2_000_003;
        set_exit_code(id, 17);
        assert!(snapshot().iter().all(|s| s.id != id));
    }

    #[tokio::test]
    #[serial_test::serial(global_runs)]
    async fn mark_exited_from_log_adopts_the_log_buffers_exit_code() {
        let id = allocate_run_id();
        let (handle, _rx) = make_handle();
        handle.logs.close(7);
        register(id, handle);

        mark_exited_from_log(id);

        assert_eq!(status(id), Some(RunStatus::Exited { code: 7 }));
        deregister(id);
    }

    #[tokio::test]
    #[serial_test::serial(global_runs)]
    async fn mark_exited_from_log_leaves_a_run_running_while_its_log_is_open() {
        let id = allocate_run_id();
        let (handle, _rx) = make_handle();
        register(id, handle);

        mark_exited_from_log(id);

        assert_eq!(status(id), Some(RunStatus::Running));
        deregister(id);
    }

    #[tokio::test]
    async fn mark_exited_from_log_is_noop_when_run_not_registered() {
        let id = allocate_run_id() + 2_000_004;
        mark_exited_from_log(id);
        assert_eq!(status(id), None);
    }

    #[tokio::test]
    #[serial_test::serial(global_runs)]
    async fn a_finished_run_stays_listed_with_readable_logs_until_it_is_removed() {
        let id = allocate_run_id();
        let (handle, _rx) = make_handle();
        handle
            .logs
            .append(crate::run_log::StreamKind::Stdout, b"done");
        handle.logs.close(0);
        register(id, handle);

        mark_exited_from_log(id);

        let listed = snapshot().into_iter().find(|s| s.id == id);
        assert_eq!(
            listed.map(|s| s.status),
            Some(RunStatus::Exited { code: 0 }),
            "a finished run must remain listed as exited",
        );
        let logs = log_buffer(id).expect("a finished run's logs stay readable while it is listed");
        assert_eq!(logs.read_from(0).chunks[0].bytes, b"done");

        assert_eq!(remove_if_exited(id), RemoveOutcome::Removed);
        assert_eq!(status(id), None, "rm finally drops the finished run");
    }

    #[tokio::test]
    #[serial_test::serial(global_runs)]
    async fn status_reports_running_then_exited_and_none_when_unknown() {
        let id = allocate_run_id();
        assert_eq!(status(id), None);

        let (handle, _rx) = make_handle();
        register(id, handle);
        assert_eq!(status(id), Some(RunStatus::Running));

        set_exit_code(id, 5);
        assert_eq!(status(id), Some(RunStatus::Exited { code: 5 }));

        deregister(id);
    }

    #[test]
    fn snapshot_from_none_returns_empty_vec() {
        assert!(snapshot_from(None).is_empty());
    }

    #[tokio::test]
    async fn inspect_returns_summary_and_launch_config_for_a_registered_run() {
        let id = allocate_run_id();
        let (mut handle, _rx) = make_handle();
        handle.image = "some-image:1".to_string();
        handle.command = "echo hi".to_string();
        handle.config.cpus = 4;
        handle.config.mem_mib = 2048;
        register(id, handle);

        let details = inspect(id).expect("registered run must be inspectable");
        assert_eq!(details.summary.id, id);
        assert_eq!(details.summary.image, "some-image:1");
        assert_eq!(details.summary.status, RunStatus::Running);
        assert_eq!(details.config.cpus, 4);
        assert_eq!(details.config.mem_mib, 2048);

        deregister(id);
    }

    #[tokio::test]
    async fn inspect_returns_none_for_unknown_run() {
        let id = allocate_run_id() + 5_000_000;
        assert!(inspect(id).is_none());
    }

    #[tokio::test]
    async fn log_buffer_returns_the_registered_runs_buffer() {
        let id = allocate_run_id();
        let (handle, _rx) = make_handle();
        handle
            .logs
            .append(crate::run_log::StreamKind::Stdout, b"hello");
        register(id, handle);

        let buf = log_buffer(id).expect("registered run must expose its log buffer");
        assert_eq!(buf.read_from(0).chunks[0].bytes, b"hello");

        deregister(id);
    }

    #[tokio::test]
    async fn log_buffer_returns_none_for_unknown_run() {
        let id = allocate_run_id() + 4_000_000;
        assert!(log_buffer(id).is_none());
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

    fn set_status(map: &HashMap<u32, RunHandle>, id: u32, status: RunStatus) {
        *map.get(&id).unwrap().status.lock().unwrap() = status;
    }

    #[tokio::test]
    async fn remove_if_exited_from_drops_an_exited_entry_but_refuses_a_running_one() {
        let mut map = HashMap::new();
        let (handle, _rx) = make_handle();
        map.insert(1, handle);

        assert_eq!(
            remove_if_exited_from(Some(&mut map), 1),
            RemoveOutcome::Running
        );
        assert!(map.contains_key(&1), "a running run must not be removed");

        set_status(&map, 1, RunStatus::Exited { code: 0 });
        assert_eq!(
            remove_if_exited_from(Some(&mut map), 1),
            RemoveOutcome::Removed
        );
        assert!(!map.contains_key(&1), "an exited run must be removed");
    }

    #[test]
    fn remove_if_exited_from_reports_not_found_for_absent_id_and_empty_registry() {
        assert_eq!(remove_if_exited_from(None, 1), RemoveOutcome::NotFound);
        let mut empty: HashMap<u32, RunHandle> = HashMap::new();
        assert_eq!(
            remove_if_exited_from(Some(&mut empty), 1),
            RemoveOutcome::NotFound
        );
    }

    #[tokio::test]
    async fn prune_exited_from_removes_every_exited_run_and_keeps_running_ones() {
        let mut map = HashMap::new();
        for id in [10, 11, 12] {
            let (handle, _rx) = make_handle();
            map.insert(id, handle);
        }
        set_status(&map, 10, RunStatus::Exited { code: 0 });
        set_status(&map, 12, RunStatus::Exited { code: 3 });

        let mut removed = prune_exited_from(Some(&mut map));
        removed.sort_unstable();

        assert_eq!(removed, vec![10, 12]);
        assert!(map.contains_key(&11), "the running run must survive");
        assert!(!map.contains_key(&10) && !map.contains_key(&12));
    }

    #[test]
    fn prune_exited_from_empty_registry_returns_no_ids() {
        assert!(prune_exited_from(None).is_empty());
    }

    #[tokio::test]
    async fn remove_if_exited_removes_an_exited_run_from_the_live_registry() {
        let id = allocate_run_id();
        let (handle, _rx) = make_handle();
        register(id, handle);
        set_exit_code(id, 0);

        assert_eq!(remove_if_exited(id), RemoveOutcome::Removed);
        assert_eq!(status(id), None);
    }

    #[tokio::test]
    async fn remove_if_exited_refuses_a_running_run_in_the_live_registry() {
        let id = allocate_run_id();
        let (handle, _rx) = make_handle();
        register(id, handle);

        assert_eq!(remove_if_exited(id), RemoveOutcome::Running);
        assert_eq!(status(id), Some(RunStatus::Running));

        deregister(id);
    }

    #[tokio::test]
    async fn remove_if_exited_reports_not_found_for_an_unknown_run() {
        let id = allocate_run_id() + 6_000_000;
        assert_eq!(remove_if_exited(id), RemoveOutcome::NotFound);
    }
}
