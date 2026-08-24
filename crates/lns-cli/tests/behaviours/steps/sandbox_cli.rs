use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use cucumber::{given, then, when};
use lns_cli::artifact::{ArtifactArgs, ArtifactCommand, author, distribute};
use lns_cli::command::parse_args;
use lns_cli::connector::LocalBoxFuture;
use lns_cli::sandbox::{SandboxArgs, SandboxCommand};
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

#[given("the policy file parses with one allow rule")]
fn canned_policy_doc(w: &mut BehaviourWorld) {
    w.sandbox.policy = Some(serde_json::json!({
        "network": { "egress": { "http": [{ "match": "api.example.test", "verdict": "allow" }] } }
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
    fn is_dir(&self, path: &Path) -> bool {
        self.files
            .borrow()
            .keys()
            .any(|held| held.ancestors().skip(1).any(|dir| dir == path))
    }

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
    uploaded: RefCell<Vec<(String, lns_artifact::build::BuiltArtifact)>>,
    /// A mixin push that must succeed even while the sandbox push is scripted to fail, so a partial-publish scenario can assert what landed.
    fail_after: Option<usize>,
}

impl distribute::Producer for StepProducer {
    fn push_built<'a>(
        &'a self,
        built: &'a lns_artifact::build::BuiltArtifact,
        reference: &'a str,
    ) -> LocalBoxFuture<'a, anyhow::Result<()>> {
        self.uploaded
            .borrow_mut()
            .push((reference.to_string(), built.clone()));
        let landed = self.uploaded.borrow().len();
        let outcome = match self.fail_after {
            Some(limit) if landed > limit => Err(anyhow::anyhow!("registry refused the upload")),
            _ => self
                .outcome
                .clone()
                .map_err(|m| anyhow::anyhow!(m))
                .map(|_| ()),
        };
        Box::pin(async move { outcome })
    }
}

struct StepResolver {
    versions: HashMap<String, String>,
    unlisted: std::collections::HashSet<String>,
}

impl distribute::ToolResolver for StepResolver {
    fn resolve<'a>(
        &'a self,
        tool: &'a lns_artifact::tools::ToolRef,
    ) -> LocalBoxFuture<'a, anyhow::Result<String>> {
        let outcome = self
            .versions
            .get(&tool.to_string())
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("tool {:?} is unknown to the version index", tool.name));
        Box::pin(async move { outcome })
    }

    fn verify<'a>(
        &'a self,
        tool: &'a lns_artifact::tools::ToolRef,
    ) -> LocalBoxFuture<'a, distribute::IndexVerification> {
        let verification = if self.unlisted.contains(&tool.to_string()) {
            distribute::IndexVerification::Absent
        } else {
            distribute::IndexVerification::Unavailable
        };
        Box::pin(async move { verification })
    }
}

