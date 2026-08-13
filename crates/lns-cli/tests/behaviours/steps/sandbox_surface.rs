use cucumber::{given, then, when};
use lns_cli::command::parse_args;
use lns_cli::sandbox::{DispatchEnv, SandboxArgs, SandboxCommand, TermInfo, run_with_writers};
use lns_ipc::Response;

use crate::steps::sandbox_cli::fake_sandbox_service;
use crate::world::BehaviourWorld;

fn decode_shortcut(argv: &[String]) -> SandboxCommand {
    match argv[1].as_str() {
        "pull" => SandboxCommand::Pull(parse_args(argv).expect("pull argv parses")),
        "ps" => SandboxCommand::Ps(parse_args(argv).expect("ps argv parses")),
        "stop" => SandboxCommand::Stop(parse_args(argv).expect("stop argv parses")),
        "kill" => SandboxCommand::Kill(parse_args(argv).expect("kill argv parses")),
        "rm" => SandboxCommand::Rm(parse_args(argv).expect("rm argv parses")),
        "inspect" => SandboxCommand::Inspect(parse_args(argv).expect("inspect argv parses")),
        other => panic!("verb {other} is not part of the shortcut outline"),
    }
}

async fn record_invocation(w: &mut BehaviourWorld, cmd: SandboxCommand) {
    let svc = fake_sandbox_service(w);
    let mut out: Vec<u8> = Vec::new();
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let _ = run_with_writers(
        cmd,
        DispatchEnv {
            svc: &svc,
            term: TermInfo::default(),
            registry: None,
        },
        &mut std::io::Cursor::new(""),
        &mut out,
        &mut stdout,
        &mut stderr,
    )
    .await;
    let recorded = std::mem::take(&mut *w.sandbox.requests.lock().unwrap());
    w.equivalence_requests.push(recorded);
}

#[given("the service is ready to record the request")]
fn service_records_requests(w: &mut BehaviourWorld) {
    w.sandbox.response = Some(Response::Error {
        message: "recorded".into(),
    });
    w.equivalence_requests.clear();
}

#[when(regex = r#"^the user runs the shortcut "lns (\S+) ?([^"]*)"$"#)]
async fn runs_shortcut(w: &mut BehaviourWorld, verb: String, rest: String) {
    let mut argv = vec!["lns".to_string(), verb];
    argv.extend(rest.split_whitespace().map(str::to_string));
    let cmd = decode_shortcut(&argv);
    record_invocation(w, cmd).await;
}

#[when(regex = r#"^the user runs its sandbox form "lns sandbox (\S+) ?([^"]*)"$"#)]
async fn runs_sandbox_form(w: &mut BehaviourWorld, verb: String, rest: String) {
    let mut argv = vec!["lns".to_string(), "sandbox".to_string(), verb];
    argv.extend(rest.split_whitespace().map(str::to_string));
    let args: SandboxArgs = parse_args(&argv).expect("sandbox argv parses");
    record_invocation(w, args.command).await;
}

#[then("both invocations issue the same request to the service")]
fn both_sequences_match(w: &mut BehaviourWorld) -> Result<(), String> {
    let [.., shortcut, namespace] = w.equivalence_requests.as_slice() else {
        return Err("both spellings must run before comparing their requests".into());
    };
    if shortcut.is_empty() {
        return Err("the shortcut spelling sent no request to the service".into());
    }
    if shortcut == namespace {
        Ok(())
    } else {
        Err(format!(
            "the two spellings diverged:\n  shortcut:  {shortcut:?}\n  namespace: {namespace:?}"
        ))
    }
}
