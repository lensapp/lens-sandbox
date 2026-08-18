use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use cucumber::{given, then, when};
use lns_ipc::{Response, RunStatus};
use lns_service::ipc::{StartHost, StartOptions};
use lns_service::run_record::RunRecord;
use lns_service::run_registry;
use tokio::io::AsyncWriteExt;

use crate::steps::run_lifecycle::fresh_handle;
use crate::world::BehaviourWorld;

/// A start host scripted with what `preflight` refuses, recording whether the boot was reached and with what.
struct ScriptedStartHost {
    preflight_refusal: Option<String>,
    record: Option<RunRecord>,
    served: Arc<AtomicBool>,
    booted: std::sync::Arc<std::sync::Mutex<Option<RunRecord>>>,
}

impl StartHost for ScriptedStartHost {
    async fn record(&self, run_id: &str) -> anyhow::Result<RunRecord> {
        Ok(self
            .record
            .clone()
            .unwrap_or_else(|| stopped_record(run_id, "recorded")))
    }

    async fn preflight(&self, _record: &RunRecord) -> anyhow::Result<()> {
        match &self.preflight_refusal {
            Some(message) => Err(anyhow::anyhow!("{message}")),
            None => Ok(()),
        }
    }

    async fn serve<S>(
        &self,
        _stream: &mut S,
        record: RunRecord,
        _options: StartOptions,
    ) -> anyhow::Result<()>
    where
        S: tokio::io::AsyncReadExt + AsyncWriteExt + Unpin + Send,
    {
        self.served.store(true, Ordering::SeqCst);
        *self.booted.lock().unwrap() = Some(record);
        Ok(())
    }
}

pub(crate) fn stopped_record(run_id: &str, name: &str) -> RunRecord {
    RunRecord {
        version: lns_service::run_record::CURRENT_VERSION,
        run_id: run_id.to_string(),
        name: name.to_string(),
        args: sample_args(),
        descriptor_sha256: "d".repeat(64),
        layer_digests: vec!["sha256:aaaa".into()],
        image: "registry.example.test/some-sandbox:1".into(),
        command: "sh -c true".into(),
        created_at: "2026-08-18T00:00:00Z".into(),
        finished_at: Some("2026-08-18T00:01:00Z".into()),
        exit_code: Some(0),
    }
}

fn sample_args() -> lns_ipc::RunImageArgs {
    lns_ipc::RunImageArgs {
        image: Some("registry.example.test/some-sandbox:1".into()),
        resolved_image: None,
        mixins: Vec::new(),
        composed_mixins: Vec::new(),
        name: None,
        cpus: 1,
        mem: 0,
        cpus_explicit: false,
        mem_explicit: false,
        policy_path: None,
        sandbox_user: None,
        sandbox_uid: None,
        entrypoint: None,
        hostname: None,
        cmd: vec!["sh".into(), "-c".into(), "true".into()],
        env: Vec::new(),
        workdir: None,
        debug: false,
        tty: false,
        stdin: false,
        initial_winsize: None,
        detached: true,
        published_ports: Vec::new(),
        volumes: Vec::new(),
        binds: Vec::new(),
        auto_remove: false,
        verify_sandbox: false,
        definition: None,
        definition_dir: None,
        authored_egress: None,
        packed_filesets: Vec::new(),
    }
}

