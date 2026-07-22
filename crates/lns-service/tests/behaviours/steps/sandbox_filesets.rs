use std::path::Path;

use cucumber::{given, then, when};
use lns_service::artifact::fileset::{
    MaterializedFilesets, OWNED_MANIFEST_PATH, SnapshotDir, SnapshotEntry, inline_fileset_specs,
    local_fileset_specs,
};
use lns_service::artifact::{published_fileset_problems, resolved_from_sandbox};

use crate::world::BehaviourWorld;

fn sandbox_json(filesets: &str) -> Vec<u8> {
    format!(
        r#"{{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{{"name":"some-sandbox"}},"spec":{{"image":"registry.example.test/runtime:1","filesets":[{filesets}]}}}}"#
    )
    .into_bytes()
}

#[given(regex = r#"^a published sandbox declaring a digest-pinned fileset at "([^"]+)"$"#)]
fn published_with_pinned_fileset(world: &mut BehaviourWorld, mount: String) {
    world.fileset_definition = Some(sandbox_json(&format!(
        r#"{{"ref":"registry.example.test/team/skills@sha256:{}","mountPath":"{mount}"}}"#,
        "a".repeat(64)
    )));
}

#[given("a published sandbox declaring a fileset by floating tag")]
fn published_with_floating_fileset(world: &mut BehaviourWorld) {
    world.fileset_definition = Some(sandbox_json(
        r#"{"ref":"registry.example.test/team/skills:latest","mountPath":"/root/.agent/skills"}"#,
    ));
}

#[given("a published sandbox declaring a local path fileset")]
fn published_with_path_fileset(world: &mut BehaviourWorld) {
    world.fileset_definition = Some(sandbox_json(
        r#"{"path":"./skills","mountPath":"/root/.agent/skills"}"#,
    ));
}

#[when("the sandbox is planned")]
fn plan_published_sandbox(world: &mut BehaviourWorld) {
    let json = world
        .fileset_definition
        .take()
        .expect("the scenario must declare a sandbox");
    let def = lns_artifact::sandbox::parse(&json).expect("valid sandbox fixture");
    let resolved = resolved_from_sandbox(&def);
    world.fileset_problems = Some(published_fileset_problems(&resolved));
    let mut materialized = MaterializedFilesets::default();
    inline_fileset_specs(&resolved.inline_filesets, &mut materialized);
    capture_materialized(world, materialized);
    world.fileset_plan = Some(resolved);
}

fn capture_materialized(world: &mut BehaviourWorld, materialized: MaterializedFilesets) {
    let specs = materialized.into_specs();
    world.fileset_manifest = specs
        .iter()
        .find(|spec| spec.guest_path == OWNED_MANIFEST_PATH)
        .map(|spec| match &spec.source {
            lns_service::runtime_layer::RuntimeSource::Bytes(body) => {
                String::from_utf8(body.clone()).expect("utf8 manifest")
            }
            other => panic!("the chown manifest must be inline bytes, got {other:?}"),
        });
    world.fileset_contents = specs
        .iter()
        .filter_map(|spec| match &spec.source {
            lns_service::runtime_layer::RuntimeSource::Bytes(body)
                if spec.guest_path != OWNED_MANIFEST_PATH =>
            {
                Some((
                    spec.guest_path.clone(),
                    String::from_utf8(body.clone()).expect("utf8 inline file"),
                ))
            }
            _ => None,
        })
        .collect();
    world.fileset_specs = Some(specs.into_iter().map(|spec| spec.guest_path).collect());
}

#[then(regex = r#"^the resolved plan carries the fileset ref at "([^"]+)"$"#)]
fn plan_carries_fileset(world: &mut BehaviourWorld, mount: String) -> Result<(), String> {
    let problems = world.fileset_problems.as_ref().ok_or("no plan ran")?;
    if !problems.is_empty() {
        return Err(format!("the plan was unexpectedly refused: {problems:?}"));
    }
    let plan = world.fileset_plan.as_ref().ok_or("no plan captured")?;
    if plan
        .filesets
        .iter()
        .any(|fileset| fileset.paths == [mount.clone()] && fileset.reference.contains("@sha256:"))
    {
        Ok(())
    } else {
        Err(format!(
            "expected a pinned fileset at {mount:?}, got {:?}",
            plan.filesets
        ))
    }
}

#[then("the plan is refused naming the unpinned fileset ref")]
fn plan_refused_floating(world: &mut BehaviourWorld) -> Result<(), String> {
    let problems = world.fileset_problems.as_ref().ok_or("no plan ran")?;
    if problems
        .iter()
        .any(|problem| problem.contains("registry.example.test/team/skills:latest"))
    {
        Ok(())
    } else {
        Err(format!(
            "expected an unpinned-ref refusal, got {problems:?}"
        ))
    }
}

#[then("the plan is refused naming the local path")]
fn plan_refused_path(world: &mut BehaviourWorld) -> Result<(), String> {
    let problems = world.fileset_problems.as_ref().ok_or("no plan ran")?;
    if problems
        .iter()
        .any(|problem| problem.contains("local path fileset ./skills"))
    {
        Ok(())
    } else {
        Err(format!("expected a local-path refusal, got {problems:?}"))
    }
}

struct OneFileDir(String);

impl SnapshotDir for OneFileDir {
    fn entries(&self, dir: &Path) -> std::io::Result<Vec<SnapshotEntry>> {
        if dir == Path::new("/consumer/project/skills") {
            Ok(vec![SnapshotEntry {
                name: self.0.clone(),
                dir: false,
                mode: 0o644,
            }])
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no such directory",
            ))
        }
    }
}

