use crate::E2eWorld;
use crate::specutil::{arg_parser::split_args, run_cli_with_timeout_in_dir};
use cucumber::{given, then, when};
use std::time::{Duration, Instant};

const MICROVM_RUN_TIMEOUT: Duration = Duration::from_secs(120);

// `lns run` takes a published sandbox or ./lns.yaml (imageless mode and plain-image REFs are retired), so every microVM scenario boots a sandbox over this base.
// The ECR mirror of Docker Hub: anonymous pulls there hit hard rate limits when the suite boots ~45 guests.
const MICROVM_IMAGE: &str = "public.ecr.aws/docker/library/alpine:3.20";

static PINNED_MICROVM_IMAGE: std::sync::OnceLock<String> = std::sync::OnceLock::new();

fn linux_host_arch_resolver(entries: &[oci_client::manifest::ImageIndexEntry]) -> Option<String> {
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "amd64",
        other => other,
    };
    entries
        .iter()
        .find(|e| {
            !e.media_type.contains("attestation")
                && e.platform.as_ref().is_some_and(|p| {
                    p.os.to_string() == "linux" && p.architecture.to_string() == arch
                })
        })
        .map(|e| e.digest.clone())
}

fn image_pin_cache_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/e2e-microvm-image-pin")
}

fn cached_image_pin() -> Option<String> {
    let contents = std::fs::read_to_string(image_pin_cache_path()).ok()?;
    let (tag, pinned) = contents.trim().split_once(' ')?;
    (tag == MICROVM_IMAGE && pinned.contains("@sha256:")).then(|| pinned.to_string())
}

// Resolve the base tag to a digest-pinned reference once per checkout (a digest is immutable, and the pin file keeps repeated suite invocations from tripping ECR's rate limit): pinned refs hit the daemon's manifest cache, so ~45 booted guests cost one registry round-trip.
fn pinned_microvm_image() -> String {
    PINNED_MICROVM_IMAGE
        .get_or_init(|| {
            if let Some(pin) = cached_image_pin() {
                return pin;
            }
            let pinned = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(async {
                    let reference: oci_client::Reference =
                        MICROVM_IMAGE.parse().expect("base ref parses");
                    // Mirror the daemon's linux/host-arch index resolution so the pin lands on the platform manifest digest the daemon verifies against.
                    let client = oci_client::Client::new(oci_client::client::ClientConfig {
                        platform_resolver: Some(Box::new(linux_host_arch_resolver)),
                        ..Default::default()
                    });
                    let (_, digest) = client
                        .pull_image_manifest(
                            &reference,
                            &oci_client::secrets::RegistryAuth::Anonymous,
                        )
                        .await
                        .expect("resolve the base image digest");
                    format!(
                        "{}/{}@{digest}",
                        reference.registry(),
                        reference.repository()
                    )
                })
            });
            let _ = std::fs::write(
                image_pin_cache_path(),
                format!("{MICROVM_IMAGE} {pinned}\n"),
            );
            pinned
        })
        .clone()
}

fn microvm_project(world: &mut E2eWorld) -> std::path::PathBuf {
    let dir = world
        .project
        .get_or_insert_with(|| tempfile::TempDir::new().expect("project tempdir"));
    let root = dir.path().to_path_buf();
    let mut spec_tail = String::new();
    if let Some(command) = &world.project_command {
        spec_tail.push_str(&format!("\n  command: {command}"));
    }
    if !world.project_env.is_empty() {
        spec_tail.push_str("\n  env:");
        for (key, value) in &world.project_env {
            spec_tail.push_str(&format!("\n    {key}: {value}"));
        }
    }
    if !world.project_integrations.is_empty() {
        spec_tail.push_str("\n  integrations:");
        for id in &world.project_integrations {
            spec_tail.push_str(&format!("\n    - {id}"));
        }
    }
    if !world.project_credentials.is_empty() {
        spec_tail.push_str("\n  credentials:");
        for (name, env, required) in &world.project_credentials {
            spec_tail.push_str(&format!(
                "\n    - name: {name}\n      env: {env}\n      required: {required}"
            ));
        }
    }
    let definition = format!(
        "apiVersion: lns.run/v1\nkind: Sandbox\nmetadata:\n  name: e2e-microvm\nspec:\n  image: {}{spec_tail}\n",
        pinned_microvm_image()
    );
    std::fs::write(root.join("lns.yaml"), definition).expect("write project lns.yaml");
    std::fs::write(
        root.join("lns-policy.yaml"),
        "network:\n  allowedRoutes: []\n  defaultVerdict: ask\n  defaultTransport: direct\n",
    )
    .expect("write the project policy");
    root
}

