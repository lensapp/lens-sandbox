use cucumber::{given, then, when};
use lns_service::artifact::plan_local_sandbox;
use lns_service::artifact::policy::merge_effective;

use crate::world::BehaviourWorld;

/// Drives an already-resolved sandbox through the launch path, so a mixin scenario sees the policy the gate would enforce.
pub(crate) fn launch_resolved(
    w: &mut BehaviourWorld,
    resolved: anyhow::Result<lns_service::artifact::assembly::ResolvedSandbox>,
) {
    let rig = w.declared.get_or_insert_with(Default::default);
    let resolved = match resolved {
        Ok(resolved) => resolved,
        Err(e) => {
            rig.error = Some(format!("{e:#}"));
            return;
        }
    };
    rig.tools = resolved.tools.clone();
    rig.running_policy = Some(merge_effective(
        resolved.policy.as_ref(),
        None,
        &rig.overlay,
    ));
}

#[when("the sandbox is launched")]
fn sandbox_launched(w: &mut BehaviourWorld) {
    let definition = declared_definition(w);
    launch_resolved(
        w,
        plan_local_sandbox(definition.as_bytes(), &Default::default()),
    );
}

fn declared_definition(w: &mut BehaviourWorld) -> String {
    w.declared
        .get_or_insert_with(Default::default)
        .definition
        .clone()
        .expect("a Given step must declare the definition")
}

#[given("the directory's lns-local-mixin.yaml denies all by default")]
fn overlay_denies_by_default(w: &mut BehaviourWorld) {
    let rig = w.declared.get_or_insert_with(Default::default);
    rig.overlay.add_rule(lns_policy::RouteRule::deny_host("*"));
}

#[then(regex = r#"^a workload request to "([^"]+)" is denied by policy$"#)]
fn request_denied_by_policy(w: &mut BehaviourWorld, host: String) -> Result<(), String> {
    let policy = w
        .declared
        .as_ref()
        .and_then(|r| r.running_policy.as_ref())
        .ok_or("no running policy was produced")?;
    match super::mixin_resolution::gate_verdict(policy, &host) {
        Some(lns_policy::Verdict::Deny) => Ok(()),
        other => Err(format!(
            "expected {host} to be denied, the gate's first match gave {other:?}; routes: {:?}",
            policy.network.egress.http
        )),
    }
}

#[then("the launch is refused")]
fn launch_refused(w: &mut BehaviourWorld) -> Result<(), String> {
    match w.declared.as_ref().and_then(|r| r.error.as_ref()) {
        Some(_) => Ok(()),
        None => Err("the launch was not refused".to_string()),
    }
}
