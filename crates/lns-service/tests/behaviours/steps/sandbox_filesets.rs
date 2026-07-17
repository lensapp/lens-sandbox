use std::path::Path;

use cucumber::{given, then, when};
use lns_service::artifact::fileset::{SnapshotDir, SnapshotEntry, local_fileset_specs};
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
    world.fileset_plan = Some(resolved);
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
    let specs = local_fileset_specs(&OneFileDir(file), &resolved.local_filesets)
        .expect("the snapshot must plan");
    world.fileset_specs = Some(specs.into_iter().map(|spec| spec.guest_path).collect());
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
