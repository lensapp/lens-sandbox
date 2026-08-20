use std::path::Path;

use cucumber::{given, then, when};
use lns_cli::cli::RunArgs;
use lns_cli::command::parse_args;
use lns_cli::run::declarative::{Defaults, compose_ports};
use lns_cli::run::summary::print_run_summary;

use crate::world::{BehaviourWorld, TEST_HOST};

const TEST_LOOPBACK: &str = "127.0.0.1";

fn ports_yaml(ports: &str) -> String {
    let entries: String = ports
        .split(" and ")
        .map(|spec| match spec.split_once(':') {
            Some((host, container)) => {
                format!("    - host: {host}\n      container: {container}\n")
            }
            None => format!("    - container: {spec}\n"),
        })
        .collect();
    format!(
        "apiVersion: lns.run/v1\nkind: sandbox\nname: some-sandbox\nspec:\n  image: example.test/runtime:1\n  ports:\n{entries}"
    )
}

fn definition(world: &BehaviourWorld) -> lns_artifact::sandbox::Definition {
    let yaml = world
        .author_files
        .get(Path::new("/work/lns.yaml"))
        .expect("the scenario must install lns.yaml");
    let value: serde_json::Value = serde_yaml::from_str(yaml).expect("valid fixture yaml");
    let json = serde_json::to_vec(&value).expect("serializable fixture yaml");
    lns_artifact::sandbox::parse(&json).expect("valid sandbox fixture")
}

fn published_view(def: &lns_artifact::sandbox::Definition) -> lns_ipc::SandboxView {
    lns_ipc::SandboxView {
        mixins: def.spec.mixins.clone(),
        pinned_mixins: Vec::new(),
        contributions: Vec::new(),
        reference: "registry.example.test/team/sandbox:1".into(),
        digest: format!("sha256:{}", "a".repeat(64)),
        image: def.spec.image.clone(),
        workdir: def.spec.workdir.clone(),
        user: None,
        mounts: Vec::new(),
        ports: def
            .spec
            .ports
            .iter()
            .map(|port| lns_ipc::SandboxPort {
                host: port
                    .host
                    .map(|host| u16::try_from(host).expect("fixture host port")),
                container: u16::try_from(port.container).expect("fixture container port"),
            })
            .collect(),
        filesets: Vec::new(),
        connectors: Vec::new(),
        env: Vec::new(),
        credentials: Vec::new(),
        tools: Vec::new(),
        scripts: Vec::new(),
        policy_flags: Vec::new(),
        cpus: None,
        mem_mib: None,
        disk_bytes: None,
    }
}

fn summarize(world: &mut BehaviourWorld, defaults: &Defaults, flags: &str, local: bool) {
    let mut argv = vec!["lns".to_string(), "run".to_string()];
    argv.extend(flags.split_whitespace().map(str::to_string));
    if !local {
        argv.push("registry.example.test/team/sandbox:1".to_string());
    }
    let mut args: RunArgs = parse_args(&argv).expect("port flags must parse");
    let composed = compose_ports(
        &defaults.ports,
        std::mem::take(&mut args.publish),
        local || args.publish_declared,
    )
    .expect("declared ports must compose");
    args.publish = composed.published;
    args.declared_unpublished = composed.declared_unpublished;
    if world.cwd.is_none() {
        world.cwd = Some(tempfile::TempDir::new().expect("create tempdir"));
    }
    let cwd = world.cwd.as_ref().expect("cwd").path().to_path_buf();
    let mut buf = Vec::<u8>::new();
    print_run_summary(
        &args,
        lns_cli::run::summary::resolved_size(Default::default(), &args),
        lns_cli::run::summary::DecisionsSite::one_directory(&cwd),
        &mut buf,
    )
    .expect("print_run_summary");
    world.summary_output = String::from_utf8(buf).expect("non-utf8 summary output");
}

#[given(regex = r"^an lns\.yaml declaring ports? (.+)$")]
fn lns_yaml_declaring_ports(world: &mut BehaviourWorld, ports: String) {
    world
        .author_files
        .insert("/work/lns.yaml".into(), ports_yaml(&ports));
}

