use crate::world::BehaviourWorld;
use cucumber::{given, then, when};
use lns_policy::RouteRule;
use lns_service::artifact::audit::{
    BundleRunAudit, ComponentDigest, WithOverrideRecord, record_bundle_run,
};
use lns_service::artifact::policy::{EffectivePolicy, PolicySource};
use lns_service::artifact::signature::Verdict;

fn digest(seed: &str) -> String {
    format!("sha256:{seed:0>64}")
}

#[given("a bundle assembled from a sandbox base image and two filesets")]
async fn assembled_bundle(world: &mut BehaviourWorld) {
    world.audit().components = vec![
        ComponentDigest {
            name: "sandbox".into(),
            digest: digest("base"),
        },
        ComponentDigest {
            name: "fileset-0".into(),
            digest: digest("skills"),
        },
        ComponentDigest {
            name: "fileset-1".into(),
            digest: digest("settings"),
        },
    ];
}

#[given("a bundle run with a --with fileset override")]
async fn with_override(world: &mut BehaviourWorld) {
    world.audit().with_overrides = vec![WithOverrideRecord {
        name: "extra".into(),
        digest: digest("extra"),
    }];
}

#[given("a bundle run governed by the bundle's shipped policy under a local overlay")]
async fn governed_by_layered_policy(world: &mut BehaviourWorld) {
    let rig = world.audit();
    rig.effective = EffectivePolicy {
        baseline: PolicySource::BundleShipped,
        overlay: Some(PolicySource::CurrentDirectory),
        replaced_bundle_baseline: false,
    };
    rig.policy
        .add_rule(RouteRule::allow_host("api.example.test"));
}

#[given(regex = r#"^a bundle whose agent uses integration "([^"]+)"$"#)]
async fn agent_uses_integration(world: &mut BehaviourWorld, id: String) {
    world.audit().integrations.push(id);
}

#[given("a bundle signed by a trusted key")]
async fn signed_by_trusted(world: &mut BehaviourWorld) {
    world.audit().verdict = Verdict::Verified;
}

#[when("the run is recorded in the audit chain")]
async fn record_run(world: &mut BehaviourWorld) {
    let rig = world.audit();
    rig.record = Some(record_bundle_run(
        rig.bundle_digest.clone(),
        rig.components.clone(),
        rig.with_overrides.clone(),
        &rig.effective,
        &rig.policy,
        rig.integrations.clone(),
        &rig.verdict,
    ));
}

fn record(world: &mut BehaviourWorld) -> BundleRunAudit {
    world
        .audit()
        .record
        .clone()
        .expect("the run must have been recorded")
}

#[then("the audit record names the bundle digest")]
async fn names_bundle_digest(world: &mut BehaviourWorld) {
    let expected = world.audit().bundle_digest.clone();
    assert_eq!(record(world).bundle_digest, expected);
}

#[then("the audit record names the digest of every resolved component")]
async fn names_component_digests(world: &mut BehaviourWorld) {
    let expected = world.audit().components.clone();
    let recorded = record(world).components;
    for component in &expected {
        assert!(
            recorded.iter().any(|c| c.digest == component.digest),
            "component {} digest missing from the audit record",
            component.name,
        );
    }
    assert_eq!(recorded.len(), expected.len());
}

#[then("the audit record names the --with override and its digest")]
async fn names_override(world: &mut BehaviourWorld) {
    let recorded = record(world).with_overrides;
    assert_eq!(recorded.len(), 1);
    assert_eq!(recorded[0].name, "extra");
    assert!(recorded[0].digest.starts_with("sha256:"));
}

#[then("the audit record names the effective policy source")]
async fn names_policy_source(world: &mut BehaviourWorld) {
    assert_eq!(
        record(world).effective_policy_source,
        PolicySource::BundleShipped.describe()
    );
}

#[then("the audit record carries a hash of the effective policy")]
async fn carries_policy_hash(world: &mut BehaviourWorld) {
    assert!(record(world).effective_policy_hash.starts_with("sha256:"));
}

#[then(regex = r#"^the audit record names the integration identity "([^"]+)" in effect$"#)]
async fn names_integration(world: &mut BehaviourWorld, id: String) {
    assert!(record(world).integrations.contains(&id));
}

#[then("the audit record carries the signature and trust verdict")]
async fn carries_verdict(world: &mut BehaviourWorld) {
    assert_eq!(record(world).signature_verdict, "verified");
}
