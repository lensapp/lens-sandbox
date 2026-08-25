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

static ACTIVE: Mutex<Option<HashMap<String, RunEntry>>> = Mutex::new(None);

/// A listed run: alive with a session behind it, or stopped with only its record — restartable until removed.
pub enum RunEntry {
    Live(RunHandle),
    Stopped(StoppedRun),
}

pub struct StoppedRun {
    pub record: crate::run_record::RunRecord,
}

impl RunEntry {
    fn name(&self) -> &str {
        match self {
            RunEntry::Live(h) => &h.name,
            RunEntry::Stopped(s) => &s.record.name,
        }
    }

    fn set_name(&mut self, name: String) {
        match self {
            RunEntry::Live(h) => h.name = name,
            RunEntry::Stopped(s) => s.record.name = name,
        }
    }

    fn as_live(&self) -> Option<&RunHandle> {
        match self {
            RunEntry::Live(h) => Some(h),
            RunEntry::Stopped(_) => None,
        }
    }

    fn as_live_mut(&mut self) -> Option<&mut RunHandle> {
        match self {
            RunEntry::Live(h) => Some(h),
            RunEntry::Stopped(_) => None,
        }
    }

    fn status(&self) -> RunStatus {
        match self {
            RunEntry::Live(h) => h.status.lock().map(|s| *s).unwrap_or(RunStatus::Running),
            RunEntry::Stopped(s) => RunStatus::Exited {
                code: s.record.exit_code.unwrap_or(-1),
            },
        }
    }
}

pub struct RunHandle {
    pub cancel_tx: oneshot::Sender<i32>,
    pub detach_tx: Mutex<Option<oneshot::Sender<()>>>,
    pub task: JoinHandle<()>,
    pub input_tx: Option<mpsc::Sender<SessionInput>>,
    pub exec_sessions: HashMap<String, mpsc::Sender<SessionInput>>,
    pub connector: Option<std::sync::Arc<dyn GuestTransport>>,
    pub name: String,
    pub image: String,
    pub command: String,
    pub started: String,
    pub status: std::sync::Mutex<RunStatus>,
    pub logs: std::sync::Arc<crate::run_log::RunLogBuffer>,
    pub config: lns_ipc::RunConfig,
    /// Everything an `lns exec` into this run needs to land in the same sandbox the workload is in.
    pub exec_environment: ExecEnvironment,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExecEnvironment {
    /// The run's resolved workload env, so a diagnostic command does not have to re-export it by hand.
    pub session_env: Vec<String>,
    /// What this run's declared tools contribute, so `lns exec` resolves them the way the workload does.
    pub tools: crate::workload_env::ToolRuntime,
    /// `env_var=placeholder` for each connector that seeds one, so an exec can exercise a token path the workload uses without ever holding the real value.
    pub placeholders: Vec<(String, String)>,
    /// The workload's working directory; `docker exec` inherits it, and an exec in `/` makes `sh -lc` write to the wrong place.
    pub workdir: Option<String>,
    /// Which identity vars (`HOME`, `USER`) the author declared, so an exec keeps them the way the supervisor honors `LENS_SANDBOX_WORKLOAD_*` — an image's own ENV must not outrank the run-as identity.
    pub declared_identity_keys: Vec<String>,
}

pub fn allocate_run_id() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    lns_ipc::hex_encode(&bytes)
}

pub fn register(run_id: String, handle: RunHandle) {
    let mut g = ACTIVE.lock().expect("ACTIVE poisoned");
    g.get_or_insert_with(HashMap::new)
        .insert(run_id, RunEntry::Live(handle));
}

pub fn register_stopped(stopped: StoppedRun) {
    let mut g = ACTIVE.lock().expect("ACTIVE poisoned");
    g.get_or_insert_with(HashMap::new)
        .insert(stopped.record.run_id.clone(), RunEntry::Stopped(stopped));
}

pub fn rebuild_from_records(records: Vec<crate::run_record::RunRecord>) {
    let mut g = ACTIVE.lock().expect("ACTIVE poisoned");
    rebuild_in(g.get_or_insert_with(HashMap::new), records);
}

fn rebuild_in(map: &mut HashMap<String, RunEntry>, records: Vec<crate::run_record::RunRecord>) {
    for record in records {
        map.entry(record.run_id.clone())
            .or_insert(RunEntry::Stopped(StoppedRun { record }));
    }
}

/// Replace a startable entry with the live handle booting over its preserved state; the name carries over.
pub fn transition_to_live(run_id: &str, mut handle: RunHandle) -> Result<String, String> {
    let mut g = ACTIVE.lock().expect("ACTIVE poisoned");
    let map = g.get_or_insert_with(HashMap::new);
    match map.get(run_id) {
        Some(entry) if is_exited(entry) => {
            handle.name = entry.name().to_string();
            let name = handle.name.clone();
            map.insert(run_id.to_string(), RunEntry::Live(handle));
            Ok(name)
        }
        Some(_) => Err(format!("run {run_id} is already running")),
        None => Err(format!("no such run: {run_id}")),
    }
}

