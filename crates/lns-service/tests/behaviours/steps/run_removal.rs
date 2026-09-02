use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use cucumber::{given, then, when};
use lns_ipc::{Response, RunStatus};
use lns_service::run_registry;

use crate::steps::run_lifecycle::fresh_handle;
use crate::steps::run_start_stopped::{
    clear_name, hold_serial, register_stopped_named, stopped_record,
};
use crate::world::BehaviourWorld;

const CACHE_ROOT: &str = "/cache";

struct FakeRemover {
    calls: Arc<Mutex<Vec<PathBuf>>>,
}

impl lns_service::run::RemoveDir for FakeRemover {
    fn remove_dir_all(&self, dir: &Path) -> std::io::Result<()> {
        self.calls.lock().unwrap().push(dir.to_path_buf());
        Ok(())
    }
}

fn remover(w: &BehaviourWorld) -> FakeRemover {
    FakeRemover {
        calls: w.rm_reclaimed.clone(),
    }
}

struct RecordFs {
    files: Mutex<std::collections::HashMap<PathBuf, Vec<u8>>>,
    run_dirs: Vec<PathBuf>,
}

impl RecordFs {
    fn empty() -> Self {
        Self {
            files: Mutex::new(Default::default()),
            run_dirs: Vec::new(),
        }
    }
}

impl lns_service::image_store::Fs for RecordFs {
    async fn read_dir(&self, _dir: &Path) -> std::io::Result<Vec<PathBuf>> {
        if self.run_dirs.is_empty() {
            Err(std::io::Error::from(std::io::ErrorKind::NotFound))
        } else {
            Ok(self.run_dirs.clone())
        }
    }

    async fn read(&self, p: &Path) -> std::io::Result<Vec<u8>> {
        self.files
            .lock()
            .unwrap()
            .get(p)
            .cloned()
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))
    }

    async fn write(&self, p: &Path, bytes: &[u8]) -> std::io::Result<()> {
        self.files
            .lock()
            .unwrap()
            .insert(p.to_path_buf(), bytes.to_vec());
        Ok(())
    }

    async fn remove_file(&self, p: &Path) -> std::io::Result<()> {
        self.files
            .lock()
            .unwrap()
            .remove(p)
            .map(|_| ())
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))
    }
}

async fn drive_remove(w: &mut BehaviourWorld, handle: &str, force: bool) {
    hold_serial(w).await;
    let fake = remover(w);
    w.response = Some(
        lns_service::ipc::remove_run_with(
            handle,
            force,
            &fake,
            Path::new(CACHE_ROOT),
            |id, _sig| async move {
                run_registry::set_exit_code(&id, 137);
                Response::Acknowledged
            },
            |_, _| {},
        )
        .await,
    );
    w.startrun_status_after = run_registry::resolve(handle)
        .ok()
        .and_then(|id| run_registry::status(&id));
    for id in w.startrun_cleanup.drain(..) {
        run_registry::deregister(&id);
    }
    w.startrun_serial = None;
}

#[given(regex = r#"^a stopped run named "([^"]+)"$"#)]
async fn a_stopped_run_named(w: &mut BehaviourWorld, name: String) {
    hold_serial(w).await;
    let id = register_stopped_named(w, &name);
    w.startrun_target = Some(id);
}

#[given(regex = r#"^a running run named "([^"]+)"$"#)]
async fn a_running_run_named(w: &mut BehaviourWorld, name: String) {
    hold_serial(w).await;
    clear_name(&name);
    let id = run_registry::allocate_run_id();
    run_registry::register_named(
        id.clone(),
        Some(name),
        fresh_handle("some-image", lns_ipc::RunConfig::default()),
    )
    .expect("the scenario's running run registers");
    w.startrun_cleanup.push(id.clone());
    w.startrun_target = Some(id);
}

#[when(regex = r#"^I run "lns rm ([^-"][^"]*)"$"#)]
async fn i_run_lns_rm(w: &mut BehaviourWorld, handle: String) {
    drive_remove(w, &handle, false).await;
}

#[when(regex = r#"^I run "lns rm -f ([^"]+)"$"#)]
async fn i_run_lns_rm_force(w: &mut BehaviourWorld, handle: String) {
    drive_remove(w, &handle, true).await;
}

#[when("I remove the run")]
async fn i_remove_the_run(w: &mut BehaviourWorld) {
    let target = w
        .startrun_target
        .clone()
        .expect("the scenario registered a target run");
    drive_remove(w, &target, false).await;
    if let Some(rig) = w.image.as_mut() {
        rig.active
            .retain(|s| Some(&s.id) != w.startrun_target.as_ref());
    }
}

#[then(regex = r#"^it prints "([^"]+)"$"#)]
fn it_prints_the_handle(w: &mut BehaviourWorld, _handle: String) -> Result<(), String> {
    match &w.response {
        Some(Response::Acknowledged) => Ok(()),
        other => Err(format!(
            "the CLI echoes the handle only on an acknowledged removal, got {other:?}"
        )),
    }
}

