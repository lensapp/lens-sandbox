use crate::E2eWorld;
use crate::specutil::{
    CliResult, assert_contains, assert_eq_int, assert_ne_int, run_cli_with_env, service_binary,
};
use cucumber::{given, then, when};

pub(crate) fn start_service(world: &mut E2eWorld) {
    start_service_with(world, &[]);
}

pub(crate) fn start_service_with(world: &mut E2eWorld, extra: &[(&str, &str)]) {
    world.ensure_service_dir();
    let mut envs: Vec<(&str, std::ffi::OsString)> = vec![
        (
            "LNS_SOCKET_PATH",
            world.service_socket.clone().unwrap().into(),
        ),
        ("LNS_SERVICE_BIN", service_binary().into()),
    ];
    if let Some(home) = &world.home {
        envs.push(("HOME", home.path().into()));
        envs.push(("LNS_HOME", home.path().join(".lns").into()));
    }
    envs.extend(extra.iter().map(|(k, v)| (*k, std::ffi::OsString::from(v))));
    let result = run_cli_with_env(["service", "start"], envs);
    assert!(
        result.exit_code == 0,
        "lns service start failed: stdout={:?} stderr={:?}\n--- service.log ---\n{}",
        result.stdout,
        result.stderr,
        read_service_log(world),
    );
}

fn read_service_log(world: &E2eWorld) -> String {
    let Some(socket) = &world.service_socket else {
        return "(no socket path on the world)".to_string();
    };
    let Some(log_path) = socket.parent().map(|dir| dir.join("service.log")) else {
        return "(socket path has no parent directory)".to_string();
    };
    std::fs::read_to_string(&log_path)
        .unwrap_or_else(|e| format!("(could not read {}: {e})", log_path.display()))
}

fn parse_pid(output: &str) -> Option<u64> {
    for line in output.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("PID:") {
            return rest.trim().parse().ok();
        }
    }
    None
}

fn parse_uptime(output: &str) -> Option<u64> {
    for line in output.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("Uptime:") {
            let rest = rest.trim().trim_end_matches('s');
            return rest.parse().ok();
        }
    }
    None
}

#[given("no Lens Sandbox service is running")]
fn no_service_running(world: &mut E2eWorld) {
    world.ensure_service_dir();
    world.shutdown_service();
}

#[given("the Lens Sandbox service is running")]
fn service_is_running(world: &mut E2eWorld) {
    start_service(world);
}

#[given("the Lens Sandbox service is running in that home")]
fn service_is_running_in_home(world: &mut E2eWorld) {
    assert!(
        world.home.is_some(),
        "Given a clean lns cache home before starting the service in it"
    );
    start_service(world);
}

#[given("the Lens Sandbox service is running headless in that home")]
fn service_is_running_headless_in_home(world: &mut E2eWorld) {
    assert!(
        world.home.is_some(),
        "Given a clean lns cache home before starting the service in it"
    );
    start_service_with(world, &[("LNS_HEADLESS", "1")]);
}

#[when(regex = r#"^the user connects connector "([^"]+)"$"#)]
fn connect_connector(world: &mut E2eWorld, id: String) {
    let home = world
        .home
        .as_ref()
        .expect("a home holds the connector catalog")
        .path()
        .to_path_buf();
    let mut envs: Vec<(&str, std::ffi::OsString)> = vec![("HOME", home.clone().into())];
    if let Some(sock) = &world.service_socket {
        envs.push(("LNS_SOCKET_PATH", sock.clone().into()));
    }
    let result = crate::specutil::run_cli_with_timeout_in_dir(
        &home,
        vec!["connector".to_string(), "connect".to_string(), id],
        envs,
        std::time::Duration::from_secs(30),
    );
    world.result = Some(result);
}

#[when("I run `lns service start`")]
fn run_service_start(world: &mut E2eWorld) {
    world.ensure_service_dir();
    let service_bin = service_binary();
    let socket = world.service_socket.as_ref().unwrap().clone();
    let sock_str = socket.to_string_lossy().into_owned();
    let bin_str = service_bin.to_string_lossy().into_owned();
    let envs = [
        ("LNS_SOCKET_PATH", sock_str.as_str()),
        ("LNS_SERVICE_BIN", bin_str.as_str()),
    ];
    let result = run_cli_with_env(["service", "start"], envs);
    world.result = Some(result);
}