fn socket_env(world: &E2eWorld) -> Vec<(&'static str, std::ffi::OsString)> {
    world
        .service_socket
        .as_ref()
        .map(|socket| vec![("LNS_SOCKET_PATH", socket.clone().into())])
        .unwrap_or_default()
}

// Anchored to the CLI's "✓ started run <id>" status line: the workload transcript precedes it and legitimately contains phrases like "run as root".
fn parse_run_id(text: &str) -> Option<String> {
    let marker = text.find("started run ")?;
    let id: String = text[marker + "started run ".len()..]
        .chars()
        .take_while(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        .collect();
    (!id.is_empty()).then_some(id)
}

fn run_microvm(world: &mut E2eWorld, run_args: Vec<String>, cmd_line: &str) {
    run_microvm_of(world, run_args, cmd_line);
}

fn run_microvm_of(world: &mut E2eWorld, mut run_args: Vec<String>, cmd_line: &str) {
    run_args.push("--".to_string());
    run_args.extend(split_args(cmd_line));
    run_lns_microvm(world, run_args);
}

fn run_lns_microvm(world: &mut E2eWorld, tail: Vec<String>) {
    let project = microvm_project(world);
    let mut args = vec!["run".to_string()];
    if let Some(policy) = &world.policy_path {
        args.push("--policy".to_string());
        args.push(policy.to_string_lossy().into_owned());
    }
    args.extend(tail);
    let result =
        run_cli_with_timeout_in_dir(&project, args, socket_env(world), MICROVM_RUN_TIMEOUT);
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

#[given(regex = r#"^the home's integration catalog declares "([^"]+)" managing "([^"]+)"$"#)]
fn home_catalog_declares(world: &mut E2eWorld, id: String, env: String) {
    let home = world
        .home
        .as_ref()
        .expect("Given a clean lns cache home before writing a catalog");
    let catalog = format!(
        "integrations:\n  - id: {id}\n    authKind: credential\n    routes:\n      - match: api.{id}.example\n    credential:\n      envVar: {env}\n      placeholder: {id}-LNSPLACEHOLDER0000000000\n      injections:\n        - kind: bearer_header\n          domain: api.{id}.example\n"
    );
    std::fs::write(home.path().join(".lns-integrations.yaml"), catalog)
        .expect("write the user integration catalog");
}

#[given(regex = r#"^the project definition declares integration "([^"]+)"$"#)]
fn project_declares_integration(world: &mut E2eWorld, id: String) {
    world.project_integrations.push(id);
}

#[given(regex = r#"^the project definition requires credential "([^"]+)" injected as "([^"]+)"$"#)]
fn project_requires_credential(world: &mut E2eWorld, id: String, env: String) {
    world.project_credentials.push((id, env, true));
}

#[given(regex = r#"^the project definition sets command "([^"]+)"$"#)]
fn project_sets_command(world: &mut E2eWorld, command: String) {
    world.project_command = Some(command);
}

#[given(regex = r#"^the project definition sets env "([^"]+)=([^"]*)"$"#)]
fn project_sets_env(world: &mut E2eWorld, key: String, value: String) {
    world.project_env.push((key, value));
}

#[given(
    regex = r#"^the home's integration catalog declares an oauth integration "([^"]+)" signing in at "([^"]+)"$"#
)]
fn home_catalog_declares_oauth(world: &mut E2eWorld, id: String, endpoint: String) {
    let home = world
        .home
        .as_ref()
        .expect("Given a clean lns cache home before writing a catalog");
    let catalog = format!(
        "integrations:\n  - id: {id}\n    authKind: oauth\n    oauth:\n      clientId: some-client\n      deviceAuthorizationEndpoint: {endpoint}/device\n      tokenEndpoint: {endpoint}/token\n      envVar: SOME_OAUTH_TOKEN\n      placeholder: {id}-LNSPLACEHOLDER0000000000\n"
    );
    std::fs::write(home.path().join(".lns-integrations.yaml"), catalog)
        .expect("write the user integration catalog");
}

