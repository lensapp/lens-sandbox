use crate::E2eWorld;
use crate::specutil::{arg_parser::split_args, run_cli_with_timeout};
use cucumber::{then, when};
use std::time::{Duration, Instant};

const MICROVM_RUN_TIMEOUT: Duration = Duration::from_secs(120);

fn socket_env(world: &E2eWorld) -> Vec<(&'static str, std::ffi::OsString)> {
    world
        .service_socket
        .as_ref()
        .map(|socket| vec![("LNS_SOCKET_PATH", socket.clone().into())])
        .unwrap_or_default()
}

fn parse_run_id(text: &str) -> Option<u32> {
    let marker = text.find("run #")?;
    text[marker + "run #".len()..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .ok()
}

fn run_microvm(world: &mut E2eWorld, mut run_args: Vec<String>, cmd_line: &str) {
    let mut args = vec!["run".to_string()];
    args.append(&mut run_args);
    args.push("--".to_string());
    args.extend(split_args(cmd_line));
    let result = run_cli_with_timeout(args, socket_env(world), MICROVM_RUN_TIMEOUT);
    world.last_run_id = parse_run_id(&format!("{}\n{}", result.stdout, result.stderr));
    world.result = Some(result);
}

fn track_volume(world: &mut E2eWorld, name: &str) {
    if !world.created_volumes.iter().any(|v| v == name) {
        world.created_volumes.push(name.to_string());
    }
}

#[when(regex = r#"^the user runs a microVM command "([^"]*)"$"#)]
fn run_command(world: &mut E2eWorld, cmd_line: String) {
    run_microvm(world, vec![], &cmd_line);
}

#[when(regex = r#"^the user runs a microVM command "([^"]*)" with volume "([^"]+)" at "([^"]+)"$"#)]
fn run_command_with_volume(world: &mut E2eWorld, cmd_line: String, name: String, path: String) {
    track_volume(world, &name);
    run_microvm(
        world,
        vec!["-v".into(), format!("{name}:{path}")],
        &cmd_line,
    );
}

#[when(
    regex = r#"^the user runs a microVM command "([^"]*)" with read-only volume "([^"]+)" at "([^"]+)"$"#
)]
fn run_command_with_ro_volume(world: &mut E2eWorld, cmd_line: String, name: String, path: String) {
    track_volume(world, &name);
    run_microvm(
        world,
        vec!["-v".into(), format!("{name}:{path}:ro")],
        &cmd_line,
    );
}

#[when(
    regex = r#"^the user starts a detached microVM command "([^"]*)" with volume "([^"]+)" at "([^"]+)"$"#
)]
fn start_detached_with_volume(world: &mut E2eWorld, cmd_line: String, name: String, path: String) {
    track_volume(world, &name);
    run_microvm(
        world,
        vec!["-d".into(), "-v".into(), format!("{name}:{path}")],
        &cmd_line,
    );
    if let Some(id) = world.last_run_id {
        world.detached_runs.push(id);
    }
}

#[then(regex = r#"^the audit chain for that run records volume "([^"]+)" at "([^"]+)"$"#)]
fn audit_records_volume(world: &mut E2eWorld, name: String, path: String) -> Result<(), String> {
    let run_id = world
        .last_run_id
        .ok_or("no run id was captured from the run output")?;
    let log_path = lns_ipc::audit_log_for_run(&run_id.to_string())
        .map_err(|e| format!("resolving audit log path for run {run_id}: {e}"))?;
    let contents = std::fs::read_to_string(&log_path)
        .map_err(|e| format!("reading audit chain {}: {e}", log_path.display()))?;
    let recorded = contents.lines().any(|line| {
        line.contains("volume_attached") && line.contains(&name) && line.contains(&path)
    });
    if recorded {
        Ok(())
    } else {
        Err(format!(
            "no volume_attached event for {name:?} at {path:?} in {}:\n{contents}",
            log_path.display()
        ))
    }
}

#[then(regex = r#"^volume "([^"]+)" is released$"#)]
fn volume_released(world: &mut E2eWorld, name: String) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let listing = world.run_with_service_env(&["volume", "ls"]);
        let held = listing
            .stdout
            .lines()
            .any(|line| line.contains(&name) && line.contains("run #"));
        if !held {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "volume {name:?} was still in use after 30s:\n{}",
                listing.stdout
            ));
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}
