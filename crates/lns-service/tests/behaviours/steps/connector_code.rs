//! What every `kind: code` feature says the same way: how a method carrying a
//! component is described, how a connect is driven, and what stands afterwards.

use cucumber::{given, then, when};

use crate::connector_rig::ConnectorRig;
use crate::world::BehaviourWorld;

pub fn rig(w: &mut BehaviourWorld) -> &mut ConnectorRig {
    w.connector.get_or_insert_with(Default::default)
}

/// The method these features describe, as the document spells it. `fixture` is the component this scenario's connector carries.
pub fn code_method(
    w: &mut BehaviourWorld,
    method: &str,
    fixture: &str,
    hosts: &[&str],
    exec: bool,
) {
    let auth = serde_json::json!({
        "kind": "code",
        "component": "./sign-in.wasm",
        "outputs": ["access_token"],
        "hosts": hosts,
        "exec": exec,
    });
    rig(w).add_code_method(
        serde_json::json!({
            "name": method,
            "auth": auth,
            "egress": { "http": [{ "match": "api.some-provider.example", "verdict": "allow" }] },
            "credentials": [{
                "envVar": "SOME_TOKEN",
                "placeholder": PLACEHOLDER,
                "injections": [{ "kind": "bearer_header", "domain": "api.some-provider.example" }],
            }],
        }),
        fixture,
    );
    rig(w).install();
}

/// The one placeholder these features arm, spelled the way a document must spell one.
pub const PLACEHOLDER: &str = "some_token_LNSPLACEHOLDER0000000000";

#[given(regex = r#"^its method "([^"]+)" is a code method$"#)]
fn its_method_is_a_code_method(w: &mut BehaviourWorld, method: String) {
    code_method(
        w,
        &method,
        "granting",
        &["auth.some-provider.example"],
        false,
    );
}

#[given(regex = r#"^its method "([^"]+)" is a code method declaring the host "([^"]+)"$"#)]
fn a_code_method_declaring_the_host(w: &mut BehaviourWorld, method: String, host: String) {
    code_method(w, &method, "granting", &[&host], false);
}

#[given(regex = r#"^its method "([^"]+)" is a code method declaring host execution$"#)]
fn a_code_method_declaring_host_execution(w: &mut BehaviourWorld, method: String) {
    code_method(w, &method, "running", &[], true);
}

#[given(regex = r#"^its method "([^"]+)" is a code method declaring no host execution$"#)]
fn a_code_method_declaring_no_host_execution(w: &mut BehaviourWorld, method: String) {
    code_method(w, &method, "running", &[], false);
}

#[when(regex = r#"^the machine connects "([^"]+)"$"#)]
#[given(regex = r#"^the machine connects "([^"]+)"$"#)]
fn the_machine_connects(w: &mut BehaviourWorld, method: String) {
    rig(w).begin(&method);
    if !rig(w).fields_asked_for().is_empty() {
        rig(w).answer_held();
    }
}

#[given(regex = r#"^the run "([^"]+)" grants "([^"]+)" through that connection$"#)]
fn the_run_grants_through_that_connection(w: &mut BehaviourWorld, run: String, method: String) {
    rig(w).begin(&method);
    if !rig(w).fields_asked_for().is_empty() {
        rig(w).answer_held();
    }
    rig(w).grant_through_the_connection(&run, &method);
    assert_eq!(rig(w).error, None, "the run must hold the grant");
}

#[given(regex = r#"^the machine holds a connection for "([^"]+)"$"#)]
fn the_machine_comes_to_hold_a_connection(w: &mut BehaviourWorld, name: String) {
    let method = one_method_of(w, &name);
    rig(w).begin(&method);
    if !rig(w).fields_asked_for().is_empty() {
        rig(w).answer_held();
    }
    assert!(
        !rig(w).connections_of(&name).is_empty(),
        "this scenario starts from a connection {name} holds"
    );
}

#[then(regex = r#"^the machine holds a connection for "([^"]+)"$"#)]
fn the_machine_holds_a_connection_for(w: &mut BehaviourWorld, name: String) {
    assert!(
        !rig(w).connections_of(&name).is_empty(),
        "the machine must hold a connection for {name}"
    );
}

fn one_method_of(w: &mut BehaviourWorld, name: &str) -> String {
    rig(w)
        .methods_of(name)
        .first()
        .expect("the connector declares a method")
        .name
        .clone()
}

#[then(regex = r#"^the connect fails$"#)]
fn the_connect_fails(w: &mut BehaviourWorld) {
    assert!(
        rig(w).error.is_some(),
        "the connect must fail rather than store what it never got"
    );
    let name = rig(w).last_declared_name().expect("a connector");
    assert!(
        rig(w).connections_of(&name).is_empty(),
        "a connect that failed stores no connection"
    );
}

#[then(regex = r#"^"([^"]+)" is still offered$"#)]
fn the_method_is_still_offered(w: &mut BehaviourWorld, method: String) {
    let name = rig(w).last_declared_name().expect("a connector");
    assert!(
        rig(w).methods_of(&name).iter().any(|m| m.name == method),
        "the method must still be one the card can offer"
    );
    assert_eq!(
        rig(w).offered_to("1a2b3c4d"),
        vec![name],
        "a connect that did not finish decides nothing, so the offer stands"
    );
}

#[given(regex = r#"^no component runtime starts on this machine$"#)]
fn no_component_runtime_starts(w: &mut BehaviourWorld) {
    rig(w).no_component_runtime_starts();
}

#[when(regex = r#"^the machine disconnects "([^"]+)"$"#)]
fn the_machine_disconnects(w: &mut BehaviourWorld, _name: String) {
    let before = rig(w).mechanisms.calls();
    let tells_anyone = rig(w).holds_a_component_runtime();
    rig(w).disconnect();
    assert_eq!(
        rig(w).mechanisms.calls() > before,
        tells_anyone,
        "the press authorises a revoke, so a machine with a mechanism tells it and one without has nobody to tell (§3.2.6)"
    );
}
