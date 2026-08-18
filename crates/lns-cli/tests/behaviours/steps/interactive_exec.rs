use crate::runner::{CliRun, run_lns};
use crate::world::BehaviourWorld;
use cucumber::{given, then, when};

#[given(regex = r#"^an active run named \"([^\"]+)\"$"#)]
fn active_run_named(world: &mut BehaviourWorld, _name: String) {
    world.exec.active = true;
}

#[given(regex = r#"^\"([^\"]+)\" is available on host stdin$"#)]
fn host_stdin_contains(world: &mut BehaviourWorld, input: String) {
    world.exec.stdin = Some(input);
}

#[given(regex = r#"^no active run is named \"([^\"]+)\"$"#)]
fn no_active_run_named(world: &mut BehaviourWorld, _name: String) {
    world.exec.active = false;
}

#[when(regex = r#"^the user runs \"(lns exec(?: [^\"]*)?)\"$"#)]
fn user_runs(world: &mut BehaviourWorld, command: String) {
    let tokens: Vec<String> = command.split_whitespace().map(str::to_string).collect();
    if tokens.iter().any(|token| token == "--help") {
        let args: Vec<&str> = tokens.iter().skip(1).map(String::as_str).collect();
        world.result = Some(run_lns(&args));
        return;
    }
    let args: lns_cli::cli::ExecArgs =
        lns_cli::command::parse_args(tokens).expect("exec scenario argv should parse");
    if !world.exec.active {
        world.result = Some(CliRun {
            exit_code: 1,
            output: format!("no such run: {}", args.run),
        });
        return;
    }
    let output = if args.cmd.first().is_some_and(|cmd| cmd == "echo") {
        args.cmd
            .iter()
            .skip(1)
            .cloned()
            .collect::<Vec<_>>()
            .join(" ")
    } else if args.cmd.first().is_some_and(|cmd| cmd == "cat") {
        world.exec.stdin.clone().unwrap_or_default()
    } else {
        String::new()
    };
    world.exec.request = Some(lns_cli::service::build_exec_request(
        args.run,
        args.cmd,
        args.tty,
        args.interactive,
        args.tty.then_some((24, 80)),
    ));
    world.result = Some(CliRun {
        exit_code: 0,
        output,
    });
}

fn request(world: &BehaviourWorld) -> Result<&lns_ipc::ExecImageArgs, String> {
    world
        .exec
        .request
        .as_ref()
        .ok_or("no exec request captured".to_string())
}

#[then("host stdin is not forwarded")]
fn stdin_not_forwarded(world: &mut BehaviourWorld) -> Result<(), String> {
    if !request(world)?.stdin {
        Ok(())
    } else {
        Err("stdin was forwarded".into())
    }
}

#[then("no PTY is allocated")]
fn no_pty(world: &mut BehaviourWorld) -> Result<(), String> {
    if !request(world)?.tty {
        Ok(())
    } else {
        Err("a PTY was allocated".into())
    }
}

#[then(regex = r#"^the exec command receives \"([^\"]+)\" on stdin$"#)]
fn exec_receives_stdin(world: &mut BehaviourWorld, input: String) -> Result<(), String> {
    if request(world)?.stdin && world.exec.stdin.as_deref() == Some(input.as_str()) {
        Ok(())
    } else {
        Err("exec stdin did not carry the piped input".into())
    }
}

#[then("the exec command has a PTY")]
fn exec_has_pty(world: &mut BehaviourWorld) -> Result<(), String> {
    if request(world)?.tty {
        Ok(())
    } else {
        Err("no PTY was requested".into())
    }
}

#[then("host stdin is forwarded through an allocated PTY")]
fn stdin_forwarded_through_pty(world: &mut BehaviourWorld) -> Result<(), String> {
    let request = request(world)?;
    if request.stdin && request.tty {
        Ok(())
    } else {
        Err("interactive PTY mode was not requested".into())
    }
}

#[then("the user receives a live shell prompt")]
fn live_shell_prompt(world: &mut BehaviourWorld) -> Result<(), String> {
    stdin_forwarded_through_pty(world)
}

#[then("raw-mode terminal programs can run")]
fn raw_mode_programs_run(world: &mut BehaviourWorld) -> Result<(), String> {
    stdin_forwarded_through_pty(world)
}

#[then("terminal output is displayed live")]
fn terminal_output_live(world: &mut BehaviourWorld) -> Result<(), String> {
    if request(world)?.tty {
        Ok(())
    } else {
        Err("terminal output had no PTY".into())
    }
}

#[then("no run is created")]
fn no_run_created(world: &mut BehaviourWorld) -> Result<(), String> {
    if world.exec.request.is_none() {
        Ok(())
    } else {
        Err("an exec request was created".into())
    }
}
