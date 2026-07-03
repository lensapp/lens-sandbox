use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use lns_ipc::{RunStatus, validate_run_name};

use crate::run_name;
use rand::RngCore;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::vm::GuestTransport;
use crate::vm::session_client::SessionInput;

static NEXT_NAME_SEQ: AtomicUsize = AtomicUsize::new(0);

static ACTIVE: Mutex<Option<HashMap<String, RunHandle>>> = Mutex::new(None);

pub struct RunHandle {
    pub cancel_tx: oneshot::Sender<i32>,
    pub detach_tx: Mutex<Option<oneshot::Sender<()>>>,
    pub task: JoinHandle<()>,
    pub input_tx: Option<mpsc::Sender<SessionInput>>,
    pub connector: Option<std::sync::Arc<dyn GuestTransport>>,
    pub name: String,
    pub image: String,
    pub command: String,
    pub started: String,
    pub status: std::sync::Mutex<RunStatus>,
    pub logs: std::sync::Arc<crate::run_log::RunLogBuffer>,
    pub config: lns_ipc::RunConfig,
}

pub fn allocate_run_id() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    lns_ipc::hex_encode(&bytes)
}

pub fn register(run_id: String, handle: RunHandle) {
    let mut g = ACTIVE.lock().expect("ACTIVE poisoned");
    g.get_or_insert_with(HashMap::new).insert(run_id, handle);
}

pub fn register_named(
    run_id: String,
    requested: Option<String>,
    handle: RunHandle,
) -> Result<String, String> {
    let mut next_name = || run_name::name_for_index(NEXT_NAME_SEQ.fetch_add(1, Ordering::Relaxed));
    let mut g = ACTIVE.lock().expect("ACTIVE poisoned");
    let map = g.get_or_insert_with(HashMap::new);
    register_named_in(map, run_id, requested, handle, &mut next_name)
}