fn run_author_verb(w: &mut BehaviourWorld, cmd: &ArtifactCommand) {
    let cwd = Path::new("/work");
    let fs = StepFs {
        files: RefCell::new(w.author_files.clone()),
    };
    let mut out: Vec<u8> = Vec::new();
    let result = match cmd {
        ArtifactCommand::Init(args) => {
            author::init(&fs, cwd, args.kind, args.file.as_deref(), &mut out)
        }
        ArtifactCommand::Validate(args) => {
            author::validate(&fs, cwd, args.kind, args.file.as_deref(), &mut out)
        }
        ArtifactCommand::Inspect(args) => author::inspect_local(
            &fs,
            cwd,
            args.reference.as_deref(),
            args.file.as_deref(),
            &args.mixins,
            &mut out,
        ),
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

#[when(regex = r#"^the user runs sandbox command "([^"]+)"$"#)]
async fn run_sandbox_command(w: &mut BehaviourWorld, cmd: String) {
    drive_sandbox_command(w, &cmd).await;
}

pub(crate) async fn drive_sandbox_command(w: &mut BehaviourWorld, cmd: &str) {
    let mut argv: Vec<&str> = vec!["lns", "sandbox"];
    argv.extend(cmd.split_whitespace());
    let args: SandboxArgs = parse_args(&argv).expect("sandbox argv must parse");
    let svc = fake_sandbox_service(w);
    let mut out: Vec<u8> = Vec::new();
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let result = run_with_writers(
        &args.command,
        &svc,
        TermInfo {
            stdin_is_tty: w.sandbox.stdin_is_tty,
            stdout_is_terminal: false,
        },
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

#[when(regex = r#"^the user runs artifact command "([^"]+)"$"#)]
async fn run_artifact_command(w: &mut BehaviourWorld, cmd: String) {
    drive_artifact_command(w, &cmd).await;
}

pub(crate) async fn drive_artifact_command(w: &mut BehaviourWorld, cmd: &str) {
    let mut argv: Vec<&str> = vec!["lns", "artifact"];
    argv.extend(cmd.split_whitespace());
    let mut args: ArtifactArgs = parse_args(&argv).expect("artifact argv must parse");
    lns_cli::artifact::apply_registry_default(&mut args.command, None);

    if author::is_offline(&args.command) {
        run_author_verb(w, &args.command);
        return;
    }

    if let ArtifactCommand::Push(push_args) = &args.command {
        run_push_verb(w, push_args).await;
        return;
    }

    let svc = fake_sandbox_service(w);
    let mut out: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let answer = w
        .sandbox
        .prompt_answer
        .clone()
        .map(|answer| format!("{answer}\n"))
        .unwrap_or_default();
    let result = lns_cli::artifact::run_with_writers(
        &args.command,
        &svc,
        TermInfo {
            stdin_is_tty: w.sandbox.stdin_is_tty,
            stdout_is_terminal: false,
        },
        &mut std::io::Cursor::new(answer),
        &mut out,
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

async fn run_push_verb(w: &mut BehaviourWorld, push_args: &lns_cli::artifact::PushArgs) {
    let fs = StepFs {
        files: RefCell::new(w.author_files.clone()),
    };
    let producer = StepProducer {
        outcome: w.push_outcome.clone().unwrap_or(Err(
            "the push must refuse before reaching the producer".into(),
        )),
        uploaded: RefCell::new(Vec::new()),
        fail_after: w.push_fails_after,
    };
    let mut out: Vec<u8> = Vec::new();
    let path = author::selected_definition_path(push_args.file.as_deref(), Path::new("/work"));
    let project_dir = path.parent().unwrap_or(Path::new("/work")).to_path_buf();
    let result = match author::load_definition_json_at(&fs, &path) {
        Ok(doc) if push_args.dry_run => {
            distribute::push_dry_run(&fs, &project_dir, &doc, &push_args.reference, &mut out)
        }
        Ok(doc) => {
            let resolver = StepResolver {
                versions: w.tool_index.clone(),
                unlisted: w.unlisted_pins.clone(),
            };
            let mut input = std::io::Cursor::new(
                w.sandbox
                    .prompt_answer
                    .clone()
                    .unwrap_or_default()
                    .into_bytes(),
            );
            distribute::push(
                distribute::PushPorts {
                    fs: &fs,
                    cwd: &project_dir,
                    producer: &producer,
                    resolver: &resolver,
                },
                &doc,
                &push_args.reference,
                distribute::Confirm {
                    assume_yes: push_args.assume_yes,
                    interactive: w.sandbox.stdin_is_tty,
                    input: &mut input,
                },
                &mut out,
            )
            .await
        }
        Err(e) => Err(e),
    };
    let uploaded = producer.uploaded.into_inner();
    w.pushed_refs = uploaded
        .iter()
        .map(|(reference, _)| reference.clone())
        .collect();
    w.pushed_layers = uploaded
        .last()
        .map(|(_, built)| built.fileset_layers().map(|l| l.digest.clone()).collect())
        .unwrap_or_default();
    w.pushed_doc = uploaded.last().and_then(|(_, built)| {
        built
            .blobs
            .iter()
            .find(|blob| blob.media_type.ends_with(".config.v1+json"))
            .map(|blob| blob.data.clone())
    });
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

#[given(regex = r#"^the published sandbox declares tool "([^"]+)"$"#)]
fn published_sandbox_declares_tool(w: &mut BehaviourWorld, tool: String) {
    let Some(Response::ImageInspected { inspection }) = &mut w.sandbox.inspect_image_response
    else {
        panic!("the registry sandbox must be staged before its tools");
    };
    let lns_ipc::ArtifactInspection::Sandbox(view) = inspection else {
        panic!("the staged artifact must be a sandbox");
    };
    view.tools.push(tool);
}

#[given(regex = r#"^the user will answer "([^"]+)" to the sandbox prompt$"#)]
fn sandbox_prompt_answer(w: &mut BehaviourWorld, answer: String) {
    w.sandbox.prompt_answer = Some(answer);
    w.sandbox.stdin_is_tty = true;
}

#[given("sandbox input is non-interactive")]
fn sandbox_input_is_noninteractive(w: &mut BehaviourWorld) {
    w.sandbox.stdin_is_tty = false;
}

#[then("the service received no pull request")]
fn service_received_no_pull(w: &mut BehaviourWorld) {
    let requests = w.sandbox.requests.lock().unwrap();
    assert!(
        !requests
            .iter()
            .any(|request| matches!(request, Request::PullImage { .. })),
        "got {requests:?}"
    );
}

#[then("the pull request is bound to the inspected digest")]
fn pull_is_bound_to_inspected_digest(w: &mut BehaviourWorld) {
    let requests = w.sandbox.requests.lock().unwrap();
    let inspected = requests.iter().find_map(|request| match request {
        Request::InspectImage { image, .. } => Some(image),
        _ => None,
    });
    let pulled = requests.iter().find_map(|request| match request {
        Request::PullImage {
            image,
            expected_digest,
        } => Some((image, expected_digest)),
        _ => None,
    });
    let (pulled_image, expected_digest) = pulled.expect("a pull request");
    assert_eq!(Some(pulled_image), inspected);
    assert_eq!(expected_digest, &format!("sha256:{}", "a".repeat(64)));
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

#[given(regex = r"^the service reports one running sandbox whose stats probe fails$")]
fn canned_running_with_a_failing_stats_probe(w: &mut BehaviourWorld) {
    canned_running_with_stats(w, 0, 0);
    w.sandbox.stats_response = Some(Response::Error {
        message: "sampling guest stats failed: opening capture vsock to broker: \
                  connect_to_guest_port(1029) timed out after 10s"
            .into(),
    });
}

#[given(regex = r"^the service reports no runs$")]
fn canned_no_runs(w: &mut BehaviourWorld) {
    w.sandbox.response = Some(Response::RunList { runs: Vec::new() });
}

#[when(regex = r#"^the user runs "lns inspect ([^"]+)"$"#)]
async fn run_lns_inspect(w: &mut BehaviourWorld, tail: String) {
    let mut reference = None;
    let mut mixins = Vec::new();
    let mut words = tail.split_whitespace();
    while let Some(word) = words.next() {
        match word {
            "--mixin" => mixins.extend(words.next().map(str::to_string)),
            target => reference = Some(target.to_string()),
        }
    }
    let svc = fake_sandbox_service(w);
    let Some(reference) = reference else { return };
    // The shortcut asks both namespaces and refuses to guess; only a settled answer runs.
    let owner = match lns_cli::shortcut::which(&svc, "inspect", &reference).await {
        Ok(owner) => owner,
        Err(e) => {
            w.result = Some(CliRun {
                exit_code: 1,
                output: format!("{e:#}"),
            });
            return;
        }
    };
    let mut out: Vec<u8> = Vec::new();
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let result = if owner == lns_cli::shortcut::Owner::Artifact {
        lns_cli::artifact::run_with_writers(
            &lns_cli::artifact::ArtifactCommand::Inspect(lns_cli::artifact::InspectArgs {
                reference: Some(reference),
                mixins,
                file: None,
            }),
            &svc,
            TermInfo::default(),
            &mut std::io::Cursor::new(""),
            &mut out,
            &mut stderr,
        )
        .await
    } else {
        run_with_writers(
            &SandboxCommand::Inspect(lns_cli::sandbox::SandboxInspectArgs {
                output: lns_cli::output::OutputArgs {
                    format: lns_cli::output::Format::Table,
                },
                run: reference,
            }),
            &svc,
            TermInfo::default(),
            &mut out,
            &mut stdout,
            &mut stderr,
        )
        .await
    };
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

#[when(regex = r#"^the user runs "lns rm ([^"]+)"$"#)]
async fn run_lns_rm(w: &mut BehaviourWorld, operand: String) {
    let svc = fake_sandbox_service(w);
    let owner = match lns_cli::shortcut::which(&svc, "rm", &operand).await {
        Ok(owner) => owner,
        Err(e) => {
            w.result = Some(CliRun {
                exit_code: 1,
                output: format!("{e:#}"),
            });
            return;
        }
    };
    let mut out: Vec<u8> = Vec::new();
    let mut stdout: Vec<u8> = Vec::new();
    let mut stderr: Vec<u8> = Vec::new();
    let result = if owner == lns_cli::shortcut::Owner::Artifact {
        lns_cli::artifact::run_with_writers(
            &lns_cli::artifact::ArtifactCommand::Rm(lns_cli::artifact::RmArgs {
                reference: operand,
            }),
            &svc,
            TermInfo::default(),
            &mut std::io::Cursor::new(""),
            &mut out,
            &mut stderr,
        )
        .await
    } else {
        run_with_writers(
            &SandboxCommand::Rm(lns_cli::sandbox::SandboxRmArgs { run: operand }),
            &svc,
            TermInfo::default(),
            &mut out,
            &mut stdout,
            &mut stderr,
        )
        .await
    };
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
        &SandboxCommand::Ls(lns_cli::sandbox::SandboxLsArgs {
            output: lns_cli::output::OutputArgs {
                format: lns_cli::output::Format::Table,
            },
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
