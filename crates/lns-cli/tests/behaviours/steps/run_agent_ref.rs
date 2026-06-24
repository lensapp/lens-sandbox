use crate::world::BehaviourWorld;
use clap::Parser;
use cucumber::{given, then, when};
use lns_cli::cli::{Cli, Command};
use lns_cli::registry::RegistryClient;
use lns_cli::run::resolve::resolve_into_run_args;
use lns_policy::artifact::Family;

async fn store_agent(world: &mut BehaviourWorld, reference: &str, yaml: &str) {
    let blob = lns_policy::artifact::to_config_blob(yaml.as_bytes()).expect("agent yaml");
    world
        .registry
        .push_artifact(
            reference,
            &Family::Agent.artifact_type(),
            &Family::Agent.config_media_type(),
            &blob,
            &[],
        )
        .await
        .expect("store agent artifact");
}

#[given(regex = r#"^an agent artifact "([^"]+)" with image "([^"]+)" and command "([^"]+)"$"#)]
async fn given_agent_with_command(
    world: &mut BehaviourWorld,
    reference: String,
    image: String,
    command: String,
) {
    let yaml = format!(
        "apiVersion: lens.dev/v1alpha1\nkind: Agent\n\
         metadata:\n  name: some-agent\n\
         spec:\n  image: {image}\n  command: '{command}'\n"
    );
    store_agent(world, &reference, &yaml).await;
}

#[given(regex = r#"^an agent artifact "([^"]+)" needing credential "([^"]+)"$"#)]
async fn given_agent_needs_credential(
    world: &mut BehaviourWorld,
    reference: String,
    credential: String,
) {
    let yaml = format!(
        "apiVersion: lens.dev/v1alpha1\nkind: Agent\n\
         metadata:\n  name: some-agent\n\
         spec:\n  image: some-image:1\n  \
         credentials:\n    - {{ name: {credential}, env: SOME_TOKEN }}\n"
    );
    store_agent(world, &reference, &yaml).await;
}

#[when(regex = r#"^the developer launches "([^"]+)"$"#)]
async fn when_developer_runs(world: &mut BehaviourWorld, reference: String) {
    let cli = Cli::try_parse_from(["lns", "run", &reference]).expect("parse run argv");
    let Command::Run(args) = cli.command else {
        panic!("expected a run command");
    };
    let available = world.available_creds.clone();
    let mut buf = Vec::new();
    match resolve_into_run_args(args, &world.registry, &available, &mut buf).await {
        Ok((args, guard)) => {
            world.resolved_image = args.image;
            world.resolved_cmd = args.cmd;
            world.resolved_policy = args.policy;
            world.resolve_guard = guard;
        }
        Err(e) => world.resolve_error = Some(format!("{e:#}")),
    }
    world.resolve_writer = String::from_utf8_lossy(&buf).into_owned();
}

#[then(regex = r#"^the resolved image is "([^"]+)"$"#)]
fn then_resolved_image(world: &mut BehaviourWorld, expected: String) -> Result<(), String> {
    match &world.resolved_image {
        Some(image) if *image == expected => Ok(()),
        other => Err(format!(
            "expected resolved image {expected:?}, got {other:?}"
        )),
    }
}

#[then(regex = r#"^the resolved command is "([^"]+)"$"#)]
fn then_resolved_command(world: &mut BehaviourWorld, expected: String) -> Result<(), String> {
    let actual = world.resolved_cmd.join(" ");
    if actual == expected {
        Ok(())
    } else {
        Err(format!("expected command {expected:?}, got {actual:?}"))
    }
}

#[then(regex = r#"^a credential warning names "([^"]+)"$"#)]
fn then_credential_warning(world: &mut BehaviourWorld, name: String) -> Result<(), String> {
    let w = &world.resolve_writer;
    if w.contains(&name) && w.contains("lns connect") {
        Ok(())
    } else {
        Err(format!(
            "expected a connect warning naming {name:?}, got: {w:?}"
        ))
    }
}

#[then("the run is refused because the reference is not runnable")]
fn then_refused(world: &mut BehaviourWorld) -> Result<(), String> {
    match &world.resolve_error {
        Some(e) if e.contains("cannot be run directly") => Ok(()),
        other => Err(format!("expected a not-runnable refusal, got {other:?}")),
    }
}
