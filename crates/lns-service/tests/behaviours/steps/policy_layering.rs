use crate::world::BehaviourWorld;
use cucumber::{given, then, when};
use lns_policy::{Policy, RouteRule, Verdict};
use lns_service::artifact::policy::{LayeredDecision, LayeredPolicy};

#[given(regex = r#"^a bundle whose policy allows "([^"]+)"$"#)]
async fn bundle_allows(world: &mut BehaviourWorld, host: String) {
    let rig = world.policy();
    rig.bundle_ships_policy = true;
    rig.bundle_policy.add_rule(RouteRule::allow_host(host));
}

#[given(regex = r#"^a bundle whose policy has no rule for "([^"]+)"$"#)]
async fn bundle_has_no_rule(world: &mut BehaviourWorld, _host: String) {
    world.policy().bundle_ships_policy = true;
}

#[given(regex = r#"^the current directory's "lns-policy.yaml" denies "([^"]+)"$"#)]
async fn overlay_denies(world: &mut BehaviourWorld, host: String) {
    let rig = world.policy();
    rig.cwd_present = true;
    rig.overlay_policy
        .get_or_insert_with(Policy::default)
        .add_rule(RouteRule::deny_host(host));
}

#[given(regex = r#"^the current directory's "lns-policy.yaml" has no rule for "([^"]+)"$"#)]
async fn overlay_has_no_rule(world: &mut BehaviourWorld, _host: String) {
    let rig = world.policy();
    rig.cwd_present = true;
    rig.overlay_policy.get_or_insert_with(Policy::default);
}

#[when(regex = r#"^the bundle is run with --policy that has no rule for "([^"]+)"$"#)]
async fn run_with_replacement(world: &mut BehaviourWorld, _host: String) {
    world.policy().replacement = Some(Policy::default());
}

#[when(regex = r#"^the workload requests "([^"]+)"$"#)]
async fn workload_requests(world: &mut BehaviourWorld, host: String) {
    let rig = world.policy();
    let baseline = rig
        .replacement
        .clone()
        .unwrap_or_else(|| rig.bundle_policy.clone());
    let layered = LayeredPolicy {
        baseline,
        overlay: rig.overlay_policy.clone(),
    };
    rig.decision = Some(layered.decide(&host));
}

fn decision(world: &mut BehaviourWorld) -> LayeredDecision {
    world
        .policy()
        .decision
        .expect("a step must have evaluated the layered policy")
}

#[then("the request is denied")]
async fn request_denied(world: &mut BehaviourWorld) {
    assert_eq!(decision(world), LayeredDecision::Denied);
}

#[then(r#"the request is held pending a decision under defaultVerdict "ask""#)]
async fn request_held(world: &mut BehaviourWorld) {
    assert_eq!(decision(world), LayeredDecision::HeldPending);
}

#[then("the bundle's allow does not apply")]
async fn bundle_allow_ignored(world: &mut BehaviourWorld) {
    assert_ne!(decision(world), LayeredDecision::Allowed);
}

#[then(regex = r#"^the current directory's "lns-policy.yaml" gains an allow rule for "([^"]+)"$"#)]
async fn overlay_gains_allow(world: &mut BehaviourWorld, host: String) {
    let rig = world.approval();
    let on_disk = Policy::load_or_default(&rig.policy_path).expect("load overlay");
    assert!(
        on_disk
            .network
            .allowed_routes
            .iter()
            .any(|r| r.match_pattern == host && r.verdict == Verdict::Allow),
        "the local overlay must gain an allow rule for {host}",
    );
}

#[then("the bundle's shipped policy is unchanged")]
async fn bundle_unchanged(world: &mut BehaviourWorld) {
    assert!(
        world
            .policy()
            .bundle_policy
            .network
            .allowed_routes
            .is_empty(),
        "a runtime approval must not mutate the bundle's shipped policy",
    );
}
