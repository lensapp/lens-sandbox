use crate::world::BehaviourWorld;
use cucumber::{given, then, when};
use lns_policy::{RouteRule, Verdict};
use lns_service::artifact::policy::{guardrail_flags, run_summary};

#[given(r#"a bundle whose policy has defaultVerdict "allow""#)]
async fn default_verdict_allow(world: &mut BehaviourWorld) {
    let rig = world.policy();
    rig.bundle_ships_policy = true;
    rig.bundle_policy.network.default_verdict = Verdict::Allow;
}

#[given(regex = r#"^a bundle whose policy allows the CIDR "([^"]+)"$"#)]
async fn allows_cidr(world: &mut BehaviourWorld, cidr: String) {
    let rig = world.policy();
    rig.bundle_ships_policy = true;
    rig.bundle_policy.add_rule(RouteRule::allow_host(cidr));
}

#[when("the first-run summary is produced")]
async fn first_run_summary(world: &mut BehaviourWorld) {
    let rig = world.policy();
    let flags = guardrail_flags(&rig.bundle_policy);
    rig.summary = Some(run_summary(&flags));
}

fn summary(world: &mut BehaviourWorld) -> String {
    world
        .policy()
        .summary
        .clone()
        .expect("the first-run summary must have been produced")
}

#[then("the run summary prominently flags the permissive defaultVerdict")]
async fn flags_default_verdict(world: &mut BehaviourWorld) {
    let summary = summary(world);
    assert!(summary.contains("over-broad"), "got: {summary}");
    assert!(summary.contains("defaultVerdict"), "got: {summary}");
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
