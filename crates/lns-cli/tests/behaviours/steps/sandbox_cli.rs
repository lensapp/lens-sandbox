use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use cucumber::{given, then, when};
use lns_cli::command::parse_args;
use lns_cli::integration::LocalBoxFuture;
use lns_cli::sandbox::{SandboxArgs, SandboxCommand, author, distribute};
use lns_cli::sandbox::{SandboxService, TermInfo, run_with_writers};
use lns_cli::service::client::BoxFuture;
use lns_ipc::{
    Request, Response, RunConfig, RunDetails, RunStatsInfo, RunStatus, RunSummary, WireFrame,
    encode_frame, encode_wire_frame,
};

use crate::runner::CliRun;
use crate::world::BehaviourWorld;

pub(crate) struct FakeSandboxService {
    response: Option<Response>,
    stats_response: Option<Response>,
    inspect_image_response: Option<Response>,
    remove_image_response: Option<Response>,
    frames: Vec<Vec<u8>>,
    unreachable: bool,
    policy: Option<serde_json::Value>,
    requests: Arc<Mutex<Vec<Request>>>,
}

impl SandboxService for FakeSandboxService {
    type Stream = tokio::io::DuplexStream;

    fn one_shot(&self, request: Request) -> BoxFuture<'_, anyhow::Result<Response>> {
        let response = match &request {
            Request::RunStats { .. } => self
                .stats_response
                .clone()
                .or_else(|| self.response.clone()),
            Request::InspectImage { .. } => self
                .inspect_image_response
                .clone()
                .or_else(|| self.response.clone()),
            Request::RemoveImage { .. } => self
                .remove_image_response
                .clone()
                .or_else(|| self.response.clone()),
            _ => self.response.clone(),
        };
        self.requests.lock().unwrap().push(request);
        let unreachable = self.unreachable;
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

fn hexid(n: u32) -> String {
    format!("{n:08x}{}", "0".repeat(24))
}

fn details_with(image: &str, config: RunConfig) -> Response {
    Response::RunInspect {
        details: Box::new(RunDetails {
            summary: RunSummary {
                id: hexid(3),
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

#[given(regex = r#"^the run (\d+) stream carries stdout "([^"]*)" then ends$"#)]
fn stream_then_ends(w: &mut BehaviourWorld, run_id: u32, text: String) {
    w.sandbox.frames = vec![
        encode_frame(&Response::RunStarted {
            run_id: hexid(run_id),
        })
        .unwrap(),
        encode_wire_frame(&WireFrame::Stdout(text.into_bytes())).unwrap(),
        encode_frame(&Response::Acknowledged).unwrap(),
    ];
}

#[given(regex = r#"^the run (\d+) stream carries stdout "([^"]*)" then exits with code (\d+)$"#)]
fn stream_then_exits(w: &mut BehaviourWorld, run_id: u32, text: String, code: i32) {
    w.sandbox.frames = vec![
        encode_frame(&Response::RunStarted {
            run_id: hexid(run_id),
        })
        .unwrap(),
        encode_wire_frame(&WireFrame::Stdout(text.into_bytes())).unwrap(),
        encode_frame(&Response::RunExit { code }).unwrap(),
    ];
}

#[given(regex = r#"^the run (\d+) stream opens with error "([^"]+)"$"#)]
fn stream_opens_with_error(w: &mut BehaviourWorld, _run_id: u32, message: String) {
    w.sandbox.frames = vec![encode_frame(&Response::Error { message }).unwrap()];
}

struct StepFs {
    files: RefCell<HashMap<PathBuf, String>>,
}

impl author::Fs for StepFs {
    fn read_to_string(&self, path: &Path) -> std::io::Result<String> {
        self.files
            .borrow()
            .get(path)
            .cloned()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no such file"))
    }
    fn write(&self, path: &Path, contents: &str) -> std::io::Result<()> {
        self.files
            .borrow_mut()
            .insert(path.to_path_buf(), contents.to_string());
        Ok(())
    }
    fn exists(&self, path: &Path) -> bool {
        self.files.borrow().contains_key(path)
    }
    fn read_limited(&self, path: &Path, max_bytes: u64) -> std::io::Result<Vec<u8>> {
        let mut bytes = self.read_to_string(path)?.into_bytes();
        bytes.truncate(max_bytes.saturating_add(1) as usize);
        Ok(bytes)
    }
    fn dir_entries(&self, dir: &Path) -> std::io::Result<Vec<author::DirEntry>> {
        author::map_dir_entries(self.files.borrow().keys(), dir)
    }
}

struct StepProducer {
    outcome: Result<String, String>,
    docs: RefCell<Vec<Vec<u8>>>,
    filesets: RefCell<Vec<String>>,
}

impl distribute::Producer for StepProducer {
    fn build_and_push<'a>(
        &'a self,
        doc: &'a [u8],
        _reference: &'a str,
    ) -> LocalBoxFuture<'a, anyhow::Result<String>> {
        self.docs.borrow_mut().push(doc.to_vec());
        let outcome = self.outcome.clone().map_err(|m| anyhow::anyhow!(m));
        Box::pin(async move { outcome })
    }

    fn push_prebuilt<'a>(
        &'a self,
        _built: &'a lns_artifact::build::BuiltArtifact,
        reference: &'a str,
    ) -> LocalBoxFuture<'a, anyhow::Result<()>> {
        self.filesets.borrow_mut().push(reference.to_string());
        Box::pin(async move { Ok(()) })
    }
}

fn run_author_verb(w: &mut BehaviourWorld, cmd: &SandboxCommand) {
    let cwd = Path::new("/work");
    let fs = StepFs {
        files: RefCell::new(w.author_files.clone()),
    };
    let mut out: Vec<u8> = Vec::new();
    let result = match cmd {
        SandboxCommand::Init => author::init(&fs, cwd, &mut out),
        SandboxCommand::Validate => author::validate(&fs, cwd, &mut out),
        SandboxCommand::Inspect(args) => {
            author::inspect_local(&fs, cwd, args.run.as_deref(), &mut out)
        }
        _ => unreachable!("run_author_verb is only called for the offline author verbs"),
    };
    w.author_files = fs.files.into_inner();
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

#[when(regex = r#"^the user runs sandbox command "([^"]+)"$"#)]
async fn run_sandbox_command(w: &mut BehaviourWorld, cmd: String) {
    let mut argv: Vec<&str> = vec!["lns", "sandbox"];
    argv.extend(cmd.split_whitespace());
    let args: SandboxArgs = parse_args(&argv).expect("sandbox argv must parse");

    if author::is_offline(&args.command) {
        run_author_verb(w, &args.command);
        return;
    }

    if let SandboxCommand::Push(push_args) = &args.command {
        let fs = StepFs {
            files: RefCell::new(w.author_files.clone()),
        };
        let producer = StepProducer {
            outcome: w.push_outcome.clone().unwrap_or(Err(
                "the push must refuse before reaching the producer".into(),
            )),
            docs: RefCell::new(Vec::new()),
            filesets: RefCell::new(Vec::new()),
        };
        let mut out: Vec<u8> = Vec::new();
        let result = match author::load_definition_json(&fs, Path::new("/work")) {
            Ok(doc) if push_args.dry_run => distribute::push_dry_run(
                &fs,
                Path::new("/work"),
                &doc,
                &push_args.reference,
                &mut out,
            ),
            Ok(doc) => {
                distribute::push(
                    &fs,
                    Path::new("/work"),
                    &producer,
                    &doc,
                    &push_args.reference,
                    &mut out,
                )
                .await
            }
            Err(e) => Err(e),
        };
        w.pushed_filesets = producer.filesets.into_inner();
        w.pushed_doc = producer.docs.into_inner().into_iter().next();
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
        return;
    }

    let svc = fake_sandbox_service(w);

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

#[given(
    regex = r"^the service reports one running sandbox using (\d+) permille cpu and (\d+) bytes$"
)]
fn canned_running_with_stats(w: &mut BehaviourWorld, permille: u32, used: u64) {
    w.sandbox.response = Some(Response::RunList {
        runs: vec![RunSummary {
            id: hexid(3),
            name: "reviewer".into(),
            image: "some-image".into(),
            command: "some-command".into(),
            status: RunStatus::Running,
            started: "2026-01-01T00:00:00Z".into(),
        }],
    });
    w.sandbox.stats_response = Some(Response::RunStats {
        stats: RunStatsInfo {
            cpu_permille: permille,
            mem_used_bytes: used,
            mem_total_bytes: 536_870_912,
        },
    });
}

pub(crate) fn fake_sandbox_service(w: &BehaviourWorld) -> FakeSandboxService {
    FakeSandboxService {
        response: w.sandbox.response.clone(),
        stats_response: w.sandbox.stats_response.clone(),
        inspect_image_response: w.sandbox.inspect_image_response.clone(),
        remove_image_response: w.sandbox.remove_image_response.clone(),
        frames: w.sandbox.frames.clone(),
        unreachable: w.sandbox.unreachable,
        policy: w.sandbox.policy.clone(),
        requests: w.sandbox.requests.clone(),
    }
}

#[when(regex = r#"^the user runs "lns inspect ([^"]+)"$"#)]
async fn run_lns_inspect(w: &mut BehaviourWorld, reference: String) {
    let svc = fake_sandbox_service(w);
    let mut out: Vec<u8> = Vec::new();
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let result = run_with_writers(
        &SandboxCommand::Inspect(lns_cli::sandbox::SandboxInspectArgs {
            run: Some(reference),
        }),
        &svc,
        TermInfo::default(),
        &mut out,
        &mut stdout,
        &mut stderr,
    )
    .await;
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

#[when(regex = r#"^the user runs "lns ps"$"#)]
async fn run_lns_ps(w: &mut BehaviourWorld) {
    let svc = fake_sandbox_service(w);
    let mut out: Vec<u8> = Vec::new();
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let result = run_with_writers(
        &SandboxCommand::Ps,
        &svc,
        TermInfo::default(),
        &mut out,
        &mut stdout,
        &mut stderr,
    )
    .await;
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
