use cucumber::{given, then, when};

use crate::connector_rig::ConnectorRig;
use crate::world::BehaviourWorld;

fn rig(w: &mut BehaviourWorld) -> &mut ConnectorRig {
    w.connector.get_or_insert_with(Default::default)
}

#[given(regex = r#"^its method "([^"]+)" draws "([^"]+)" from the auth output "([^"]+)"$"#)]
fn its_method_draws_from_the_output(
    w: &mut BehaviourWorld,
    method: String,
    variable: String,
    field: String,
) {
    let credential = credential(&method, &variable, Some(&field));
    rig(w).add_method(authenticating(&method, credential));
}

#[given(regex = r#"^its method "([^"]+)" draws "([^"]+)" from no named output$"#)]
fn its_method_draws_from_no_named_output(w: &mut BehaviourWorld, method: String, variable: String) {
    let credential = credential(&method, &variable, None);
    rig(w).add_method(authenticating(&method, credential));
}

fn credential(method: &str, variable: &str, field: Option<&str>) -> serde_json::Value {
    let mut credential = serde_json::json!({
        "envVar": variable,
        "placeholder": format!("{method}_LNSPLACEHOLDER0000000000"),
        "injections": [{ "kind": "bearer_header", "domain": "api.some-provider.example" }],
    });
    if let Some(field) = field {
        credential["field"] = serde_json::json!(field);
    }
    credential
}

fn authenticating(method: &str, credential: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "name": method,
        "auth": { "kind": "token" },
        "egress": { "http": [{ "match": "api.some-provider.example", "verdict": "allow" }] },
        "credentials": [credential],
    })
}

#[when(regex = r#"^the machine connects "([^"]+)" with "([^"]+)"$"#)]
fn the_machine_connects_with(w: &mut BehaviourWorld, method: String, value: String) {
    rig(w).connect(&method, &value);
}

#[when(regex = r#"^the run "([^"]+)" grants "([^"]+)" through that connection$"#)]
fn the_run_grants_through_that_connection(w: &mut BehaviourWorld, run: String, method: String) {
    rig(w).grant_through_the_connection(&run, &method);
}

#[then(regex = r#"^the run is supplied "([^"]+)" for "([^"]+)"$"#)]
fn the_run_is_supplied_for(w: &mut BehaviourWorld, expected: String, variable: String) {
    assert_eq!(
        rig(w).supplied_to("1a2b3c4d", &variable),
        Some(expected),
        "the value the connect asked for must be the value the grant arms the credential with"
    );
}
