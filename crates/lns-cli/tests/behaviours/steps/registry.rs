use crate::runner::CliRun;
use crate::world::BehaviourWorld;
use cucumber::{given, then, when};
use lns_cli::cli::{PullArgs, PushArgs};
use lns_cli::registry;
use std::path::PathBuf;

fn cwd(world: &mut BehaviourWorld) -> PathBuf {
    if world.cwd.is_none() {
        world.cwd = Some(tempfile::TempDir::new().expect("create tempdir"));
    }
    world.cwd.as_ref().unwrap().path().to_path_buf()
}

fn sha256_token(output: &str) -> Option<String> {
    output
        .split_whitespace()
        .find(|t| t.starts_with("sha256:"))
        .map(str::to_string)
}

async fn run_push(world: &mut BehaviourWorld, args: PushArgs) {
    let dir = cwd(world);
    let client = world.registry.clone();
    let mut buf = Vec::new();
    let run = match registry::push(&args, &dir, &client, &mut buf).await {
        Ok(exit_code) => CliRun {
            exit_code,
            output: String::from_utf8_lossy(&buf).into_owned(),
        },
        Err(e) => CliRun {
            exit_code: 1,
            output: format!("{e:#}"),
        },
    };
    world.push_digest = sha256_token(&run.output);
    world.result = Some(run);
}

async fn run_pull(world: &mut BehaviourWorld, args: PullArgs) {
    let client = world.registry.clone();
    let mut buf = Vec::new();
    let run = match registry::pull(&args, &client, &mut buf).await {
        Ok(exit_code) => CliRun {
            exit_code,
            output: String::from_utf8_lossy(&buf).into_owned(),
        },
        Err(e) => CliRun {
            exit_code: 1,
            output: format!("{e:#}"),
        },
    };
    world.pull_digest = sha256_token(&run.output);
    world.result = Some(run);
}

#[given(regex = r#"^a policy file "([^"]+)"$"#)]
fn a_policy_file(world: &mut BehaviourWorld, file: String) {
    let dir = cwd(world);
    std::fs::write(dir.join(file), "network:\n  defaultVerdict: ask\n").expect("write policy file");
}

#[given(regex = r#"^an agent file "([^"]+)"$"#)]
fn an_agent_file(world: &mut BehaviourWorld, file: String) {
    let dir = cwd(world);
    let agent = "apiVersion: lens.dev/v1alpha1\nkind: Agent\nmetadata:\n  name: hermes\nspec:\n  image: alpine:3.20\n";
    std::fs::write(dir.join(file), agent).expect("write agent file");
}

#[when(regex = r#"^the developer pushes "([^"]+)" to "([^"]+)"$"#)]
async fn push_inferred(world: &mut BehaviourWorld, source: String, reference: String) {
    run_push(
        world,
        PushArgs {
            source,
            reference,
            family: None,
            content: None,
        },
    )
    .await;
}

#[when(regex = r#"^the developer pushes "([^"]+)" to "([^"]+)" with family "([^"]+)"$"#)]
async fn push_with_family(
    world: &mut BehaviourWorld,
    source: String,
    reference: String,
    family: String,
) {
    run_push(
        world,
        PushArgs {
            source,
            reference,
            family: Some(family),
            content: None,
        },
    )
    .await;
}

#[when(regex = r#"^the developer pulls "([^"]+)" to "([^"]+)"$"#)]
async fn pull_to(world: &mut BehaviourWorld, reference: String, file: String) {
    let out = cwd(world).join(file);
    run_pull(
        world,
        PullArgs {
            reference,
            output: Some(out),
        },
    )
    .await;
}

#[then(regex = r"^the push reports a sha256 digest$")]
fn push_reports_digest(world: &mut BehaviourWorld) -> Result<(), String> {
    match &world.push_digest {
        Some(d) if d.starts_with("sha256:") => Ok(()),
        other => Err(format!("expected a sha256 digest, got {other:?}")),
    }
}

#[then(regex = r"^the pull reports the same digest as the push$")]
fn pull_matches_push(world: &mut BehaviourWorld) -> Result<(), String> {
    match (&world.push_digest, &world.pull_digest) {
        (Some(p), Some(q)) if p == q => Ok(()),
        (p, q) => Err(format!("push digest {p:?} != pull digest {q:?}")),
    }
}

#[then(regex = r#"^the file "([^"]+)" contains "([^"]+)"$"#)]
fn file_contains(world: &mut BehaviourWorld, file: String, needle: String) -> Result<(), String> {
    let dir = cwd(world);
    let body = std::fs::read_to_string(dir.join(&file)).map_err(|e| e.to_string())?;
    if body.contains(&needle) {
        Ok(())
    } else {
        Err(format!("{file} does not contain {needle:?}: {body}"))
    }
}
