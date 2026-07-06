use crate::world::BehaviourWorld;
use cucumber::{given, then, when};
use lns_service::artifact::assembly::{Override, ResolvedFileset, apply_with, assemble};

#[given(regex = r#"^a bundle declaring a fileset "([^"]+)" mounting "([^"]+)"$"#)]
async fn declaring_fileset(world: &mut BehaviourWorld, name: String, path: String) {
    world.artifact().bundle.filesets.push(ResolvedFileset {
        name,
        paths: vec![path],
    });
}

fn fileset_override(name: &str, path: &str) -> Override {
    Override {
        kind: "FileSet".into(),
        name: name.into(),
        mount_path: Some(path.into()),
    }
}

fn run_with_overrides(world: &mut BehaviourWorld) {
    let bundle = world.artifact().bundle.clone();
    let overrides = world.artifact().overrides.clone();
    match apply_with(bundle, &overrides) {
        Ok(resolved) => world.artifact().assembled = Some(assemble(&resolved)),
        Err(e) => world.artifact().override_error = Some(format!("{e:#}")),
    }
}

#[when(regex = r#"^the bundle is run with --with a fileset "([^"]+)" mounting "([^"]+)"$"#)]
async fn run_with_one(world: &mut BehaviourWorld, name: String, path: String) {
    world.artifact().overrides = vec![fileset_override(&name, &path)];
    run_with_overrides(world);
}

#[when(
    regex = r#"^the bundle is run with --with a fileset "([^"]+)" mounting "([^"]+)" and --with a fileset "([^"]+)" mounting "([^"]+)"$"#
)]
async fn run_with_two(
    world: &mut BehaviourWorld,
    name_a: String,
    path_a: String,
    name_b: String,
    path_b: String,
) {
    world.artifact().overrides = vec![
        fileset_override(&name_a, &path_a),
        fileset_override(&name_b, &path_b),
    ];
    run_with_overrides(world);
}

#[when(regex = r#"^the bundle is run with --with an artifact of kind "([^"]+)"$"#)]
async fn run_with_unsupported(world: &mut BehaviourWorld, kind: String) {
    world.artifact().overrides = vec![Override {
        kind,
        name: "override".into(),
        mount_path: Some("/x".into()),
    }];
    run_with_overrides(world);
}

#[then("the run is refused because the override kind is unsupported")]
async fn refused_override_kind(world: &mut BehaviourWorld) {
    let err = world
        .artifact()
        .override_error
        .clone()
        .expect("expected an override refusal");
    assert!(
        err.contains("unsupported"),
        "expected an unsupported-kind refusal, got: {err}",
    );
}

#[then("nothing is assembled")]
async fn nothing_assembled(world: &mut BehaviourWorld) {
    assert!(
        world.artifact().assembled.is_none(),
        "a refused override must not assemble a workload",
    );
}