#[when("the user runs the sandbox definition")]
fn run_definition_only(world: &mut E2eWorld) {
    run_lns_microvm(world, vec![]);
}

#[when(regex = r#"^the user runs a microVM command "([^"]*)"$"#)]
fn run_command(world: &mut E2eWorld, cmd_line: String) {
    run_microvm(world, vec![], &cmd_line);
}

#[when(regex = r#"^the user runs a microVM command "([^"]*)" with hostname "([^"]+)"$"#)]
fn run_command_with_hostname(world: &mut E2eWorld, cmd_line: String, hostname: String) {
    run_microvm(world, vec!["-h".into(), hostname], &cmd_line);
}

#[when(regex = r#"^the user runs a microVM command "([^"]*)" as user "([^"]+)"$"#)]
fn run_command_as_user(world: &mut E2eWorld, cmd_line: String, user: String) {
    run_microvm(world, vec!["-u".into(), user], &cmd_line);
}

#[when(regex = r#"^the user runs a microVM command "([^"]*)" with the -it cluster$"#)]
fn run_command_with_it_cluster(world: &mut E2eWorld, cmd_line: String) {
    run_microvm(world, vec!["-it".into()], &cmd_line);
}

#[when(regex = r#"^the user runs a microVM command "([^"]*)" with auto-remove$"#)]
fn run_command_auto_remove(world: &mut E2eWorld, cmd_line: String) {
    run_microvm(world, vec!["--rm".into()], &cmd_line);
}

#[then("that run is no longer listed")]
fn run_no_longer_listed(world: &mut E2eWorld) -> Result<(), String> {
    let id = last_run(world)?;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let listing = world.run_with_service_env(&["sandbox", "ls"]);
        let listed = listing
            .stdout
            .lines()
            .any(|line| line.split_whitespace().any(|tok| tok == id));
        if !listed {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "run {id} was still listed after 30s:\n{}",
                listing.stdout
            ));
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

#[when(regex = r#"^the user execs "([^"]*)" into that run without a separator$"#)]
fn exec_without_separator(world: &mut E2eWorld, cmd_line: String) -> Result<(), String> {
    let id = last_run(world)?;
    let mut argv = vec![
        "exec".to_string(),
        "-i=false".to_string(),
        "-t=false".to_string(),
        id,
    ];
    argv.extend(split_args(&cmd_line));
    let borrowed: Vec<&str> = argv.iter().map(String::as_str).collect();
    world.result = Some(world.run_with_service_env(&borrowed));
    Ok(())
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

#[when(regex = r#"^the user runs a microVM command "([^"]*)" with a keyed bind at "([^"]+)"$"#)]
fn run_command_with_keyed_bind(world: &mut E2eWorld, cmd_line: String, target: String) {
    let src = host_bind_source(world);
    run_microvm(
        world,
        vec![
            "--mount".into(),
            format!("type=bind,source={src},target={target}"),
        ],
        &cmd_line,
    );
}

#[when(
    regex = r#"^the user runs a microVM command "([^"]*)" with a read-only keyed bind at "([^"]+)"$"#
)]
fn run_command_with_ro_keyed_bind(world: &mut E2eWorld, cmd_line: String, target: String) {
    let src = host_bind_source(world);
    run_microvm(
        world,
        vec![
            "--mount".into(),
            format!("type=bind,source={src},target={target},readonly"),
        ],
        &cmd_line,
    );
}

