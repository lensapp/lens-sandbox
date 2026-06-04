#[path = "../../lns-service/tests/_shared/cucumber_step_audit.rs"]
mod audit;

#[test]
fn every_step_definition_matches_some_feature_phrase() {
    audit::assert_no_dead_steps(env!("CARGO_MANIFEST_DIR"));
}
