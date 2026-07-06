use crate::world::BehaviourWorld;
use cucumber::{given, then, when};
use lns_service::artifact::spec::Kind;
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

#[given("a pulled reference with no artifact type but a bundle config media type")]
async fn bundle_by_config_media_type(world: &mut BehaviourWorld) {
    world.artifact().artifact_type = None;
    world.artifact().config_media_type = Some(Kind::AgentSystem.config_media_type());
}

#[given("a pulled reference whose manifest is a fileset artifact")]
async fn fileset_artifact(world: &mut BehaviourWorld) {
    world.artifact().artifact_type = Some(Kind::FileSet.artifact_type());
}

#[when("the run resolves the reference for launch")]
async fn resolve_for_launch(world: &mut BehaviourWorld) {
    let artifact_type = world.artifact().artifact_type.clone();
    let config_media_type = world.artifact().config_media_type.clone();
    match dispatch(artifact_type.as_deref(), config_media_type.as_deref()) {
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
    assert_eq!(
        world.artifact().path,
        Some(RunPath::SingleImage),
        "a plain image must resolve to the single-image path, never assembly",
    );
    assert!(
        world.artifact().error.is_none(),
        "a plain image must not be refused"
    );
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

#[then("the run is refused because the artifact is not directly runnable")]
async fn refused_not_runnable(world: &mut BehaviourWorld) {
    let err = world
        .artifact()
        .error
        .clone()
        .expect("expected a refusal error");
    assert!(
        err.contains("not directly runnable"),
        "expected a not-runnable refusal, got: {err}",
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
