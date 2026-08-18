use cucumber::{given, then, when};
use lns_ipc::{Request, Response, WireFrame, encode_frame, encode_wire_frame};

use crate::steps::sandbox_cli::drive_sandbox_command;
use crate::world::BehaviourWorld;

fn run_started_frames(run_id: &str) -> Vec<Vec<u8>> {
    vec![
        encode_frame(&Response::RunStarted {
            run_id: run_id.to_string(),
        })
        .unwrap(),
    ]
}

#[given(regex = r#"^a run named "([^"]+)" that was stopped$"#)]
fn a_stopped_run_named(w: &mut BehaviourWorld, name: String) {
    w.sandbox.frames = run_started_frames("aa07000000000000000000000000aa07");
    w.start_target = Some(name);
}

#[given(regex = r#"^a stopped run with id (\d+)$"#)]
fn a_stopped_run_with_id(w: &mut BehaviourWorld, id: u32) {
    w.sandbox.frames = run_started_frames(&format!("{id:08x}{}", "0".repeat(24)));
    w.start_target = Some(id.to_string());
}

#[given("a stopped run whose workload exits with code 3 after restart")]
fn a_stopped_run_exiting_three(w: &mut BehaviourWorld) {
    w.sandbox.frames = vec![
        encode_frame(&Response::RunStarted {
            run_id: "aa07000000000000000000000000aa07".into(),
        })
        .unwrap(),
        encode_wire_frame(&WireFrame::Stdout(b"restarted workload output".to_vec())).unwrap(),
        encode_frame(&Response::RunExit { code: 3 }).unwrap(),
    ];
    w.start_target = Some("reviewer".into());
}

#[when(regex = r#"^I run "lns start -a" on it$"#)]
async fn i_run_lns_start_attached(w: &mut BehaviourWorld) {
    let target = w.start_target.clone().expect("a stopped run was staged");
    drive_sandbox_command(w, &format!("start -a {target}")).await;
}

#[then(regex = r#"^it prints "([^"]+)"$"#)]
fn it_prints(w: &mut BehaviourWorld, expected: String) -> Result<(), String> {
    let run = w.result.as_ref().ok_or("no invocation ran")?;
    if run.output.contains(&expected) {
        Ok(())
    } else {
        Err(format!(
            "the handle the user typed is the one echoed back; got {:?}",
            run.output
        ))
    }
}

#[then("it exits 0")]
fn it_exits_zero(w: &mut BehaviourWorld) -> Result<(), String> {
    let run = w.result.as_ref().ok_or("no invocation ran")?;
    if run.exit_code == 0 {
        Ok(())
    } else {
        Err(format!(
            "expected exit 0, got {} ({:?})",
            run.exit_code, run.output
        ))
    }
}

#[then(regex = r#"^the run "([^"]+)" is running$"#)]
fn the_run_is_running(w: &mut BehaviourWorld, handle: String) -> Result<(), String> {
    let requests = w.sandbox.requests.lock().unwrap();
    if requests
        .iter()
        .any(|r| matches!(r, Request::StartRun { run, attach: false, .. } if run == &handle))
    {
        Ok(())
    } else {
        Err(format!(
            "a detached start asks the service and leaves; got {requests:?}"
        ))
    }
}

#[then("I see the workload output")]
fn i_see_the_workload_output(w: &mut BehaviourWorld) -> Result<(), String> {
    let stdout = String::from_utf8_lossy(&w.sandbox.workload_stdout);
    if stdout.contains("restarted workload output") {
        Ok(())
    } else {
        Err(format!("attached start relays output; got {stdout:?}"))
    }
}

#[then("it exits 3")]
fn it_exits_three(w: &mut BehaviourWorld) -> Result<(), String> {
    let run = w.result.as_ref().ok_or("no invocation ran")?;
    if run.exit_code == 3 {
        Ok(())
    } else {
        Err(format!(
            "-a adopts the workload's exit code; got {} ({:?})",
            run.exit_code, run.output
        ))
    }
}
