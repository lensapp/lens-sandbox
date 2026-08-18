use crate::world::BehaviourWorld;
use cucumber::{given, then, when};

fn pending() -> ! {
    panic!("pending exec session routing implementation")
}

#[given(regex = r#"^an active run named \"([^\"]+)\"$"#)]
fn active_run_named(_world: &mut BehaviourWorld, _name: String) {
    pending()
}

#[given("its primary session is attached to another client")]
fn primary_attached_elsewhere(_world: &mut BehaviourWorld) {
    pending()
}

#[given("its primary session is running")]
fn primary_running(_world: &mut BehaviourWorld) {
    pending()
}

#[given("two interactive exec sessions are active")]
fn two_exec_sessions(_world: &mut BehaviourWorld) {
    pending()
}

#[when(regex = r#"^the user runs \"(lns exec(?: [^\"]*)?)\"$"#)]
fn user_runs(_world: &mut BehaviourWorld, _command: String) {
    pending()
}

#[when("the first exec client resizes its terminal")]
fn first_exec_resizes(_world: &mut BehaviourWorld) {
    pending()
}

#[when("the first exec client sends SIGINT")]
fn first_exec_signals(_world: &mut BehaviourWorld) {
    pending()
}

#[when("the user enters the detach chord in the first exec session")]
fn first_exec_detaches(_world: &mut BehaviourWorld) {
    pending()
}

#[when("the first exec client disconnects unexpectedly")]
fn first_exec_disconnects(_world: &mut BehaviourWorld) {
    pending()
}

#[when("the user execs a non-interactive command that writes to stdout and stderr")]
fn user_execs_noninteractive(_world: &mut BehaviourWorld) {
    pending()
}

#[then("the user receives a live shell prompt")]
fn live_shell_prompt(_world: &mut BehaviourWorld) {
    pending()
}

#[then("the primary session remains attached and usable")]
fn primary_remains_attached(_world: &mut BehaviourWorld) {
    pending()
}

#[then("only the first exec session receives the new dimensions")]
fn first_exec_receives_resize(_world: &mut BehaviourWorld) {
    pending()
}

#[then("only the first exec session receives SIGINT")]
fn first_exec_receives_signal(_world: &mut BehaviourWorld) {
    pending()
}

#[then("the primary session is unaffected")]
fn primary_unaffected(_world: &mut BehaviourWorld) {
    pending()
}

#[then("the second exec session remains usable")]
fn second_exec_usable(_world: &mut BehaviourWorld) {
    pending()
}

#[then("the first exec session is terminated")]
fn first_exec_terminated(_world: &mut BehaviourWorld) {
    pending()
}

#[then("its CLI returns successfully")]
fn cli_returns_successfully(_world: &mut BehaviourWorld) {
    pending()
}

#[then("the primary session remains running")]
fn primary_remains_running(_world: &mut BehaviourWorld) {
    pending()
}

#[then("only the first exec session is cancelled")]
fn first_exec_cancelled(_world: &mut BehaviourWorld) {
    pending()
}

#[then("both output streams are returned")]
fn output_streams_returned(_world: &mut BehaviourWorld) {
    pending()
}

#[then("the CLI returns the command's exit status")]
fn cli_returns_exit_status(_world: &mut BehaviourWorld) {
    pending()
}

#[then("the exec output is not added to the primary session's captured logs")]
fn exec_output_not_logged(_world: &mut BehaviourWorld) {
    pending()
}