#[when(
    regex = r#"^the user runs a microVM command "([^"]*)" with keyed volume "([^"]+)" at "([^"]+)"$"#
)]
fn run_command_with_keyed_volume(
    world: &mut E2eWorld,
    cmd_line: String,
    name: String,
    path: String,
) {
    track_volume(world, &name);
    run_microvm(
        world,
        vec![
            "--mount".into(),
            format!("type=volume,source={name},target={path}"),
        ],
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

static RUN_SANDBOX_SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

// `lns run` refuses a plain OCI image, so image-driven scenarios publish a minimal sandbox over it to the in-process registry and run that reference.
async fn published_sandbox_wrapping(world: &mut E2eWorld, image: &str) -> String {
    let image = if image == MICROVM_IMAGE {
        pinned_microvm_image()
    } else {
        image.to_string()
    };
    let host = world
        .registry
        .get_or_insert_with(crate::registry::LocalRegistry::start)
        .host();
    let seq = RUN_SANDBOX_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let reference: oci_client::Reference = format!("{host}/e2e-run-sandbox:{seq}")
        .parse()
        .expect("run-sandbox ref parses");
    let doc = format!(
        r#"{{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{{"name":"e2e-run-sandbox"}},"spec":{{"image":"{image}"}}}}"#
    );
    let built = lns_artifact::build::build_artifact(doc.as_bytes()).expect("build run sandbox");
    let client = oci_client::Client::new(oci_client::client::ClientConfig {
        protocol: oci_client::client::ClientProtocol::Http,
        ..Default::default()
    });
    for blob in &built.blobs {
        client
            .push_blob(&reference, blob.data.clone(), &blob.digest)
            .await
            .expect("push run-sandbox blob");
    }
    client
        .push_manifest_raw(
            &reference,
            built.manifest.clone(),
            http::HeaderValue::from_str(&built.manifest_media_type).expect("media type header"),
        )
        .await
        .expect("push run-sandbox manifest");
    reference.whole()
}

#[when(regex = r#"^the user runs image "([^"]+)" with command "([^"]*)"$"#)]
async fn run_image_command(world: &mut E2eWorld, image: String, cmd_line: String) {
    let reference = published_sandbox_wrapping(world, &image).await;
    run_microvm_of(world, vec![reference], &cmd_line);
}

#[when(regex = r#"^the user runs image "([^"]+)" with command "([^"]*)" and no separator$"#)]
async fn run_image_command_no_separator(world: &mut E2eWorld, image: String, cmd_line: String) {
    let reference = published_sandbox_wrapping(world, &image).await;
    let mut tail = vec![reference];
    tail.extend(split_args(&cmd_line));
    run_lns_microvm(world, tail);
}

#[when(
    regex = r#"^the user runs image "([^"]+)" with entrypoint "([^"]+)" and command "([^"]*)"$"#
)]
async fn run_image_with_entrypoint(
    world: &mut E2eWorld,
    image: String,
    entrypoint: String,
    cmd_line: String,
) {
    let reference = published_sandbox_wrapping(world, &image).await;
    run_microvm_of(
        world,
        vec!["--entrypoint".into(), entrypoint, reference],
        &cmd_line,
    );
}

#[when(regex = r#"^the user runs image "([^"]+)" as user "([^"]+)" with command "([^"]*)"$"#)]
async fn run_image_as_user(world: &mut E2eWorld, image: String, user: String, cmd_line: String) {
    let reference = published_sandbox_wrapping(world, &image).await;
    run_microvm_of(world, vec!["-u".into(), user, reference], &cmd_line);
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

#[when(regex = r#"^the user starts a detached microVM command "([^"]*)" with auto-remove$"#)]
fn start_detached_auto_remove(world: &mut E2eWorld, cmd_line: String) {
    run_microvm(world, vec!["-d".into(), "--rm".into()], &cmd_line);
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

#[when("the user lists running sandboxes")]
fn list_running_sandboxes(world: &mut E2eWorld) {
    world.result = Some(world.run_with_service_env(&["ps"]));
}

#[then("the output lists that run")]
fn output_lists_that_run(world: &mut E2eWorld) -> Result<(), String> {
    let id = last_run(world)?;
    let short = lns_ipc::short_run_id(&id);
    let res = world.result.as_ref().ok_or("no CLI run captured")?;
    let combined = format!("{}{}", res.stdout, res.stderr);
    if combined.contains(short) {
        Ok(())
    } else {
        Err(format!(
            "expected the listing to show run {short}, got:\n{combined}"
        ))
    }
}

#[given("a network policy holding an ask default and a direct-transport allow route")]
fn policy_ask_with_direct_route(world: &mut E2eWorld) {
    write_policy(
        world,
        "network:\n  defaultVerdict: ask\n  defaultTransport: direct\n  allowedRoutes:\n    - match: api.example.test\n      verdict: allow\n      transport: direct\n",
    );
}

#[given("a network policy that denies all egress")]
fn policy_deny_all(world: &mut E2eWorld) {
    write_policy(world, "network:\n  defaultVerdict: deny\n");
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
