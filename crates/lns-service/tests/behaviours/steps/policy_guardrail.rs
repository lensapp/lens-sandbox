use crate::world::BehaviourWorld;
use cucumber::{given, then, when};
use lns_policy::{RouteRule, TcpEgressRule};
use lns_service::artifact::policy::{guardrail_flags, run_summary};

#[given(regex = r#"^a sandbox whose policy allows the CIDR "([^"]+)"$"#)]
async fn allows_cidr(world: &mut BehaviourWorld, cidr: String) {
    let rig = world.policy();
    rig.sandbox_ships_policy = true;
    rig.sandbox_policy.add_rule(RouteRule::allow_host(cidr));
}

#[given(regex = r#"^a sandbox whose policy allows "([^"]+)"$"#)]
async fn allows_host(world: &mut BehaviourWorld, host: String) {
    let rig = world.policy();
    rig.sandbox_ships_policy = true;
    rig.sandbox_policy.add_rule(RouteRule::allow_host(host));
}

#[given(regex = r#"^a sandbox whose policy splices the CIDR "([^"]+)" raw$"#)]
async fn splices_cidr_raw(world: &mut BehaviourWorld, cidr: String) {
    let rig = world.policy();
    rig.sandbox_ships_policy = true;
    rig.sandbox_policy
        .network
        .egress
        .tcp
        .push(TcpEgressRule::allow_destination(cidr));
}

#[when("the first-run summary is produced")]
async fn first_run_summary(world: &mut BehaviourWorld) {
    let rig = world.policy();
    let flags = guardrail_flags(&rig.sandbox_policy);
    rig.summary = Some(run_summary(&flags));
}

fn summary(world: &mut BehaviourWorld) -> String {
    world
        .policy()
        .summary
        .clone()
        .expect("the first-run summary must have been produced")
}

#[then("the run summary prominently flags the wildcard allow")]
async fn flags_wildcard(world: &mut BehaviourWorld) {
    let summary = summary(world);
    assert!(summary.contains("over-broad"), "got: {summary}");
    assert!(summary.contains("wildcard"), "got: {summary}");
}

#[then("the run summary prominently flags the broad CIDR allow")]
async fn flags_broad_cidr(world: &mut BehaviourWorld) {
    let summary = summary(world);
    assert!(summary.contains("over-broad"), "got: {summary}");
    assert!(summary.contains("CIDR"), "got: {summary}");
}