#[when("I run `lns service start` again")]
fn run_service_start_again(world: &mut E2eWorld) {
    run_service_start(world);
}

#[when("I run `lns service stop`")]
fn run_service_stop(world: &mut E2eWorld) {
    let result = world.run_with_service_env(&["service", "stop"]);
    world.result = Some(result);
}

#[when("I run `lns service status`")]
fn run_service_status(world: &mut E2eWorld) {
    let result = world.run_with_service_env(&["service", "status"]);
    world.result = Some(result);
}

#[when("I run an `lns` command that requires the service")]
fn run_command_requiring_service(world: &mut E2eWorld) {
    let result = world.run_with_service_env(&["ps"]);
    world.result = Some(result);
}

#[when("I run `lns version` or `lns help`")]
fn run_version_or_help(world: &mut E2eWorld) {
    world.ensure_service_dir();
    let r1 = world.run_with_service_env(&["--version"]);
    let r2 = world.run_with_service_env(&["help"]);
    world.results = vec![r1, r2];
}

#[when("I run `lns service status` from one terminal")]
fn run_status_from_one_terminal(world: &mut E2eWorld) {
    let result = world.run_with_service_env(&["service", "status"]);
    world.results.push(result);
}

#[when("later run `lns service status` from another terminal")]
fn run_status_from_another_terminal(world: &mut E2eWorld) {
    std::thread::sleep(std::time::Duration::from_secs(1));
    let result = world.run_with_service_env(&["service", "status"]);
    world.results.push(result);
}

#[when("two `lns service status` commands run concurrently from different terminals")]
fn run_concurrent_commands(world: &mut E2eWorld) {
    use crate::specutil::lns_binary;
    let socket_path = world
        .service_socket
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned());

    let socket_path2 = socket_path.clone();
    let h1 = std::thread::spawn(move || {
        let mut cmd = std::process::Command::new(lns_binary());
        cmd.args(["service", "status"]);
        if let Some(s) = &socket_path {
            cmd.env("LNS_SOCKET_PATH", s);
        }
        let output = cmd.output().expect("spawn lns");
        CliResult {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            exit_code: output.status.code().unwrap_or(-1),
        }
    });
    let h2 = std::thread::spawn(move || {
        let mut cmd = std::process::Command::new(lns_binary());
        cmd.args(["service", "status"]);
        if let Some(s) = &socket_path2 {
            cmd.env("LNS_SOCKET_PATH", s);
        }
        let output = cmd.output().expect("spawn lns");
        CliResult {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            exit_code: output.status.code().unwrap_or(-1),
        }
    });

    world.results = vec![h1.join().unwrap(), h2.join().unwrap()];
}

#[then("`lns service start` exits successfully")]
fn start_exits_successfully(world: &mut E2eWorld) -> Result<(), String> {
    let res = world.result.as_ref().ok_or("no CLI run captured")?;
    assert_eq_int(0, res.exit_code, "exit code")
}

#[then("`lns service start` exits successfully, reporting that the service is already running")]
fn start_exits_already_running(world: &mut E2eWorld) -> Result<(), String> {
    let res = world.result.as_ref().ok_or("no CLI run captured")?;
    assert_eq_int(0, res.exit_code, "exit code")?;
    assert_contains(&res.stdout, "already running", "stdout")
}

#[then("no second service is started")]
fn no_second_service(world: &mut E2eWorld) -> Result<(), String> {
    let res = world.result.as_ref().ok_or("no CLI run captured")?;
    assert_eq_int(0, res.exit_code, "exit code")
}

#[then("`lns service stop` exits successfully")]
fn stop_exits_successfully(world: &mut E2eWorld) -> Result<(), String> {
    let res = world.result.as_ref().ok_or("no CLI run captured")?;
    assert_eq_int(0, res.exit_code, "exit code")
}

#[then("`lns service stop` exits successfully, reporting that nothing was running")]
fn stop_exits_nothing_running(world: &mut E2eWorld) -> Result<(), String> {
    let res = world.result.as_ref().ok_or("no CLI run captured")?;
    assert_eq_int(0, res.exit_code, "exit code")?;
    assert_contains(&res.stdout, "not running", "stdout")
}

