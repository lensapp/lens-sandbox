use crate::world::BehaviourWorld;
use cucumber::{given, then, when};
use lns_service::artifact::spec::Kind;
use lns_service::artifact::{RunPath, dispatch_run};

#[given("a pulled reference whose manifest has no artifact type")]
async fn no_artifact_type(world: &mut BehaviourWorld) {
    world.artifact().artifact_type = None;
}

#[given(regex = r#"^a pulled reference whose manifest artifact type is "([^"]+)"$"#)]
async fn explicit_artifact_type(world: &mut BehaviourWorld, artifact_type: String) {
    world.artifact().artifact_type = Some(artifact_type);
}

#[given("a pulled reference whose manifest is a fileset artifact")]
async fn fileset_artifact(world: &mut BehaviourWorld) {
    world.artifact().artifact_type = Some(Kind::FileSet.artifact_type());
}

#[given("a pulled reference whose manifest is a mixin artifact")]
async fn mixin_artifact(world: &mut BehaviourWorld) {
    world.artifact().artifact_type = Some(Kind::Mixin.artifact_type());
}

#[given("a pulled reference whose manifest is a sandbox artifact")]
async fn sandbox_artifact(world: &mut BehaviourWorld) {
    world.artifact().artifact_type = Some(Kind::Sandbox.artifact_type());
}

#[when("the run resolves the reference for launch")]
async fn resolve_for_launch(world: &mut BehaviourWorld) {
    let artifact_type = world.artifact().artifact_type.clone();
    let config_media_type = world.artifact().config_media_type.clone();
    match dispatch_run(
        artifact_type.as_deref(),
        config_media_type.as_deref(),
        "some-registry.example/some-ref:1",
        true,
    ) {
        Ok(path) => world.artifact().path = Some(path),
        Err(e) => world.artifact().error = Some(format!("{e:#}")),
    }
}

#[then("the run is refused because the reference is not a sandbox")]
async fn refused_not_a_sandbox(world: &mut BehaviourWorld) {
    let err = world
        .artifact()
        .error
        .clone()
        .expect("expected a refusal error");
    assert!(
        err.contains("not a sandbox"),
        "expected a not-a-sandbox refusal, got: {err}",
    );
}

#[then(regex = r#"^the refusal points at "([^"]+)"$"#)]
async fn refusal_points_at(world: &mut BehaviourWorld, hint: String) {
    let err = world
        .artifact()
        .error
        .clone()
        .expect("expected a refusal error");
    assert!(
        err.contains(&hint),
        "refusal should mention {hint}, got: {err}"
    );
}

#[then("the run launches the sandbox directly")]
async fn launches_sandbox(world: &mut BehaviourWorld) {
    assert_eq!(world.artifact().path, Some(RunPath::Sandbox));
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
