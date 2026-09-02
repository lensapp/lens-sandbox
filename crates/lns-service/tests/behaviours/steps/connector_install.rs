use cucumber::{given, then, when};

use crate::connector_rig::ConnectorRig;
use crate::world::BehaviourWorld;

fn rig(w: &mut BehaviourWorld) -> &mut ConnectorRig {
    w.connector.get_or_insert_with(Default::default)
}

#[given(regex = r#"^the connector "([^"]+)" serves "([^"]+)"$"#)]
fn the_connector_serves(w: &mut BehaviourWorld, name: String, destination: String) {
    rig(w).declare(&name, &destination);
}

#[given(regex = r#"^its method "([^"]+)" allows "([^"]+)" and sets "([^"]+)"$"#)]
fn its_method_allows_and_sets(
    w: &mut BehaviourWorld,
    method: String,
    destination: String,
    variable: String,
) {
    rig(w).add_method(serde_json::json!({
        "name": method,
        "auth": { "kind": "token" },
        "egress": { "http": [{ "match": destination, "verdict": "allow" }] },
        "credentials": [{
            "envVar": variable,
            "placeholder": format!("{}_LNSPLACEHOLDER0000000000", variable.to_lowercase()),
        }],
    }));
}

#[given(regex = r#"^its method "([^"]+)" carries the block "([^"]+)"$"#)]
fn its_method_carries_the_block(w: &mut BehaviourWorld, method: String, block: String) {
    rig(w).add_method(serde_json::json!({
        "name": method,
        "auth": { "kind": "token" },
        block: [],
    }));
}

#[given(regex = r#"^the machine installs the connector$"#)]
#[when(regex = r#"^the machine installs the connector$"#)]
fn the_machine_installs_the_connector(w: &mut BehaviourWorld) {
    rig(w).install();
}

#[given(regex = r#"^the run "([^"]+)" granted the method "([^"]+)"$"#)]
#[when(regex = r#"^the run "([^"]+)" grants the method "([^"]+)"$"#)]
fn the_run_granted_the_method(w: &mut BehaviourWorld, run: String, method: String) {
    let rig = rig(w);
    let name = rig
        .last_declared_name()
        .expect("a connector must be described before a run grants it");
    rig.grant(&run, &name, &method);
}

#[when(regex = r#"^the machine uninstalls the connector "([^"]+)"$"#)]
fn the_machine_uninstalls_the_connector(w: &mut BehaviourWorld, name: String) {
    rig(w).store().uninstall(&name).expect("uninstall");
}

#[when(regex = r#"^the machine lists its connectors$"#)]
fn the_machine_lists_its_connectors(w: &mut BehaviourWorld) {
    let rig = rig(w);
    rig.listed = rig.store().installed().expect("list the installed set");
}

#[then(regex = r#"^the install succeeds$"#)]
fn the_install_succeeds(w: &mut BehaviourWorld) {
    assert_eq!(rig(w).error, None, "the install must not be refused");
}

#[then(regex = r#"^the install is refused$"#)]
fn the_install_is_refused(w: &mut BehaviourWorld) {
    assert!(
        rig(w).error.is_some(),
        "the install must be refused where a later launch could not decide"
    );
}

#[then(regex = r#"^the machine holds the connector "([^"]+)"$"#)]
fn the_machine_holds_the_connector(w: &mut BehaviourWorld, name: String) {
    assert!(
        rig(w).installed_named(&name).is_some(),
        "{name} must be installed"
    );
}

#[then(regex = r#"^the machine holds no connector "([^"]+)"$"#)]
fn the_machine_holds_no_connector(w: &mut BehaviourWorld, name: String) {
    assert!(
        rig(w).installed_named(&name).is_none(),
        "{name} must no longer be installed"
    );
}

#[then(regex = r#"^the machine holds no connection for "([^"]+)"$"#)]
fn the_machine_holds_no_connection_for(w: &mut BehaviourWorld, name: String) {
    assert!(
        rig(w).connections_of(&name).is_empty(),
        "installing grants nothing, so it connects nothing"
    );
}

#[then(regex = r#"^the refusal says "([^"]+)" already serves that destination$"#)]
fn the_refusal_says_already_serves(w: &mut BehaviourWorld, installed: String) {
    let refusal = refusal_of(w);
    assert!(refusal.contains(&installed), "{refusal}");
    assert!(refusal.contains("serves"), "{refusal}");
}

#[then(regex = r#"^the refusal names the block "([^"]+)"$"#)]
fn the_refusal_names_the_block(w: &mut BehaviourWorld, block: String) {
    let refusal = refusal_of(w);
    assert!(refusal.contains(&block), "{refusal}");
}

#[then(regex = r#"^the refusal names the variable "([^"]+)"$"#)]
fn the_refusal_names_the_variable(w: &mut BehaviourWorld, variable: String) {
    let refusal = refusal_of(w);
    assert!(refusal.contains(&variable), "{refusal}");
}

#[then(regex = r#"^the run "([^"]+)" still grants the method "([^"]+)"$"#)]
fn the_run_still_grants_the_method(w: &mut BehaviourWorld, run: String, method: String) {
    let name = rig(w)
        .last_declared_name()
        .expect("a connector was declared");
    assert_eq!(
        rig(w).granted_method(&run, &name),
        Some(method),
        "uninstalling stops the offer; it does not retract a grant"
    );
}

#[then(regex = r#"^the run "([^"]+)" is offered nothing$"#)]
fn the_run_is_offered_nothing(w: &mut BehaviourWorld, run: String) {
    assert!(
        rig(w).offered_to(&run).is_empty(),
        "this run granted the connector, so it is not offered again"
    );
}

#[then(regex = r#"^the run "([^"]+)" is still offered the connector$"#)]
fn the_run_is_still_offered(w: &mut BehaviourWorld, run: String) {
    let name = rig(w)
        .last_declared_name()
        .expect("a connector was declared");
    assert_eq!(
        rig(w).offered_to(&run),
        vec![name],
        "a grant belongs to one run, so another run is still asked"
    );
}

#[then(regex = r#"^the list names "([^"]+)" serving "([^"]+)"$"#)]
fn the_list_names_serving(w: &mut BehaviourWorld, name: String, destination: String) {
    let rig = rig(w);
    let entry = rig
        .listed
        .iter()
        .find(|e| e.name == name)
        .unwrap_or_else(|| panic!("{name} missing from the list"));
    let parsed = lns_artifact::connector::parse(&entry.document).expect("a listed document parses");
    assert!(
        parsed.spec.serves.contains(&destination),
        "the list must say what {name} serves, got {:?}",
        parsed.spec.serves
    );
}

#[then(regex = r#"^the list marks the method "([^"]+)" as needing a connect$"#)]
fn the_list_marks_the_method_as_needing_a_connect(w: &mut BehaviourWorld, method: String) {
    let name = rig(w)
        .last_declared_name()
        .expect("a connector was declared");
    let methods = rig(w).methods_of(&name);
    let found = methods
        .iter()
        .find(|m| m.name == method)
        .unwrap_or_else(|| panic!("{method} missing from {name}"));
    assert!(
        found.auth.is_some(),
        "a method that authenticates is one the user must connect"
    );
}

fn refusal_of(w: &mut BehaviourWorld) -> String {
    rig(w)
        .error
        .clone()
        .expect("the install must have been refused")
}