#[then("the command exits with a non-zero status")]
fn command_exits_nonzero(world: &mut E2eWorld) -> Result<(), String> {
    let res = world.result.as_ref().ok_or("no CLI run captured")?;
    assert_ne_int(0, res.exit_code, "exit code")
}

#[then(
    "the error message reads: \"Lens Sandbox is not running. Run `lns service start` to start it.\""
)]
fn error_message_reads(world: &mut E2eWorld) -> Result<(), String> {
    let res = world.result.as_ref().ok_or("no CLI run captured")?;
    let combined = format!("{}{}", res.stdout, res.stderr);
    assert_contains(
        &combined,
        "Lens Sandbox is not running. Run `lns service start` to start it.",
        "error output",
    )
}

#[then("the command completes successfully")]
fn command_completes_successfully(world: &mut E2eWorld) -> Result<(), String> {
    if world.results.is_empty() {
        let res = world.result.as_ref().ok_or("no CLI run captured")?;
        return assert_eq_int(0, res.exit_code, "exit code");
    }
    for (i, res) in world.results.iter().enumerate() {
        assert_eq_int(0, res.exit_code, &format!("exit code of result {i}"))?;
    }
    Ok(())
}

#[then("no service is started as a side effect")]
fn no_service_side_effect(world: &mut E2eWorld) -> Result<(), String> {
    if let Some(sock) = &world.service_socket
        && sock.exists()
    {
        return Err(format!(
            "socket file exists at {}, service was started",
            sock.display()
        ));
    }
    Ok(())
}

#[then("the command reports that the service is running")]
fn reports_service_running(world: &mut E2eWorld) -> Result<(), String> {
    let res = world.result.as_ref().ok_or("no CLI run captured")?;
    assert_contains(&res.stdout, "is running", "stdout")
}

#[then("the report includes the service's PID, uptime, and version")]
fn report_includes_details(world: &mut E2eWorld) -> Result<(), String> {
    let res = world.result.as_ref().ok_or("no CLI run captured")?;
    assert_contains(&res.stdout, "PID:", "stdout")?;
    assert_contains(&res.stdout, "Uptime:", "stdout")?;
    assert_contains(&res.stdout, "Version:", "stdout")
}

#[then("the command reports that no service is running")]
fn reports_no_service(world: &mut E2eWorld) -> Result<(), String> {
    let res = world.result.as_ref().ok_or("no CLI run captured")?;
    assert_contains(&res.stdout, "is not running", "stdout")
}

#[then("both invocations report the same PID")]
fn both_same_pid(world: &mut E2eWorld) -> Result<(), String> {
    if world.results.len() < 2 {
        return Err("need at least 2 results".into());
    }
    let pid1 =
        parse_pid(&world.results[0].stdout).ok_or("could not parse PID from first result")?;
    let pid2 =
        parse_pid(&world.results[1].stdout).ok_or("could not parse PID from second result")?;
    if pid1 == pid2 {
        Ok(())
    } else {
        Err(format!("PIDs differ: {pid1} vs {pid2}"))
    }
}

#[then("the second invocation reports a strictly greater uptime than the first")]
fn second_uptime_greater(world: &mut E2eWorld) -> Result<(), String> {
    if world.results.len() < 2 {
        return Err("need at least 2 results".into());
    }
    let u1 =
        parse_uptime(&world.results[0].stdout).ok_or("could not parse uptime from first result")?;
    let u2 = parse_uptime(&world.results[1].stdout)
        .ok_or("could not parse uptime from second result")?;
    if u2 > u1 {
        Ok(())
    } else {
        Err(format!("expected second uptime ({u2}) > first ({u1})"))
    }
}

#[then("both observe consistent state from the same service instance")]
fn both_consistent_state(world: &mut E2eWorld) -> Result<(), String> {
    both_same_pid(world)
}

#[then("neither invocation corrupts or races the other")]
fn no_corruption(world: &mut E2eWorld) -> Result<(), String> {
    for (i, res) in world.results.iter().enumerate() {
        assert_eq_int(0, res.exit_code, &format!("exit code of result {i}"))?;
        if parse_pid(&res.stdout).is_none() {
            return Err(format!("result {i} has no parseable PID"));
        }
    }
    Ok(())
}
