use crate::runner::run_one_shot;
use crate::world::BehaviourWorld;
use cucumber::{given, then, when};
use lns_ipc::Request;

pub fn register_run(run_id: String) {
    register_run_with(run_id, "some-image", lns_ipc::RunConfig::default());
}

pub fn fresh_handle(
    image: &str,
    config: lns_ipc::RunConfig,
) -> lns_service::run_registry::RunHandle {
    let (cancel_tx, _cancel_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async {});
    lns_service::run_registry::RunHandle {
        cancel_tx,
        detach_tx: std::sync::Mutex::new(None),
        task,
        input_tx: None,
        connector: None,
        name: String::new(),
        image: image.into(),
        command: "some-command".into(),
        started: "2026-01-01T00:00:00Z".into(),
        status: std::sync::Mutex::new(lns_ipc::RunStatus::Running),
        logs: std::sync::Arc::new(lns_service::run_log::RunLogBuffer::default()),
        config,
        tool_bin_paths: Vec::new(),
    }
}

pub fn register_run_with(run_id: String, image: &str, config: lns_ipc::RunConfig) {
    lns_service::run_registry::register(run_id, fresh_handle(image, config));
}

#[given("a registered run that has already exited")]
async fn registered_exited_run(world: &mut BehaviourWorld) {
    let run_id = lns_service::run_registry::allocate_run_id();
    register_run(run_id.clone());
    lns_service::run_registry::set_exit_code(&run_id, 0);
    world.lifecycle_run = Some(run_id);
}

#[given("a registered run that is still running")]
async fn registered_running_run(world: &mut BehaviourWorld) {
    let run_id = lns_service::run_registry::allocate_run_id();
    register_run(run_id.clone());
    world.lifecycle_run = Some(run_id);
}

#[when(regex = r#"^a RemoveRun request for run (\d+) arrives$"#)]
async fn remove_unknown_run(world: &mut BehaviourWorld, run_id: u32) {
    world.response = Some(
        run_one_shot(
            &Request::RemoveRun {
                run: run_id.to_string(),
            },
            world.started_at(),
        )
        .await,
    );
}

#[when("a RemoveRun request for that run arrives")]
async fn remove_registered_run(world: &mut BehaviourWorld) {
    let run_id = world
        .lifecycle_run
        .clone()
        .expect("a run must be registered first");
    world.response = Some(
        run_one_shot(
            &Request::RemoveRun {
                run: run_id.to_string(),
            },
            world.started_at(),
        )
        .await,
    );
    lns_service::run_registry::deregister(&run_id);
}

#[when(regex = r#"^a StopRun request for run (\d+) arrives$"#)]
async fn stop_unknown_run(world: &mut BehaviourWorld, run_id: u32) {
    world.response = Some(
        run_one_shot(
            &Request::StopRun {
                run: run_id.to_string(),
                timeout_secs: 1,
            },
            world.started_at(),
        )
        .await,
    );
}

#[when("a StopRun request for that run arrives")]
async fn stop_registered_run(world: &mut BehaviourWorld) {
    let run_id = world
        .lifecycle_run
        .clone()
        .expect("a run must be registered first");
    world.response = Some(
        run_one_shot(
            &Request::StopRun {
                run: run_id.to_string(),
                timeout_secs: 1,
            },
            world.started_at(),
        )
        .await,
    );
    lns_service::run_registry::deregister(&run_id);
}

#[then("the response is RunStopped without force")]
fn then_run_stopped_unforced(world: &mut BehaviourWorld) -> Result<(), String> {
    match world.response.as_ref().ok_or("no response captured")? {
        lns_ipc::Response::RunStopped { forced: false } => Ok(()),
        other => Err(format!(
            "expected RunStopped {{ forced: false }}, got {other:?}"
        )),
    }
}

#[given(regex = r#"^a registered run launched from "([^"]+)" with (\d+) cpus and (\d+) MiB$"#)]
async fn registered_run_with_config(
    world: &mut BehaviourWorld,
    image: String,
    cpus: u8,
    mem_mib: usize,
) {
    let run_id = lns_service::run_registry::allocate_run_id();
    register_run_with(
        run_id.clone(),
        &image,
        lns_ipc::RunConfig {
            cpus,
            mem_mib,
            ..Default::default()
        },
    );
    world.lifecycle_run = Some(run_id);
}

#[when(regex = r#"^an InspectRun request for run (\d+) arrives$"#)]
async fn inspect_unknown_run(world: &mut BehaviourWorld, run_id: u32) {
    world.response = Some(
        run_one_shot(
            &Request::InspectRun {
                run: run_id.to_string(),
            },
            world.started_at(),
        )
        .await,
    );
}

#[when("an InspectRun request for that run arrives")]
async fn inspect_registered_run(world: &mut BehaviourWorld) {
    let run_id = world
        .lifecycle_run
        .clone()
        .expect("a run must be registered first");
    world.response = Some(
        run_one_shot(
            &Request::InspectRun {
                run: run_id.to_string(),
            },
            world.started_at(),
        )
        .await,
    );
    lns_service::run_registry::deregister(&run_id);
}

fn inspect_details(world: &BehaviourWorld) -> Result<&lns_ipc::RunDetails, String> {
    match world.response.as_ref().ok_or("no response captured")? {
        lns_ipc::Response::RunInspect { details } => Ok(details),
        other => Err(format!("expected RunInspect, got {other:?}")),
    }
}

#[then(regex = r#"^the inspect details name image "([^"]+)"$"#)]
fn then_inspect_image(world: &mut BehaviourWorld, image: String) -> Result<(), String> {
    let details = inspect_details(world)?;
    if details.summary.image == image {
        Ok(())
    } else {
        Err(format!(
            "expected image {image:?}, got {:?}",
            details.summary.image
        ))
    }
}

#[then(regex = r#"^the inspect details report (\d+) cpus and (\d+) MiB$"#)]
fn then_inspect_resources(
    world: &mut BehaviourWorld,
    cpus: u8,
    mem_mib: usize,
) -> Result<(), String> {
    let details = inspect_details(world)?;
    if details.config.cpus == cpus && details.config.mem_mib == mem_mib {
        Ok(())
    } else {
        Err(format!(
            "expected {cpus} cpus / {mem_mib} MiB, got {} / {}",
            details.config.cpus, details.config.mem_mib
        ))
    }
}

#[then("the inspect details report the run as running")]
fn then_inspect_running(world: &mut BehaviourWorld) -> Result<(), String> {
    let details = inspect_details(world)?;
    match details.summary.status {
        lns_ipc::RunStatus::Running => Ok(()),
        other => Err(format!("expected Running, got {other:?}")),
    }
}