pub fn stopped_run_names() -> Vec<String> {
    let g = ACTIVE.lock().expect("ACTIVE poisoned");
    stopped_names_in(g.as_ref())
}

fn stopped_names_in(map: Option<&HashMap<String, RunEntry>>) -> Vec<String> {
    let Some(map) = map else {
        return Vec::new();
    };
    let mut names: Vec<String> = map
        .values()
        .filter(|e| is_exited(e))
        .map(|e| e.name().to_string())
        .collect();
    names.sort_unstable();
    names
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
    map: &mut HashMap<String, RunEntry>,
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
    map.insert(run_id, RunEntry::Live(handle));
    Ok(name)
}

pub fn ensure_name_available(name: &str) -> Result<(), String> {
    let g = ACTIVE.lock().expect("ACTIVE poisoned");
    check_name_available(g.as_ref(), name)
}

fn check_name_available(map: Option<&HashMap<String, RunEntry>>, name: &str) -> Result<(), String> {
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
    map: &HashMap<String, RunEntry>,
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

fn name_holder(map: &HashMap<String, RunEntry>, name: &str) -> Option<String> {
    map.iter()
        .find(|(_, e)| e.name() == name)
        .map(|(id, _)| id.clone())
}

pub fn resolve(handle: &str) -> Result<String, String> {
    let g = ACTIVE.lock().expect("ACTIVE poisoned");
    resolve_in(g.as_ref(), handle)
}

/// Resolve a handle and read its status under one lock, so the answer can never name a run that vanished in between.
pub fn resolve_status(handle: &str) -> Result<(String, RunStatus), String> {
    let g = ACTIVE.lock().expect("ACTIVE poisoned");
    let id = resolve_in(g.as_ref(), handle)?;
    let status = g
        .as_ref()
        .and_then(|m| m.get(&id))
        .map(|e| e.status())
        .expect("resolve_in only returns ids present in the map");
    Ok((id, status))
}

fn resolve_in(map: Option<&HashMap<String, RunEntry>>, handle: &str) -> Result<String, String> {
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
    map: Option<&mut HashMap<String, RunEntry>>,
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
        .set_name(new_name.to_string());
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
        .and_then(|e| e.as_live())
        .and_then(|h| h.input_tx.clone())
}

pub fn register_exec_session(
    run_id: &str,
    session_id: String,
    input_tx: mpsc::Sender<SessionInput>,
) -> bool {
    let mut g = ACTIVE.lock().expect("ACTIVE poisoned");
    let Some(handle) = g
        .as_mut()
        .and_then(|runs| runs.get_mut(run_id))
        .and_then(RunEntry::as_live_mut)
    else {
        return false;
    };
    handle.exec_sessions.insert(session_id, input_tx);
    true
}

pub fn deregister_exec_session(run_id: &str, session_id: &str) -> bool {
    let mut g = ACTIVE.lock().expect("ACTIVE poisoned");
    g.as_mut()
        .and_then(|runs| runs.get_mut(run_id))
        .and_then(RunEntry::as_live_mut)
        .and_then(|handle| handle.exec_sessions.remove(session_id))
        .is_some()
}

pub fn session_input_sender(target: &lns_ipc::SessionTarget) -> Option<mpsc::Sender<SessionInput>> {
    match target {
        lns_ipc::SessionTarget::Primary { run_id } => input_sender(run_id),
        lns_ipc::SessionTarget::Exec { run_id, session_id } => {
            let g = ACTIVE.lock().expect("ACTIVE poisoned");
            g.as_ref()
                .and_then(|runs| runs.get(run_id))
                .and_then(RunEntry::as_live)
                .and_then(|handle| handle.exec_sessions.get(session_id))
                .cloned()
        }
    }
}

pub fn log_buffer(run_id: &str) -> Option<std::sync::Arc<crate::run_log::RunLogBuffer>> {
    let g = ACTIVE.lock().expect("ACTIVE poisoned");
    g.as_ref()
        .and_then(|m| m.get(run_id))
        .and_then(|e| e.as_live())
        .map(|h| h.logs.clone())
}

pub fn connector(run_id: &str) -> Option<std::sync::Arc<dyn GuestTransport>> {
    let g = ACTIVE.lock().expect("ACTIVE poisoned");
    g.as_ref()
        .and_then(|m| m.get(run_id))
        .and_then(|e| e.as_live())
        .and_then(|h| h.connector.clone())
}

pub fn exec_environment(run_id: &str) -> ExecEnvironment {
    let g = ACTIVE.lock().expect("ACTIVE poisoned");
    g.as_ref()
        .and_then(|m| m.get(run_id))
        .and_then(|e| e.as_live())
        .map(|h| h.exec_environment.clone())
        .unwrap_or_default()
}

pub fn set_connector(run_id: &str, connector: std::sync::Arc<dyn GuestTransport>) {
    let mut g = ACTIVE.lock().expect("ACTIVE poisoned");
    if let Some(h) = g
        .as_mut()
        .and_then(|m| m.get_mut(run_id))
        .and_then(|e| e.as_live_mut())
    {
        h.connector = Some(connector);
    }
}

/// The connector is what makes a run exec-able, so both publish in one mutation — an exec that saw the gate open but not the environment would run without it and with no error.
pub fn set_connector_with_environment(
    run_id: &str,
    connector: std::sync::Arc<dyn GuestTransport>,
    exec_environment: ExecEnvironment,
) {
    let mut g = ACTIVE.lock().expect("ACTIVE poisoned");
    if let Some(h) = g
        .as_mut()
        .and_then(|m| m.get_mut(run_id))
        .and_then(|e| e.as_live_mut())
    {
        h.exec_environment = exec_environment;
        h.connector = Some(connector);
    }
}

/// Record the VM size a sandbox run resolved to (from its resources) so `lns inspect` reports what actually booted, not the pre-resolution request.
pub fn set_resolved_size(run_id: &str, cpus: u8, mem_mib: usize) {
    let mut g = ACTIVE.lock().expect("ACTIVE poisoned");
    if let Some(h) = g
        .as_mut()
        .and_then(|m| m.get_mut(run_id))
        .and_then(|e| e.as_live_mut())
    {
        h.config.cpus = cpus;
        h.config.mem_mib = mem_mib;
    }
}

pub(crate) fn set_resolved_command_and_env(run_id: &str, command: &[String], env: &[String]) {
    let mut g = ACTIVE.lock().expect("ACTIVE poisoned");
    if let Some(h) = g
        .as_mut()
        .and_then(|m| m.get_mut(run_id))
        .and_then(|e| e.as_live_mut())
    {
        h.command = command.join(" ");
        h.config.env = env.to_vec();
    }
}

pub fn set_exit_code(run_id: &str, code: i32) {
    let g = ACTIVE.lock().expect("ACTIVE poisoned");
    if let Some(h) = g
        .as_ref()
        .and_then(|m| m.get(run_id))
        .and_then(|e| e.as_live())
        && let Ok(mut s) = h.status.lock()
    {
        *s = RunStatus::Exited { code };
    }
}

pub fn mark_exited_from_log(run_id: &str) {
    let g = ACTIVE.lock().expect("ACTIVE poisoned");
    if let Some(h) = g
        .as_ref()
        .and_then(|m| m.get(run_id))
        .and_then(|e| e.as_live())
        && let Some(code) = h.logs.exit()
        && let Ok(mut s) = h.status.lock()
    {
        *s = RunStatus::Exited { code };
    }
}

pub fn status(run_id: &str) -> Option<RunStatus> {
    let g = ACTIVE.lock().expect("ACTIVE poisoned");
    g.as_ref().and_then(|m| m.get(run_id)).map(|e| e.status())
}

pub fn snapshot() -> Vec<lns_ipc::RunSummary> {
    let g = ACTIVE.lock().expect("ACTIVE poisoned");
    snapshot_from(g.as_ref())
}

fn snapshot_from(map: Option<&HashMap<String, RunEntry>>) -> Vec<lns_ipc::RunSummary> {
    let Some(map) = map else {
        return Vec::new();
    };
    map.iter().map(|(id, e)| summary_of(id, e)).collect()
}

fn summary_of(id: &str, e: &RunEntry) -> lns_ipc::RunSummary {
    let (image, command, started) = match e {
        RunEntry::Live(h) => (h.image.clone(), h.command.clone(), h.started.clone()),
        RunEntry::Stopped(s) => (
            s.record.image.clone(),
            s.record.command.clone(),
            s.record.created_at.clone(),
        ),
    };
    lns_ipc::RunSummary {
        id: id.to_string(),
        name: e.name().to_string(),
        image,
        command,
        status: e.status(),
        started,
    }
}

pub fn inspect(run_id: &str) -> Option<lns_ipc::RunDetails> {
    let g = ACTIVE.lock().expect("ACTIVE poisoned");
    g.as_ref()
        .and_then(|m| m.get(run_id))
        .map(|e| lns_ipc::RunDetails {
            summary: summary_of(run_id, e),
            config: match e {
                RunEntry::Live(h) => h.config.clone(),
                RunEntry::Stopped(s) => lns_ipc::RunConfig::from_run_args(&s.record.args),
            },
        })
}

pub fn cancel(run_id: &str) -> bool {
    let removed = {
        let mut g = ACTIVE.lock().expect("ACTIVE poisoned");
        g.as_mut().and_then(|map| match map.remove(run_id) {
            Some(RunEntry::Live(handle)) => Some(handle),
            Some(stopped) => {
                map.insert(run_id.to_string(), stopped);
                None
            }
            None => None,
        })
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
    let Some(entry) = g.as_ref().and_then(|m| m.get(run_id)) else {
        return DetachOutcome::NotFound;
    };
    let Some(handle) = entry.as_live() else {
        return DetachOutcome::NotAttached;
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

pub enum RemoveOutcome {
    Removed(Box<RunEntry>),
    Running,
    NotFound,
}

pub fn remove_if_exited(run_id: &str) -> RemoveOutcome {
    let mut g = ACTIVE.lock().expect("ACTIVE poisoned");
    remove_if_exited_from(g.as_mut(), run_id)
}

/// Puts back an entry `remove_if_exited` handed out, because its files refused to go.
pub fn restore(run_id: String, entry: RunEntry) {
    let mut g = ACTIVE.lock().expect("ACTIVE poisoned");
    g.get_or_insert_with(HashMap::new).insert(run_id, entry);
}

fn remove_if_exited_from(
    map: Option<&mut HashMap<String, RunEntry>>,
    run_id: &str,
) -> RemoveOutcome {
    let Some(map) = map else {
        return RemoveOutcome::NotFound;
    };
    match map.remove(run_id) {
        None => RemoveOutcome::NotFound,
        Some(e) if is_exited(&e) => RemoveOutcome::Removed(Box::new(e)),
        Some(e) => {
            map.insert(run_id.to_string(), e);
            RemoveOutcome::Running
        }
    }
}

pub fn prune_exited() -> Vec<String> {
    let mut g = ACTIVE.lock().expect("ACTIVE poisoned");
    prune_exited_from(g.as_mut())
}

fn prune_exited_from(map: Option<&mut HashMap<String, RunEntry>>) -> Vec<String> {
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

fn is_exited(e: &RunEntry) -> bool {
    matches!(e.status(), RunStatus::Exited { .. })
}

/// A registered-but-idle run, for tests in this crate that need one to exist.
#[cfg(test)]
pub(crate) fn test_handle() -> (RunHandle, oneshot::Receiver<i32>) {
    let (cancel_tx, cancel_rx) = oneshot::channel::<i32>();
    let task = tokio::spawn(std::future::pending::<()>());
    (
        RunHandle {
            cancel_tx,
            detach_tx: Mutex::new(None),
            task,
            input_tx: None,
            exec_sessions: Default::default(),
            connector: None,
            name: String::new(),
            image: String::new(),
            command: String::new(),
            started: String::new(),
            status: std::sync::Mutex::new(RunStatus::Running),
            logs: std::sync::Arc::new(crate::run_log::RunLogBuffer::default()),
            config: lns_ipc::RunConfig::default(),
            exec_environment: Default::default(),
        },
        cancel_rx,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_handle() -> (RunHandle, oneshot::Receiver<i32>) {
        super::test_handle()
    }

    fn gen_amber() -> String {
        "amber_otter".to_string()
    }

    async fn named_handle(map: &mut HashMap<String, RunEntry>, id: &str, name: &str) {
        let (mut h, _rx) = make_handle();
        h.name = name.to_string();
        map.insert(id.to_string(), RunEntry::Live(h));
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
        assert_eq!(map.get("aa01").unwrap().name(), "reviewer");
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
            map.get("1a2b3c4d0000000000000000000000aa").unwrap().name(),
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
        assert_eq!(map.get("aa03").unwrap().name(), "reviewer");
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
    async fn a_runs_exec_environment_is_readable_for_the_life_of_the_run() {
        // `lns exec` enters the same guest later and has to compose the same env, the same PATH and the same tool vars.
        let id = allocate_run_id();
        let (mut h, _rx) = make_handle();
        h.exec_environment = exec_environment_fixture();
        register_named(id.clone(), None, h).unwrap();
        assert_eq!(exec_environment(&id), exec_environment_fixture());
        assert_eq!(exec_environment("ghost"), Default::default());
        deregister(&id);
        assert_eq!(exec_environment(&id), Default::default());
    }

    fn exec_environment_fixture() -> ExecEnvironment {
        ExecEnvironment {
            session_env: vec!["HOME=/workspace".to_string()],
            tools: tool_runtime(),
            placeholders: vec![(
                "SOME_TOKEN".to_string(),
                "some-provider_LNSPLACEHOLDER0000".to_string(),
            )],
            workdir: Some("/workspace".to_string()),
            declared_identity_keys: vec!["HOME".to_string()],
        }
    }

    fn tool_runtime() -> crate::workload_env::ToolRuntime {
        crate::workload_env::ToolRuntime {
            bin_paths: vec!["/.lens/tools/some-tool/1.2.3/bin".to_string()],
            env: vec![(
                "SOME_TOOL_HOME".to_string(),
                "/.lens/tools/some-tool/1.2.3/home".to_string(),
            )],
        }
    }

    #[tokio::test]
    #[serial_test::serial(global_runs)]
    async fn a_run_never_becomes_exec_able_before_its_environment_is_readable() {
        // The connector is the gate `lns exec` passes; publishing it a moment before the environment would let an exec in that window run without it and with no error.
        let id = allocate_run_id();
        let (h, _rx) = make_handle();
        register_named(id.clone(), None, h).unwrap();
        set_connector_with_environment(
            &id,
            std::sync::Arc::new(StubTransport),
            exec_environment_fixture(),
        );
        assert!(connector(&id).is_some());
        assert_eq!(exec_environment(&id), exec_environment_fixture());
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
    async fn two_exec_sessions_on_one_run_route_input_independently() {
        let run_id = allocate_run_id();
        let (mut handle, _cancel_rx) = make_handle();
        let (primary_tx, mut primary_rx) = mpsc::channel::<SessionInput>(1);
        handle.input_tx = Some(primary_tx);
        register(run_id.clone(), handle);

        let (first_tx, mut first_rx) = mpsc::channel::<SessionInput>(1);
        let (second_tx, mut second_rx) = mpsc::channel::<SessionInput>(1);
        register_exec_session(&run_id, "exec-1".to_string(), first_tx);
        register_exec_session(&run_id, "exec-2".to_string(), second_tx);

        let target = lns_ipc::SessionTarget::Exec {
            run_id: run_id.clone(),
            session_id: "exec-1".to_string(),
        };
        session_input_sender(&target)
            .expect("the first exec session should be addressable")
            .send(SessionInput::StdinBytes(b"first only".to_vec()))
            .await
            .expect("the first exec session should accept input");

        assert!(matches!(
            first_rx.recv().await,
            Some(SessionInput::StdinBytes(bytes)) if bytes == b"first only"
        ));
        assert!(primary_rx.try_recv().is_err());
        assert!(second_rx.try_recv().is_err());

        deregister(&run_id);
    }

    #[tokio::test]
    async fn registering_an_exec_session_for_a_missing_run_is_refused() {
        let (input_tx, _input_rx) = mpsc::channel::<SessionInput>(1);
        assert!(!register_exec_session(
            "missing-run",
            "exec-1".to_string(),
            input_tx,
        ));
    }

    #[tokio::test]
    async fn deregistering_one_exec_session_leaves_the_primary_and_sibling_addressable() {
        let run_id = allocate_run_id();
        let (mut handle, _cancel_rx) = make_handle();
        let (primary_tx, _primary_rx) = mpsc::channel::<SessionInput>(1);
        handle.input_tx = Some(primary_tx);
        register(run_id.clone(), handle);

        let (first_tx, _first_rx) = mpsc::channel::<SessionInput>(1);
        let (second_tx, _second_rx) = mpsc::channel::<SessionInput>(1);
        assert!(register_exec_session(
            &run_id,
            "exec-1".to_string(),
            first_tx
        ));
        assert!(register_exec_session(
            &run_id,
            "exec-2".to_string(),
            second_tx
        ));

        assert!(deregister_exec_session(&run_id, "exec-1"));
        assert!(
            session_input_sender(&lns_ipc::SessionTarget::Exec {
                run_id: run_id.clone(),
                session_id: "exec-1".to_string(),
            })
            .is_none()
        );
        assert!(
            session_input_sender(&lns_ipc::SessionTarget::Exec {
                run_id: run_id.clone(),
                session_id: "exec-2".to_string(),
            })
            .is_some()
        );
        assert!(
            session_input_sender(&lns_ipc::SessionTarget::Primary {
                run_id: run_id.clone(),
            })
            .is_some()
        );

        deregister(&run_id);
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

        assert!(matches!(remove_if_exited(&id), RemoveOutcome::Removed(_)));
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
    async fn set_resolved_size_updates_the_inspected_config() {
        let id = allocate_run_id();
        let (mut handle, _rx) = make_handle();
        handle.config.cpus = 1;
        handle.config.mem_mib = 512;
        register(id.clone(), handle);

        set_resolved_size(&id, 4, 2048);
        let details = inspect(&id).expect("registered run must be inspectable");
        assert_eq!(details.config.cpus, 4);
        assert_eq!(details.config.mem_mib, 2048);

        set_resolved_size("deadbeef00000000000000000000000f", 8, 8);

        deregister(&id);
    }

    #[tokio::test]
    async fn set_resolved_command_and_env_updates_the_inspected_launch_config() {
        let id = allocate_run_id();
        let (mut handle, _rx) = make_handle();
        handle.command = "request-command".to_string();
        handle.config.env = vec!["REQUEST_ENV=old".to_string()];
        register(id.clone(), handle);

        set_resolved_command_and_env(
            &id,
            &["agent".to_string(), "serve".to_string()],
            &["MODE=production".to_string(), "PORT=4000".to_string()],
        );
        let details = inspect(&id).expect("registered run must be inspectable");
        assert_eq!(details.summary.command, "agent serve");
        assert_eq!(
            details.config.env,
            vec!["MODE=production".to_string(), "PORT=4000".to_string()]
        );

        set_resolved_command_and_env(
            "deadbeef00000000000000000000000f",
            &["ignored".to_string()],
            &["IGNORED=1".to_string()],
        );

        deregister(&id);
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
        register(id.clone(), handle);

        let row = snapshot()
            .into_iter()
            .find(|s| s.id == *id)
            .expect("registered handle must appear in snapshot");
        assert_eq!(row.image, "alpine:latest");
        assert_eq!(row.command, "sleep 1");
        assert_eq!(row.started, "2026-05-21 10:00:00");
        assert_eq!(row.status, RunStatus::Running);

        deregister(&id);
    }

    fn set_status(map: &HashMap<String, RunEntry>, id: &str, status: RunStatus) {
        *map.get(id)
            .unwrap()
            .as_live()
            .unwrap()
            .status
            .lock()
            .unwrap() = status;
    }

    #[tokio::test]
    async fn remove_if_exited_from_drops_an_exited_entry_but_refuses_a_running_one() {
        let mut map = HashMap::new();
        let (handle, _rx) = make_handle();
        map.insert("aa01".to_string(), RunEntry::Live(handle));

        assert!(matches!(
            remove_if_exited_from(Some(&mut map), "aa01"),
            RemoveOutcome::Running
        ));
        assert!(
            map.contains_key("aa01"),
            "a running run must not be removed"
        );

        set_status(&map, "aa01", RunStatus::Exited { code: 0 });
        assert!(matches!(
            remove_if_exited_from(Some(&mut map), "aa01"),
            RemoveOutcome::Removed(_)
        ));
        assert!(!map.contains_key("aa01"), "an exited run must be removed");
    }

    #[test]
    fn remove_if_exited_from_reports_not_found_for_absent_id_and_empty_registry() {
        assert!(matches!(
            remove_if_exited_from(None, "aa01"),
            RemoveOutcome::NotFound
        ));
        let mut empty: HashMap<String, RunEntry> = HashMap::new();
        assert!(matches!(
            remove_if_exited_from(Some(&mut empty), "aa01"),
            RemoveOutcome::NotFound
        ));
    }

    #[tokio::test]
    async fn prune_exited_from_removes_every_exited_run_and_keeps_running_ones() {
        let mut map = HashMap::new();
        for id in ["aa10", "aa11", "aa12"] {
            let (handle, _rx) = make_handle();
            map.insert(id.to_string(), RunEntry::Live(handle));
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

        assert!(matches!(remove_if_exited(&id), RemoveOutcome::Removed(_)));
        assert_eq!(status(&id), None);
    }

    #[tokio::test]
    async fn remove_if_exited_refuses_a_running_run_in_the_live_registry() {
        let id = allocate_run_id();
        let (handle, _rx) = make_handle();
        register(id.clone(), handle);

        assert!(matches!(remove_if_exited(&id), RemoveOutcome::Running));
        assert_eq!(status(&id), Some(RunStatus::Running));

        deregister(&id);
    }

    #[tokio::test]
    async fn remove_if_exited_reports_not_found_for_an_unknown_run() {
        let id = "deadbeef000000000000000000000011".to_string();
        assert!(matches!(remove_if_exited(&id), RemoveOutcome::NotFound));
    }

    fn stopped_record(id: &str, name: &str) -> crate::run_record::RunRecord {
        let mut record = crate::run_record::test_record(id);
        record.name = name.to_string();
        record.exit_code = Some(3);
        record.finished_at = Some("2026-08-18T00:01:00Z".into());
        record
    }

    #[test]
    fn a_stopped_entry_resolves_by_name_and_id_like_a_live_one() {
        let mut map = HashMap::new();
        map.insert(
            "1a2b3c4d0000000000000000000000aa".to_string(),
            RunEntry::Stopped(StoppedRun {
                record: stopped_record("1a2b3c4d0000000000000000000000aa", "reviewer"),
            }),
        );
        assert_eq!(
            resolve_in(Some(&map), "reviewer"),
            Ok("1a2b3c4d0000000000000000000000aa".to_string())
        );
        assert_eq!(
            resolve_in(Some(&map), "1a2b"),
            Ok("1a2b3c4d0000000000000000000000aa".to_string())
        );
    }

    #[tokio::test]
    async fn a_stopped_entry_keeps_its_name_reserved_against_new_runs() {
        let mut map = HashMap::new();
        map.insert(
            "aa07".to_string(),
            RunEntry::Stopped(StoppedRun {
                record: stopped_record("aa07", "reviewer"),
            }),
        );
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

    #[test]
    fn a_stopped_entry_lists_as_exited_with_its_recorded_launch_facts() {
        let mut map = HashMap::new();
        map.insert(
            "aa07".to_string(),
            RunEntry::Stopped(StoppedRun {
                record: stopped_record("aa07", "reviewer"),
            }),
        );
        let rows = snapshot_from(Some(&map));
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.name, "reviewer");
        assert_eq!(row.status, RunStatus::Exited { code: 3 });
        assert_eq!(row.image, "registry.example.test/some-sandbox:1");
        assert_eq!(row.command, "sh -c true");
        assert_eq!(row.started, "2026-08-18T00:00:00Z");
    }

    #[test]
    fn a_stopped_entry_is_removable_and_prunable() {
        let mut map = HashMap::new();
        map.insert(
            "aa07".to_string(),
            RunEntry::Stopped(StoppedRun {
                record: stopped_record("aa07", "reviewer"),
            }),
        );
        assert!(matches!(
            remove_if_exited_from(Some(&mut map), "aa07"),
            RemoveOutcome::Removed(_)
        ));
        map.insert(
            "aa08".to_string(),
            RunEntry::Stopped(StoppedRun {
                record: stopped_record("aa08", "builder"),
            }),
        );
        assert_eq!(prune_exited_from(Some(&mut map)), vec!["aa08".to_string()]);
    }

    #[tokio::test]
    #[serial_test::serial(global_runs)]
    async fn a_stopped_entry_has_no_session_to_cancel_detach_or_feed() {
        let id = allocate_run_id();
        register_stopped(StoppedRun {
            record: stopped_record(&id, &format!("rev-{id}")),
        });
        assert!(!cancel(&id), "cancel of a stopped run is a no-op");
        assert!(
            status(&id).is_some(),
            "a failed cancel must not remove the stopped run"
        );
        assert_eq!(request_detach(&id), DetachOutcome::NotAttached);
        assert!(input_sender(&id).is_none());
        assert!(log_buffer(&id).is_none());
        assert!(connector(&id).is_none());
        deregister(&id);
    }

    #[tokio::test]
    #[serial_test::serial(global_runs)]
    async fn transition_to_live_boots_a_stopped_entry_under_its_reserved_name() {
        let id = allocate_run_id();
        register_stopped(StoppedRun {
            record: stopped_record(&id, &format!("rev-{id}")),
        });
        let (h, _rx) = make_handle();
        let name = transition_to_live(&id, h).unwrap();
        assert_eq!(name, format!("rev-{id}"));
        assert_eq!(status(&id), Some(RunStatus::Running));
        deregister(&id);
    }

    #[tokio::test]
    #[serial_test::serial(global_runs)]
    async fn transition_to_live_replaces_a_run_that_exited_in_this_session() {
        let id = allocate_run_id();
        let (h, _rx) = make_handle();
        register_named(id.clone(), Some(format!("rev-{id}")), h).unwrap();
        set_exit_code(&id, 0);
        let (fresh, _rx2) = make_handle();
        let name = transition_to_live(&id, fresh).unwrap();
        assert_eq!(name, format!("rev-{id}"));
        assert_eq!(status(&id), Some(RunStatus::Running));
        deregister(&id);
    }

    #[tokio::test]
    #[serial_test::serial(global_runs)]
    async fn transition_to_live_refuses_a_running_run_and_an_unknown_one() {
        let id = allocate_run_id();
        let (h, _rx) = make_handle();
        register_named(id.clone(), Some(format!("rev-{id}")), h).unwrap();
        let (fresh, _rx2) = make_handle();
        assert!(
            transition_to_live(&id, fresh)
                .unwrap_err()
                .contains("already running")
        );
        deregister(&id);
        let (fresh2, _rx3) = make_handle();
        assert!(
            transition_to_live(&id, fresh2)
                .unwrap_err()
                .contains("no such run")
        );
    }

    #[test]
    fn stopped_names_in_an_uninitialised_registry_is_empty() {
        assert!(stopped_names_in(None).is_empty());
    }

    #[tokio::test]
    async fn a_stopped_run_can_be_renamed_and_inspected_but_not_mutated_live() {
        let mut map = HashMap::new();
        map.insert(
            "aa07".to_string(),
            RunEntry::Stopped(StoppedRun {
                record: stopped_record("aa07", "reviewer"),
            }),
        );
        rename_in(Some(&mut map), "reviewer", "auditor").unwrap();
        assert_eq!(map.get("aa07").unwrap().name(), "auditor");
        assert!(map.get_mut("aa07").unwrap().as_live_mut().is_none());
    }

    #[tokio::test]
    #[serial_test::serial(global_runs)]
    async fn inspect_of_a_stopped_run_reports_its_recorded_launch_config() {
        let id = allocate_run_id();
        let mut record = stopped_record(&id, &format!("rev-{id}"));
        record.args.cpus = 7;
        register_stopped(StoppedRun { record });
        let details = inspect(&id).expect("a stopped run is inspectable");
        assert_eq!(details.config.cpus, 7);
        assert_eq!(details.summary.status, RunStatus::Exited { code: 3 });
        set_connector_with_environment(
            &id,
            std::sync::Arc::new(StubTransport),
            exec_environment_fixture(),
        );
        assert!(
            connector(&id).is_none(),
            "a stopped run has no live handle to hang a connector on"
        );
        deregister(&id);
    }

    #[tokio::test]
    #[serial_test::serial(global_runs)]
    async fn resolve_status_answers_handle_and_state_in_one_step() {
        let id = allocate_run_id();
        register_stopped(StoppedRun {
            record: stopped_record(&id, &format!("rev-{id}")),
        });
        let (resolved, status) = resolve_status(&format!("rev-{id}")).unwrap();
        assert_eq!(resolved, id);
        assert_eq!(status, RunStatus::Exited { code: 3 });
        deregister(&id);
        assert!(resolve_status(&id).unwrap_err().contains("no such run"));
    }

    #[tokio::test]
    #[serial_test::serial(global_runs)]
    async fn rebuild_from_records_populates_the_live_registry() {
        let id = allocate_run_id();
        rebuild_from_records(vec![stopped_record(&id, &format!("rev-{id}"))]);
        assert_eq!(resolve(&format!("rev-{id}")), Ok(id.clone()));
        deregister(&id);
    }

    #[tokio::test]
    #[serial_test::serial(global_runs)]
    async fn cancel_of_a_stopped_run_is_a_no_op_that_keeps_the_entry() {
        let id = allocate_run_id();
        register_stopped(StoppedRun {
            record: stopped_record(&id, &format!("rev-{id}")),
        });
        assert!(!cancel(&id));
        assert!(
            status(&id).is_some(),
            "cancel must not remove a stopped run"
        );
        deregister(&id);
    }

    #[test]
    fn rebuild_in_lists_every_recorded_run_as_stopped() {
        let mut map = HashMap::new();
        rebuild_in(
            &mut map,
            vec![
                stopped_record("aa07", "reviewer"),
                stopped_record("aa08", "builder"),
            ],
        );
        assert_eq!(
            resolve_in(Some(&map), "reviewer"),
            Ok("aa07".to_string()),
            "a rebuilt run resolves by its recorded name"
        );
        assert_eq!(
            stopped_names_in(Some(&map)),
            vec!["builder".to_string(), "reviewer".to_string()]
        );
    }

    #[tokio::test]
    async fn rebuild_in_never_displaces_a_listed_run() {
        let mut map = HashMap::new();
        named_handle(&mut map, "aa07", "reviewer").await;
        rebuild_in(&mut map, vec![stopped_record("aa07", "stale-name")]);
        assert_eq!(map.get("aa07").unwrap().name(), "reviewer");
        assert!(map.get("aa07").unwrap().as_live().is_some());
    }

    #[tokio::test]
    async fn stopped_run_names_lists_every_startable_run_sorted() {
        let mut map = HashMap::new();
        map.insert(
            "aa07".to_string(),
            RunEntry::Stopped(StoppedRun {
                record: stopped_record("aa07", "reviewer"),
            }),
        );
        let (live, _rx) = make_handle();
        let mut live = RunEntry::Live(live);
        live.set_name("runner".to_string());
        map.insert("aa08".to_string(), live);
        let (exited, _rx2) = make_handle();
        let mut exited = RunEntry::Live(exited);
        exited.set_name("builder".to_string());
        map.insert("aa09".to_string(), exited);
        set_status(&map, "aa09", RunStatus::Exited { code: 0 });
        assert_eq!(
            stopped_names_in(Some(&map)),
            vec!["builder".to_string(), "reviewer".to_string()],
            "exited runs are startable, live ones are not"
        );
    }
}