#[given(regex = r"^a published sandbox declaring ports? (.+)$")]
fn published_sandbox_declaring_ports(world: &mut BehaviourWorld, ports: String) {
    world
        .author_files
        .insert("/work/lns.yaml".into(), ports_yaml(&ports));
}

#[when("the local sandbox is run with no port flags")]
fn local_run_no_flags(world: &mut BehaviourWorld) {
    let defaults = Defaults::from_definition(&definition(world), Some(TEST_HOST));
    summarize(world, &defaults, "", true);
}

#[when(regex = r"^the local sandbox is run with `([^`]+)`$")]
fn local_run_with_flags(world: &mut BehaviourWorld, flags: String) {
    let defaults = Defaults::from_definition(&definition(world), Some(TEST_HOST));
    summarize(world, &defaults, &flags, true);
}

#[when("the sandbox reference is run with no port flags")]
fn published_run_no_flags(world: &mut BehaviourWorld) {
    let defaults = Defaults::from_view(&published_view(&definition(world)));
    summarize(world, &defaults, "", false);
}

#[when(regex = r"^the sandbox reference is run with `([^`]+)`$")]
fn published_run_with_flags(world: &mut BehaviourWorld, flags: String) {
    let defaults = Defaults::from_view(&published_view(&definition(world)));
    summarize(world, &defaults, &flags, false);
}

#[then(regex = r"^the summary notes port (\d+) is declared but not published$")]
fn summary_notes_declared_unpublished(
    world: &mut BehaviourWorld,
    port: String,
) -> Result<(), String> {
    let has_port = world.summary_output.contains(&format!("Declared:  {port}"));
    let has_note = world
        .summary_output
        .contains("not published; opt in with -P");
    if has_port && has_note {
        Ok(())
    } else {
        Err(format!(
            "expected a declared-but-unpublished note for port {port} in:\n{}",
            world.summary_output
        ))
    }
}

fn compose(world: &mut BehaviourWorld, flags: &str) {
    let defaults = Defaults::from_definition(&definition(world), Some(TEST_HOST));
    let mut argv = vec!["lns".to_string(), "run".to_string()];
    argv.extend(flags.split_whitespace().map(str::to_string));
    let mut args: RunArgs = parse_args(&argv).expect("port flags must parse");
    match compose_ports(&defaults.ports, std::mem::take(&mut args.publish), true) {
        Ok(composed) => world.composed_ports = composed.published,
        Err(e) => world.port_composition_error = Some(format!("{e:#}")),
    }
}

#[when(regex = r"^the local sandbox is run with `([^`]+)` and the ports are composed$")]
fn compose_with_flags(world: &mut BehaviourWorld, flags: String) {
    compose(world, &flags);
}

#[then(regex = r"^composing the ports is refused, naming host port (\d+)$")]
fn composition_refused(world: &mut BehaviourWorld, port: String) -> Result<(), String> {
    let binding = format!("{TEST_LOOPBACK}:{port} is asked to forward");
    match &world.port_composition_error {
        Some(message) if message.contains(&binding) => Ok(()),
        Some(message) => Err(format!(
            "the refusal must name the {TEST_LOOPBACK}:{port} binding: {message}"
        )),
        None => Err(format!(
            "two mappings on host port {port} must be refused before boot"
        )),
    }
}

#[then(regex = r"^`([\d.]+):(\d+) -> (\d+)` is published exactly once$")]
fn published_exactly_once(
    world: &mut BehaviourWorld,
    ip: String,
    host: u16,
    container: u16,
) -> Result<(), String> {
    let matching = world
        .composed_ports
        .iter()
        .filter(|port| {
            port.host_ip.to_string() == ip
                && port.host_port == host
                && port.container_port == container
        })
        .count();
    if matching == 1 {
        Ok(())
    } else {
        Err(format!(
            "{ip}:{host} -> {container} must be published once, not {matching} times: {:?}",
            world.composed_ports
        ))
    }
}