pub(crate) async fn hold_serial(w: &mut BehaviourWorld) {
    if w.startrun_serial.is_none() {
        static SERIAL: std::sync::OnceLock<Arc<tokio::sync::Mutex<()>>> =
            std::sync::OnceLock::new();
        let lock = SERIAL
            .get_or_init(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();
        w.startrun_serial = Some(lock.lock_owned().await);
    }
}

pub(crate) fn clear_name(name: &str) {
    if let Ok(id) = run_registry::resolve(name) {
        run_registry::deregister(&id);
    }
}

pub(crate) fn register_stopped_named(w: &mut BehaviourWorld, name: &str) -> String {
    clear_name(name);
    let id = run_registry::allocate_run_id();
    run_registry::register_stopped(lns_service::run_registry::StoppedRun {
        record: stopped_record(&id, name),
    });
    w.startrun_cleanup.push(id.clone());
    id
}

#[given(regex = r#"^a run named "([^"]+)" that is running$"#)]
async fn a_running_run(w: &mut BehaviourWorld, name: String) {
    hold_serial(w).await;
    clear_name(&name);
    let id = run_registry::allocate_run_id();
    run_registry::register_named(
        id.clone(),
        Some(name.clone()),
        fresh_handle("some-image", lns_ipc::RunConfig::default()),
    )
    .expect("the scenario's running run registers");
    w.startrun_cleanup.push(id);
    w.startrun_target = Some(name);
}

#[given(regex = r#"^stopped runs named "([^"]+)" and "([^"]+)"$"#)]
async fn two_stopped_runs(w: &mut BehaviourWorld, first: String, second: String) {
    hold_serial(w).await;
    register_stopped_named(w, &first);
    register_stopped_named(w, &second);
}

// The lock is the fixture: it keeps sibling scenarios' stopped runs out of the global listing.
#[given("no stopped runs")]
async fn no_stopped_runs(w: &mut BehaviourWorld) {
    hold_serial(w).await;
}

#[given("a stopped run that published host port 8080")]
async fn a_stopped_run_with_a_port(w: &mut BehaviourWorld) {
    hold_serial(w).await;
    let id = register_stopped_named(w, "port-holder");
    w.startrun_target = Some(id);
}

#[given("another process now holds host port 8080")]
fn the_port_is_taken(w: &mut BehaviourWorld) {
    w.startrun_refusal = Some("host port 8080 is already in use".into());
}

#[given(regex = r#"^a stopped run that used volume "([^"]+)"$"#)]
async fn a_stopped_run_with_a_volume(w: &mut BehaviourWorld, _volume: String) {
    hold_serial(w).await;
    let id = register_stopped_named(w, "volume-user");
    w.startrun_target = Some(id);
}

#[given(regex = r#"^a running run currently holds volume "([^"]+)"$"#)]
fn a_running_run_holds_the_volume(w: &mut BehaviourWorld, volume: String) {
    clear_name("volume-holder");
    let id = run_registry::allocate_run_id();
    run_registry::register_named(
        id.clone(),
        Some("volume-holder".into()),
        fresh_handle("some-image", lns_ipc::RunConfig::default()),
    )
    .expect("the scenario's holder registers");
    w.startrun_cleanup.push(id.clone());
    let holder = lns_ipc::short_run_id(&id).to_string();
    w.startrun_refusal = Some(format!(
        "volume {volume:?} is held by run {holder}; stop or remove it first"
    ));
    w.startrun_volume_holder = Some(holder);
}

#[given("a stopped run with a bind whose host directory no longer exists")]
async fn a_stopped_run_with_a_dead_bind(w: &mut BehaviourWorld) {
    hold_serial(w).await;
    let id = register_stopped_named(w, "bind-user");
    w.startrun_target = Some(id);
    w.startrun_refusal =
        Some("bind source /host/dir/that-vanished no longer exists on the host".into());
}

#[given("a stopped run whose upper.img has been deleted from its run dir")]
async fn a_stopped_run_with_no_upper(w: &mut BehaviourWorld) {
    hold_serial(w).await;
    let id = register_stopped_named(w, "damaged");
    w.startrun_target = Some(id);
    w.startrun_refusal =
        Some("run damaged's state is damaged: its writable layer is missing".into());
}

#[given("a stopped run started from an lns.yaml that has since changed")]
async fn a_stopped_run_from_an_edited_yaml(w: &mut BehaviourWorld) {
    hold_serial(w).await;
    let id = register_stopped_named(w, "yaml-frozen");
    let mut record = stopped_record(&id, "yaml-frozen");
    record.args.env = vec!["FROZEN=at-launch".into()];
    record.args.cmd = vec!["agent".into(), "serve".into()];
    w.startrun_target = Some(id);
    w.startrun_record = Some(record);
}

#[then("it boots with the recorded image, command, env, mounts, and ports")]
fn it_boots_with_the_recorded_config(w: &mut BehaviourWorld) -> Result<(), String> {
    let recorded = w
        .startrun_record
        .as_ref()
        .expect("the scenario recorded a launch");
    let booted = w.startrun_booted.lock().unwrap();
    let booted = booted
        .as_ref()
        .ok_or("the run never reached its boot".to_string())?;
    if booted.args == recorded.args && booted.descriptor_sha256 == recorded.descriptor_sha256 {
        Ok(())
    } else {
        Err("a restart boots exactly what the record says, nothing else".into())
    }
}

#[then("the changed lns.yaml has no effect on it")]
fn the_changed_yaml_has_no_effect(w: &mut BehaviourWorld) -> Result<(), String> {
    let booted = w.startrun_booted.lock().unwrap();
    let booted = booted
        .as_ref()
        .ok_or("the run never reached its boot".to_string())?;
    if booted.args.env == vec!["FROZEN=at-launch".to_string()] {
        Ok(())
    } else {
        Err(format!(
            "the record is the only config source at start; booted env: {:?}",
            booted.args.env
        ))
    }
}

#[when(regex = r#"^I run "lns start ([^"]+)"$"#)]
async fn i_run_lns_start(w: &mut BehaviourWorld, handle: String) {
    drive_start_stopped(w, &handle).await;
}

#[when("I start it")]
async fn i_start_it(w: &mut BehaviourWorld) {
    let target = w
        .startrun_target
        .clone()
        .expect("the scenario registered a target run");
    drive_start_stopped(w, &target).await;
}

async fn drive_start_stopped(w: &mut BehaviourWorld, handle: &str) {
    hold_serial(w).await;
    let served = Arc::new(AtomicBool::new(false));
    let host = ScriptedStartHost {
        preflight_refusal: w.startrun_refusal.clone(),
        record: w.startrun_record.clone(),
        served: served.clone(),
        booted: w.startrun_booted.clone(),
    };
    let (mut client, mut server) = tokio::io::duplex(64 * 1024);
    lns_service::ipc::start_stopped_run(&mut server, handle, &host, StartOptions::default())
        .await
        .expect("a refused start is an answer, not a transport failure");
    drop(server);

    w.startrun_served = served.load(Ordering::SeqCst);
    w.startrun_status_after = run_registry::resolve(handle)
        .ok()
        .and_then(|id| run_registry::status(&id));
    let mut frames = Vec::new();
    while let Ok(bytes) = lns_ipc::read_frame_bytes_async(&mut client).await {
        if let Ok(response) = lns_ipc::decode_frame::<Response, _>(&mut bytes.as_slice()) {
            frames.push(response);
        }
    }
    w.startrun_frames = frames;
    for id in w.startrun_cleanup.drain(..) {
        run_registry::deregister(&id);
    }
    w.startrun_serial = None;
}

fn first_error(w: &BehaviourWorld) -> Result<&str, String> {
    match w.startrun_frames.first() {
        Some(Response::Error { message }) => Ok(message),
        other => Err(format!(
            "expected a refusal the client can read, got {other:?}"
        )),
    }
}

#[then("it exits 0")]
fn it_exits_zero(w: &mut BehaviourWorld) -> Result<(), String> {
    if let Some(resp) = &w.response {
        return match resp {
            Response::Error { message } => Err(format!("expected success, got error: {message}")),
            _ => Ok(()),
        };
    }
    match w.startrun_frames.first() {
        Some(Response::RunStarted { .. }) => Ok(()),
        other => Err(format!("expected RunStarted, got {other:?}")),
    }
}

#[then("the run is unchanged")]
fn the_run_is_unchanged(w: &mut BehaviourWorld) -> Result<(), String> {
    if w.startrun_served {
        return Err("an already-running run must not boot again".into());
    }
    match w.startrun_status_after {
        Some(RunStatus::Running) => Ok(()),
        other => Err(format!("the run must stay running, got {other:?}")),
    }
}

#[then("it exits 1")]
fn it_exits_one(w: &mut BehaviourWorld) -> Result<(), String> {
    if let Some(resp) = &w.response {
        return match resp {
            Response::Error { .. } => Ok(()),
            other => Err(format!("expected an error, got {other:?}")),
        };
    }
    first_error(w).map(|_| ())
}

#[then(regex = r#"^the error names "([^"]+)" and "([^"]+)" as the stopped runs that exist$"#)]
fn the_error_lists_stopped_runs(
    w: &mut BehaviourWorld,
    first: String,
    second: String,
) -> Result<(), String> {
    let message = first_error(w)?;
    if message.contains(&first) && message.contains(&second) {
        Ok(())
    } else {
        Err(format!(
            "the error is the only place a user discovers what is startable; got: {message}"
        ))
    }
}

#[then("the error says there are no stopped runs")]
fn the_error_says_no_stopped_runs(w: &mut BehaviourWorld) -> Result<(), String> {
    let message = first_error(w)?;
    if message.contains("no stopped runs") {
        Ok(())
    } else {
        Err(format!(
            "expected the no-stopped-runs answer, got: {message}"
        ))
    }
}

#[then("it exits non-zero with an error naming port 8080")]
fn error_names_the_port(w: &mut BehaviourWorld) -> Result<(), String> {
    let message = first_error(w)?;
    if message.contains("8080") {
        Ok(())
    } else {
        Err(format!("the conflict must name the port, got: {message}"))
    }
}

#[then("the run remains stopped with its state untouched")]
fn remains_stopped_untouched(w: &mut BehaviourWorld) -> Result<(), String> {
    remains_stopped(w)
}

#[then("the run remains stopped")]
fn remains_stopped(w: &mut BehaviourWorld) -> Result<(), String> {
    if w.startrun_served {
        return Err("a refused start must not boot anything".into());
    }
    match w.startrun_status_after {
        Some(RunStatus::Exited { .. }) => Ok(()),
        other => Err(format!("the run must stay stopped, got {other:?}")),
    }
}

#[then(regex = r#"^it exits non-zero with an error naming "([^"]+)" and its holder$"#)]
fn error_names_volume_and_holder(w: &mut BehaviourWorld, volume: String) -> Result<(), String> {
    let holder = w
        .startrun_volume_holder
        .clone()
        .expect("the scenario recorded the holder");
    let message = first_error(w)?;
    if message.contains(&volume) && message.contains(&holder) {
        Ok(())
    } else {
        Err(format!(
            "the conflict must name the volume and who holds it, got: {message}"
        ))
    }
}

#[then("it exits non-zero with an error naming the missing path")]
fn error_names_the_missing_path(w: &mut BehaviourWorld) -> Result<(), String> {
    let message = first_error(w)?;
    if message.contains("/host/dir/that-vanished") {
        Ok(())
    } else {
        Err(format!("the conflict must name the path, got: {message}"))
    }
}

#[then("it exits non-zero with an error saying the run's state is damaged")]
fn error_says_state_is_damaged(w: &mut BehaviourWorld) -> Result<(), String> {
    let message = first_error(w)?;
    if message.contains("state is damaged") {
        Ok(())
    } else {
        Err(format!("expected the damaged-state answer, got: {message}"))
    }
}
