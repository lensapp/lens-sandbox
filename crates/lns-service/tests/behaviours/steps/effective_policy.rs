use crate::world::BehaviourWorld;
use cucumber::{given, then, when};
use lns_service::artifact::policy::{PolicySource, resolve_effective};

#[given("a bundle that ships a policy")]
async fn bundle_ships_policy(world: &mut BehaviourWorld) {
    world.policy().bundle_ships_policy = true;
}

#[given("a plain image run")]
async fn plain_image_run(world: &mut BehaviourWorld) {
    world.policy().bundle_ships_policy = false;
}

#[given(r#"the current directory has an "lns-policy.yaml""#)]
async fn cwd_has_policy(world: &mut BehaviourWorld) {
    world.policy().cwd_present = true;
}

#[when("the run reports its effective policy")]
async fn report_effective(world: &mut BehaviourWorld) {
    let rig = world.policy();
    rig.effective = Some(resolve_effective(
        rig.bundle_ships_policy,
        rig.cwd_present,
        rig.explicit_policy.take(),
    ));
}

#[when(regex = r#"^the bundle is run with --policy pointing at "([^"]+)"$"#)]
async fn run_with_policy_flag(world: &mut BehaviourWorld, path: String) {
    let rig = world.policy();
    rig.effective = Some(resolve_effective(
        rig.bundle_ships_policy,
        rig.cwd_present,
        Some(path),
    ));
}

fn effective(world: &mut BehaviourWorld) -> String {
    world
        .policy()
        .effective
        .as_ref()
        .expect("a step must have reported the effective policy")
        .disclose()
}

#[then("the baseline source is named as the bundle's shipped policy")]
async fn baseline_is_bundle_shipped(world: &mut BehaviourWorld) {
    assert!(
        world
            .policy()
            .effective
            .as_ref()
            .expect("effective policy")
            .bundle_baseline()
            .is_some(),
        "the baseline must be the bundle's shipped policy",
    );
    assert!(effective(world).contains(&PolicySource::BundleShipped.describe()));
}

#[then(r#"the local overlay is named as the current directory's "lns-policy.yaml""#)]
async fn overlay_is_cwd(world: &mut BehaviourWorld) {
    let eff = world
        .policy()
        .effective
        .as_ref()
        .expect("effective policy")
        .clone();
    assert_eq!(eff.overlay, Some(PolicySource::CurrentDirectory));
    assert!(eff.disclose().contains("local overlay"));
}

#[then(r#"the policy source is named as the current directory's "lns-policy.yaml""#)]
async fn source_is_cwd(world: &mut BehaviourWorld) {
    assert!(effective(world).contains(&PolicySource::CurrentDirectory.describe()));
}

#[then("no bundle baseline is named")]
async fn no_bundle_baseline(world: &mut BehaviourWorld) {
    assert!(
        world
            .policy()
            .effective
            .as_ref()
            .expect("effective policy")
            .bundle_baseline()
            .is_none(),
        "a plain image run must not name a bundle baseline",
    );
    assert!(!effective(world).contains("bundle's shipped policy"));
}

#[then(regex = r#"^the baseline source is named as "([^"]+)"$"#)]
async fn baseline_named_path(world: &mut BehaviourWorld, path: String) {
    assert!(effective(world).contains(&path));
}

#[then("the bundle's shipped policy is reported as replaced")]
async fn shipped_policy_replaced(world: &mut BehaviourWorld) {
    let eff = world
        .policy()
        .effective
        .as_ref()
        .expect("effective policy")
        .clone();
    assert!(eff.replaced_bundle_baseline);
    assert!(eff.disclose().contains("replaced"));
}