fn register_named_in(
    map: &mut HashMap<String, RunHandle>,
    run_id: String,
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

fn check_name_available(
    map: Option<&HashMap<String, RunHandle>>,
    name: &str,
) -> Result<(), String> {
    validate_run_name(name)?;
    match map.and_then(|m| name_holder(m, name)) {
        Some(holder) => Err(format!(
            "name {name:?} already in use by run {}",
            lns_ipc::short_run_id(&holder)
        )),
        None => Ok(()),
    }
}

fn unique_auto_name(
    map: &HashMap<String, RunHandle>,
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

fn name_holder(map: &HashMap<String, RunHandle>, name: &str) -> Option<String> {
    map.iter()
        .find(|(_, h)| h.name == name)
        .map(|(id, _)| id.clone())
}

pub fn resolve(handle: &str) -> Result<String, String> {
    let g = ACTIVE.lock().expect("ACTIVE poisoned");
    resolve_in(g.as_ref(), handle)
}

fn resolve_in(map: Option<&HashMap<String, RunHandle>>, handle: &str) -> Result<String, String> {
    if handle.is_empty() {
        return Err(format!("no such run: {handle}"));
    }
    let Some(map) = map else {
        return Err(format!("no such run: {handle}"));
    };
    if map.contains_key(handle) {
        return Ok(handle.to_string());
    }
    if let Some(id) = name_holder(map, handle) {
        return Ok(id);
    }
    let mut prefix_matches = map.keys().filter(|k| k.starts_with(handle));
    match (prefix_matches.next(), prefix_matches.next()) {
        (Some(id), None) => Ok(id.clone()),
        (Some(_), Some(_)) => Err(format!("ambiguous run id prefix: {handle}")),
        (None, _) => Err(format!("no such run: {handle}")),
    }
}

pub fn rename(handle: &str, new_name: &str) -> Result<(), String> {
    let mut g = ACTIVE.lock().expect("ACTIVE poisoned");
    rename_in(g.as_mut(), handle, new_name)
}

fn rename_in(
    map: Option<&mut HashMap<String, RunHandle>>,
    handle: &str,
    new_name: &str,
) -> Result<(), String> {
    validate_run_name(new_name)?;
    let map = map.ok_or_else(|| format!("no such run: {handle}"))?;
    let target = resolve_in(Some(&*map), handle)?;
    if let Some(holder) = name_holder(map, new_name)
        && holder != target
    {
        return Err(format!(
            "name {new_name:?} already in use by run {}",
            lns_ipc::short_run_id(&holder)
        ));
    }
    map.get_mut(&target)
        .expect("resolve_in only returns ids present in the map")
        .name = new_name.to_string();
    Ok(())
}

pub fn deregister(run_id: &str) {
    let mut g = ACTIVE.lock().expect("ACTIVE poisoned");
    if let Some(m) = g.as_mut() {
        m.remove(run_id);
    }
}

pub fn input_sender(run_id: &str) -> Option<mpsc::Sender<SessionInput>> {
    let g = ACTIVE.lock().expect("ACTIVE poisoned");
    g.as_ref()
        .and_then(|m| m.get(run_id))
        .and_then(|h| h.input_tx.clone())
}

pub fn log_buffer(run_id: &str) -> Option<std::sync::Arc<crate::run_log::RunLogBuffer>> {
    let g = ACTIVE.lock().expect("ACTIVE poisoned");
    g.as_ref()
        .and_then(|m| m.get(run_id))
        .map(|h| h.logs.clone())
}

pub fn connector(run_id: &str) -> Option<std::sync::Arc<dyn GuestTransport>> {
    let g = ACTIVE.lock().expect("ACTIVE poisoned");
    g.as_ref()
        .and_then(|m| m.get(run_id))
        .and_then(|h| h.connector.clone())
}

pub fn set_connector(run_id: &str, connector: std::sync::Arc<dyn GuestTransport>) {
    let mut g = ACTIVE.lock().expect("ACTIVE poisoned");
    if let Some(h) = g.as_mut().and_then(|m| m.get_mut(run_id)) {
        h.connector = Some(connector);
    }
}

pub fn set_exit_code(run_id: &str, code: i32) {
    let g = ACTIVE.lock().expect("ACTIVE poisoned");
    if let Some(h) = g.as_ref().and_then(|m| m.get(run_id))
        && let Ok(mut s) = h.status.lock()
    {
        *s = RunStatus::Exited { code };
    }
}

pub fn mark_exited_from_log(run_id: &str) {
    let g = ACTIVE.lock().expect("ACTIVE poisoned");
    if let Some(h) = g.as_ref().and_then(|m| m.get(run_id))
        && let Some(code) = h.logs.exit()
        && let Ok(mut s) = h.status.lock()
    {
        *s = RunStatus::Exited { code };
    }
}

pub fn status(run_id: &str) -> Option<RunStatus> {
    let g = ACTIVE.lock().expect("ACTIVE poisoned");
    g.as_ref()
        .and_then(|m| m.get(run_id))
        .and_then(|h| h.status.lock().ok().map(|s| *s))
}

pub fn snapshot() -> Vec<lns_ipc::RunSummary> {
    let g = ACTIVE.lock().expect("ACTIVE poisoned");
    snapshot_from(g.as_ref())
}

fn snapshot_from(map: Option<&HashMap<String, RunHandle>>) -> Vec<lns_ipc::RunSummary> {
    let Some(map) = map else {
        return Vec::new();
    };
    map.iter().map(|(id, h)| summary_of(id, h)).collect()
}

fn summary_of(id: &str, h: &RunHandle) -> lns_ipc::RunSummary {
    let status = h.status.lock().map(|s| *s).unwrap_or(RunStatus::Running);
    lns_ipc::RunSummary {
        id: id.to_string(),
        name: h.name.clone(),
        image: h.image.clone(),
        command: h.command.clone(),
        status,
        started: h.started.clone(),
        published_ports: h.config.published_ports.clone(),
    }
}

pub fn inspect(run_id: &str) -> Option<lns_ipc::RunDetails> {
    let g = ACTIVE.lock().expect("ACTIVE poisoned");
    g.as_ref()
        .and_then(|m| m.get(run_id))
        .map(|h| lns_ipc::RunDetails {
            summary: summary_of(run_id, h),
            config: h.config.clone(),
        })
}

pub fn cancel(run_id: &str) -> bool {
    let removed = {
        let mut g = ACTIVE.lock().expect("ACTIVE poisoned");
        g.as_mut().and_then(|m| m.remove(run_id))
    };
    let Some(handle) = removed else {
        return false;
    };
    let _ = handle.cancel_tx.send(130);
    handle.task.abort();
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetachOutcome {
    Detached,
    NotAttached,
    NotFound,
}

pub fn request_detach(run_id: &str) -> DetachOutcome {
    let g = ACTIVE.lock().expect("ACTIVE poisoned");
    let Some(handle) = g.as_ref().and_then(|m| m.get(run_id)) else {
        return DetachOutcome::NotFound;
    };
    let Some(tx) = handle.detach_tx.lock().expect("detach_tx poisoned").take() else {
        return DetachOutcome::NotAttached;
    };
    if tx.send(()).is_ok() {
        DetachOutcome::Detached
    } else {
        DetachOutcome::NotAttached
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoveOutcome {
    Removed,
    Running,
    NotFound,
}

pub fn remove_if_exited(run_id: &str) -> RemoveOutcome {
    let mut g = ACTIVE.lock().expect("ACTIVE poisoned");
    remove_if_exited_from(g.as_mut(), run_id)
}

fn remove_if_exited_from(
    map: Option<&mut HashMap<String, RunHandle>>,
    run_id: &str,
) -> RemoveOutcome {
    let Some(map) = map else {
        return RemoveOutcome::NotFound;
    };
    match map.get(run_id) {
        None => RemoveOutcome::NotFound,
        Some(h) if is_exited(h) => {
            map.remove(run_id);
            RemoveOutcome::Removed
        }
        Some(_) => RemoveOutcome::Running,
    }
}

pub fn prune_exited() -> Vec<String> {
    let mut g = ACTIVE.lock().expect("ACTIVE poisoned");
    prune_exited_from(g.as_mut())
}

fn prune_exited_from(map: Option<&mut HashMap<String, RunHandle>>) -> Vec<String> {
    let Some(map) = map else {
        return Vec::new();
    };
    let exited: Vec<String> = map
        .iter()
        .filter(|(_, h)| is_exited(h))
        .map(|(id, _)| id.clone())
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
                detach_tx: Mutex::new(None),
                task,
                input_tx: None,
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

    async fn named_handle(map: &mut HashMap<String, RunHandle>, id: &str, name: &str) {
        let (mut h, _rx) = make_handle();
        h.name = name.to_string();
        map.insert(id.to_string(), h);
    }

    #[tokio::test]
    async fn register_named_in_keeps_an_explicit_valid_name() {
        let mut map = HashMap::new();
        let (h, _rx) = make_handle();
        let name = register_named_in(
            &mut map,
            "aa01".into(),
            Some("reviewer".into()),
            h,
            &mut (gen_amber as fn() -> String),
        )
        .unwrap();
        assert_eq!(name, "reviewer");
        assert_eq!(map.get("aa01").unwrap().name, "reviewer");
    }

    #[tokio::test]
    async fn register_named_in_rejects_a_name_held_by_a_listed_run() {
        let mut map = HashMap::new();
        named_handle(&mut map, "aa07", "reviewer").await;
        let (h, _rx) = make_handle();
        let err = register_named_in(
            &mut map,
            "aa08".into(),
            Some("reviewer".into()),
            h,
            &mut (gen_amber as fn() -> String),
        )
        .unwrap_err();
        assert!(err.contains("already in use by run aa07"), "got: {err}");
    }

    #[tokio::test]
    async fn register_named_in_rejects_an_invalid_explicit_name() {
        let mut map = HashMap::new();
        let (h, _rx) = make_handle();
        register_named_in(
            &mut map,
            "aa01".into(),
            Some("abcdef".into()),
            h,
            &mut (gen_amber as fn() -> String),
        )
        .unwrap_err();
    }

    #[tokio::test]
    async fn register_named_in_auto_generates_when_no_name_is_requested() {
        let mut map = HashMap::new();
        let (h, _rx) = make_handle();
        let name = register_named_in(
            &mut map,
            "aa01".into(),
            None,
            h,
            &mut (gen_amber as fn() -> String),
        )
        .unwrap();
        assert_eq!(name, "amber_otter");
    }

    #[tokio::test]
    async fn register_named_in_regenerates_until_the_auto_name_is_unique() {
        let mut map = HashMap::new();
        named_handle(&mut map, "aa07", "amber_otter").await;
        let (h, _rx) = make_handle();
        let mut seq = ["amber_otter", "amber_otter", "bold_falcon"].into_iter();
        let mut next_name = || seq.next().unwrap().to_string();
        let name = register_named_in(&mut map, "aa08".into(), None, h, &mut next_name).unwrap();
        assert_eq!(name, "bold_falcon");
    }

    #[tokio::test]
    async fn register_named_in_suffixes_a_name_when_the_pretty_pool_is_exhausted() {
        let mut map = HashMap::new();
        named_handle(&mut map, "aa07", "amber_otter").await;
        let (h, _rx) = make_handle();
        let mut next_name = || "amber_otter".to_string();
        let name = register_named_in(&mut map, "aa08".into(), None, h, &mut next_name).unwrap();
        assert_eq!(name, "amber_otter_0");
    }

    #[tokio::test]
    #[serial_test::serial(global_runs)]
    async fn ensure_name_available_rejects_taken_or_invalid_names_and_accepts_free_ones() {
        let id = allocate_run_id();
        let (h, _rx) = make_handle();
        register_named(id.clone(), Some(format!("rev-{id}")), h).unwrap();
        assert!(ensure_name_available(&format!("rev-{id}")).is_err());
        assert!(ensure_name_available("abcdef").is_err());
        assert!(ensure_name_available(&format!("free-{id}")).is_ok());
        deregister(&id);
    }

    #[tokio::test]
    async fn resolve_in_matches_ids_names_and_unique_prefixes() {
        let mut map = HashMap::new();
        named_handle(&mut map, "1a2b3c4d0000000000000000000000aa", "reviewer").await;
        assert_eq!(
            resolve_in(Some(&map), "1a2b3c4d0000000000000000000000aa"),
            Ok("1a2b3c4d0000000000000000000000aa".to_string())
        );
        assert_eq!(
            resolve_in(Some(&map), "reviewer"),
            Ok("1a2b3c4d0000000000000000000000aa".to_string())
        );
        assert_eq!(
            resolve_in(Some(&map), "1a2b"),
            Ok("1a2b3c4d0000000000000000000000aa".to_string()),
            "a unique id prefix resolves to the full id"
        );
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
    async fn resolve_in_rejects_an_empty_handle_instead_of_matching_the_only_run() {
        let mut map = HashMap::new();
        named_handle(&mut map, "1a2b3c4d0000000000000000000000aa", "reviewer").await;
        let err = resolve_in(Some(&map), "").unwrap_err();
        assert!(
            err.contains("no such run"),
            "an empty handle must not wildcard-match the sole run, got: {err}"
        );
    }

    #[tokio::test]
    async fn resolve_in_reports_an_ambiguous_prefix() {
        let mut map = HashMap::new();
        named_handle(&mut map, "1a2b3c4d0000000000000000000000aa", "reviewer").await;
        named_handle(&mut map, "1a2b9999000000000000000000000bbb", "auditor").await;
        let err = resolve_in(Some(&map), "1a2b").unwrap_err();
        assert!(err.contains("ambiguous run id prefix: 1a2b"), "got: {err}");
    }

    #[tokio::test]
    async fn rename_in_changes_the_name_in_place() {
        let mut map = HashMap::new();
        named_handle(&mut map, "1a2b3c4d0000000000000000000000aa", "reviewer").await;
        rename_in(Some(&mut map), "reviewer", "auditor").unwrap();
        assert_eq!(
            map.get("1a2b3c4d0000000000000000000000aa").unwrap().name,
            "auditor"
        );
        assert_eq!(
            resolve_in(Some(&map), "auditor"),
            Ok("1a2b3c4d0000000000000000000000aa".to_string())
        );
        assert!(
            resolve_in(Some(&map), "reviewer")
                .unwrap_err()
                .contains("no such run")
        );
    }

    #[tokio::test]
    async fn rename_in_to_a_held_name_is_refused() {
        let mut map = HashMap::new();
        named_handle(&mut map, "aa03", "reviewer").await;
        named_handle(&mut map, "aa04", "auditor").await;
        let err = rename_in(Some(&mut map), "auditor", "reviewer").unwrap_err();
        assert!(err.contains("already in use by run aa03"), "got: {err}");
    }

    #[tokio::test]
    async fn rename_in_to_the_runs_own_name_is_a_noop_success() {
        let mut map = HashMap::new();
        named_handle(&mut map, "aa03", "reviewer").await;
        rename_in(Some(&mut map), "reviewer", "reviewer").unwrap();
        assert_eq!(map.get("aa03").unwrap().name, "reviewer");
    }

    #[tokio::test]
    async fn rename_in_rejects_an_invalid_new_name() {
        let mut map = HashMap::new();
        named_handle(&mut map, "aa03", "reviewer").await;
        rename_in(Some(&mut map), "reviewer", "abcdef").unwrap_err();
    }

    #[tokio::test]
    async fn rename_in_reports_no_such_run_for_unknown_name_id_or_empty_registry() {
        let mut map = HashMap::new();
        named_handle(&mut map, "aa03", "reviewer").await;
        let err = rename_in(Some(&mut map), "ghost", "auditor").unwrap_err();
        assert!(err.contains("no such run: ghost"), "got: {err}");
        let err = rename_in(Some(&mut map), "ff99", "auditor").unwrap_err();
        assert!(err.contains("no such run: ff99"), "got: {err}");
        let err = rename_in(None, "ghost", "auditor").unwrap_err();
        assert!(err.contains("no such run: ghost"), "got: {err}");
    }

    #[tokio::test]
    #[serial_test::serial(global_runs)]
    async fn register_named_assigns_an_auto_name_and_resolves_by_name_and_id() {
        let id = allocate_run_id();
        let (h, _rx) = make_handle();
        let name = register_named(id.clone(), None, h).unwrap();
        assert!(!name.is_empty());
        assert_eq!(resolve(&name), Ok(id.clone()));
        assert_eq!(resolve(&id), Ok(id.clone()));
        deregister(&id);
    }

    #[tokio::test]
    #[serial_test::serial(global_runs)]
    async fn rename_via_the_global_registry_updates_the_name() {
        let id = allocate_run_id();
        let (h, _rx) = make_handle();
        register_named(id.clone(), Some(format!("rev-{id}")), h).unwrap();
        rename(&format!("rev-{id}"), &format!("aud-{id}")).unwrap();
        assert_eq!(resolve(&format!("aud-{id}")), Ok(id.clone()));
        deregister(&id);
    }

    #[tokio::test]
    async fn allocate_run_id_returns_distinct_thirty_two_char_hex() {
        let a = allocate_run_id();
        let b = allocate_run_id();
        assert_ne!(a, b);
        assert_eq!(a.len(), 32);
        assert!(
            a.bytes()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    #[tokio::test]
    async fn cancel_unknown_run_returns_false() {
        assert!(!cancel("ffffffffffffffffffffffffffffffff"));
    }

    #[tokio::test]
    async fn cancel_fires_oneshot_with_130() {
        let id = allocate_run_id();
        let (handle, cancel_rx) = make_handle();
        register(id.clone(), handle);

        assert!(cancel(&id));

        assert_eq!(cancel_rx.await.expect("cancel oneshot delivered"), 130);
    }

    #[tokio::test]
    async fn cancel_twice_only_fires_once() {
        let id = allocate_run_id();
        let (handle, _rx) = make_handle();
        register(id.clone(), handle);

        assert!(cancel(&id));
        assert!(!cancel(&id), "second cancel of the same id must be a no-op");
    }

    #[tokio::test]
    async fn request_detach_unknown_run_reports_not_found() {
        let id = "ffffffffffffffffffffffffffffffff".to_string();
        assert_eq!(request_detach(&id), DetachOutcome::NotFound);
    }

    #[tokio::test]
    async fn request_detach_fires_the_detach_signal_once() {
        let id = allocate_run_id();
        let (mut handle, _cancel_rx) = make_handle();
        let (detach_tx, detach_rx) = oneshot::channel::<()>();
        handle.detach_tx = Mutex::new(Some(detach_tx));
        register(id.clone(), handle);

        assert_eq!(request_detach(&id), DetachOutcome::Detached);
        assert!(detach_rx.await.is_ok(), "detach signal must be delivered");
        assert_eq!(
            request_detach(&id),
            DetachOutcome::NotAttached,
            "a registered run whose detach signal was already consumed is not attached, not absent",
        );
        deregister(&id);
    }

    #[tokio::test]
    async fn request_detach_reports_not_attached_when_the_pump_has_gone() {
        let id = allocate_run_id();
        let (mut handle, _cancel_rx) = make_handle();
        let (detach_tx, detach_rx) = oneshot::channel::<()>();
        drop(detach_rx);
        handle.detach_tx = Mutex::new(Some(detach_tx));
        register(id.clone(), handle);

        assert_eq!(
            request_detach(&id),
            DetachOutcome::NotAttached,
            "a run whose pump already returned can no longer be detached",
        );
        deregister(&id);
    }

    #[tokio::test]
    async fn deregister_clears_registry() {
        let id = allocate_run_id();
        let (handle, _rx) = make_handle();
        register(id.clone(), handle);
        deregister(&id);
        assert!(!cancel(&id));
    }

    struct StubTransport;
    impl crate::vm::GuestTransport for StubTransport {
        fn connect(
            &self,
            _port: u32,
            _timeout: std::time::Duration,
        ) -> futures_util::future::BoxFuture<'_, anyhow::Result<std::os::fd::RawFd>> {
            Box::pin(async { Ok(0) })
        }
        fn request_stop(&self) {}
    }

    #[tokio::test]
    async fn input_sender_returns_none_for_unknown_id() {
        let id = "deadbeef00000000000000000000000a".to_string();
        assert!(input_sender(&id).is_none());
    }

    #[tokio::test]
    async fn input_sender_returns_clone_when_handle_has_one() {
        let id = allocate_run_id();
        let (mut handle, _rx) = make_handle();
        let (tx, _input_rx) = mpsc::channel::<SessionInput>(1);
        handle.input_tx = Some(tx);
        register(id.clone(), handle);

        let cloned = input_sender(&id);
        assert!(cloned.is_some());

        deregister(&id);
    }

    #[tokio::test]
    async fn connector_returns_none_for_unknown_id() {
        let id = "deadbeef00000000000000000000000b".to_string();
        assert!(connector(&id).is_none());
    }

    #[tokio::test]
    async fn connector_returns_none_when_handle_has_no_connector() {
        let id = allocate_run_id();
        let (handle, _rx) = make_handle();
        register(id.clone(), handle);

        assert!(connector(&id).is_none());

        deregister(&id);
    }

    #[tokio::test]
    async fn set_connector_populates_handle_field() {
        let id = allocate_run_id();
        let (handle, _rx) = make_handle();
        register(id.clone(), handle);

        set_connector(&id, std::sync::Arc::new(StubTransport));

        let stored = connector(&id).expect("connector should be stored");
        stored.request_stop();
        assert_eq!(
            stored
                .connect(1, std::time::Duration::ZERO)
                .await
                .expect("stub connect"),
            0
        );

        deregister(&id);
    }

    #[tokio::test]
    async fn set_connector_is_noop_when_run_not_registered() {
        let id = "deadbeef00000000000000000000000c".to_string();
        set_connector(&id, std::sync::Arc::new(StubTransport));

        assert!(connector(&id).is_none());
    }

    #[tokio::test]
    #[serial_test::serial(global_runs)]
    async fn set_exit_code_flips_status_to_exited() {
        let id = allocate_run_id();
        let (handle, _rx) = make_handle();
        register(id.clone(), handle);

        set_exit_code(&id, 42);

        let summary = snapshot().into_iter().find(|s| s.id == *id).unwrap();
        assert_eq!(summary.status, RunStatus::Exited { code: 42 });

        deregister(&id);
    }

    #[tokio::test]
    async fn set_exit_code_is_noop_when_run_not_registered() {
        let id = "deadbeef00000000000000000000000d".to_string();
        set_exit_code(&id, 17);
        assert!(snapshot().iter().all(|s| s.id != *id));
    }

    #[tokio::test]
    #[serial_test::serial(global_runs)]
    async fn mark_exited_from_log_adopts_the_log_buffers_exit_code() {
        let id = allocate_run_id();
        let (handle, _rx) = make_handle();
        handle.logs.close(7);
        register(id.clone(), handle);

        mark_exited_from_log(&id);

        assert_eq!(status(&id), Some(RunStatus::Exited { code: 7 }));
        deregister(&id);
    }

    #[tokio::test]
    #[serial_test::serial(global_runs)]
    async fn mark_exited_from_log_leaves_a_run_running_while_its_log_is_open() {
        let id = allocate_run_id();
        let (handle, _rx) = make_handle();
        register(id.clone(), handle);

        mark_exited_from_log(&id);

        assert_eq!(status(&id), Some(RunStatus::Running));
        deregister(&id);
    }

    #[tokio::test]
    async fn mark_exited_from_log_is_noop_when_run_not_registered() {
        let id = "deadbeef00000000000000000000000e".to_string();
        mark_exited_from_log(&id);
        assert_eq!(status(&id), None);
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
        register(id.clone(), handle);

        mark_exited_from_log(&id);

        let listed = snapshot().into_iter().find(|s| s.id == *id);
        assert_eq!(
            listed.map(|s| s.status),
            Some(RunStatus::Exited { code: 0 }),
            "a finished run must remain listed as exited",
        );
        let logs = log_buffer(&id).expect("a finished run's logs stay readable while it is listed");
        assert_eq!(logs.read_from(0).chunks[0].bytes, b"done");

        assert_eq!(remove_if_exited(&id), RemoveOutcome::Removed);
        assert_eq!(status(&id), None, "rm finally drops the finished run");
    }

    #[tokio::test]
    #[serial_test::serial(global_runs)]
    async fn status_reports_running_then_exited_and_none_when_unknown() {
        let id = allocate_run_id();
        assert_eq!(status(&id), None);

        let (handle, _rx) = make_handle();
        register(id.clone(), handle);
        assert_eq!(status(&id), Some(RunStatus::Running));

        set_exit_code(&id, 5);
        assert_eq!(status(&id), Some(RunStatus::Exited { code: 5 }));

        deregister(&id);
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
        register(id.clone(), handle);

        let details = inspect(&id).expect("registered run must be inspectable");
        assert_eq!(details.summary.id, *id);
        assert_eq!(details.summary.image, "some-image:1");
        assert_eq!(details.summary.status, RunStatus::Running);
        assert_eq!(details.config.cpus, 4);
        assert_eq!(details.config.mem_mib, 2048);

        deregister(&id);
    }

    #[tokio::test]
    async fn inspect_returns_none_for_unknown_run() {
        let id = "deadbeef00000000000000000000000f".to_string();
        assert!(inspect(&id).is_none());
    }

    #[tokio::test]
    async fn log_buffer_returns_the_registered_runs_buffer() {
        let id = allocate_run_id();
        let (handle, _rx) = make_handle();
        handle
            .logs
            .append(crate::run_log::StreamKind::Stdout, b"hello");
        register(id.clone(), handle);

        let buf = log_buffer(&id).expect("registered run must expose its log buffer");
        assert_eq!(buf.read_from(0).chunks[0].bytes, b"hello");

        deregister(&id);
    }

    #[tokio::test]
    async fn log_buffer_returns_none_for_unknown_run() {
        let id = "deadbeef000000000000000000000010".to_string();
        assert!(log_buffer(&id).is_none());
    }

    #[tokio::test]
    async fn snapshot_lists_registered_handles_with_running_status() {
        let id = allocate_run_id();
        let (mut handle, _rx) = make_handle();
        handle.image = "alpine:latest".to_string();
        handle.command = "sleep 1".to_string();
        handle.started = "2026-05-21 10:00:00".to_string();
        handle.config.published_ports = vec![lns_ipc::PortPublish {
            host_ip: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            host_port: 8080,
            container_port: 80,
            protocol: lns_ipc::Protocol::Tcp,
        }];
        register(id.clone(), handle);

        let row = snapshot()
            .into_iter()
            .find(|s| s.id == *id)
            .expect("registered handle must appear in snapshot");
        assert_eq!(row.image, "alpine:latest");
        assert_eq!(row.command, "sleep 1");
        assert_eq!(row.started, "2026-05-21 10:00:00");
        assert_eq!(row.status, RunStatus::Running);
        assert_eq!(
            row.published_ports,
            vec![lns_ipc::PortPublish {
                host_ip: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
                host_port: 8080,
                container_port: 80,
                protocol: lns_ipc::Protocol::Tcp,
            }],
            "the snapshot must carry the launch config's published ports",
        );

        deregister(&id);
    }

    fn set_status(map: &HashMap<String, RunHandle>, id: &str, status: RunStatus) {
        *map.get(id).unwrap().status.lock().unwrap() = status;
    }

    #[tokio::test]
    async fn remove_if_exited_from_drops_an_exited_entry_but_refuses_a_running_one() {
        let mut map = HashMap::new();
        let (handle, _rx) = make_handle();
        map.insert("aa01".to_string(), handle);

        assert_eq!(
            remove_if_exited_from(Some(&mut map), "aa01"),
            RemoveOutcome::Running
        );
        assert!(
            map.contains_key("aa01"),
            "a running run must not be removed"
        );

        set_status(&map, "aa01", RunStatus::Exited { code: 0 });
        assert_eq!(
            remove_if_exited_from(Some(&mut map), "aa01"),
            RemoveOutcome::Removed
        );
        assert!(!map.contains_key("aa01"), "an exited run must be removed");
    }

    #[test]
    fn remove_if_exited_from_reports_not_found_for_absent_id_and_empty_registry() {
        assert_eq!(remove_if_exited_from(None, "aa01"), RemoveOutcome::NotFound);
        let mut empty: HashMap<String, RunHandle> = HashMap::new();
        assert_eq!(
            remove_if_exited_from(Some(&mut empty), "aa01"),
            RemoveOutcome::NotFound
        );
    }

    #[tokio::test]
    async fn prune_exited_from_removes_every_exited_run_and_keeps_running_ones() {
        let mut map = HashMap::new();
        for id in ["aa10", "aa11", "aa12"] {
            let (handle, _rx) = make_handle();
            map.insert(id.to_string(), handle);
        }
        set_status(&map, "aa10", RunStatus::Exited { code: 0 });
        set_status(&map, "aa12", RunStatus::Exited { code: 3 });

        let mut removed = prune_exited_from(Some(&mut map));
        removed.sort_unstable();

        assert_eq!(removed, vec!["aa10".to_string(), "aa12".to_string()]);
        assert!(map.contains_key("aa11"), "the running run must survive");
        assert!(!map.contains_key("aa10") && !map.contains_key("aa12"));
    }

    #[test]
    fn prune_exited_from_empty_registry_returns_no_ids() {
        assert!(prune_exited_from(None).is_empty());
    }

    #[tokio::test]
    async fn remove_if_exited_removes_an_exited_run_from_the_live_registry() {
        let id = allocate_run_id();
        let (handle, _rx) = make_handle();
        register(id.clone(), handle);
        set_exit_code(&id, 0);

        assert_eq!(remove_if_exited(&id), RemoveOutcome::Removed);
        assert_eq!(status(&id), None);
    }

    #[tokio::test]
    async fn remove_if_exited_refuses_a_running_run_in_the_live_registry() {
        let id = allocate_run_id();
        let (handle, _rx) = make_handle();
        register(id.clone(), handle);

        assert_eq!(remove_if_exited(&id), RemoveOutcome::Running);
        assert_eq!(status(&id), Some(RunStatus::Running));

        deregister(&id);
    }

    #[tokio::test]
    async fn remove_if_exited_reports_not_found_for_an_unknown_run() {
        let id = "deadbeef000000000000000000000011".to_string();
        assert_eq!(remove_if_exited(&id), RemoveOutcome::NotFound);
    }
}
