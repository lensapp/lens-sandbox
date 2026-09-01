use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::runner::CliRun;
use crate::world::{BehaviourWorld, ScriptedTerminal};
use cucumber::{given, then, when};
use lns_cli::command::parse_args;
use lns_cli::connector::{self, ConnectorArgs, ConnectorService};
use lns_cli::local_future::LocalBoxFuture;
use lns_ipc::{ConnectorConnectionView, ConnectorMethodView, ConnectorView, Request, Response};

const CWD: &str = "/work";
const DIGEST: &str = "sha256:abc";

fn view(name: &str, serves: &str, connections: Vec<&str>) -> ConnectorView {
    ConnectorView {
        name: name.to_string(),
        digest: DIGEST.to_string(),
        serves: vec![serves.to_string()],
        methods: vec![
            ConnectorMethodView {
                name: "token".to_string(),
                label: "API token".to_string(),
                auth_label: Some("token".to_string()),
                offerable: true,
                opens: vec!["api.some-provider.example".to_string()],
                writes: vec!["/home/agent/.some-provider".to_string()],
                env: vec!["SOME_REGION".to_string()],
                credentials: vec!["SOME_TOKEN".to_string()],
                asks: vec!["token".to_string()],
                help: Some("Create one under Settings then Tokens.".to_string()),
                overrides: None,
            },
            ConnectorMethodView {
                name: "open".to_string(),
                label: "open".to_string(),
                auth_label: None,
                offerable: true,
                opens: Vec::new(),
                writes: Vec::new(),
                env: Vec::new(),
                credentials: Vec::new(),
                asks: Vec::new(),
                help: None,
                overrides: None,
            },
        ],
        connections: connections
            .into_iter()
            .map(|label| ConnectorConnectionView {
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
    connected: Option<String>,
    granted: Option<(String, Option<String>)>,
    disconnected: Option<usize>,
    forgot: Option<bool>,
    grant_unchanged: bool,
    run_is_unknown: bool,
    held: Vec<ConnectorView>,
    dropped_connections: Option<usize>,
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
            connected: rig.connected.clone(),
            granted: rig.granted.clone(),
            disconnected: rig.disconnected,
            forgot: rig.forgot,
            grant_unchanged: rig.grant_unchanged,
            run_is_unknown: rig.run_is_unknown,
            held: rig.held.clone(),
            dropped_connections: rig.dropped_connections,
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
            Request::UninstallConnector { name } => match self.dropped_connections {
                Some(dropped_connections) if self.installed_name.as_deref() == Some(name) => {
                    Response::ConnectorUninstalled {
                        name: name.clone(),
                        dropped_connections,
                    }
                }
                _ => Response::ConnectorUnknown { name: name.clone() },
            },
            Request::ConnectConnector {
                name, connection, ..
            } => Response::ConnectorConnected {
                name: name.clone(),
                connection: self.connected.clone().unwrap_or_else(|| connection.clone()),
                invalidated: Vec::new(),
            },
            Request::DisconnectConnector { name, .. } => Response::ConnectorDisconnected {
                name: name.clone(),
                dropped: self.disconnected.unwrap_or(0),
            },
            Request::InspectRun { run } if self.run_is_unknown => {
                Response::RunUnknown { run: run.clone() }
            }
            Request::InspectRun { run } => Response::RunInspect {
                details: Box::new(lns_ipc::RunDetails {
                    summary: lns_ipc::RunSummary {
                        id: "1a2b3c4d0000000000000000000000aa".into(),
                        name: run.clone(),
                        image: "someimage".into(),
                        command: "sh".into(),
                        status: lns_ipc::RunStatus::Running,
                        started: "2026-08-31T00:00:00Z".into(),
                    },
                    config: lns_ipc::RunConfig::default(),
                }),
            },
            Request::GrantConnector { name, method, .. } => {
                let (granted, displaced) = self
                    .granted
                    .clone()
                    .unwrap_or_else(|| (method.clone(), None));
                Response::ConnectorGranted {
                    name: name.clone(),
                    method: granted,
                    connection: None,
                    displaced,
                    unchanged: self.grant_unchanged,
                    reserved: self.run_is_unknown,
                }
            }
            Request::ForgetConnector { name, .. } => Response::ConnectorForgotten {
                name: name.clone(),
                had_decision: self.forgot.unwrap_or(false),
                reserved: self.run_is_unknown,
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

#[given(expr = "the machine holds the connection {string} of {string} for method {string}")]
fn machine_holds_connection(
    world: &mut BehaviourWorld,
    label: String,
    connector: String,
    method: String,
) {
    let held = world
        .connector
        .held
        .iter_mut()
        .find(|view| view.name == connector)
        .expect("the connector must be installed before it holds a connection");
    held.connections.push(lns_ipc::ConnectorConnectionView {
        label,
        method,
        authority: Vec::new(),
    });
}

#[given(expr = "the service holds no connectors")]
fn service_holds_none(world: &mut BehaviourWorld) {
    world.connector.held.clear();
}

#[given(expr = "the service uninstalls {string} dropping {int} connections")]
fn service_uninstalls(world: &mut BehaviourWorld, name: String, dropped: usize) {
    world.connector.installed_name = Some(name);
    world.connector.dropped_connections = Some(dropped);
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
            let answers = world.connector.answers.clone().unwrap_or_default();
            let mut terminal = match &world.connector.answers {
                Some(_) => ScriptedTerminal::answering(
                    &answers.iter().map(String::as_str).collect::<Vec<_>>(),
                ),
                None => ScriptedTerminal::absent(),
            };
            let mut out = Vec::new();
            let mut prompt = Vec::new();
            let outcome = connector::run(
                &args.command,
                &svc,
                &mut terminal,
                &PathBuf::from(CWD),
                None,
                &mut out,
                &mut prompt,
            )
            .await;
            let asked = String::from_utf8(prompt).expect("connector prompt is utf-8");
            let text = asked + &String::from_utf8(out).expect("connector output is utf-8");
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
    assert!(line.contains("connect first"), "got: {line}");
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

#[then(expr = "the output says it holds no connection")]
fn says_no_connection(world: &mut BehaviourWorld) {
    let run = run_of(world);
    let row = run
        .output
        .lines()
        .find(|l| l.contains("some-provider"))
        .unwrap_or_else(|| panic!("no row names the connector: {}", run.output));
    assert!(
        row.trim_end().ends_with("none"),
        "a connector with no connection is the normal state after an install, so the CONNECTIONS cell says so: {row}"
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

#[then(expr = "the output says runs keep what they granted")]
fn says_grants_outlive(world: &mut BehaviourWorld) {
    let run = run_of(world);
    assert!(
        run.output.contains("keeps that decision"),
        "uninstalling stops the offer; it does not retract a grant: {}",
        run.output
    );
}

#[then(expr = "the output says {int} connections were dropped")]
fn says_connections_dropped(world: &mut BehaviourWorld, dropped: usize) {
    let run = run_of(world);
    assert!(
        run.output
            .contains(&format!("dropped {dropped} connection")),
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

#[given(expr = "the service connects {string} as {string}")]
fn service_connects(world: &mut BehaviourWorld, _name: String, connection: String) {
    world.connector.connected = Some(connection);
}

#[given(expr = "the service reports the grant unchanged for {string}")]
fn service_grant_unchanged(world: &mut BehaviourWorld, _name: String) {
    world.connector.grant_unchanged = true;
    world.connector.granted = Some(("token".to_string(), None));
}

#[given(expr = "the service grants {string} the method {string}")]
fn service_grants(world: &mut BehaviourWorld, _name: String, method: String) {
    world.connector.granted = Some((method, None));
}

#[given(expr = "the service grants {string} the method {string} replacing {string}")]
fn service_grants_replacing(
    world: &mut BehaviourWorld,
    _name: String,
    method: String,
    displaced: String,
) {
    world.connector.granted = Some((method, Some(displaced)));
}

#[given(expr = "the service disconnects {string} dropping {int} connections")]
fn service_disconnects(world: &mut BehaviourWorld, _name: String, dropped: usize) {
    world.connector.disconnected = Some(dropped);
}

#[given(expr = "the service forgets a decision about {string}")]
fn service_forgets(world: &mut BehaviourWorld, _name: String) {
    world.connector.forgot = Some(true);
}

#[given(expr = "the service forgets nothing about {string}")]
fn service_forgets_nothing(world: &mut BehaviourWorld, _name: String) {
    world.connector.forgot = Some(false);
}

#[given(expr = "the user types {string}")]
fn the_user_types(world: &mut BehaviourWorld, answer: String) {
    world
        .connector
        .answers
        .get_or_insert_with(Vec::new)
        .push(answer);
}

#[given(expr = "there is no terminal")]
fn there_is_no_terminal(world: &mut BehaviourWorld) {
    world.connector.answers = None;
}

#[then(expr = "the connector output does not contain {string}")]
fn output_omits(world: &mut BehaviourWorld, secret: String) {
    let run = run_of(world);
    assert!(
        !run.output.contains(&secret),
        "a pasted token must never reach the screen: {}",
        run.output
    );
}

#[then(expr = "the output says connecting is not granting")]
fn says_connect_is_not_grant(world: &mut BehaviourWorld) {
    let run = run_of(world);
    assert!(
        run.output.contains("this grants nothing"),
        "got: {}",
        run.output
    );
}

#[then(expr = "the output says it was granted")]
fn says_granted(world: &mut BehaviourWorld) {
    let run = run_of(world);
    assert!(
        run.output.contains("granted some-provider"),
        "got: {}",
        run.output
    );
}

#[then(expr = "the output says nothing was granted")]
fn says_nothing_granted_after_declining(world: &mut BehaviourWorld) {
    let run = run_of(world);
    assert!(
        run.output.contains("nothing was granted"),
        "got: {}",
        run.output
    );
}

#[then(expr = "the output says it replaced {string}")]
fn says_replaced(world: &mut BehaviourWorld, displaced: String) {
    let run = run_of(world);
    assert!(
        run.output.contains(&format!("replaced {displaced}")),
        "got: {}",
        run.output
    );
}

#[then(expr = "the output says it holds no connection to disconnect")]
fn says_no_connection_to_disconnect(world: &mut BehaviourWorld) {
    let run = run_of(world);
    assert!(
        run.output.contains("holds no connection to disconnect"),
        "got: {}",
        run.output
    );
}

#[then(expr = "the output says it stays installed")]
fn says_stays_installed(world: &mut BehaviourWorld) {
    let run = run_of(world);
    assert!(
        run.output.contains("stays installed"),
        "got: {}",
        run.output
    );
}

#[then(expr = "the output says it forgot the decision")]
fn says_forgot(world: &mut BehaviourWorld) {
    let run = run_of(world);
    assert!(
        run.output.contains("cleared reviewer's decision"),
        "got: {}",
        run.output
    );
}

#[then(expr = "the disclosure names what the method opens")]
fn discloses_opens(world: &mut BehaviourWorld) {
    let run = run_of(world);
    assert!(
        run.output.contains("opens") && run.output.contains("api.some-provider.example"),
        "method egress is not bounded by `serves`, so it has to be disclosed: {}",
        run.output
    );
}

#[then(expr = "the disclosure names the file it writes")]
fn discloses_writes(world: &mut BehaviourWorld) {
    let run = run_of(world);
    assert!(
        run.output.contains("/home/agent/.some-provider"),
        "got: {}",
        run.output
    );
}

#[then(expr = "the disclosure names the variables it sets")]
fn discloses_sets(world: &mut BehaviourWorld) {
    let run = run_of(world);
    for named in ["SOME_REGION", "SOME_TOKEN"] {
        assert!(
            run.output.contains(named),
            "{named} missing: {}",
            run.output
        );
    }
}

#[given(expr = "no run answers to that name")]
fn no_run_answers(world: &mut BehaviourWorld) {
    world.connector.run_is_unknown = true;
}

#[then(expr = "the disclosure says no run is named {string}")]
fn discloses_no_such_run(world: &mut BehaviourWorld, run_name: String) {
    let run = run_of(world);
    assert!(
        run.output.contains(&format!("no run is named {run_name}")),
        "the user learns it is a reservation before consenting, not after: {}",
        run.output
    );
}

#[then(expr = "the output says it reserved the decision")]
fn says_reserved(world: &mut BehaviourWorld) {
    let run = run_of(world);
    assert!(
        run.output.contains("reserved some-provider"),
        "the service decides whether it reserved, and the output reports what it wrote: {}",
        run.output
    );
}

#[then(expr = "the output says it forgot the reservation")]
fn says_forgot_reservation(world: &mut BehaviourWorld) {
    let run = run_of(world);
    assert!(
        run.output.contains("reservation about some-provider"),
        "got: {}",
        run.output
    );
}

#[then(expr = "the disclosure names the run {string}")]
fn discloses_the_run(world: &mut BehaviourWorld, run_name: String) {
    let run = run_of(world);
    assert!(
        run.output
            .contains(&format!("granting some-provider to {run_name}")),
        "a grant belongs to a run, so the disclosure has to name which: {}",
        run.output
    );
}

#[then(expr = "the disclosure says the method carries nothing to disclose")]
fn discloses_nothing(world: &mut BehaviourWorld) {
    let run = run_of(world);
    assert!(
        run.output.contains("opens    nothing"),
        "an empty payload is stated in its own column rather than left looking omitted: {}",
        run.output
    );
}

#[then(expr = "the output says it was already granted")]
fn says_already_granted(world: &mut BehaviourWorld) {
    let run = run_of(world);
    assert!(run.output.contains("already grants"), "got: {}", run.output);
}

#[then(expr = "the prompt suggests the name {string}")]
fn prompt_suggests_the_name(world: &mut BehaviourWorld, suggested: String) {
    let run = run_of(world);
    assert!(
        run.output.contains(&format!("[{suggested}]")),
        "the mechanism suggests a name and the user confirms it (cli-spec §3.3): {}",
        run.output
    );
}

#[then(expr = "the prompt names the authentication {string}")]
fn prompt_names_the_authentication(world: &mut BehaviourWorld, authentication: String) {
    let run = run_of(world);
    assert!(
        run.output.contains(&authentication),
        "the ask names the sign-in the value comes from, which is what the user pastes: {}",
        run.output
    );
}

#[then(expr = "the prompt says the value is not shown")]
fn prompt_says_hidden(world: &mut BehaviourWorld) {
    let run = run_of(world);
    assert!(run.output.contains("not shown"), "got: {}", run.output);
}
