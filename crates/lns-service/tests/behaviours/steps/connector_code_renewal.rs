use cucumber::{given, then, when};

use super::connector_code::{PLACEHOLDER, rig};
use crate::world::BehaviourWorld;

/// Five minutes out is within reach of the schedule, so the pass that follows is the one lns would have made anyway.
const FIVE_MINUTES: u64 = 300_000;

#[given(regex = r#"^the credential expires in five minutes$"#)]
fn the_credential_expires_in_five_minutes(w: &mut BehaviourWorld) {
    rig(w).the_connection_runs_out_at(FIVE_MINUTES);
}

#[given(regex = r#"^every renewal attempt fails$"#)]
fn every_renewal_attempt_fails(w: &mut BehaviourWorld) {
    rig(w).its_component_is("refusing");
}

#[given(regex = r#"^the connection holds the scopes it consented to$"#)]
fn the_connection_holds_the_scopes(w: &mut BehaviourWorld) {
    let name = rig(w).last_declared_name().expect("a connector");
    let method = one_method(w);
    rig(w).begin(&method);
    let held = rig(w).connections_of(&name);
    assert!(!held.is_empty(), "the connect must have made one");
    assert!(
        held.values().all(|held| !held.authority.0.is_empty()),
        "this connect reported what it was allowed to do, and the connection recorded it"
    );
}

#[given(regex = r#"^its method "([^"]+)" is a code method writing a credentials file$"#)]
fn a_code_method_writing_a_credentials_file(w: &mut BehaviourWorld, method: String) {
    rig(w).add_code_method(
        serde_json::json!({
            "name": method,
            "auth": {
                "kind": "code",
                "component": "./sign-in.wasm",
                "outputs": ["access_token"],
                "hosts": ["auth.some-provider.example"],
            },
            "credentials": [{ "envVar": "SOME_TOKEN", "placeholder": PLACEHOLDER, "injections": [{ "kind": "bearer_header", "domain": "api.some-provider.example" }] }],
            "filesets": [{
                "guestPath": "~/.some-provider",
                "inline": { "credentials.json": format!("{{\"token\":\"{PLACEHOLDER}\"}}") },
            }],
        }),
        "granting",
    );
    rig(w).install();
}

#[given(regex = r#"^its component fails on revoke$"#)]
fn its_component_fails_on_revoke(w: &mut BehaviourWorld) {
    rig(w).its_component_is("clinging");
}

/// Reaching one is the whole of this step: a connection lns has no reason to renew is one this pass would skip, so the moment it runs out is set here.
#[when(regex = r#"^lns reaches that connection on its refresh schedule$"#)]
#[when(regex = r#"^its component returns a renewal$"#)]
fn lns_reaches_that_connection(w: &mut BehaviourWorld) {
    rig(w).the_connection_runs_out_at(FIVE_MINUTES);
    assert_eq!(
        rig(w).refresh_at(0).len(),
        1,
        "the connection is running out, so the schedule reaches it"
    );
}

#[when(regex = r#"^its component returns a renewal naming no scopes$"#)]
fn a_renewal_naming_no_scopes(w: &mut BehaviourWorld) {
    rig(w).its_component_is("keeping");
    rig(w).the_connection_runs_out_at(FIVE_MINUTES);
    assert_eq!(
        rig(w).refresh_at(0).len(),
        1,
        "the connection was renewed, so what it kept is what this scenario reads"
    );
}

#[when(regex = r#"^the credential's expiry passes$"#)]
fn the_credentials_expiry_passes(w: &mut BehaviourWorld) {
    rig(w).the_connection_runs_out_at(FIVE_MINUTES);
    assert!(
        rig(w).refresh_at(FIVE_MINUTES + 1).is_empty(),
        "every attempt fails, so the moment passes with nothing renewed"
    );
}

#[then(regex = r#"^the run's placeholder is armed with the renewed value$"#)]
fn the_placeholder_is_armed_with_the_renewed_value(w: &mut BehaviourWorld) {
    let armed = rig(w)
        .supplied_to("1a2b3c4d", "SOME_TOKEN")
        .unwrap_or_default();
    assert!(
        armed.contains("first-renewed"),
        "the run reads the value the renewal produced, at the placeholder it already holds: {armed}"
    );
}

#[then(regex = r#"^the run is not restarted$"#)]
fn the_run_is_not_restarted(w: &mut BehaviourWorld) {
    let name = rig(w).last_declared_name().expect("a connector");
    assert_eq!(
        rig(w).granted_method("1a2b3c4d", &name).as_deref(),
        Some("sign-in"),
        "a renewal that landed drops no grant, so nothing asks the run to decide again"
    );
}

#[then(regex = r#"^the run's placeholder is unarmed$"#)]
fn the_placeholder_is_unarmed(w: &mut BehaviourWorld) {
    assert_eq!(
        rig(w)
            .supplied_to_at("1a2b3c4d", "SOME_TOKEN", FIVE_MINUTES + 1)
            .as_deref(),
        Some(""),
        "the values ran out, so the injection carries none and the placeholder is left unarmed (§3.2.4)"
    );
}

#[then(regex = r#"^the connection stands, run out, so the card can offer it again$"#)]
fn the_connection_stands_run_out(w: &mut BehaviourWorld) {
    let name = rig(w).last_declared_name().expect("a connector");
    let held = rig(w).connections_of(&name);
    assert!(!held.is_empty(), "the connection stands, unrenewed");
    assert!(
        held.values()
            .all(|connection| connection.has_run_out(FIVE_MINUTES + 1)),
        "it has run out, which is what the card offers to connect again"
    );
}

#[then(regex = r#"^the connection keeps the scopes it already had$"#)]
fn the_connection_keeps_its_scopes(w: &mut BehaviourWorld) {
    let name = rig(w).last_declared_name().expect("a connector");
    let held = rig(w).connections_of(&name);
    assert_eq!(
        held.values()
            .flat_map(|connection| connection.authority.0.iter().cloned())
            .collect::<Vec<_>>(),
        ["read"],
        "a renewal restates the values, not who the connection is, so lns carries the scopes forward (§3.2.6)"
    );
}

#[then(regex = r#"^the file the guest holds carries the placeholder$"#)]
fn the_file_carries_the_placeholder(w: &mut BehaviourWorld) {
    assert!(
        credentials_file(w).contains(PLACEHOLDER),
        "a fileset carries the placeholder and the boundary arms it, however the value was renewed"
    );
}

#[then(regex = r#"^the file the guest holds carries no renewed value$"#)]
fn the_file_carries_no_renewed_value(w: &mut BehaviourWorld) {
    let written = credentials_file(w);
    assert!(
        !written.contains("renewed") && !written.contains("first"),
        "no route but the placeholder carries a value into a guest: {written}"
    );
}

fn credentials_file(w: &mut BehaviourWorld) -> String {
    rig(w)
        .file_written_to("1a2b3c4d", "~/.some-provider/credentials.json")
        .expect("the granted method writes the credentials file")
}

fn one_method(w: &mut BehaviourWorld) -> String {
    let name = rig(w).last_declared_name().expect("a connector");
    rig(w)
        .methods_of(&name)
        .first()
        .expect("the connector declares a method")
        .name
        .clone()
}
