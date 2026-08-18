use crate::runner::{CliRun, run_lns};
use crate::world::BehaviourWorld;
use cucumber::{given, then, when};
use lns_ipc::{
    Request, Response, WireFrame, decode_frame, encode_frame, encode_wire_frame,
    read_frame_bytes_async,
};
use tokio::io::AsyncWriteExt;

#[given(regex = r#"^an active run named \"([^\"]+)\"$"#)]
fn active_run_named(world: &mut BehaviourWorld, _name: String) {
    world.exec.active = true;
}

#[given(regex = r#"^no active run is named \"([^\"]+)\"$"#)]
fn no_active_run_named(world: &mut BehaviourWorld, _name: String) {
    world.exec.active = false;
}

struct FakeExecService {
    request: Option<lns_ipc::ExecImageArgs>,
    session_started: bool,
}

async fn fake_exec_service(mut server: tokio::io::DuplexStream, active: bool) -> FakeExecService {
    let bytes = read_frame_bytes_async(&mut server)
        .await
        .expect("the CLI opens by sending one request frame");
    let request: Request =
        decode_frame(&mut &bytes[..]).expect("the CLI's opening frame is a Request");
    let Request::ExecImage(args) = request else {
        panic!("expected an ExecImage request, got {request:?}");
    };
    if !active {
        let refusal = encode_frame(&Response::Error {
            message: format!("no such run: {}", args.run),
        })
        .expect("encode refusal");
        let _ = server.write_all(&refusal).await;
        return FakeExecService {
            request: Some(args),
            session_started: false,
        };
    }
    let started = encode_frame(&Response::ExecStarted {
        run_id: "run-1".into(),
        session_id: "exec-1".into(),
    })
    .expect("encode ExecStarted");
    server.write_all(&started).await.expect("send ExecStarted");
    let canned_output = match args.argv.first().map(String::as_str) {
        Some("echo") => Some(args.argv[1..].join(" ").into_bytes()),
        _ => None,
    };
    if let Some(bytes) = canned_output {
        let frame = encode_wire_frame(&WireFrame::Stdout(bytes)).expect("encode stdout frame");
        server.write_all(&frame).await.expect("send stdout frame");
    }
    let exit = encode_wire_frame(&WireFrame::Json(Response::RunExit { code: 0 }))
        .expect("encode exit frame");
    server.write_all(&exit).await.expect("send exit frame");
    FakeExecService {
        request: Some(args),
        session_started: true,
    }
}

#[when(regex = r#"^the user runs \"(lns exec(?: [^\"]*)?)\"$"#)]
async fn user_runs(world: &mut BehaviourWorld, command: String) {
    let tokens: Vec<String> = command.split_whitespace().map(str::to_string).collect();
    if tokens.iter().any(|token| token == "--help") {
        let args: Vec<&str> = tokens.iter().skip(1).map(String::as_str).collect();
        world.result = Some(run_lns(&args));
        return;
    }
    let args: lns_cli::cli::ExecArgs =
        lns_cli::command::parse_args(tokens).expect("exec scenario argv should parse");
    let winsize = args.tty.then_some((24, 80));

    let (client, server) = tokio::io::duplex(4096);
    let service = tokio::spawn(fake_exec_service(server, world.exec.active));

    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let outcome = lns_cli::service::exec_image_on_stream(
        client,
        None,
        args,
        winsize,
        false,
        &mut stdout,
        &mut stderr,
    )
    .await;

    let service = service.await.expect("the fake service completes");
    world.exec.request = service.request;
    world.exec.session_started = service.session_started;

    let mut output = String::from_utf8_lossy(&stdout).into_owned();
    output.push_str(&String::from_utf8_lossy(&stderr));
    world.result = Some(match outcome {
        Ok(code) => CliRun {
            exit_code: code,
            output,
        },
        Err(e) => CliRun {
            exit_code: 1,
            output: format!("{output}{e:#}"),
        },
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

#[then("the exec request forwards host stdin")]
fn exec_request_forwards_stdin(world: &mut BehaviourWorld) -> Result<(), String> {
    if request(world)?.stdin {
        Ok(())
    } else {
        Err("the wire request did not forward stdin".into())
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

#[then("no exec session is started")]
fn no_exec_session_started(world: &mut BehaviourWorld) -> Result<(), String> {
    if !world.exec.session_started {
        Ok(())
    } else {
        Err("the service started an exec session".into())
    }
}
