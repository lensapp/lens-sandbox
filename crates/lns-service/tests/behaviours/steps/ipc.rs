use crate::runner::run_one_shot;
use crate::world::BehaviourWorld;
use cucumber::{given, then, when};
use lns_ipc::Request;
use std::time::{Duration, Instant};

#[given("a fresh service handler")]
fn fresh_handler(world: &mut BehaviourWorld) {
    world.started_at = Some(Instant::now());
}

#[given(regex = r#"^a service handler that has been running for at least (\d+) seconds?$"#)]
fn handler_with_uptime(world: &mut BehaviourWorld, secs: u64) {
    world.started_at = Instant::now().checked_sub(Duration::from_secs(secs));
}

#[when("a Ping request arrives")]
async fn when_ping(world: &mut BehaviourWorld) {
    world.response = Some(run_one_shot(&Request::Ping, world.started_at()).await);
}

#[when("a Status request arrives")]
async fn when_status(world: &mut BehaviourWorld) {
    world.response = Some(run_one_shot(&Request::Status, world.started_at()).await);
}

#[when("a Shutdown request arrives")]
async fn when_shutdown(world: &mut BehaviourWorld) {
    world.response = Some(run_one_shot(&Request::Shutdown, world.started_at()).await);
}

#[when(regex = r#"^an Unknown request with method "([^"]+)" arrives$"#)]
async fn when_unknown(world: &mut BehaviourWorld, method: String) {
    world.response = Some(run_one_shot(&Request::Unknown { method }, world.started_at()).await);
}

#[when(regex = r#"^a CancelRun request for run (\d+) arrives$"#)]
async fn when_cancel_run(world: &mut BehaviourWorld, run_id: u32) {
    world.response = Some(
        run_one_shot(
            &Request::CancelRun {
                run_id: run_id.to_string(),
            },
            world.started_at(),
        )
        .await,
    );
}

#[then("the response is Pong")]
fn then_pong(world: &mut BehaviourWorld) -> Result<(), String> {
    match world.response.as_ref().ok_or("no response captured")? {
        lns_ipc::Response::Pong => Ok(()),
        other => Err(format!("expected Pong, got {other:?}")),
    }
}

#[then("the response is Status")]
fn then_status(world: &mut BehaviourWorld) -> Result<(), String> {
    match world.response.as_ref().ok_or("no response captured")? {
        lns_ipc::Response::Status(_) => Ok(()),
        other => Err(format!("expected Status, got {other:?}")),
    }
}

#[then("the response is ShuttingDown")]
fn then_shutting_down(world: &mut BehaviourWorld) -> Result<(), String> {
    match world.response.as_ref().ok_or("no response captured")? {
        lns_ipc::Response::ShuttingDown => Ok(()),
        other => Err(format!("expected ShuttingDown, got {other:?}")),
    }
}

#[then("the response is Error")]
fn then_error(world: &mut BehaviourWorld) -> Result<(), String> {
    match world.response.as_ref().ok_or("no response captured")? {
        lns_ipc::Response::Error { .. } => Ok(()),
        other => Err(format!("expected Error, got {other:?}")),
    }
}

#[then(expr = "the response is RunUnknown for run {string}")]
fn then_run_unknown(world: &mut BehaviourWorld, run: String) -> Result<(), String> {
    match world.response.as_ref().ok_or("no response captured")? {
        lns_ipc::Response::RunUnknown { run: named } if *named == run => Ok(()),
        other => Err(format!("expected RunUnknown for {run:?}, got {other:?}")),
    }
}

#[then("the response is Acknowledged")]
fn then_acknowledged(world: &mut BehaviourWorld) -> Result<(), String> {
    match world.response.as_ref().ok_or("no response captured")? {
        lns_ipc::Response::Acknowledged => Ok(()),
        other => Err(format!("expected Acknowledged, got {other:?}")),
    }
}

#[then("the response pid matches the current process")]
fn then_pid_matches(world: &mut BehaviourWorld) -> Result<(), String> {
    match world.response.as_ref().ok_or("no response captured")? {
        lns_ipc::Response::Status(info) => {
            if info.pid == std::process::id() {
                Ok(())
            } else {
                Err(format!(
                    "expected pid {}, got {}",
                    std::process::id(),
                    info.pid
                ))
            }
        }
        other => Err(format!("expected Status, got {other:?}")),
    }
}

#[then("the response version matches the lns-service package version")]
fn then_version_matches(world: &mut BehaviourWorld) -> Result<(), String> {
    let expected = env!("CARGO_PKG_VERSION");
    match world.response.as_ref().ok_or("no response captured")? {
        lns_ipc::Response::Status(info) => {
            if info.version == expected {
                Ok(())
            } else {
                Err(format!(
                    "expected version {expected:?}, got {:?}",
                    info.version
                ))
            }
        }
        other => Err(format!("expected Status, got {other:?}")),
    }
}

#[then(regex = r#"^the response uptime is at least (\d+) seconds?$"#)]
fn then_uptime_at_least(world: &mut BehaviourWorld, secs: u64) -> Result<(), String> {
    match world.response.as_ref().ok_or("no response captured")? {
        lns_ipc::Response::Status(info) => {
            if info.uptime_secs >= secs {
                Ok(())
            } else {
                Err(format!(
                    "expected uptime ≥ {secs}, got {}",
                    info.uptime_secs
                ))
            }
        }
        other => Err(format!("expected Status, got {other:?}")),
    }
}

#[then(regex = r#"^the error message contains "([^"]+)"$"#)]
fn then_error_contains(world: &mut BehaviourWorld, needle: String) -> Result<(), String> {
    match world.response.as_ref().ok_or("no response captured")? {
        lns_ipc::Response::Error { message } => {
            if message.contains(&needle) {
                Ok(())
            } else {
                Err(format!(
                    "expected error message to contain {needle:?}, got {message:?}"
                ))
            }
        }
        other => Err(format!("expected Error, got {other:?}")),
    }
}
