use cucumber::{given, then, when};

use super::connector_code::rig;
use crate::world::BehaviourWorld;

// no-op: how a component answers a refusal is the component's decision, and these scenarios use one that gives up.
#[given(regex = r#"^its component fails when a call is refused$"#)]
fn its_component_fails_when_a_call_is_refused(_w: &mut BehaviourWorld) {}

#[given(regex = r#"^its component never returns$"#)]
fn its_component_never_returns(w: &mut BehaviourWorld) {
    rig(w).its_component_is("hanging");
}

#[when(regex = r#"^its component calls "([^"]+)" while connecting$"#)]
fn its_component_calls_while_connecting(w: &mut BehaviourWorld, host: String) {
    rig(w).its_component_is(match host.as_str() {
        "auth.some-provider.example" => "fetching",
        _ => "straying",
    });
    let method = one_method(w);
    rig(w).begin(&method);
}

#[when(regex = r#"^its component tries to run a host program$"#)]
#[when(regex = r#"^its component runs a host program while connecting$"#)]
fn its_component_tries_to_run_a_host_program(w: &mut BehaviourWorld) {
    let method = one_method(w);
    rig(w).begin(&method);
}

#[then(regex = r#"^the call is refused$"#)]
fn the_call_is_refused(w: &mut BehaviourWorld) {
    assert!(
        rig(w).lent.fetched.lock().expect("http lock").is_empty(),
        "a bound that was crossed is held before anything leaves the machine"
    );
    assert!(
        refusals(w).iter().any(|row| row.detail.contains("reached")),
        "the refusal is written down, so a bound nobody watched being crossed is still readable"
    );
}

#[then(regex = r#"^the attempt is refused$"#)]
fn the_attempt_is_refused(w: &mut BehaviourWorld) {
    assert!(
        rig(w).lent.started.lock().expect("exec lock").is_empty(),
        "a method that declared no host execution starts nothing"
    );
    assert!(
        refusals(w).iter().any(|row| row.detail.contains("ran")),
        "the refused attempt is written down"
    );
}

#[when(regex = r#"^its component returns a renewal reporting an expiry one second away$"#)]
fn a_renewal_reporting_a_second(w: &mut BehaviourWorld) {
    // Swapped after the grant, which is safe here: what this scenario reads is the schedule, and the schedule reads connections rather than grants.
    rig(w).the_component_becomes("hurrying");
    rig(w).the_connection_runs_out_at(1000);
    assert_eq!(
        rig(w).refresh_at(0).len(),
        1,
        "the connection is running out, so this pass reaches it"
    );
}

#[then(regex = r#"^the next refresh is no sooner than the floor lns sets$"#)]
fn the_next_refresh_is_no_sooner_than_the_floor(w: &mut BehaviourWorld) {
    let floor = lns_service::connector::refresh::FLOOR_MILLIS;
    assert!(
        rig(w).due_at(floor - 1).is_empty(),
        "the component reported a second, and lns owns the schedule: nothing is due before the floor"
    );
    assert_eq!(
        rig(w).due_at(floor).len(),
        1,
        "once the floor has passed, lns reaches it again"
    );
}

#[then(regex = r#"^the component is stopped at its deadline$"#)]
fn the_component_is_stopped_at_its_deadline(w: &mut BehaviourWorld) {
    let refusal = rig(w)
        .error
        .clone()
        .expect("a component that never returns is stopped");
    assert_eq!(
        refusal, "this connector's component did more work than one call may do",
        "the connect says which bound stopped it"
    );
}

#[when(regex = r#"^the workload requests "([^"]+)"$"#)]
fn the_workload_requests(w: &mut BehaviourWorld, _destination: String) {
    let before = rig(w).mechanisms.calls();
    let supplied = rig(w).what_a_request_reads("1a2b3c4d", 0);
    assert_eq!(
        supplied.len(),
        1,
        "the run granted this connector, so its request is answered from what it granted"
    );
    w.connector
        .as_mut()
        .expect("a rig")
        .mechanism_calls_before_the_request = before;
}

#[then(regex = r#"^no connect runs$"#)]
#[then(regex = r#"^no revoke runs$"#)]
fn no_component_call_runs(w: &mut BehaviourWorld) {
    let before = rig(w).mechanism_calls_before_the_request;
    assert_eq!(
        rig(w).mechanisms.calls(),
        before,
        "only a person's press may run a component, and a request is not one (§3.2.6)"
    );
}

fn refusals(w: &mut BehaviourWorld) -> Vec<lns_audit::Row> {
    rig(w)
        .lent
        .ledger()
        .into_iter()
        .filter(|row| row.detail.contains("refused"))
        .collect()
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
