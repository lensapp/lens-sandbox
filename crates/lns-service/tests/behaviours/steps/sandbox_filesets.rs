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
        r#"{{"apiVersion":"lns.run/v1","kind":"sandbox","name":"some-sandbox","spec":{{"image":"registry.example.test/runtime:1","filesets":[{filesets}]}}}}"#
    )
    .into_bytes()
}

/// The reference the scenario's own artifact was pulled at, digest-pinned as every published run's is.
const SANDBOX_ARTIFACT: &str = "registry.example.test/team/sandbox@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

#[given(regex = r#"^a published sandbox declaring a path fileset at "([^"]+)"$"#)]
fn published_with_path_fileset(world: &mut BehaviourWorld, mount: String) {
    world.fileset_definition = Some(sandbox_json(&format!(
        r#"{{"path":"./skills","mountPath":"{mount}"}}"#
    )));
}

#[given(regex = r#"^its artifact carries (\d+) packed layers?$"#)]
fn artifact_carries_layers(world: &mut BehaviourWorld, layers: usize) {
    world.fileset_artifact_layers = Some(layers);
}

#[when("the sandbox is planned")]
fn plan_published_sandbox(world: &mut BehaviourWorld) {
    let json = world
        .fileset_definition
        .take()
        .expect("the scenario must declare a sandbox");
    let def = lns_artifact::sandbox::parse(&json).expect("valid sandbox fixture");
    let carriers = [(
        lns_artifact::merge::ROOT_LABEL.to_string(),
        lns_service::artifact::Carrier {
            reference: SANDBOX_ARTIFACT.to_string(),
            layers: (0..world.fileset_artifact_layers.take().unwrap_or_default())
                .map(|i| lns_service::artifact::PackedLayer {
                    digest: format!("sha256:{}", (b'a' + i as u8) as char),
                    size: 512,
                })
                .collect(),
        },
    )]
    .into();
    let packed = match lns_service::artifact::correlate_packed_filesets(
        &lns_artifact::merge::own_fileset_origins(&def.spec),
        &[(
            lns_artifact::merge::ROOT_LABEL.to_string(),
            lns_artifact::merge::path_filesets(&def.spec).count(),
        )]
        .into(),
        &carriers,
    ) {
        Ok(packed) => packed,
        Err(problems) => {
            world.fileset_problems = Some(problems);
            return;
        }
    };
    let resolved = resolved_from_sandbox(&def, &packed);
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

#[then(regex = r#"^the plan pulls the fileset at "([^"]+)" from the sandbox's own artifact$"#)]
fn plan_pulls_fileset_from_its_own_artifact(
    world: &mut BehaviourWorld,
    mount: String,
) -> Result<(), String> {
    let problems = world.fileset_problems.as_ref().ok_or("no plan ran")?;
    if !problems.is_empty() {
        return Err(format!("the plan was unexpectedly refused: {problems:?}"));
    }
    let plan = world.fileset_plan.as_ref().ok_or("no plan captured")?;
    if plan
        .packed_filesets
        .iter()
        .any(|fileset| fileset.mount_path == mount && fileset.source.reference == SANDBOX_ARTIFACT)
    {
        Ok(())
    } else {
        Err(format!(
            "the files and the declaration that mounts them share one digest, so the layer has to come out of the approved artifact; got {:?} and {:?}",
            plan.packed_filesets, plan.local_filesets
        ))
    }
}

#[then("the plan is refused naming the layer count")]
fn plan_refused_layer_count(world: &mut BehaviourWorld) -> Result<(), String> {
    let problems = world.fileset_problems.as_ref().ok_or("no plan ran")?;
    if problems
        .iter()
        .any(|problem| problem.contains("declares 1 path fileset(s) but its artifact carries 0"))
    {
        Ok(())
    } else {
        Err(format!(
            "an artifact carries one layer per path entry, so a mismatch has to refuse the run rather than leave a mount unseeded; got {problems:?}"
        ))
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
    let resolved = resolved_from_sandbox(&def, &Default::default());
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

#[then("the plan accepts the inline fileset with nothing to pull")]
fn plan_accepts_inline_with_nothing_to_pull(world: &mut BehaviourWorld) -> Result<(), String> {
    let problems = world.fileset_problems.as_ref().ok_or("no plan ran")?;
    let plan = world.fileset_plan.as_ref().ok_or("no plan captured")?;
    if problems.is_empty() && plan.packed_filesets.is_empty() && plan.inline_filesets.len() == 1 {
        Ok(())
    } else {
        Err(format!(
            "inline content lives in the document itself, so nothing is fetched for it; got problems={problems:?}, packed={:?}, inline={:?}",
            plan.packed_filesets, plan.inline_filesets
        ))
    }
}

struct StagedHost {
    files: std::collections::HashMap<std::path::PathBuf, u32>,
    home: Option<std::path::PathBuf>,
}

impl lns_service::artifact::fileset::HostFileProbe for StagedHost {
    fn home(&self) -> Option<std::path::PathBuf> {
        self.home.clone()
    }

    fn stat(
        &self,
        path: &Path,
    ) -> std::io::Result<Option<lns_service::artifact::fileset::HostFileFacts>> {
        Ok(self
            .files
            .get(path)
            .map(|mode| lns_service::artifact::fileset::HostFileFacts {
                mode: *mode,
                is_regular_file: true,
            }))
    }
}

#[given(
    regex = r#"^a (published sandbox|definition) declaring an? (optional )?hostPath fileset "([^"]+)" at "([^"]+)"$"#
)]
fn definition_with_host_path_fileset(
    world: &mut BehaviourWorld,
    _who: String,
    optional: String,
    source: String,
    mount: String,
) {
    let optional = !optional.is_empty();
    world.fileset_definition = Some(sandbox_json(&format!(
        r#"{{"hostPath":"{source}","mountPath":"{mount}","optional":{optional}}}"#
    )));
}

#[given(regex = r#"^the host file "([^"]+)" exists with mode 0([0-7]+)$"#)]
fn host_file_exists(world: &mut BehaviourWorld, path: String, mode: String) {
    let mode = u32::from_str_radix(&mode, 8).expect("octal mode");
    world
        .host_files
        .insert(std::path::PathBuf::from(path), mode);
}

#[given(regex = r#"^this machine's home directory is "([^"]+)"$"#)]
fn machine_home_is(world: &mut BehaviourWorld, home: String) {
    world.host_home = Some(std::path::PathBuf::from(home));
}

#[when("the host files are planned")]
fn plan_host_files(world: &mut BehaviourWorld) {
    let json = world
        .fileset_definition
        .take()
        .expect("the scenario must declare a definition");
    let def = lns_artifact::sandbox::parse(&json).expect("valid sandbox fixture");
    let resolved = resolved_from_sandbox(&def, &Default::default());
    let probe = StagedHost {
        files: world.host_files.clone(),
        home: world.host_home.clone(),
    };
    let mut materialized = MaterializedFilesets::default();
    match lns_service::artifact::fileset::host_fileset_specs(
        &probe,
        &resolved.host_filesets,
        &[],
        &mut materialized,
    ) {
        Ok(()) => {
            world.fileset_problems = Some(Vec::new());
            world.host_file_writes = materialized
                .specs
                .iter()
                .filter_map(|spec| match &spec.source {
                    lns_service::runtime_layer::RuntimeSource::HostFile(path) => {
                        Some((path.to_string_lossy().into_owned(), spec.guest_path.clone()))
                    }
                    _ => None,
                })
                .collect();
            capture_materialized(world, materialized);
        }
        Err(error) => world.fileset_problems = Some(vec![format!("{error:#}")]),
    }
}

#[then(regex = r#"^the plan carries a host-file write from "([^"]+)" to "([^"]+)"$"#)]
fn plan_carries_host_file_write(
    world: &mut BehaviourWorld,
    source: String,
    guest_path: String,
) -> Result<(), String> {
    if world
        .host_file_writes
        .contains(&(source.clone(), guest_path.clone()))
    {
        Ok(())
    } else {
        Err(format!(
            "a hostPath fileset must plan as a launch-time snapshot of the host file, not a live share — expected ({source:?}, {guest_path:?}) among {:?}",
            world.host_file_writes
        ))
    }
}

#[then("the plan carries no guest-write spec")]
fn plan_carries_no_spec(world: &mut BehaviourWorld) -> Result<(), String> {
    let problems = world.fileset_problems.as_ref().ok_or("no plan ran")?;
    let specs = world.fileset_specs.as_ref().ok_or("no plan captured")?;
    if problems.is_empty() && specs.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "expected an empty plan, got problems={problems:?} specs={specs:?}"
        ))
    }
}

#[then(regex = r#"^the plan is refused naming "([^"]+)"$"#)]
fn plan_refused_naming(world: &mut BehaviourWorld, needle: String) -> Result<(), String> {
    let problems = world.fileset_problems.as_ref().ok_or("no plan ran")?;
    if problems.iter().any(|problem| problem.contains(&needle)) {
        Ok(())
    } else {
        Err(format!(
            "expected a refusal naming {needle:?}, got {problems:?}"
        ))
    }
}

#[then("the plan accepts the hostPath fileset")]
fn plan_accepts_host_path(world: &mut BehaviourWorld) -> Result<(), String> {
    let problems = world.fileset_problems.as_ref().ok_or("no plan ran")?;
    let plan = world.fileset_plan.as_ref().ok_or("no plan captured")?;
    if problems.is_empty() && plan.local_filesets.is_empty() && plan.host_filesets.len() == 1 {
        Ok(())
    } else {
        Err(format!(
            "expected one accepted hostPath fileset, got problems={problems:?}, local={:?}, host={:?}",
            plan.local_filesets, plan.host_filesets
        ))
    }
}
