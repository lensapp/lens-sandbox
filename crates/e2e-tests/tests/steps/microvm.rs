use crate::E2eWorld;
use crate::specutil::{arg_parser::split_args, run_cli_with_timeout};
use cucumber::{given, then, when};
use std::time::{Duration, Instant};

const MICROVM_RUN_TIMEOUT: Duration = Duration::from_secs(120);

fn socket_env(world: &E2eWorld) -> Vec<(&'static str, std::ffi::OsString)> {
    world
        .service_socket
        .as_ref()
        .map(|socket| vec![("LNS_SOCKET_PATH", socket.clone().into())])
        .unwrap_or_default()
}

fn parse_run_id(text: &str) -> Option<String> {
    let marker = text.find("run ")?;
    let id: String = text[marker + "run ".len()..]
        .chars()
        .take_while(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        .collect();
    (!id.is_empty()).then_some(id)
}

fn run_microvm(world: &mut E2eWorld, mut run_args: Vec<String>, cmd_line: &str) {
    let mut args = vec!["run".to_string()];
    if let Some(policy) = &world.policy_path {
        args.push("--policy".to_string());
        args.push(policy.to_string_lossy().into_owned());
    }
    args.append(&mut run_args);
    args.push("--".to_string());
    args.extend(split_args(cmd_line));
    let result = run_cli_with_timeout(args, socket_env(world), MICROVM_RUN_TIMEOUT);
    world.last_run_id = parse_run_id(&format!("{}\n{}", result.stdout, result.stderr));
    world.result = Some(result);
}

fn last_run(world: &E2eWorld) -> Result<String, String> {
    world
        .last_run_id
        .clone()
        .ok_or_else(|| "no run id was captured from the run output".to_string())
}

fn write_policy(world: &mut E2eWorld, yaml: &str) {
    let dir = tempfile::TempDir::new().expect("tempdir for policy");
    let path = dir.path().join("lns-policy.yaml");
    std::fs::write(&path, yaml).expect("write policy file");
    world.policy_dir = Some(dir);
    world.policy_path = Some(path);
}

fn track_volume(world: &mut E2eWorld, name: &str) {
    if !world.created_volumes.iter().any(|v| v == name) {
        world.created_volumes.push(name.to_string());
    }
}

fn host_bind_source(world: &E2eWorld) -> String {
    world
        .host_bind_dir
        .as_ref()
        .expect("a host directory must be created before binding it")
        .path()
        .to_string_lossy()
        .into_owned()
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
    if let Some(id) = world.last_run_id.clone() {
        world.detached_runs.push(id);
    }
}

fn resolve_run_audit_log(run_id: &str) -> Result<std::path::PathBuf, String> {
    let direct = lns_ipc::audit_log_for_run(run_id)
        .map_err(|e| format!("resolving audit log path for run {run_id}: {e}"))?;
    if direct.exists() {
        return Ok(direct);
    }
    let runs_root = direct
        .parent()
        .and_then(std::path::Path::parent)
        .ok_or("audit log path has no runs root")?;
    std::fs::read_dir(runs_root)
        .map_err(|e| format!("reading runs root {}: {e}", runs_root.display()))?
        .flatten()
        .find(|entry| entry.file_name().to_string_lossy().starts_with(run_id))
        .map(|entry| entry.path().join("audit.jsonl"))
        .filter(|candidate| candidate.exists())
        .ok_or_else(|| {
            format!(
                "no run directory matching {run_id:?} under {}",
                runs_root.display()
            )
        })
}

#[then(regex = r#"^the audit chain for that run records volume "([^"]+)" at "([^"]+)"$"#)]
fn audit_records_volume(world: &mut E2eWorld, name: String, path: String) -> Result<(), String> {
    let run_id = last_run(world)?;
    let log_path = resolve_run_audit_log(&run_id)?;
    let contents = std::fs::read_to_string(&log_path)
        .map_err(|e| format!("reading audit chain {}: {e}", log_path.display()))?;
    let recorded = contents.lines().any(|line| {
        line.contains("\"lns_kind\":\"volume\"") && line.contains(&name) && line.contains(&path)
    });
    if recorded {
        Ok(())
    } else {
        Err(format!(
            "no volume mount event for {name:?} at {path:?} in {}:\n{contents}",
            log_path.display()
        ))
    }
}

#[then(regex = r#"^"lns audit" for that run reports an "([^"]+)" event naming "([^"]+)"$"#)]
fn audit_reports_event(world: &mut E2eWorld, kind: String, name: String) -> Result<(), String> {
    let id = last_run(world)?;
    let result = world.run_with_service_env(&["audit", id.as_str(), "--kind", kind.as_str()]);
    let out = format!("{}{}", result.stdout, result.stderr);
    if out.contains(&kind) && out.contains(&name) {
        Ok(())
    } else {
        Err(format!(
            "`lns audit {id} --kind {kind}` did not report a {kind:?} event naming {name:?}:\n{out}"
        ))
    }
}

#[then("the audit log for that run records the denied egress with the client endpoint and process")]
fn audit_egress_records_client(world: &mut E2eWorld) -> Result<(), String> {
    let run_id = last_run(world)?;
    let log_path = resolve_run_audit_log(&run_id)?;
    let contents = std::fs::read_to_string(&log_path)
        .map_err(|e| format!("reading audit log {}: {e}", log_path.display()))?;
    let line = contents
        .lines()
        .find(|l| l.contains("1.1.1.1") && l.contains("\"src_endpoint\""))
        .ok_or_else(|| {
            format!(
                "no egress event carrying src_endpoint for 1.1.1.1 in {}:\n{contents}",
                log_path.display()
            )
        })?;
    if line.contains("\"actor\"") && line.contains("\"process\"") && line.contains("\"pid\"") {
        Ok(())
    } else {
        Err(format!(
            "egress event carried src_endpoint but no actor.process:\n{line}"
        ))
    }
}

#[given(regex = r#"^a host directory with a file "([^"]+)" containing "([^"]*)"$"#)]
fn host_dir_with_file(world: &mut E2eWorld, name: String, content: String) {
    let dir = tempfile::TempDir::new().expect("tempdir for host bind");
    std::fs::write(dir.path().join(&name), content).expect("seed host bind file");
    world.host_bind_dir = Some(dir);
}

#[when(regex = r#"^the user runs a microVM command "([^"]*)" with a host bind at "([^"]+)"$"#)]
fn run_command_with_host_bind(world: &mut E2eWorld, cmd_line: String, target: String) {
    let src = host_bind_source(world);
    run_microvm(
        world,
        vec!["-v".into(), format!("{src}:{target}")],
        &cmd_line,
    );
}

#[when(
    regex = r#"^the user runs a microVM command "([^"]*)" with a read-only host bind at "([^"]+)"$"#
)]
fn run_command_with_ro_host_bind(world: &mut E2eWorld, cmd_line: String, target: String) {
    let src = host_bind_source(world);
    run_microvm(
        world,
        vec!["-v".into(), format!("{src}:{target}:ro")],
        &cmd_line,
    );
}

