use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use cucumber::{then, when};
use lns_cli::cli::RunArgs;
use lns_cli::command::parse_args;
use lns_cli::run::summary::{fileset_source_display, print_run_summary};
use lns_cli::run::target::{RunTarget, resolve};
use lns_cli::sandbox::author::{DirEntry, Fs, map_dir_entries};

use crate::runner::CliRun;
use crate::world::BehaviourWorld;

struct StepFs {
    files: RefCell<HashMap<PathBuf, String>>,
}

impl Fs for StepFs {
    fn read_to_string(&self, path: &Path) -> std::io::Result<String> {
        self.files
            .borrow()
            .get(path)
            .cloned()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no such file"))
    }
    fn write(&self, path: &Path, contents: &str) -> std::io::Result<()> {
        self.files
            .borrow_mut()
            .insert(path.to_path_buf(), contents.to_string());
        Ok(())
    }
    fn exists(&self, path: &Path) -> bool {
        self.files.borrow().contains_key(path)
    }
    fn read(&self, path: &Path) -> std::io::Result<Vec<u8>> {
        self.read_to_string(path).map(String::into_bytes)
    }
    fn dir_entries(&self, dir: &Path) -> std::io::Result<Vec<DirEntry>> {
        map_dir_entries(self.files.borrow().keys(), dir)
    }
}

fn resolve_local(world: &BehaviourWorld) -> anyhow::Result<RunTarget> {
    let fs = StepFs {
        files: RefCell::new(world.author_files.clone()),
    };
    resolve(None, &fs, Path::new("/work"))
}

#[when("the local sandbox run is prepared")]
fn local_run_prepared(world: &mut BehaviourWorld) {
    let target = resolve_local(world).expect("the local sandbox must resolve");
    world.wire_definition = target.definition_json();
    let mut args: RunArgs = parse_args(["lns", "run"]).expect("bare run must parse");
    if let RunTarget::Local { def, .. } = &target {
        args.filesets = def
            .spec
            .filesets
            .iter()
            .map(|fileset| (fileset_source_display(fileset), fileset.mount_path.clone()))
            .collect();
    }
    if world.cwd.is_none() {
        world.cwd = Some(tempfile::TempDir::new().expect("create tempdir"));
    }
    let cwd = world.cwd.as_ref().expect("cwd").path().to_path_buf();
    let mut buf = Vec::<u8>::new();
    print_run_summary(&args, &cwd, &mut buf).expect("print_run_summary");
    world.summary_output = String::from_utf8(buf).expect("non-utf8 summary output");
}

#[when("the user runs the local sandbox")]
fn user_runs_local_sandbox(world: &mut BehaviourWorld) {
    world.result = Some(match resolve_local(world) {
        Ok(_) => CliRun {
            exit_code: 0,
            output: String::new(),
        },
        Err(e) => CliRun {
            exit_code: 1,
            output: format!("{e:#}"),
        },
    });
}

#[then("the definition sent to the service roots the fileset path under the project")]
fn wire_definition_roots_path(world: &mut BehaviourWorld) -> Result<(), String> {
    let wire = world.wire_definition.as_ref().ok_or("no wire definition")?;
    if wire.contains("\"/work/skills\"") {
        Ok(())
    } else {
        Err(format!("expected a project-rooted path in: {wire}"))
    }
}

#[then("the definition sent to the service carries the fileset ref unchanged")]
fn wire_definition_keeps_ref(world: &mut BehaviourWorld) -> Result<(), String> {
    let wire = world.wire_definition.as_ref().ok_or("no wire definition")?;
    if wire.contains(&format!(
        "registry.example.test/team/skills@sha256:{}",
        "a".repeat(64)
    )) {
        Ok(())
    } else {
        Err(format!("expected the declared ref verbatim in: {wire}"))
    }
}

#[then(regex = r"^the run summary shows a Fileset line `([^`]+)`$")]
fn summary_shows_fileset_line(world: &mut BehaviourWorld, expected: String) -> Result<(), String> {
    let needle = format!("Fileset:   {expected}");
    if world.summary_output.contains(&needle) {
        Ok(())
    } else {
        Err(format!("expected {needle:?} in:\n{}", world.summary_output))
    }
}

#[then(regex = r#"^the command fails naming "([^"]+)"$"#)]
fn command_fails_naming(world: &mut BehaviourWorld, needle: String) -> Result<(), String> {
    let result = world.result.as_ref().ok_or("no CLI run captured")?;
    if result.exit_code != 0 && result.output.contains(&needle) {
        Ok(())
    } else {
        Err(format!(
            "expected a failure naming {needle:?}, got exit {} with: {}",
            result.exit_code, result.output
        ))
    }
}
