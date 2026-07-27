use crate::world::BehaviourWorld;
use cucumber::{given, then, when};
use lns_service::workload_cwd;
use lns_service::workload_env::run_workload_env;

fn flag_values(cmd: &str, flags: &[&str]) -> Vec<String> {
    let toks: Vec<&str> = cmd.split_whitespace().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        if flags.contains(&toks[i]) && i + 1 < toks.len() {
            out.push(toks[i + 1].to_string());
            i += 2;
        } else {
            i += 1;
        }
    }
    out
}

fn cli_workdir(cmd: &str) -> Option<String> {
    flag_values(cmd, &["-w", "--workdir"]).into_iter().next()
}

#[given(regex = r"^the image declares WORKDIR (\S+)$")]
fn image_declares_workdir(world: &mut BehaviourWorld, dir: String) {
    world.image_workdir = Some(dir);
}

#[when(regex = r"^the working directory is resolved for `lns run ([^`]+)`$")]
fn resolve_working_directory(world: &mut BehaviourWorld, cmd: String) {
    world.resolved_workdir = Some(workload_cwd::resolve(
        cli_workdir(&cmd).as_deref(),
        world.image_workdir.as_deref(),
    ));
}

#[when(regex = r"^the user runs `lns run ([^`]+)` under a policy$")]
fn user_runs_supervised(world: &mut BehaviourWorld, cmd: String) {
    compose_run_env(world, &cmd, Some("some-agent-command"));
}

#[when(regex = r"^the user runs `lns run ([^`]+)` without a policy$")]
fn user_runs_unsupervised(world: &mut BehaviourWorld, cmd: String) {
    compose_run_env(world, &cmd, None);
}

fn compose_run_env(world: &mut BehaviourWorld, cmd: &str, agent_command: Option<&str>) {
    let user_env = flag_values(cmd, &["-e", "--env"]);
    let workdir =
        workload_cwd::resolve(cli_workdir(cmd).as_deref(), world.image_workdir.as_deref());
    world.composed_env = Some(run_workload_env(
        world.image_env.as_deref(),
        &user_env,
        agent_command,
        workdir.as_deref(),
        &world.managed_vars,
        &[],
    ));
}

#[then(regex = r"^the workload working directory is (\S+)$")]
fn workdir_is(world: &mut BehaviourWorld, dir: String) -> Result<(), String> {
    match world.resolved_workdir.as_ref() {
        Some(Some(got)) if *got == dir => Ok(()),
        Some(got) => Err(format!("expected {dir:?}, got {got:?}")),
        None => Err("no working directory was resolved".to_string()),
    }
}

#[then("no working directory is forced on the workload")]
fn workdir_unset(world: &mut BehaviourWorld) -> Result<(), String> {
    match world.resolved_workdir.as_ref() {
        Some(None) => Ok(()),
        Some(Some(got)) => Err(format!("expected no workdir, got {got:?}")),
        None => Err("no working directory was resolved".to_string()),
    }
}

#[then(regex = r#"^the supervised workload env pins WORKSPACE_PATH to "([^"]+)"$"#)]
fn workspace_path_pinned(world: &mut BehaviourWorld, dir: String) -> Result<(), String> {
    let env = &world.composed_env.as_ref().ok_or("no composed env")?.env;
    let last = env
        .iter()
        .rev()
        .find_map(|e| e.strip_prefix("WORKSPACE_PATH="))
        .ok_or_else(|| format!("no WORKSPACE_PATH in {env:?}"))?;
    if last == dir {
        Ok(())
    } else {
        Err(format!("WORKSPACE_PATH pinned to {last:?}, want {dir:?}"))
    }
}

#[then("the workload env carries no WORKSPACE_PATH entry")]
fn no_workspace_path(world: &mut BehaviourWorld) -> Result<(), String> {
    let env = &world.composed_env.as_ref().ok_or("no composed env")?.env;
    if env.iter().any(|e| e.starts_with("WORKSPACE_PATH=")) {
        Err(format!("unexpected WORKSPACE_PATH in {env:?}"))
    } else {
        Ok(())
    }
}
