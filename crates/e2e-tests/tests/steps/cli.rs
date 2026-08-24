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

#[given("the home config file declares an invalid run.cpus default")]
fn home_config_with_invalid_cpus(world: &mut E2eWorld) {
    let home = world
        .home
        .as_ref()
        .expect("Given a clean lns cache home first");
    let dir = home.path().join(".lns");
    std::fs::create_dir_all(&dir).expect("create config dir");
    std::fs::write(dir.join("config.yaml"), "run:\n  cpus: 0\n").expect("write config");
}

#[when(regex = r#"^I run "([^"]*)"$"#)]
fn i_run(world: &mut E2eWorld, cmd_line: String) {
    let args = split_args(&cmd_line);
    let mut envs: Vec<(&str, std::ffi::OsString)> = Vec::new();
    if let Some(home) = &world.home {
        envs.push(("HOME", home.path().into()));
        envs.push(("LNS_HOME", home.path().join(".lns").into()));
    }
    if let Some(socket) = &world.service_socket {
        envs.push(("LNS_SOCKET_PATH", socket.clone().into()));
    }
    let result = if envs.is_empty() {
        run_cli(args)
    } else {
        run_cli_with_env(args, envs)
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
        ("LNS_HOME", home.path().join(".lns")),
    ];
    world.result = Some(run_cli_with_closed_stdout(args, envs));
}

#[then(regex = r#"^the exit code is (\d+)$"#)]
fn exit_code_is(world: &mut E2eWorld, code: i32) -> Result<(), String> {
    let res = world.result.as_ref().ok_or("no CLI run captured")?;
    assert_eq_int(code, res.exit_code, "exit code").map_err(|e| {
        format!(
            "{e}\nstdout: {}\nstderr: {}",
            res.stdout.trim_end(),
            res.stderr.trim_end()
        )
    })
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

#[then(regex = r#"^the output contains `([^`]*)`$"#)]
fn output_contains_literal(world: &mut E2eWorld, needle: String) -> Result<(), String> {
    output_contains(world, needle)
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

#[then(regex = r#"^the output shows "([^"]*)" before "([^"]*)"$"#)]
fn output_shows_before(world: &mut E2eWorld, first: String, second: String) -> Result<(), String> {
    let res = world.result.as_ref().ok_or("no CLI run captured")?;
    let out = &res.stdout;
    let first_at = out
        .find(&first)
        .ok_or_else(|| format!("output missing {first:?}:\n{out}"))?;
    let second_at = out
        .find(&second)
        .ok_or_else(|| format!("output missing {second:?}:\n{out}"))?;
    if first_at < second_at {
        Ok(())
    } else {
        Err(format!(
            "expected {first:?} before {second:?}, but order was reversed:\n{out}"
        ))
    }
}
