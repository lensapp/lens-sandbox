use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::runner::CliRun;
use crate::world::BehaviourWorld;
use cucumber::{given, then, when};
use lns_cli::command::parse_args;
use lns_cli::connector::{self, ConnectorArgs, ConnectorService};
use lns_cli::local_future::LocalBoxFuture;
use lns_ipc::{ConnectorMethodView, ConnectorProfileView, ConnectorView, Request, Response};

const CWD: &str = "/work";
const DIGEST: &str = "sha256:abc";

fn view(name: &str, serves: &str, profiles: Vec<&str>) -> ConnectorView {
    ConnectorView {
        name: name.to_string(),
        digest: DIGEST.to_string(),
        serves: vec![serves.to_string()],
        methods: vec![
            ConnectorMethodView {
                name: "token".to_string(),
                label: "API token".to_string(),
                needs_connect: true,
                offerable: true,
            },
            ConnectorMethodView {
                name: "open".to_string(),
                label: "open".to_string(),
                needs_connect: false,
                offerable: true,
            },
        ],
        profiles: profiles
            .into_iter()
            .map(|label| ConnectorProfileView {
                label: label.to_string(),
                method: "token".to_string(),
                authority: Vec::new(),
            })
            .collect(),
    }
}

/// Stands in for the running service: answers each connector request from the rig's scripted state.
struct FakeConnectorService {
    installed: Option<ConnectorView>,
    installed_name: Option<String>,
    held: Vec<ConnectorView>,
    dropped_profiles: Option<usize>,
    refuse_message: Option<String>,
    unreachable: bool,
    requests: Arc<Mutex<Vec<Request>>>,
}

impl FakeConnectorService {
    fn from_world(world: &BehaviourWorld) -> Self {
        let rig = &world.connector;
        Self {
            installed: rig.installed.clone(),
            installed_name: rig.installed_name.clone(),
            held: rig.held.clone(),
            dropped_profiles: rig.dropped_profiles,
            refuse_message: rig.refuse_message.clone(),
            unreachable: rig.unreachable,
            requests: rig.requests.clone(),
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
            Request::InstallConnector { .. } => Response::ConnectorInstalled {
                connector: self
                    .installed
                    .clone()
                    .expect("the scenario staged no connector to install"),
            },
            Request::ListConnectors => Response::ConnectorList {
                connectors: self.held.clone(),
            },
            Request::UninstallConnector { name } => match self.dropped_profiles {
                Some(dropped_profiles) if self.installed_name.as_deref() == Some(name) => {
                    Response::ConnectorUninstalled {
                        name: name.clone(),
                        dropped_profiles,
                    }
                }
                _ => Response::ConnectorUnknown { name: name.clone() },
            },
            other => panic!("unexpected connector request {other:?}"),
        })
    }
}

impl ConnectorService for FakeConnectorService {
    fn request(&self, req: Request) -> LocalBoxFuture<'_, Option<Response>> {
        self.requests.lock().unwrap().push(req.clone());
        let resp = self.respond(&req);
        Box::pin(async move { resp })
    }
}

#[given(expr = "the service installs {string} serving {string}")]
fn service_installs(world: &mut BehaviourWorld, name: String, serves: String) {
    world.connector.installed = Some(view(&name, &serves, Vec::new()));
}

#[given(expr = "the service holds the connector {string} serving {string}")]
fn service_holds(world: &mut BehaviourWorld, name: String, serves: String) {
    world.connector.held.push(view(&name, &serves, Vec::new()));
}

#[given(expr = "the service holds no connectors")]
fn service_holds_none(world: &mut BehaviourWorld) {
    world.connector.held.clear();
}

#[given(expr = "the service uninstalls {string} dropping {int} profiles")]
fn service_uninstalls(world: &mut BehaviourWorld, name: String, dropped: usize) {
    world.connector.installed_name = Some(name);
    world.connector.dropped_profiles = Some(dropped);
}

#[given(expr = "the connector service refuses with {string}")]
fn connector_service_refuses(world: &mut BehaviourWorld, message: String) {
    world.connector.refuse_message = Some(message);
}

#[given(expr = "the connector service is unreachable")]
fn service_unreachable(world: &mut BehaviourWorld) {
    world.connector.unreachable = true;
}

#[when(expr = "the user runs connector command {string}")]
async fn run_connector(world: &mut BehaviourWorld, tail: String) {
    let mut argv = vec!["lns".to_string(), "connector".to_string()];
    argv.extend(tail.split_whitespace().map(str::to_string));
    let run = match parse_args::<ConnectorArgs, _, _>(&argv) {
        Ok(args) => {
            let svc = FakeConnectorService::from_world(world);
            let mut out = Vec::new();
            let outcome = connector::run(&args.command, &svc, &PathBuf::from(CWD), &mut out).await;
            let text = String::from_utf8(out).expect("connector output is utf-8");
            match outcome {
                Ok(exit_code) => CliRun {
                    exit_code,
                    output: text,
                },
                Err(e) => CliRun {
                    exit_code: 1,
                    output: format!("{text}error: {e:#}"),
                },
            }
        }
        Err(e) => CliRun {
            exit_code: 2,
            output: format!("error: {e}"),
        },
    };
    world.connector.run = Some(run);
}