#[given(
    regex = r#"^a local definition declaring a path fileset containing "([^"]+)" at "([^"]+)"$"#
)]
fn local_definition_with_path_fileset(world: &mut BehaviourWorld, file: String, mount: String) {
    world.fileset_definition = Some(sandbox_json(&format!(
        r#"{{"path":"/consumer/project/skills","mountPath":"{mount}"}}"#
    )));
    world.fileset_snapshot_file = Some(file);
}

#[given(
    regex = r#"^a local definition declaring a root-owned path fileset containing "([^"]+)" at "([^"]+)"$"#
)]
fn local_definition_with_root_fileset(world: &mut BehaviourWorld, file: String, mount: String) {
    world.fileset_definition = Some(sandbox_json(&format!(
        r#"{{"path":"/consumer/project/skills","mountPath":"{mount}","owner":"root"}}"#
    )));
    world.fileset_snapshot_file = Some(file);
}

#[when("the local definition is planned")]
fn plan_local_definition(world: &mut BehaviourWorld) {
    let json = world
        .fileset_definition
        .take()
        .expect("the scenario must declare a definition");
    let def = lns_artifact::sandbox::parse(&json).expect("valid sandbox fixture");
    let resolved = resolved_from_sandbox(&def);
    let file = world
        .fileset_snapshot_file
        .take()
        .expect("the scenario must stage a file");
    let mut materialized = MaterializedFilesets::default();
    local_fileset_specs(
        &OneFileDir(file),
        &resolved.local_filesets,
        &mut materialized,
    )
    .expect("the snapshot must plan");
    capture_materialized(world, materialized);
}

#[given(
    regex = r#"^a sandbox declaring inline file \"([^\"]+)\" with content `([^`]*)` at \"([^\"]+)\"$"#
)]
fn sandbox_with_inline_file(
    world: &mut BehaviourWorld,
    file: String,
    content: String,
    mount: String,
) {
    world.fileset_definition = Some(sandbox_json(&format!(
        r#"{{"inline":{{"{file}":{content:?}}},"mountPath":"{mount}"}}"#
    )));
}

#[given(regex = r#"^a sandbox declaring root-owned inline file \"([^\"]+)\" at \"([^\"]+)\"$"#)]
fn sandbox_with_root_inline_file(world: &mut BehaviourWorld, file: String, mount: String) {
    world.fileset_definition = Some(sandbox_json(&format!(
        r#"{{"inline":{{"{file}":"pinned"}},"mountPath":"{mount}","owner":"root"}}"#
    )));
}

#[given(regex = r#"^a published sandbox declaring an inline file at \"([^\"]+)\"$"#)]
fn published_with_inline_file(world: &mut BehaviourWorld, mount: String) {
    world.fileset_definition = Some(sandbox_json(&format!(
        r#"{{"inline":{{"settings.json":"published"}},"mountPath":"{mount}"}}"#
    )));
}

#[then(regex = r#"^the plan ships a chown manifest listing "([^"]+)"$"#)]
fn plan_ships_manifest(world: &mut BehaviourWorld, path: String) -> Result<(), String> {
    let manifest = world
        .fileset_manifest
        .as_ref()
        .ok_or("the plan shipped no chown manifest")?;
    if manifest.lines().any(|line| line == path) {
        Ok(())
    } else {
        Err(format!("expected {path:?} in the manifest:\n{manifest}"))
    }
}

#[then("the plan ships no chown manifest")]
fn plan_ships_no_manifest(world: &mut BehaviourWorld) -> Result<(), String> {
    match &world.fileset_manifest {
        None => Ok(()),
        Some(manifest) => Err(format!(
            "a root-owned fileset must not transfer to the workload, got:\n{manifest}"
        )),
    }
}

#[then(regex = r#"^the plan carries a guest-write spec for "([^"]+)"$"#)]
fn plan_carries_spec(world: &mut BehaviourWorld, guest_path: String) -> Result<(), String> {
    let specs = world.fileset_specs.as_ref().ok_or("no plan ran")?;
    if specs.contains(&guest_path) {
        Ok(())
    } else {
        Err(format!("expected {guest_path:?} among {specs:?}"))
    }
}

#[then(
    regex = r#"^the plan carries an inline guest-write spec for \"([^\"]+)\" with content `([^`]*)`$"#
)]
fn plan_carries_inline_content(
    world: &mut BehaviourWorld,
    guest_path: String,
    content: String,
) -> Result<(), String> {
    match world.fileset_contents.get(&guest_path) {
        Some(actual) if actual == &content => Ok(()),
        actual => Err(format!(
            "expected {guest_path:?} to contain {content:?}, got {actual:?}"
        )),
    }
}

#[then(regex = r#"^the plan carries an inline guest-write spec for \"([^\"]+)\"$"#)]
fn plan_carries_inline_spec(world: &mut BehaviourWorld, guest_path: String) -> Result<(), String> {
    plan_carries_spec(world, guest_path)
}

#[then("the plan accepts the inline fileset without a fileset ref")]
fn plan_accepts_inline_without_ref(world: &mut BehaviourWorld) -> Result<(), String> {
    let problems = world.fileset_problems.as_ref().ok_or("no plan ran")?;
    let plan = world.fileset_plan.as_ref().ok_or("no plan captured")?;
    if problems.is_empty() && plan.filesets.is_empty() && plan.inline_filesets.len() == 1 {
        Ok(())
    } else {
        Err(format!(
            "expected one accepted inline fileset, got problems={problems:?}, refs={:?}, inline={:?}",
            plan.filesets, plan.inline_filesets
        ))
    }
}
