use crate::world::BehaviourWorld;
use cucumber::{given, then, when};

#[given("the broker reports that the guest got no DHCP lease")]
fn broker_reports_no_lease(world: &mut BehaviourWorld) {
    world.broker_refusal = Some(lns_session::BrokerExitReason::NoDhcpLease);
}

#[when("the service handles the broker outcome")]
fn service_handles_outcome(world: &mut BehaviourWorld) {
    let reason = world.broker_refusal.expect("the broker outcome was staged");
    let outcome = Err(anyhow::Error::new(
        lns_service::vm::session_client::BrokerRefusal { reason },
    ));
    world.broker_exit_reason =
        lns_service::run::broker_exit_reason(&outcome).map(|reason| reason.as_str().to_string());
    let context = lns_service::ocsf_audit::OcsfCtx::at_unix(
        "aa380".into(),
        "calm-finch".into(),
        1_788_499_200,
    );
    world.broker_exit_audit = Some(lns_service::ocsf_audit::broker_exit_event(
        &context, 1, reason,
    ));
}

#[then(expr = "the run exit reason is {string}")]
fn exit_reason_is(world: &mut BehaviourWorld, expected: String) -> Result<(), String> {
    match world.broker_exit_reason.as_deref() {
        Some(actual) if actual == expected => Ok(()),
        actual => Err(format!("expected exit reason {expected:?}, got {actual:?}")),
    }
}

#[then(expr = "the audit event kind is {string}")]
fn audit_kind_is(world: &mut BehaviourWorld, expected: String) -> Result<(), String> {
    let actual = world
        .broker_exit_audit
        .as_ref()
        .and_then(|event| event.get("unmapped"))
        .and_then(serde_json::Value::as_object)
        .and_then(|unmapped| unmapped.get("lns_kind"))
        .and_then(serde_json::Value::as_str);
    if actual == Some(expected.as_str()) {
        Ok(())
    } else {
        Err(format!("expected audit kind {expected:?}, got {actual:?}"))
    }
}
