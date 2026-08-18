use crate::runner::run_lns;
use crate::world::BehaviourWorld;
use cucumber::{given, then, when};

fn pending() -> ! {
    panic!("pending interactive exec implementation")
}

#[given(regex = r#"^an active run named \"([^\"]+)\"$"#)]
fn active_run_named(_world: &mut BehaviourWorld, _name: String) {
    pending()
}

#[given(regex = r#"^\"([^\"]+)\" is available on host stdin$"#)]
fn host_stdin_contains(_world: &mut BehaviourWorld, _input: String) {
    pending()
}

#[given(regex = r#"^no active run is named \"([^\"]+)\"$"#)]
fn no_active_run_named(_world: &mut BehaviourWorld, _name: String) {
    pending()
}

#[when(regex = r#"^the user runs \"(lns exec(?: [^\"]*)?)\"$"#)]
fn user_runs(world: &mut BehaviourWorld, command: String) {
    let args: Vec<&str> = command
        .strip_prefix("lns ")
        .expect("interactive exec scenarios invoke lns")
        .split_whitespace()
        .collect();
    world.result = Some(run_lns(&args));
}

#[then("host stdin is not forwarded")]
fn stdin_not_forwarded(_world: &mut BehaviourWorld) {
    pending()
}

#[then("no PTY is allocated")]
fn no_pty(_world: &mut BehaviourWorld) {
    pending()
}

#[then(regex = r#"^the exec command receives \"([^\"]+)\" on stdin$"#)]
fn exec_receives_stdin(_world: &mut BehaviourWorld, _input: String) {
    pending()
}

#[then("the exec command has a PTY")]
fn exec_has_pty(_world: &mut BehaviourWorld) {
    pending()
}

#[then("host stdin is forwarded through an allocated PTY")]
fn stdin_forwarded_through_pty(_world: &mut BehaviourWorld) {
    pending()
}

#[then("the user receives a live shell prompt")]
fn live_shell_prompt(_world: &mut BehaviourWorld) {
    pending()
}

#[then("raw-mode terminal programs can run")]
fn raw_mode_programs_run(_world: &mut BehaviourWorld) {
    pending()
}

#[then("terminal output is displayed live")]
fn terminal_output_live(_world: &mut BehaviourWorld) {
    pending()
}

#[then("no run is created")]
fn no_run_created(_world: &mut BehaviourWorld) {
    pending()
}
