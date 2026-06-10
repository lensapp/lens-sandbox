use crate::runner::run_one_shot;
use crate::world::BehaviourWorld;
use cucumber::{given, then, when};
use lns_ipc::Request;

pub fn register_run(run_id: u32) {
    let (cancel_tx, _cancel_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async {});
    lns_service::run_registry::register(
        run_id,
        lns_service::run_registry::RunHandle {
            cancel_tx,
            task,
            #[cfg(target_os = "macos")]
            input_tx: None,
            #[cfg(target_os = "macos")]
            connector: None,
            image: "some-image".into(),
            command: "some-command".into(),
            started: "2026-01-01T00:00:00Z".into(),
            status: std::sync::Mutex::new(lns_ipc::RunStatus::Running),
            logs: std::sync::Arc::new(lns_service::run_log::RunLogBuffer::default()),
        },
    );
}

#[given("a registered run that has already exited")]
async fn registered_exited_run(world: &mut BehaviourWorld) {
    let run_id = lns_service::run_registry::allocate_run_id();
    register_run(run_id);
    lns_service::run_registry::set_exit_code(run_id, 0);
    world.lifecycle_run = Some(run_id);
}

#[when(regex = r#"^a StopRun request for run (\d+) arrives$"#)]
async fn stop_unknown_run(world: &mut BehaviourWorld, run_id: u32) {
    world.response = Some(
        run_one_shot(
            &Request::StopRun {
                run_id,
                timeout_secs: 1,
            },
            world.started_at(),
        )
        .await,
    );
}

#[when("a StopRun request for that run arrives")]
async fn stop_registered_run(world: &mut BehaviourWorld) {
    let run_id = world.lifecycle_run.expect("a run must be registered first");
    world.response = Some(
        run_one_shot(
            &Request::StopRun {
                run_id,
                timeout_secs: 1,
            },
            world.started_at(),
        )
        .await,
    );
    lns_service::run_registry::deregister(run_id);
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