#[then("the run's dir, record, and writable layer are gone")]
fn the_run_dir_is_gone(w: &mut BehaviourWorld) -> Result<(), String> {
    let id = w.startrun_target.as_ref().expect("target recorded");
    let expected = Path::new(CACHE_ROOT).join("runs").join(id);
    if w.rm_reclaimed.lock().unwrap().contains(&expected) {
        Ok(())
    } else {
        Err(format!(
            "removal owns the whole run dir; reclaimed: {:?}",
            w.rm_reclaimed.lock().unwrap()
        ))
    }
}

#[then(regex = r#"^the name "([^"]+)" is free for a new run$"#)]
fn the_name_is_free(_w: &mut BehaviourWorld, name: String) -> Result<(), String> {
    run_registry::ensure_name_available(&name)
        .map_err(|held| format!("a removed run must release its name: {held}"))
}

#[then("it exits non-zero")]
fn it_exits_non_zero(w: &mut BehaviourWorld) -> Result<(), String> {
    match &w.response {
        Some(Response::Error { .. }) => Ok(()),
        other => Err(format!("expected a refusal, got {other:?}")),
    }
}

#[then(regex = r#"^the error says to stop it first with "([^"]+)" or force with "([^"]+)"$"#)]
fn the_error_hints_stop_or_force(
    w: &mut BehaviourWorld,
    stop_hint: String,
    force_hint: String,
) -> Result<(), String> {
    match &w.response {
        Some(Response::Error { message })
            if message.contains(&stop_hint) && message.contains(&force_hint) =>
        {
            Ok(())
        }
        other => Err(format!(
            "the refusal must hand the user both ways out, got {other:?}"
        )),
    }
}

#[then("the run keeps running")]
fn the_run_keeps_running(w: &mut BehaviourWorld) -> Result<(), String> {
    match w.startrun_status_after {
        Some(RunStatus::Running) => Ok(()),
        other => Err(format!("a refused rm must change nothing, got {other:?}")),
    }
}

#[then("the run is killed and its state removed")]
fn the_run_is_killed_and_removed(w: &mut BehaviourWorld) -> Result<(), String> {
    if w.startrun_status_after.is_some() {
        return Err("the run must leave the registry".into());
    }
    the_run_dir_is_gone(w)
}

#[then(regex = r#"^it exits 1 with an error naming "([^"]+)"$"#)]
fn it_exits_one_naming(w: &mut BehaviourWorld, handle: String) -> Result<(), String> {
    match &w.response {
        Some(Response::Error { message }) if message.contains(&handle) => Ok(()),
        other => Err(format!(
            "expected an error naming {handle:?}, got {other:?}"
        )),
    }
}

#[given("a stopped run holding the only reference to a cached image")]
async fn a_stopped_run_pinning_an_image(w: &mut BehaviourWorld) {
    hold_serial(w).await;
    let id = register_stopped_named(w, "pin-holder");
    w.startrun_target = Some(id.clone());
    let reference = "registry.example.test/pinned-sandbox:1";
    w.image().pull(reference, "sha256:pinned-digest", 64).await;
    w.image().active = vec![lns_ipc::RunSummary {
        id,
        name: "pin-holder".into(),
        image: reference.into(),
        command: "sh -c true".into(),
        status: RunStatus::Exited { code: 0 },
        created: "2026-08-18T00:00:00Z".into(),
        started: "2026-08-18T00:00:00Z".into(),
    }];
}

#[then("the image is removable")]
async fn the_image_is_removable(w: &mut BehaviourWorld) -> Result<(), String> {
    let rig = w.image.as_mut().expect("the scenario cached an image");
    rig.remove("registry.example.test/pinned-sandbox:1").await;
    match (&rig.last_removed, &rig.last_error) {
        (Some(_), None) => Ok(()),
        (_, err) => Err(format!(
            "removing the run must release the image pin, got {err:?}"
        )),
    }
}

#[given(regex = r#"^"lns run( --rm)?" whose workload exits$"#)]
async fn a_run_whose_workload_exits(w: &mut BehaviourWorld, rm_flag: String) {
    hold_serial(w).await;
    let auto_remove = !rm_flag.is_empty();
    clear_name("auto-rm");
    let id = run_registry::allocate_run_id();
    run_registry::register_named(
        id.clone(),
        Some("auto-rm".into()),
        fresh_handle("some-image", lns_ipc::RunConfig::default()),
    )
    .expect("the scenario's run registers");
    w.startrun_cleanup.push(id.clone());
    w.startrun_target = Some(id.clone());
    let fs = RecordFs::empty();
    let record = stopped_record(&id, "auto-rm");
    lns_service::run_record::save_with(&fs, Path::new(CACHE_ROOT), &record)
        .await
        .expect("the record saves");
    run_registry::set_exit_code(&id, 0);
    let fake = remover(w);
    lns_service::run::conclude_run(
        &fs,
        &fake,
        Path::new(CACHE_ROOT),
        &id,
        lns_service::run::RunEnd {
            code: 0,
            auto_remove,
            finished_at: "2026-08-18T00:03:00Z".into(),
        },
        |_| {},
    )
    .await;
    w.startrun_status_after = run_registry::status(&id);
}

