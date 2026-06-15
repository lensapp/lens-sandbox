use std::sync::{Arc, Mutex};

use crate::runner::CliRun;
use crate::world::BehaviourWorld;
use cucumber::{given, then, when};
use lns_cli::cli::VolumeArgs;
use lns_cli::command::parse_args;
use lns_cli::integration::LocalBoxFuture;
use lns_cli::volume::{self, VolumeService};
use lns_ipc::{Request, Response, VolumeInfo, VolumePruneFailure};

const FIXTURE_CREATED: &str = "2026-06-01T12:00:00Z";
const FIXTURE_SIZE_BYTES: u64 = 10 * 1024 * 1024 * 1024;

fn fixture_volume(name: &str, disk_bytes: u64, in_use_by: Option<u32>) -> VolumeInfo {
    VolumeInfo {
        name: name.to_string(),
        size_bytes: FIXTURE_SIZE_BYTES,
        disk_bytes,
        created: FIXTURE_CREATED.to_string(),
        in_use_by,
    }
}

/// Stands in for the running service: answers each volume request from the rig's scripted store state.
struct FakeVolumeService {
    volumes: Vec<VolumeInfo>,
    prune_plan: Option<(Vec<String>, u64)>,
    prune_failed: Vec<VolumePruneFailure>,
    refuse_message: Option<String>,
    unreachable: bool,
    requests: Arc<Mutex<Vec<Request>>>,
}

impl FakeVolumeService {
    fn from_world(world: &BehaviourWorld) -> Self {
        Self {
            volumes: world.volume.volumes.clone(),
            prune_plan: world.volume.prune_plan.clone(),
            prune_failed: world.volume.prune_failed.clone(),
            refuse_message: world.volume.refuse_message.clone(),
            unreachable: world.volume.unreachable,
            requests: world.volume.requests.clone(),
        }
    }

    fn respond(&self, req: &Request) -> Option<Response> {
        if self.unreachable {
            return None;
        }
        if let Some(message) = &self.refuse_message {
            return Some(Response::Error {
                message: message.clone(),
            });
        }
        Some(match req {
            Request::ListVolumes => Response::VolumeList {
                volumes: self.volumes.clone(),
            },
            Request::InspectVolume { name } => Response::VolumeInspect {
                volume: self
                    .volumes
                    .iter()
                    .find(|v| &v.name == name)
                    .unwrap_or_else(|| panic!("fixture has no volume {name:?}"))
                    .clone(),
            },
            Request::CreateVolume { name } => Response::VolumeCreated {
                volume: fixture_volume(name, 32 * 1024 * 1024, None),
            },
            Request::RemoveVolume { name } => Response::VolumeRemoved { name: name.clone() },
            Request::PruneVolumes => {
                let (removed, reclaimed_bytes) = self.prune_plan.clone().unwrap_or_default();
                Response::VolumesPruned {
                    removed,
                    reclaimed_bytes,
                    failed: self.prune_failed.clone(),
                }
            }
            other => panic!("unexpected volume request {other:?}"),
        })
    }
}

impl VolumeService for FakeVolumeService {
    fn request(&self, req: Request) -> LocalBoxFuture<'_, Option<Response>> {
        self.requests.lock().unwrap().push(req.clone());
        let resp = self.respond(&req);
        Box::pin(async move { resp })
    }
}

#[given(expr = "the service reports a volume {string} using {int} bytes on disk held by run {int}")]
fn reports_held_volume(world: &mut BehaviourWorld, name: String, disk: u64, run: u32) {
    world
        .volume
        .volumes
        .push(fixture_volume(&name, disk, Some(run)));
}

#[given(expr = "the service reports an idle volume {string} using {int} bytes on disk")]
fn reports_idle_volume(world: &mut BehaviourWorld, name: String, disk: u64) {
    world.volume.volumes.push(fixture_volume(&name, disk, None));
}

