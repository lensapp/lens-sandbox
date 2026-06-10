use crate::E2eWorld;
use crate::specutil::{
    arg_parser::split_args, assert_contains, assert_eq_int, assert_ne_int, run_cli,
    run_cli_with_closed_stdout, run_cli_with_env,
};
use cucumber::{given, then, when};

#[given("a clean lns cache home")]
fn fresh_home(world: &mut E2eWorld) {
    world.home = Some(tempfile::TempDir::new().expect("tempdir"));
}

#[given(regex = r#"^the home config file declares a malformed run\.env entry "([^"]+)"$"#)]
fn home_config_with_malformed_env(world: &mut E2eWorld, entry: String) {
    let home = world
        .home
        .as_ref()
        .expect("Given a clean lns cache home first");
    let config_dir = if cfg!(target_os = "macos") {
        home.path().join("Library").join("Application Support")
    } else {
        home.path().join(".config")
    };
    let dir = config_dir.join("lns");
    std::fs::create_dir_all(&dir).expect("create config dir");
    std::fs::write(
        dir.join("config.yaml"),
        format!("run:\n  env:\n    - {entry}\n"),
    )
    .expect("write config");
}

#[when(regex = r#"^I run "([^"]*)"$"#)]
fn i_run(world: &mut E2eWorld, cmd_line: String) {
    let args = split_args(&cmd_line);
    let result = match &world.home {
        Some(home) => {
            let envs = [
                ("HOME", home.path().to_path_buf()),
                ("XDG_CACHE_HOME", home.path().join(".cache")),
                ("XDG_CONFIG_HOME", home.path().join(".config")),
            ];
            run_cli_with_env(args, envs)
        }
        None => run_cli(args),
    };
    world.result = Some(result);
}

#[when(regex = r#"^I run "([^"]*)" with stdout closed$"#)]
fn i_run_with_stdout_closed(world: &mut E2eWorld, cmd_line: String) {
    let args = split_args(&cmd_line);
    let home = world
        .home
        .as_ref()
        .expect("Given a clean lns cache home before piping");
    let envs = [
        ("HOME", home.path().to_path_buf()),
        ("XDG_CACHE_HOME", home.path().join(".cache")),
    ];
    world.result = Some(run_cli_with_closed_stdout(args, envs));
}

#[then(regex = r#"^the exit code is (\d+)$"#)]
fn exit_code_is(world: &mut E2eWorld, code: i32) -> Result<(), String> {
    let res = world.result.as_ref().ok_or("no CLI run captured")?;
    assert_eq_int(code, res.exit_code, "exit code")
}

#[then("the exit code is non-zero")]
fn exit_code_nonzero(world: &mut E2eWorld) -> Result<(), String> {
    let res = world.result.as_ref().ok_or("no CLI run captured")?;
    assert_ne_int(0, res.exit_code, "exit code")
}

#[then(regex = r#"^the output contains "([^"]*)"$"#)]
fn output_contains(world: &mut E2eWorld, needle: String) -> Result<(), String> {
    let res = world.result.as_ref().ok_or("no CLI run captured")?;
    let combined = format!("{}{}", res.stdout, res.stderr);
    assert_contains(&combined, &needle, "output")
}

#[then(regex = r#"^the output does not contain "([^"]*)"$"#)]
fn output_does_not_contain(world: &mut E2eWorld, needle: String) -> Result<(), String> {
    let res = world.result.as_ref().ok_or("no CLI run captured")?;
    let combined = format!("{}{}", res.stdout, res.stderr);
    if combined.contains(&needle) {
        Err(format!(
            "expected output not to contain {needle:?}, got {combined:?}"
        ))
    } else {
        Ok(())
    }
}