#[then("no stopped run remains for it")]
fn no_stopped_run_remains(w: &mut BehaviourWorld) -> Result<(), String> {
    match w.startrun_status_after {
        None => Ok(()),
        other => Err(format!("--rm leaves no listed run behind, got {other:?}")),
    }
}

#[then("its run dir is gone")]
fn its_run_dir_is_gone(w: &mut BehaviourWorld) -> Result<(), String> {
    let out = the_run_dir_is_gone(w);
    cleanup(w);
    out
}

#[then(regex = r#"^a stopped run remains, restartable with "lns start"$"#)]
fn a_stopped_run_remains(w: &mut BehaviourWorld) -> Result<(), String> {
    let out = match w.startrun_status_after {
        Some(RunStatus::Exited { .. }) => Ok(()),
        other => Err(format!(
            "without --rm the run persists as stopped, got {other:?}"
        )),
    };
    cleanup(w);
    out
}

fn cleanup(w: &mut BehaviourWorld) {
    for id in w.startrun_cleanup.drain(..) {
        run_registry::deregister(&id);
    }
    w.startrun_serial = None;
}

#[given("two stopped runs and one running run")]
async fn two_stopped_one_running(w: &mut BehaviourWorld) {
    hold_serial(w).await;
    let a = register_stopped_named(w, "swept-one");
    let b = register_stopped_named(w, "swept-two");
    clear_name("survivor");
    let live = run_registry::allocate_run_id();
    run_registry::register_named(
        live.clone(),
        Some("survivor".into()),
        fresh_handle("some-image", lns_ipc::RunConfig::default()),
    )
    .expect("the scenario's running run registers");
    w.startrun_cleanup.push(live.clone());
    w.prune_stopped = vec![a, b];
    w.startrun_target = Some(live);
}

#[given("a run dir with no run record")]
async fn an_orphan_run_dir(w: &mut BehaviourWorld) {
    hold_serial(w).await;
    w.prune_orphan = Some("0rphan0000000000000000000000000".to_string());
}

#[when(regex = r#"^I run "lns sandbox prune --force"$"#)]
async fn i_run_prune(w: &mut BehaviourWorld) {
    hold_serial(w).await;
    let mut fs = RecordFs::empty();
    if let Some(orphan) = &w.prune_orphan {
        fs.run_dirs = vec![Path::new(CACHE_ROOT).join("runs").join(orphan)];
    }
    let fake = remover(w);
    w.response =
        Some(lns_service::ipc::prune_runs_with(&fs, &fake, Path::new(CACHE_ROOT), |_| {}).await);
    w.startrun_status_after = w
        .startrun_target
        .as_ref()
        .and_then(|id| run_registry::status(id));
    cleanup(w);
}

#[then("both stopped runs are removed")]
fn both_stopped_runs_removed(w: &mut BehaviourWorld) -> Result<(), String> {
    let Some(Response::RunsPruned { removed }) = &w.response else {
        return Err(format!("expected RunsPruned, got {:?}", w.response));
    };
    for id in &w.prune_stopped {
        if !removed.contains(id) {
            return Err(format!("stopped run {id} must be swept; got {removed:?}"));
        }
        let expected = Path::new(CACHE_ROOT).join("runs").join(id);
        if !w.rm_reclaimed.lock().unwrap().contains(&expected) {
            return Err(format!("stopped run {id}'s dir must be reclaimed"));
        }
    }
    Ok(())
}

#[then("the running run is untouched")]
fn the_running_run_is_untouched(w: &mut BehaviourWorld) -> Result<(), String> {
    match w.startrun_status_after {
        Some(RunStatus::Running) => Ok(()),
        other => Err(format!("prune must not touch a running run, got {other:?}")),
    }
}

#[then("the orphaned dir is removed")]
fn the_orphan_is_removed(w: &mut BehaviourWorld) -> Result<(), String> {
    let orphan = w
        .prune_orphan
        .as_ref()
        .expect("the scenario staged an orphan");
    let expected = Path::new(CACHE_ROOT).join("runs").join(orphan);
    if w.rm_reclaimed.lock().unwrap().contains(&expected) {
        Ok(())
    } else {
        Err(format!(
            "an orphan dir is exactly what prune exists for; reclaimed: {:?}",
            w.rm_reclaimed.lock().unwrap()
        ))
    }
}
