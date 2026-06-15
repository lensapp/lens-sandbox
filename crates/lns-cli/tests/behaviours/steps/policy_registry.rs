use crate::runner::CliRun;
use crate::world::BehaviourWorld;
use cucumber::{then, when};
use lns_cli::cli::{PolicyCommand, PolicyPullArgs, PolicyPushArgs};
use lns_cli::policy;
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

async fn run(world: &mut BehaviourWorld, cmd: PolicyCommand) -> CliRun {
    let dir = cwd(world);
    let registry = world.policy_registry.clone();
    let mut buf = Vec::new();
    match policy::run(&cmd, &dir, &registry, &mut buf).await {
        Ok(exit_code) => CliRun {
            exit_code,
            output: String::from_utf8_lossy(&buf).into_owned(),
        },
        Err(e) => CliRun {
            exit_code: 1,
            output: format!("{e:#}"),
        },
    }
}

#[when(regex = r#"^the developer pushes "([^"]+)" to "([^"]+)"$"#)]
async fn push(world: &mut BehaviourWorld, file: String, reference: String) {
    let run = run(
        world,
        PolicyCommand::Push(PolicyPushArgs {
            file: PathBuf::from(file),
            reference,
        }),
    )
    .await;
    world.push_digest = sha256_token(&run.output);
    world.result = Some(run);
}

#[when(regex = r#"^the developer pulls "([^"]+)" to "([^"]+)"$"#)]
async fn pull(world: &mut BehaviourWorld, reference: String, file: String) {
    let out_path = cwd(world).join(&file);
    let run = run(
        world,
        PolicyCommand::Pull(PolicyPullArgs {
            reference,
            output: Some(out_path),
        }),
    )
    .await;
    world.pull_digest = sha256_token(&run.output);
    world.result = Some(run);
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
