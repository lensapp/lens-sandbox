use crate::E2eWorld;
use crate::specutil::arg_parser::split_args;
use crate::specutil::{assert_contains, run_cli_in_dir};
use cucumber::{then, when};

fn substitute_pushed_ref(world: &E2eWorld, cmd_line: &str) -> String {
    match &world.pushed_ref {
        Some(reference) => cmd_line.replace("<pushed-ref>", reference),
        None => cmd_line.to_string(),
    }
}

#[when(regex = r#"^I run sandbox command "([^"]*)" against the service$"#)]
fn run_sandbox_command(world: &mut E2eWorld, cmd_line: String) {
    let mut args = vec!["sandbox".to_string()];
    args.extend(split_args(&substitute_pushed_ref(world, &cmd_line)));
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    world.result = Some(world.run_with_service_env(&arg_refs));
}

#[when(regex = r#"^I run lns "([^"]*)" against the service$"#)]
fn run_lns_command(world: &mut E2eWorld, cmd_line: String) {
    let args = split_args(&substitute_pushed_ref(world, &cmd_line));
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    world.result = Some(world.run_with_service_env(&arg_refs));
}

fn project_dir(world: &mut E2eWorld) -> std::path::PathBuf {
    world
        .project
        .get_or_insert_with(|| tempfile::TempDir::new().expect("project tempdir"))
        .path()
        .to_path_buf()
}

#[when(regex = r#"^I run "([^"]*)" in the project directory$"#)]
fn run_in_project_dir(world: &mut E2eWorld, cmd_line: String) {
    let project = project_dir(world);
    let mut envs: Vec<(String, std::ffi::OsString)> = Vec::new();
    if let Some(home) = &world.home {
        envs.push(("HOME".into(), home.path().into()));
        envs.push(("XDG_CACHE_HOME".into(), home.path().join(".cache").into()));
    }
    if let Some(socket) = &world.service_socket {
        envs.push(("LNS_SOCKET_PATH".into(), socket.clone().into()));
    }
    world.result = Some(run_cli_in_dir(&project, split_args(&cmd_line), envs));
}

#[then(regex = r#"^the project file "([^"]*)" contains "([^"]*)"$"#)]
fn project_file_contains(world: &mut E2eWorld, name: String, needle: String) -> Result<(), String> {
    let project = world.project.as_ref().ok_or("no project directory")?;
    let contents = std::fs::read_to_string(project.path().join(&name))
        .map_err(|e| format!("reading project file {name}: {e}"))?;
    assert_contains(&contents, &needle, &name)
}

#[then("the output contains the pushed reference")]
fn output_contains_pushed_ref(world: &mut E2eWorld) -> Result<(), String> {
    let reference = world.pushed_ref.clone().ok_or("no sandbox was pushed")?;
    let res = world.result.as_ref().ok_or("no CLI run captured")?;
    let combined = format!("{}{}", res.stdout, res.stderr);
    assert_contains(&combined, &reference, "output")
}

#[then("the output no longer lists the pushed reference")]
fn output_missing_pushed_ref(world: &mut E2eWorld) -> Result<(), String> {
    let reference = world.pushed_ref.clone().ok_or("no sandbox was pushed")?;
    let res = world.result.as_ref().ok_or("no CLI run captured")?;
    let combined = format!("{}{}", res.stdout, res.stderr);
    if combined.contains(&reference) {
        return Err(format!(
            "expected output not to list {reference:?}, got {combined:?}"
        ));
    }
    Ok(())
}
