use crate::world::BehaviourWorld;
use cucumber::{given, then, when};
use lns_service::artifact::signature::{RefusalReason, SignatureStatus, Verdict, gate};

#[given("a trusted signer key is configured")]
async fn trusted_key_configured(world: &mut BehaviourWorld) {
    world.artifact().trusted_keys_configured = true;
}

#[given("no trusted signer key is configured")]
async fn no_trusted_key_configured(world: &mut BehaviourWorld) {
    world.artifact().trusted_keys_configured = false;
}

#[given("a remote bundle signed by that trusted key")]
async fn signed_by_trusted(world: &mut BehaviourWorld) {
    world.artifact().signature_status = Some(SignatureStatus::SignedByTrusted);
}

#[given("a remote bundle carrying no signature")]
async fn unsigned(world: &mut BehaviourWorld) {
    world.artifact().signature_status = Some(SignatureStatus::Unsigned);
}

#[given("a remote bundle signed by a key that is not trusted")]
async fn signed_by_untrusted(world: &mut BehaviourWorld) {
    world.artifact().signature_status = Some(SignatureStatus::SignedByUntrusted);
}

fn run_gate(world: &mut BehaviourWorld, insecure: bool) {
    let rig = world.artifact();
    let status = rig
        .signature_status
        .expect("a scenario must set the bundle's signature status before running");
    rig.verdict = Some(gate(insecure, rig.trusted_keys_configured, status));
}

#[when("the bundle is run")]
async fn bundle_is_run(world: &mut BehaviourWorld) {
    run_gate(world, false);
}

#[when("the bundle is run with --insecure")]
async fn bundle_is_run_insecure(world: &mut BehaviourWorld) {
    run_gate(world, true);
}

fn verdict(world: &mut BehaviourWorld) -> Verdict {
    world
        .artifact()
        .verdict
        .clone()
        .expect("the bundle-run step must have produced a verdict")
}

#[then("verification succeeds")]
async fn verification_succeeds(world: &mut BehaviourWorld) {
    assert_eq!(verdict(world), Verdict::Verified);
}

#[then("the bundle runs")]
async fn the_bundle_runs(world: &mut BehaviourWorld) {
    assert!(
        verdict(world).allows_launch(),
        "the verdict must permit launch",
    );
}

#[then("verification is skipped")]
async fn verification_is_skipped(world: &mut BehaviourWorld) {
    assert_eq!(verdict(world), Verdict::Skipped);
}

#[then("the run is refused because the bundle is unsigned")]
async fn refused_unsigned(world: &mut BehaviourWorld) {
    assert_eq!(verdict(world), Verdict::Refused(RefusalReason::Unsigned));
}

#[then("the run is refused because the signer is not trusted")]
async fn refused_untrusted(world: &mut BehaviourWorld) {
    assert_eq!(
        verdict(world),
        Verdict::Refused(RefusalReason::UntrustedSigner)
    );
}

#[then("nothing is launched")]
async fn nothing_launched(world: &mut BehaviourWorld) {
    assert!(
        !verdict(world).allows_launch(),
        "a refused verdict must not launch anything",
    );
}

#[then("a warning is surfaced that the signature cannot be verified")]
async fn warning_surfaced(world: &mut BehaviourWorld) {
    match verdict(world) {
        Verdict::ProceedUnverified { warning } => assert!(
            warning.contains("cannot be verified"),
            "warning should explain the signature could not be verified, got: {warning}",
        ),
        other => panic!("expected a proceed-with-warning verdict, got: {other:?}"),
    }
}