fn run_of(world: &BehaviourWorld) -> &CliRun {
    world
        .connector
        .run
        .as_ref()
        .expect("a connector command must have run")
}

#[then(expr = "the connector command succeeds")]
fn command_succeeds(world: &mut BehaviourWorld) {
    let run = run_of(world);
    assert_eq!(run.exit_code, 0, "got: {}", run.output);
}

#[then(expr = "the connector command fails")]
fn command_fails(world: &mut BehaviourWorld) {
    let run = run_of(world);
    assert_ne!(run.exit_code, 0, "got: {}", run.output);
}

#[then(expr = "the connector command exits {int}")]
fn command_exits(world: &mut BehaviourWorld, code: i32) {
    let run = run_of(world);
    assert_eq!(run.exit_code, code, "got: {}", run.output);
}

#[then(expr = "the output says nothing is granted yet")]
fn says_nothing_granted(world: &mut BehaviourWorld) {
    let run = run_of(world);
    assert!(
        run.output.contains("nothing is granted yet"),
        "installing grants nothing, and the output has to say so: {}",
        run.output
    );
}

#[then(expr = "the output names the destination {string}")]
fn names_destination(world: &mut BehaviourWorld, destination: String) {
    let run = run_of(world);
    assert!(run.output.contains(&destination), "got: {}", run.output);
}

#[then(expr = "the output says the method {string} needs a connect")]
fn says_method_needs_connect(world: &mut BehaviourWorld, label: String) {
    let run = run_of(world);
    let line = run
        .output
        .lines()
        .find(|l| l.contains(&label))
        .unwrap_or_else(|| panic!("no line names {label}: {}", run.output));
    assert!(line.contains("connect to use"), "got: {line}");
}

#[then(expr = "the service was asked to install an absolute path")]
fn asked_with_absolute_path(world: &mut BehaviourWorld) {
    let asked = installed_source(world);
    assert_eq!(
        asked, "/work/some-provider",
        "the service's working directory is not the user's, so `lns` sends the absolute path"
    );
}

#[then(expr = "the service was asked to install {string}")]
fn asked_with_source(world: &mut BehaviourWorld, source: String) {
    assert_eq!(installed_source(world), source);
}

fn installed_source(world: &BehaviourWorld) -> String {
    world
        .connector
        .requests
        .lock()
        .unwrap()
        .iter()
        .find_map(|req| match req {
            Request::InstallConnector { source } => Some(source.clone()),
            _ => None,
        })
        .expect("an install request must have been sent")
}

#[then(expr = "the connector error says {string}")]
fn error_says(world: &mut BehaviourWorld, message: String) {
    let run = run_of(world);
    assert!(run.output.contains(&message), "got: {}", run.output);
}

#[then(expr = "the connector output names {string}")]
fn output_names(world: &mut BehaviourWorld, name: String) {
    let run = run_of(world);
    assert!(run.output.contains(&name), "got: {}", run.output);
}

#[then(expr = "the output says it holds no profile")]
fn says_no_profile(world: &mut BehaviourWorld) {
    let run = run_of(world);
    let row = run
        .output
        .lines()
        .find(|l| l.contains("some-provider"))
        .unwrap_or_else(|| panic!("no row names the connector: {}", run.output));
    assert!(
        row.trim_end().ends_with("none"),
        "a connector with no profile is the normal state after an install, so the PROFILES cell says so: {row}"
    );
}

#[then(expr = "the output says no connectors are installed")]
fn says_none_installed(world: &mut BehaviourWorld) {
    let run = run_of(world);
    assert!(
        run.output.contains("No connectors installed."),
        "got: {}",
        run.output
    );
}

#[then(expr = "the output says projects keep what they granted")]
fn says_grants_outlive(world: &mut BehaviourWorld) {
    let run = run_of(world);
    assert!(
        run.output.contains("keep that decision"),
        "uninstalling stops the offer; it does not retract a grant: {}",
        run.output
    );
}

#[then(expr = "the output says {int} profiles were dropped")]
fn says_profiles_dropped(world: &mut BehaviourWorld, dropped: usize) {
    let run = run_of(world);
    assert!(
        run.output.contains(&format!("dropped {dropped} profile")),
        "got: {}",
        run.output
    );
}

#[then(expr = "the output says no connector named {string} is installed")]
fn says_not_installed(world: &mut BehaviourWorld, name: String) {
    let run = run_of(world);
    assert!(
        run.output
            .contains(&format!("no connector named {name} is installed")),
        "got: {}",
        run.output
    );
}

#[then(expr = "the connector error mentions lns-service")]
fn error_mentions_service(world: &mut BehaviourWorld) {
    let run = run_of(world);
    assert!(run.output.contains("lns-service"), "got: {}", run.output);
}
