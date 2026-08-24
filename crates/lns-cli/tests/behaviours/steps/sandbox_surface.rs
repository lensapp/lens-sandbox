use cucumber::{given, then, when};
use lns_cli::artifact::{ArtifactArgs, ArtifactCommand};
use lns_cli::command::parse_args;
use lns_cli::sandbox::{SandboxArgs, SandboxCommand, TermInfo, run_with_writers};
use lns_ipc::Response;

use crate::steps::sandbox_cli::fake_sandbox_service;
use crate::world::BehaviourWorld;

/// Either namespace's decoding of one shortcut argv, so a shortcut and its namespaced form can be driven the same way.
enum Verb {
    Sandbox(SandboxCommand),
    Artifact(ArtifactCommand),
}

fn decode_shortcut(argv: &[String]) -> Verb {
    match argv[1].as_str() {
        "pull" => Verb::Artifact(ArtifactCommand::Pull(
            parse_args(argv).expect("pull parses"),
        )),
        "ps" => Verb::Sandbox(SandboxCommand::Ls(parse_args(argv).expect("ps parses"))),
        "stop" => Verb::Sandbox(SandboxCommand::Stop(parse_args(argv).expect("stop parses"))),
        "kill" => Verb::Sandbox(SandboxCommand::Kill(parse_args(argv).expect("kill parses"))),
        other => panic!("verb {other} is not part of the shortcut outline"),
    }
}

async fn record_invocation(w: &mut BehaviourWorld, verb: Verb) {
    let svc = fake_sandbox_service(w);
    let mut out: Vec<u8> = Vec::new();
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    match verb {
        Verb::Sandbox(cmd) => {
            let _ = run_with_writers(
                &cmd,
                &svc,
                TermInfo::default(),
                &mut std::io::empty(),
                &mut out,
                &mut stdout,
                &mut stderr,
            )
            .await;
        }
        Verb::Artifact(cmd) => {
            let _ = lns_cli::artifact::run_with_writers(
                &cmd,
                &svc,
                TermInfo::default(),
                &mut std::io::Cursor::new(""),
                &mut out,
                &mut stderr,
            )
            .await;
        }
    }
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

#[when(regex = r#"^the user runs its namespaced form "lns (\S+) (\S+) ?([^"]*)"$"#)]
async fn runs_namespaced_form(
    w: &mut BehaviourWorld,
    namespace: String,
    verb: String,
    rest: String,
) {
    let mut argv = vec!["lns".to_string(), namespace.clone(), verb];
    argv.extend(rest.split_whitespace().map(str::to_string));
    let cmd = if namespace == "artifact" {
        let args: ArtifactArgs = parse_args(&argv).expect("artifact argv parses");
        Verb::Artifact(args.command)
    } else {
        let args: SandboxArgs = parse_args(&argv).expect("sandbox argv parses");
        Verb::Sandbox(args.command)
    };
    record_invocation(w, cmd).await;
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
