use std::sync::{Arc, Mutex};

use crate::runner::CliRun;
use crate::world::BehaviourWorld;
use cucumber::{given, then, when};
use lns_cli::command::parse_args;
use lns_cli::image::ImageArgs;
use lns_cli::image::{self, ImageService};
use lns_cli::integration::LocalBoxFuture;
use lns_ipc::{ImageInfo, Request, Response};

const FIXTURE_PULLED: &str = "2026-06-01T12:00:00Z";

fn fixture_image(reference: &str, digest: &str, size: u64, in_use_by: Option<u32>) -> ImageInfo {
    ImageInfo {
        reference: reference.to_string(),
        digest: digest.to_string(),
        size_bytes: size,
        layers: 1,
        pulled: FIXTURE_PULLED.to_string(),
        in_use_by,
    }
}

fn full_digest(short: &str) -> String {
    let hex = short.strip_prefix("sha256:").unwrap_or(short);
    format!("sha256:{:0<64}", hex)
}

/// Stands in for the running service: answers each image request from the rig's scripted state.
struct FakeImageService {
    images: Vec<ImageInfo>,
    pull_result: Option<ImageInfo>,
    remove_result: Option<(String, u64)>,
    prune_plan: Option<(Vec<String>, u64)>,
    refuse_message: Option<String>,
    unreachable: bool,
    requests: Arc<Mutex<Vec<Request>>>,
}

impl FakeImageService {
    fn from_world(world: &BehaviourWorld) -> Self {
        Self {
            images: world.image.images.clone(),
            pull_result: world.image.pull_result.clone(),
            remove_result: world.image.remove_result.clone(),
            prune_plan: world.image.prune_plan.clone(),
            refuse_message: world.image.refuse_message.clone(),
            unreachable: world.image.unreachable,
            requests: world.image.requests.clone(),
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
            Request::ListImages => Response::ImageList {
                images: self.images.clone(),
            },
            Request::PullImage { .. } => Response::ImagePulled {
                image: self
                    .pull_result
                    .clone()
                    .expect("fixture has no pull result"),
            },
            Request::RemoveImage { .. } => {
                let (reference, reclaimed_bytes) = self
                    .remove_result
                    .clone()
                    .expect("fixture has no remove result");
                Response::ImageRemoved {
                    reference,
                    reclaimed_bytes,
                }
            }
            Request::PruneImages => {
                let (removed, reclaimed_bytes) = self.prune_plan.clone().unwrap_or_default();
                Response::ImagesPruned {
                    removed,
                    reclaimed_bytes,
                }
            }
            other => panic!("unexpected image request {other:?}"),
        })
    }
}

impl ImageService for FakeImageService {
    fn request(&self, req: Request) -> LocalBoxFuture<'_, Option<Response>> {
        self.requests.lock().unwrap().push(req.clone());
        let resp = self.respond(&req);
        Box::pin(async move { resp })
    }
}

#[given(expr = "the service reports a cached image {string} of {int} bytes used by run {int}")]
fn reports_used_image(world: &mut BehaviourWorld, reference: String, size: u64, run: u32) {
    world.image.images.push(fixture_image(
        &reference,
        &full_digest("sha256:aa"),
        size,
        Some(run),
    ));
}

#[given(expr = "the service reports an unused cached image {string} of {int} bytes")]
fn reports_unused_image(world: &mut BehaviourWorld, reference: String, size: u64) {
    world.image.images.push(fixture_image(
        &reference,
        &full_digest("sha256:aa"),
        size,
        None,
    ));
}

#[given(expr = "the service resolves pulls of {string} to digest {string}")]
fn resolves_pull(world: &mut BehaviourWorld, reference: String, digest: String) {
    world.image.pull_result = Some(fixture_image(
        &reference,
        &full_digest(&digest),
        3 * 1024 * 1024,
        None,
    ));
}

#[given(expr = "the service confirms removing {string} reclaims {int} bytes")]
fn confirms_remove(world: &mut BehaviourWorld, reference: String, bytes: u64) {
    world.image.remove_result = Some((reference, bytes));
}

#[given(expr = "the image service refuses with {string}")]
fn image_service_refuses(world: &mut BehaviourWorld, message: String) {
    world.image.refuse_message = Some(message);
}

#[given(expr = "the service will prune images {string} and {string} reclaiming {int} bytes")]
fn prune_plan(world: &mut BehaviourWorld, a: String, b: String, bytes: u64) {
    world.image.prune_plan = Some((vec![a, b], bytes));
}

#[given(expr = "the service will prune no images")]
fn prune_plan_empty(world: &mut BehaviourWorld) {
    world.image.prune_plan = Some((Vec::new(), 0));
}

#[given(expr = "the image service is unreachable")]
fn image_service_unreachable(world: &mut BehaviourWorld) {
    world.image.unreachable = true;
}

#[given(expr = "the user answers {string} to the image prune prompt")]
fn prompt_answer(world: &mut BehaviourWorld, answer: String) {
    world.image.prompt_answer = Some(answer);
}

#[when(expr = "the user runs image command {string}")]
async fn run_image(world: &mut BehaviourWorld, tail: String) {
    let mut argv = vec!["lns".to_string(), "image".to_string()];
    argv.extend(tail.split_whitespace().map(str::to_string));
    let run = match parse_args::<ImageArgs, _, _>(&argv) {
        Ok(args) => {
            let svc = FakeImageService::from_world(world);
            let stdin_text = world
                .image
                .prompt_answer
                .clone()
                .map(|a| format!("{a}\n"))
                .unwrap_or_default();
            let mut input = std::io::Cursor::new(stdin_text);
            let mut buf = Vec::<u8>::new();
            match image::run(&args.command, &svc, &mut input, &mut buf).await {
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

#[then(expr = "the listed image row for {string} ends with {string}")]
fn listed_row_ends_with(world: &mut BehaviourWorld, reference: String, suffix: String) {
    let output = &world.result.as_ref().expect("a CLI run").output;
    let row = output
        .lines()
        .find(|l| l.starts_with(&reference))
        .unwrap_or_else(|| panic!("no row for {reference:?} in {output:?}"));
    assert!(
        row.trim_end().ends_with(&suffix),
        "row {row:?} does not end with {suffix:?}"
    );
}

#[then(expr = "no image request reached the service")]
fn no_request_sent(world: &mut BehaviourWorld) {
    let requests = world.image.requests.lock().unwrap();
    assert!(requests.is_empty(), "got {requests:?}");
}