#[then(regex = r#"^the host bind directory has a file "([^"]+)" containing "([^"]*)"$"#)]
fn host_bind_has_file(world: &mut E2eWorld, name: String, expected: String) -> Result<(), String> {
    let dir = world
        .host_bind_dir
        .as_ref()
        .ok_or("no host bind directory was created")?;
    let path = dir.path().join(&name);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(got) = std::fs::read_to_string(&path)
            && got.trim() == expected
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            let got = std::fs::read_to_string(&path).unwrap_or_default();
            return Err(format!(
                "host bind file {name:?} expected {expected:?}, got {got:?}"
            ));
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

#[when(regex = r#"^the user runs a microVM command "([^"]*)" with env "([^"]+)"$"#)]
fn run_command_with_env(world: &mut E2eWorld, cmd_line: String, env: String) {
    run_microvm(world, vec!["-e".into(), env], &cmd_line);
}

#[when(regex = r#"^the user runs image "([^"]+)" with command "([^"]*)"$"#)]
fn run_image_command(world: &mut E2eWorld, image: String, cmd_line: String) {
    run_microvm(world, vec![image], &cmd_line);
}

#[when(regex = r#"^the user runs a microVM command "([^"]*)" with (\d+) vCPUs$"#)]
fn run_command_with_cpus(world: &mut E2eWorld, cmd_line: String, cpus: String) {
    run_microvm(world, vec!["--cpus".into(), cpus], &cmd_line);
}

#[when(regex = r#"^the user runs a microVM command "([^"]*)" with workdir "([^"]+)"$"#)]
fn run_command_with_workdir(world: &mut E2eWorld, cmd_line: String, dir: String) {
    run_microvm(world, vec!["-w".into(), dir], &cmd_line);
}

#[when(regex = r#"^the user starts a detached microVM command "([^"]*)"$"#)]
fn start_detached(world: &mut E2eWorld, cmd_line: String) {
    run_microvm(world, vec!["-d".into()], &cmd_line);
    if let Some(id) = world.last_run_id.clone() {
        world.detached_runs.push(id);
    }
}

#[when(regex = r#"^the user starts a detached microVM command "([^"]*)" publishing port (\d+)$"#)]
fn start_detached_publishing(world: &mut E2eWorld, cmd_line: String, port: u16) {
    run_microvm(
        world,
        vec!["-d".into(), "-p".into(), format!("{port}:{port}")],
        &cmd_line,
    );
    if let Some(id) = world.last_run_id.clone() {
        world.detached_runs.push(id);
    }
}

fn fetch_published(port: u16) -> Result<String, String> {
    use std::io::{Read, Write};
    let addr = format!("127.0.0.1:{port}");
    let deadline = Instant::now() + Duration::from_secs(90);
    let mut last;
    loop {
        match std::net::TcpStream::connect(&addr) {
            Ok(mut stream) => {
                stream.set_read_timeout(Some(Duration::from_secs(3))).ok();
                let _ =
                    stream.write_all(format!("GET / HTTP/1.0\r\nHost: {addr}\r\n\r\n").as_bytes());
                let mut buf = String::new();
                let _ = stream.read_to_string(&mut buf);
                if !buf.is_empty() {
                    return Ok(buf);
                }
                last = "connected but the response was empty".to_string();
            }
            Err(e) => last = format!("connect: {e}"),
        }
        if Instant::now() >= deadline {
            return Err(format!("port {port} never returned data ({last})"));
        }
        std::thread::sleep(Duration::from_millis(400));
    }
}

#[then(regex = r#"^the host can fetch "([^"]*)" from port (\d+)$"#)]
fn host_can_fetch(_world: &mut E2eWorld, needle: String, port: u16) -> Result<(), String> {
    let body = fetch_published(port)?;
    if body.contains(&needle) {
        Ok(())
    } else {
        Err(format!(
            "response from port {port} missing {needle:?}: {body:?}"
        ))
    }
}

#[then(regex = r#"^the host cannot connect to port (\d+)$"#)]
fn host_cannot_connect(_world: &mut E2eWorld, port: u16) -> Result<(), String> {
    let addr: std::net::SocketAddr = format!("127.0.0.1:{port}")
        .parse()
        .map_err(|e| format!("{e}"))?;
    match std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(2)) {
        Ok(_) => Err(format!("port {port} was reachable but should not be")),
        Err(_) => Ok(()),
    }
}

#[when("the user stops that run")]
fn stop_that_run(world: &mut E2eWorld) -> Result<(), String> {
    let id = last_run(world)?;
    world.result = Some(world.run_with_service_env(&["sandbox", "stop", &id]));
    Ok(())
}

#[when("the user kills that run")]
fn kill_that_run(world: &mut E2eWorld) -> Result<(), String> {
    let id = last_run(world)?;
    world.result = Some(world.run_with_service_env(&["sandbox", "kill", &id]));
    Ok(())
}

#[when("the user inspects that run")]
fn inspect_that_run(world: &mut E2eWorld) -> Result<(), String> {
    let id = last_run(world)?;
    world.result = Some(world.run_with_service_env(&["sandbox", "inspect", &id]));
    Ok(())
}

#[given("a network policy that denies all egress")]
fn policy_deny_all(world: &mut E2eWorld) {
    write_policy(
        world,
        "network:\n  defaultVerdict: deny\n  defaultTransport: direct\n",
    );
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
