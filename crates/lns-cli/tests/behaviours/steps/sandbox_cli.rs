use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use cucumber::{given, then, when};
use lns_cli::command::parse_args;
use lns_cli::sandbox::SandboxArgs;
use lns_cli::sandbox::{SandboxService, TermInfo, run_with_writers};
use lns_cli::service::client::BoxFuture;
use lns_ipc::{
    Request, Response, RunConfig, RunDetails, RunStatsInfo, RunStatus, RunSummary, WireFrame,
    encode_frame, encode_wire_frame,
};

use crate::runner::CliRun;
use crate::world::BehaviourWorld;

struct FakeSandboxService {
    response: Option<Response>,
    frames: Vec<Vec<u8>>,
    unreachable: bool,
    policy: Option<serde_json::Value>,
    requests: Arc<Mutex<Vec<Request>>>,
}

impl SandboxService for FakeSandboxService {
    type Stream = tokio::io::DuplexStream;

    fn one_shot(&self, request: Request) -> BoxFuture<'_, anyhow::Result<Response>> {
        self.requests.lock().unwrap().push(request);
        let unreachable = self.unreachable;
        let response = self.response.clone();
        Box::pin(async move {
            if unreachable {
                anyhow::bail!("no response from lns-service (is it running?)");
            }
            Ok(response.expect("scenario must can a one-shot response"))
        })
    }

    fn open_stream(&self, request: Request) -> BoxFuture<'_, anyhow::Result<Self::Stream>> {
        self.requests.lock().unwrap().push(request);
        let unreachable = self.unreachable;
        let payload: Vec<u8> = self.frames.concat();
        Box::pin(async move {
            if unreachable {
                anyhow::bail!("no response from lns-service (is it running?)");
            }
            let (client, mut server) = tokio::io::duplex(64 * 1024);
            tokio::spawn(async move {
                use tokio::io::AsyncWriteExt;
                let _ = server.write_all(&payload).await;
            });
            Ok(client)
        })
    }

    fn aux_socket(&self) -> Option<PathBuf> {
        None
    }

    fn load_policy(&self, _path: &str) -> Option<serde_json::Value> {
        self.policy.clone()
    }
}

fn details_with(image: &str, config: RunConfig) -> Response {
    Response::RunInspect {
        details: Box::new(RunDetails {
            summary: RunSummary {
                id: 3,
                name: "reviewer".into(),
                image: image.to_string(),
                command: "some-command".into(),
                status: RunStatus::Running,
                started: "2026-01-01T00:00:00Z".into(),
            },
            config,
        }),
    }
}

#[given(regex = r"^the service will answer RunStopped without force$")]
fn canned_stop_unforced(w: &mut BehaviourWorld) {
    w.sandbox.response = Some(Response::RunStopped { forced: false });
}

#[given(regex = r"^the service will answer RunStopped with force$")]
fn canned_stop_forced(w: &mut BehaviourWorld) {
    w.sandbox.response = Some(Response::RunStopped { forced: true });
}

#[given(regex = r#"^the service will answer an error "([^"]+)"$"#)]
fn canned_error(w: &mut BehaviourWorld, message: String) {
    w.sandbox.response = Some(Response::Error { message });
}

#[given("the sandbox service is unreachable")]
fn service_unreachable(w: &mut BehaviourWorld) {
    w.sandbox.unreachable = true;
}

#[given(regex = r"^the service will answer Acknowledged$")]
fn canned_acknowledged(w: &mut BehaviourWorld) {
    w.sandbox.response = Some(Response::Acknowledged);
}

#[given(regex = r"^the service will answer RunsPruned for runs (\d+) and (\d+)$")]
fn canned_pruned(w: &mut BehaviourWorld, first: u32, second: u32) {
    w.sandbox.response = Some(Response::RunsPruned {
        removed: vec![first, second],
    });
}

#[given(regex = r"^the service will answer RunsPruned for no runs$")]
fn canned_pruned_empty(w: &mut BehaviourWorld) {
    w.sandbox.response = Some(Response::RunsPruned { removed: vec![] });
}

#[then(regex = r"^the service received a RemoveRun request for run (\d+)$")]
fn then_remove_request(w: &mut BehaviourWorld, run_id: u32) -> Result<(), String> {
    let requests = w.sandbox.requests.lock().unwrap();
    let expected = Request::RemoveRun {
        run: run_id.to_string(),
    };
    if requests.contains(&expected) {
        Ok(())
    } else {
        Err(format!("expected {expected:?} among {requests:?}"))
    }
}

#[then("the service received a PruneRuns request")]
fn then_prune_request(w: &mut BehaviourWorld) -> Result<(), String> {
    let requests = w.sandbox.requests.lock().unwrap();
    if requests.contains(&Request::PruneRuns) {
        Ok(())
    } else {
        Err(format!("expected PruneRuns among {requests:?}"))
    }
}

