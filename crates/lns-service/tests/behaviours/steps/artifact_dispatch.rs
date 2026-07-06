use crate::world::BehaviourWorld;
use cucumber::{given, then, when};
use lns_service::artifact::{BUNDLE_ARTIFACT_TYPE, RunPath, dispatch};

#[given("a pulled reference whose manifest has no artifact type")]
async fn no_artifact_type(world: &mut BehaviourWorld) {
    world.artifact().artifact_type = None;
}

#[given(regex = r#"^a pulled reference whose manifest is an "([^"]+)" bundle$"#)]
async fn bundle_artifact(world: &mut BehaviourWorld, _kind: String) {
    world.artifact().artifact_type = Some(BUNDLE_ARTIFACT_TYPE.to_string());
}

#[given(regex = r#"^a pulled reference whose manifest artifact type is "([^"]+)"$"#)]
async fn explicit_artifact_type(world: &mut BehaviourWorld, artifact_type: String) {
    world.artifact().artifact_type = Some(artifact_type);
}

#[when("the run resolves the reference for launch")]
async fn resolve_for_launch(world: &mut BehaviourWorld) {
    let artifact_type = world.artifact().artifact_type.clone();
    match dispatch(artifact_type.as_deref()) {
        Ok(path) => world.artifact().path = Some(path),
        Err(e) => world.artifact().error = Some(format!("{e:#}")),
    }
}

#[then("the run launches the single image unchanged")]
async fn launches_single_image(world: &mut BehaviourWorld) {
    assert_eq!(world.artifact().path, Some(RunPath::SingleImage));
}

#[then("no bundle assembly is performed")]
async fn no_assembly(world: &mut BehaviourWorld) {
    assert_ne!(world.artifact().path, Some(RunPath::AssembleBundle));
}

#[then("the run assembles the bundle before launching")]
async fn assembles_bundle(world: &mut BehaviourWorld) {
    assert_eq!(world.artifact().path, Some(RunPath::AssembleBundle));
}

#[then("the run is refused because the artifact type is unsupported")]
async fn refused_unsupported(world: &mut BehaviourWorld) {
    assert!(
        world.artifact().error.is_some(),
        "expected a refusal, got path {:?}",
        world.artifact().path,
    );
}

#[then(regex = r#"^the refusal names the unsupported type "([^"]+)"$"#)]
async fn refusal_names_type(world: &mut BehaviourWorld, artifact_type: String) {
    let err = world
        .artifact()
        .error
        .clone()
        .expect("expected a refusal error");
    assert!(
        err.contains(&artifact_type),
        "error should name {artifact_type}, got: {err}",
    );
}