#[given(expr = "the service refuses with {string}")]
fn service_refuses(world: &mut BehaviourWorld, message: String) {
    world.volume.refuse_message = Some(message);
}

#[given(expr = "the service will prune volumes {string} and {string} reclaiming {int} bytes")]
fn prune_plan(world: &mut BehaviourWorld, a: String, b: String, bytes: u64) {
    world.volume.prune_plan = Some((vec![a, b], bytes));
}

#[given(expr = "the service will prune no volumes")]
fn prune_plan_empty(world: &mut BehaviourWorld) {
    world.volume.prune_plan = Some((Vec::new(), 0));
}

#[given(expr = "the service will fail to prune {string} with {string}")]
fn prune_failure(world: &mut BehaviourWorld, name: String, error: String) {
    world
        .volume
        .prune_failed
        .push(VolumePruneFailure { name, error });
}

#[given(expr = "the service is unreachable")]
fn service_unreachable(world: &mut BehaviourWorld) {
    world.volume.unreachable = true;
}

#[given(expr = "the user will answer {string} to the prompt")]
fn prompt_answer(world: &mut BehaviourWorld, answer: String) {
    world.volume.prompt_answer = Some(answer);
}

#[when(expr = "the user runs volume command {string}")]
async fn run_volume(world: &mut BehaviourWorld, tail: String) {
    let mut argv = vec!["lns".to_string(), "volume".to_string()];
    argv.extend(tail.split_whitespace().map(str::to_string));
    let run = match parse_args::<VolumeArgs, _, _>(&argv) {
        Ok(args) => {
            let svc = FakeVolumeService::from_world(world);
            let stdin_text = world
                .volume
                .prompt_answer
                .clone()
                .map(|a| format!("{a}\n"))
                .unwrap_or_default();
            let mut input = std::io::Cursor::new(stdin_text);
            let mut buf = Vec::<u8>::new();
            match volume::run(&args.command, &svc, &mut input, &mut buf).await {
                Ok(exit_code) => CliRun {
                    exit_code,
                    output: String::from_utf8_lossy(&buf).into_owned(),
                },
                Err(e) => CliRun {
                    exit_code: 1,
                    output: format!("{}{e:#}", String::from_utf8_lossy(&buf)),
                },
            }
        }
        Err(e) => CliRun {
            exit_code: e.exit_code(),
            output: e.to_string(),
        },
    };
    world.result = Some(run);
}

#[then(expr = "the output is JSON describing the idle volume {string} using {int} bytes on disk")]
fn output_is_volume_json(world: &mut BehaviourWorld, name: String, disk: u64) {
    let output = &world.result.as_ref().expect("a CLI run").output;
    let parsed: serde_json::Value =
        serde_json::from_str(output).unwrap_or_else(|e| panic!("not JSON ({e}): {output:?}"));
    assert_eq!(parsed["name"], serde_json::Value::String(name));
    assert_eq!(parsed["size_bytes"], serde_json::json!(FIXTURE_SIZE_BYTES));
    assert_eq!(parsed["disk_bytes"], serde_json::json!(disk));
    assert_eq!(parsed["in_use_by"], serde_json::Value::Null);
}

#[then(expr = "the listed row for {string} ends with {string}")]
fn listed_row_ends_with(world: &mut BehaviourWorld, name: String, suffix: String) {
    let output = &world.result.as_ref().expect("a CLI run").output;
    let row = output
        .lines()
        .find(|l| l.starts_with(&name))
        .unwrap_or_else(|| panic!("no row for {name:?} in {output:?}"));
    assert!(
        row.trim_end().ends_with(&suffix),
        "row {row:?} does not end with {suffix:?}"
    );
}

#[then(expr = "no request reached the service")]
fn no_request_sent(world: &mut BehaviourWorld) {
    let requests = world.volume.requests.lock().unwrap();
    assert!(requests.is_empty(), "got {requests:?}");
}