#[given(regex = r#"^the service reports a run listing with run (\d+) of image "([^"]+)" running$"#)]
fn canned_run_listing(w: &mut BehaviourWorld, run_id: u32, image: String) {
    w.sandbox.response = Some(Response::RunList {
        runs: vec![RunSummary {
            id: run_id,
            name: String::new(),
            image,
            command: "some-command".into(),
            status: RunStatus::Running,
            started: "2026-01-01T00:00:00Z".into(),
        }],
    });
}

#[then("the service received a ListRuns request")]
fn then_list_runs_request(w: &mut BehaviourWorld) -> Result<(), String> {
    let requests = w.sandbox.requests.lock().unwrap();
    if requests.contains(&Request::ListRuns { all: false }) {
        Ok(())
    } else {
        Err(format!("expected ListRuns among {requests:?}"))
    }
}

#[then(regex = r"^the service received a Kill request for run (\d+) with signal KILL$")]
fn then_kill_request(w: &mut BehaviourWorld, run_id: u32) -> Result<(), String> {
    let requests = w.sandbox.requests.lock().unwrap();
    let expected = Request::Kill {
        run: run_id.to_string(),
        signal: lns_ipc::SignalKind::Kill,
    };
    if requests.contains(&expected) {
        Ok(())
    } else {
        Err(format!("expected {expected:?} among {requests:?}"))
    }
}

#[then(regex = r#"^the output does not contain "([^"]*)"$"#)]
fn then_output_does_not_contain(w: &mut BehaviourWorld, needle: String) -> Result<(), String> {
    let output = &w.result.as_ref().ok_or("no CLI run captured")?.output;
    if output.contains(&needle) {
        Err(format!(
            "expected output not to contain {needle:?}, got {output:?}"
        ))
    } else {
        Ok(())
    }
}

#[given(
    regex = r#"^the service reports run (\d+) of image "([^"]+)" running with (\d+) cpus and (\d+) MiB$"#
)]
fn canned_inspect(w: &mut BehaviourWorld, _run_id: u32, image: String, cpus: u8, mem_mib: usize) {
    w.sandbox.response = Some(details_with(
        &image,
        RunConfig {
            cpus,
            mem_mib,
            ..Default::default()
        },
    ));
}

#[given(regex = r#"^the service reports run (\d+) with policy path "([^"]+)"$"#)]
fn canned_inspect_with_policy(w: &mut BehaviourWorld, _run_id: u32, policy_path: String) {
    w.sandbox.response = Some(details_with(
        "some-image",
        RunConfig {
            policy_path: Some(policy_path),
            ..Default::default()
        },
    ));
}

#[given(regex = r#"^the policy file parses with default verdict "([^"]+)"$"#)]
fn canned_policy_doc(w: &mut BehaviourWorld, verdict: String) {
    w.sandbox.policy = Some(serde_json::json!({
        "network": { "defaultVerdict": verdict }
    }));
}

#[given(
    regex = r"^the service reports run (\d+) using (\d+) permille cpu and (\d+) of (\d+) bytes$"
)]
fn canned_stats(w: &mut BehaviourWorld, _run_id: u32, permille: u32, used: u64, total: u64) {
    w.sandbox.response = Some(Response::RunStats {
        stats: RunStatsInfo {
            cpu_permille: permille,
            mem_used_bytes: used,
            mem_total_bytes: total,
        },
    });
}

#[given(regex = r#"^the run (\d+) stream carries stdout "([^"]*)" then ends$"#)]
fn stream_then_ends(w: &mut BehaviourWorld, run_id: u32, text: String) {
    w.sandbox.frames = vec![
        encode_frame(&Response::RunStarted { run_id }).unwrap(),
        encode_wire_frame(&WireFrame::Stdout(text.into_bytes())).unwrap(),
        encode_frame(&Response::Acknowledged).unwrap(),
    ];
}

#[given(regex = r#"^the run (\d+) stream carries stdout "([^"]*)" then exits with code (\d+)$"#)]
fn stream_then_exits(w: &mut BehaviourWorld, run_id: u32, text: String, code: i32) {
    w.sandbox.frames = vec![
        encode_frame(&Response::RunStarted { run_id }).unwrap(),
        encode_wire_frame(&WireFrame::Stdout(text.into_bytes())).unwrap(),
        encode_frame(&Response::RunExit { code }).unwrap(),
    ];
}

#[given(regex = r#"^the run (\d+) stream opens with error "([^"]+)"$"#)]
fn stream_opens_with_error(w: &mut BehaviourWorld, _run_id: u32, message: String) {
    w.sandbox.frames = vec![encode_frame(&Response::Error { message }).unwrap()];
}

