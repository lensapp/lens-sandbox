use crate::world::BehaviourWorld;
use clap::Parser;
use cucumber::gherkin::Step;
use cucumber::{given, then, when};
use lns_cli::cli::{Cli, Command};
use lns_cli::run::env_file::merged_run_env;
use std::path::PathBuf;

fn cwd(world: &mut BehaviourWorld) -> PathBuf {
    world
        .cwd
        .get_or_insert_with(|| tempfile::TempDir::new().expect("create tempdir"))
        .path()
        .to_path_buf()
}

fn merged(world: &BehaviourWorld) -> Result<&Result<Vec<String>, String>, String> {
    world
        .merged_env
        .as_ref()
        .ok_or_else(|| "no merge attempted".to_string())
}

#[given(regex = r"^the working directory contains an env file `([^`]+)`:$")]
fn env_file_with_content(world: &mut BehaviourWorld, step: &Step, name: String) {
    let content = step.docstring().expect("env file step needs a docstring");
    let dir = cwd(world);
    std::fs::write(dir.join(&name), content.trim_start_matches('\n')).expect("write env file");
}

#[when(regex = r"^the run environment is assembled for `lns run ([^`]+)`$")]
fn assemble_run_env(world: &mut BehaviourWorld, args_after_run: String) {
    let dir = cwd(world);
    let mut argv = vec!["lns".to_string(), "run".to_string()];
    argv.extend(args_after_run.split_whitespace().map(str::to_string));
    let cli = Cli::try_parse_from(&argv).expect("argv must parse against the CLI grammar");
    let Command::Run(args) = cli.command else {
        panic!("env-file rig only drives `lns run`");
    };
    let files: Vec<PathBuf> = args.env_file.iter().map(|p| dir.join(p)).collect();
    world.merged_env = Some(merged_run_env(&files, &args.env).map_err(|e| format!("{e:#}")));
}

#[then(regex = r"^the run environment contains `([^`]+)`$")]
fn run_env_contains(world: &mut BehaviourWorld, entry: String) -> Result<(), String> {
    match merged(world)? {
        Ok(env) if env.contains(&entry) => Ok(()),
        Ok(env) => Err(format!("expected {entry:?} in {env:?}")),
        Err(e) => Err(format!("merge failed: {e}")),
    }
}

#[then(regex = r"^the run environment does not contain `([^`]+)`$")]
fn run_env_does_not_contain(world: &mut BehaviourWorld, entry: String) -> Result<(), String> {
    match merged(world)? {
        Ok(env) if env.contains(&entry) => Err(format!("{entry:?} must not be in {env:?}")),
        Ok(_) => Ok(()),
        Err(e) => Err(format!("merge failed: {e}")),
    }
}

#[then(regex = r"^the merge fails naming `([^`]+)` line (\d+)$")]
fn merge_fails_naming_file_and_line(
    world: &mut BehaviourWorld,
    file: String,
    line: u32,
) -> Result<(), String> {
    match merged(world)? {
        Ok(env) => Err(format!("expected a failure, got {env:?}")),
        Err(e) if e.contains(&file) && e.contains(&format!(":{line}:")) => Ok(()),
        Err(e) => Err(format!("error does not name {file}:{line}: {e}")),
    }
}

#[then("the merge failure requires KEY=VALUE form")]
fn merge_failure_requires_kv(world: &mut BehaviourWorld) -> Result<(), String> {
    match merged(world)? {
        Ok(env) => Err(format!("expected a failure, got {env:?}")),
        Err(e) if e.contains("KEY=VALUE") => Ok(()),
        Err(e) => Err(format!("error does not require KEY=VALUE form: {e}")),
    }
}

#[then(regex = r"^the merge fails with an error mentioning `([^`]+)`$")]
fn merge_fails_mentioning(world: &mut BehaviourWorld, needle: String) -> Result<(), String> {
    match merged(world)? {
        Ok(env) => Err(format!("expected a failure, got {env:?}")),
        Err(e) if e.contains(&needle) => Ok(()),
        Err(e) => Err(format!("error does not mention {needle:?}: {e}")),
    }
}
