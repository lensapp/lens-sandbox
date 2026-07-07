use crate::world::BehaviourWorld;
use cucumber::{given, then, when};
use lns_service::artifact::assembly::{FileSource, ResolvedFileset, assemble};

#[given(regex = r#"^a bundle whose sandbox base image is "([^"]+)"$"#)]
async fn base_image(world: &mut BehaviourWorld, image: String) {
    world.artifact().bundle.base_image = image;
}

#[given("the bundle declares no filesets")]
async fn no_filesets(_world: &mut BehaviourWorld) {
    // no-op: the default resolved bundle already has no filesets
}

#[given(regex = r#"^the bundle declares a fileset "([^"]+)" mounting "([^"]+)"$"#)]
async fn declare_fileset(world: &mut BehaviourWorld, name: String, path: String) {
    world.artifact().bundle.filesets.push(ResolvedFileset {
        name,
        paths: vec![path],
        reference: String::new(),
    });
}

#[given(regex = r#"^a bundle whose sandbox base image ships "([^"]+)"$"#)]
async fn base_ships(world: &mut BehaviourWorld, path: String) {
    world.artifact().bundle.base_paths.push(path);
}

#[given(regex = r#"^the base image also ships "([^"]+)"$"#)]
async fn base_also_ships(world: &mut BehaviourWorld, path: String) {
    world.artifact().bundle.base_paths.push(path);
}

#[given(regex = r#"^a bundle whose agent invocation runs command "([^"]+)" with env "([^"]+)"$"#)]
async fn agent_invocation(world: &mut BehaviourWorld, command: String, env: String) {
    let (key, value) = env.split_once('=').expect("env fixture must be KEY=VALUE");
    world.artifact().bundle.command = Some(command);
    world
        .artifact()
        .bundle
        .env
        .insert(key.to_string(), value.to_string());
}

#[when("the bundle is assembled")]
async fn assemble_bundle(world: &mut BehaviourWorld) {
    let assembled = assemble(&world.artifact().bundle);
    world.artifact().assembled = Some(assembled);
}

#[then(regex = r#"^the assembled workload runs from base image "([^"]+)"$"#)]
async fn runs_from_base(world: &mut BehaviourWorld, image: String) {
    let workload = world
        .artifact()
        .assembled
        .as_ref()
        .expect("assembled workload");
    assert_eq!(workload.base_image, image);
}

#[then(regex = r#"^"([^"]+)" in the assembled workload comes from fileset "([^"]+)"$"#)]
async fn comes_from_fileset(world: &mut BehaviourWorld, path: String, name: String) {
    let workload = world
        .artifact()
        .assembled
        .as_ref()
        .expect("assembled workload");
    assert_eq!(
        workload.source_of(&path),
        Some(&FileSource::Fileset(name.clone())),
        "path {path} should be owned by fileset {name}",
    );
}

#[then(regex = r#"^"([^"]+)" in the assembled workload comes from the base image$"#)]
async fn comes_from_base(world: &mut BehaviourWorld, path: String) {
    let workload = world
        .artifact()
        .assembled
        .as_ref()
        .expect("assembled workload");
    assert_eq!(
        workload.source_of(&path),
        Some(&FileSource::BaseImage),
        "path {path} should be owned by the base image",
    );
}

#[then(regex = r#"^the assembled workload launches command "([^"]+)"$"#)]
async fn launches_command(world: &mut BehaviourWorld, command: String) {
    let workload = world
        .artifact()
        .assembled
        .as_ref()
        .expect("assembled workload");
    assert_eq!(workload.command.as_deref(), Some(command.as_str()));
}

#[then(regex = r#"^the assembled workload's environment carries "([^"]+)"$"#)]
async fn env_carries(world: &mut BehaviourWorld, kv: String) {
    let (key, value) = kv.split_once('=').expect("env fixture must be KEY=VALUE");
    let workload = world
        .artifact()
        .assembled
        .as_ref()
        .expect("assembled workload");
    assert_eq!(workload.env.get(key).map(String::as_str), Some(value));
}