#[when(regex = r#"^the user runs sandbox command "([^"]+)"$"#)]
async fn run_sandbox_command(w: &mut BehaviourWorld, cmd: String) {
    let mut argv: Vec<&str> = vec!["lns", "sandbox"];
    argv.extend(cmd.split_whitespace());
    let args: SandboxArgs = parse_args(&argv).expect("sandbox argv must parse");

    let svc = FakeSandboxService {
        response: w.sandbox.response.clone(),
        frames: w.sandbox.frames.clone(),
        unreachable: w.sandbox.unreachable,
        policy: w.sandbox.policy.clone(),
        requests: w.sandbox.requests.clone(),
    };

    let mut out: Vec<u8> = Vec::new();
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let result = run_with_writers(
        &args.command,
        &svc,
        TermInfo::default(),
        &mut out,
        &mut stdout,
        &mut stderr,
    )
    .await;

    w.sandbox.workload_stdout = stdout;
    w.result = Some(match result {
        Ok(exit_code) => CliRun {
            exit_code,
            output: String::from_utf8_lossy(&out).into_owned(),
        },
        Err(e) => CliRun {
            exit_code: 1,
            output: format!("{e:#}"),
        },
    });
}

#[then(regex = r"^the service received a StopRun request for run (\d+) with timeout (\d+)$")]
fn then_stop_request(w: &mut BehaviourWorld, run_id: u32, timeout: u64) -> Result<(), String> {
    let requests = w.sandbox.requests.lock().unwrap();
    let expected = Request::StopRun {
        run: run_id.to_string(),
        timeout_secs: timeout,
    };
    if requests.contains(&expected) {
        Ok(())
    } else {
        Err(format!("expected {expected:?} among {requests:?}"))
    }
}

#[then(regex = r"^the service received a RunLogs request for run (\d+) (with|without) follow$")]
fn then_logs_request(w: &mut BehaviourWorld, run_id: u32, mode: String) -> Result<(), String> {
    let requests = w.sandbox.requests.lock().unwrap();
    let expected = Request::RunLogs {
        run: run_id.to_string(),
        follow: mode == "with",
    };
    if requests.contains(&expected) {
        Ok(())
    } else {
        Err(format!("expected {expected:?} among {requests:?}"))
    }
}

#[then(regex = r"^the service received an AttachRun request for run (\d+)$")]
fn then_attach_request(w: &mut BehaviourWorld, run_id: u32) -> Result<(), String> {
    let requests = w.sandbox.requests.lock().unwrap();
    let expected = Request::AttachRun {
        run: run_id.to_string(),
    };
    if requests.contains(&expected) {
        Ok(())
    } else {
        Err(format!("expected {expected:?} among {requests:?}"))
    }
}

#[then(regex = r#"^the workload stdout contains "([^"]*)"$"#)]
fn then_workload_stdout(w: &mut BehaviourWorld, needle: String) -> Result<(), String> {
    let text = String::from_utf8_lossy(&w.sandbox.workload_stdout);
    if text.contains(&needle) {
        Ok(())
    } else {
        Err(format!(
            "expected workload stdout to contain {needle:?}, got {text:?}"
        ))
    }
}

#[given(
    regex = r#"^the service reports a run listing with run (\d+) named "([^"]+)" of image "([^"]+)" running$"#
)]
fn canned_named_run_listing(w: &mut BehaviourWorld, run_id: u32, name: String, image: String) {
    w.sandbox.response = Some(Response::RunList {
        runs: vec![RunSummary {
            id: run_id,
            name,
            image,
            command: "some-command".into(),
            status: RunStatus::Running,
            started: "2026-01-01T00:00:00Z".into(),
        }],
    });
}

#[then(regex = r#"^the service received a StopRun request for run "([^"]+)" with timeout (\d+)$"#)]
fn then_stop_request_by_handle(
    w: &mut BehaviourWorld,
    run: String,
    timeout: u64,
) -> Result<(), String> {
    let requests = w.sandbox.requests.lock().unwrap();
    let expected = Request::StopRun {
        run,
        timeout_secs: timeout,
    };
    if requests.contains(&expected) {
        Ok(())
    } else {
        Err(format!("expected {expected:?} among {requests:?}"))
    }
}

#[then(regex = r#"^the service received a RenameRun request for run "([^"]+)" to "([^"]+)"$"#)]
fn then_rename_request(
    w: &mut BehaviourWorld,
    run: String,
    new_name: String,
) -> Result<(), String> {
    let requests = w.sandbox.requests.lock().unwrap();
    let expected = Request::RenameRun { run, new_name };
    if requests.contains(&expected) {
        Ok(())
    } else {
        Err(format!("expected {expected:?} among {requests:?}"))
    }
}
