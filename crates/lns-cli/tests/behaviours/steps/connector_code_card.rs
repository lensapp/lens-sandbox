use cucumber::{given, then};

use crate::world::BehaviourWorld;

const CONNECTOR: &str = "some-provider";
const DIGEST: &str = "sha256:abc";

/// The whole world these scenarios need: an installed connector whose one method carries code, a service that grants it, and a user who says yes.
#[given(expr = "a connector whose method {string} is a code method")]
fn a_connector_whose_method_is_a_code_method(world: &mut BehaviourWorld, method: String) {
    world.connector.held.push(lns_ipc::ConnectorView {
        name: CONNECTOR.to_string(),
        digest: DIGEST.to_string(),
        serves: vec!["api.some-provider.example".to_string()],
        methods: vec![lns_ipc::ConnectorMethodView {
            name: method.clone(),
            label: method.clone(),
            auth_label: Some("code".to_string()),
            offerable: true,
            opens: Vec::new(),
            writes: Vec::new(),
            env: Vec::new(),
            credentials: vec!["SOME_TOKEN".to_string()],
            asks: Vec::new(),
            help: None,
            overrides: None,
            hosts: Vec::new(),
            runs_programs: false,
            carries_code: true,
        }],
        connections: Vec::new(),
    });
    world.connector.granted = Some((method, None));
    world.connector.answers = Some(vec!["y".to_string()]);
}

#[given(expr = "the method declares the hosts {string} and {string}")]
fn the_method_declares_the_hosts(world: &mut BehaviourWorld, first: String, second: String) {
    method_of(world).hosts = vec![first, second];
}

#[given(expr = "the method declares no hosts")]
fn the_method_declares_no_hosts(world: &mut BehaviourWorld) {
    method_of(world).hosts.clear();
}

#[given(expr = "the method declares host execution")]
fn the_method_declares_host_execution(world: &mut BehaviourWorld) {
    method_of(world).runs_programs = true;
}

#[given(expr = "the method declares no host execution")]
fn the_method_declares_no_host_execution(world: &mut BehaviourWorld) {
    method_of(world).runs_programs = false;
}

#[then(expr = "the disclosure names both hosts it may contact")]
fn the_disclosure_names_both_hosts(world: &mut BehaviourWorld) {
    let hosts = method_of(world).hosts.clone();
    assert_eq!(hosts.len(), 2, "this scenario declares two hosts");
    let output = world
        .connector
        .run
        .as_ref()
        .expect("a connector command must have run")
        .output
        .clone();
    for host in hosts {
        assert!(
            output.contains(&host),
            "a bound the card does not name is one the user cannot weigh: {output}"
        );
    }
}

fn method_of(world: &mut BehaviourWorld) -> &mut lns_ipc::ConnectorMethodView {
    world
        .connector
        .held
        .last_mut()
        .expect("the connector must be described before its method is")
        .methods
        .first_mut()
        .expect("the connector declares one method")
}
