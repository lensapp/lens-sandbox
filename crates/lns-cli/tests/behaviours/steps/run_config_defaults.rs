use crate::runner::CliRun;
use crate::world::{BehaviourWorld, ResolvedRunView};
use cucumber::{given, then, when};
use lns_cli::cli::RunArgs;
use lns_cli::command::parse_args;
use lns_cli::config;
use lns_cli::run::summary::{PolicySource, format_summary};
use lns_policy::Policy;
use std::path::Path;

fn config_path(world: &mut BehaviourWorld) -> std::path::PathBuf {
    if world.cwd.is_none() {
        world.cwd = Some(tempfile::TempDir::new().expect("create tempdir"));
    }
    world.cwd.as_ref().unwrap().path().join("config.yaml")
}

#[given(regex = r#"^the config file declares a malformed "run\.env" entry "([^"]+)"$"#)]
fn config_declares_malformed_env(world: &mut BehaviourWorld, entry: String) {
    let path = config_path(world);
    std::fs::write(&path, format!("run:\n  env:\n    - {entry}\n")).expect("seed config");
}

#[when(regex = r"^the user resolves `lns run ([^`]+)` against the configured defaults$")]
fn resolve_run_against_defaults(world: &mut BehaviourWorld, image_and_flags: String) {
    let path = config_path(world);
    let mut argv = vec!["lns".to_string(), "run".to_string()];
    argv.extend(image_and_flags.split_whitespace().map(str::to_string));
    let args: RunArgs = parse_args(&argv).expect("argv must parse against the CLI grammar");
    let defaults = match config::load_run_defaults(&path) {
        Ok(d) => d,
        Err(e) => {
            world.result = Some(CliRun {
                exit_code: 1,
                output: format!("{e:#}"),
            });
            return;
        }
    };
    let resolved = config::apply_run_defaults(args, defaults);
    world.resolved_run = Some(ResolvedRunView {
        summary: format_summary(
            &resolved,
            &Policy::default(),
            Path::new("./lns-policy.yaml"),
            &PolicySource::FoundInCwd,
        ),
        env: resolved.env.clone(),
        volumes: resolved
            .volumes
            .iter()
            .map(|v| {
                let ro = if v.read_only { ":ro" } else { "" };
                format!("{}:{}{ro}", v.name, v.target)
            })
            .collect(),
        publish: resolved
            .publish
            .iter()
            .map(|p| format!("{}:{}->{}", p.host_ip, p.host_port, p.container_port))
            .collect(),
    });
}

fn resolved(world: &BehaviourWorld) -> Result<&ResolvedRunView, String> {
    world
        .resolved_run
        .as_ref()
        .ok_or_else(|| "no resolved run captured".to_string())
}

#[then(regex = r#"^the run summary shows "([^"]+)"$"#)]
fn run_summary_shows(world: &mut BehaviourWorld, needle: String) -> Result<(), String> {
    let view = resolved(world)?;
    if view.summary.contains(&needle) {
        Ok(())
    } else {
        Err(format!(
            "expected summary to contain {needle:?}, got:\n{}",
            view.summary
        ))
    }
}

#[then(regex = r#"^the resolved env is exactly "([^"]+)"$"#)]
fn resolved_env_is(world: &mut BehaviourWorld, expected: String) -> Result<(), String> {
    expect_exactly("env", &resolved(world)?.env, &expected)
}

#[then(regex = r#"^the resolved volumes are exactly "([^"]+)"$"#)]
fn resolved_volumes_are(world: &mut BehaviourWorld, expected: String) -> Result<(), String> {
    expect_exactly("volumes", &resolved(world)?.volumes, &expected)
}

#[then(regex = r#"^the resolved ports are exactly "([^"]+)"$"#)]
fn resolved_ports_are(world: &mut BehaviourWorld, expected: String) -> Result<(), String> {
    expect_exactly("ports", &resolved(world)?.publish, &expected)
}

fn expect_exactly(what: &str, actual: &[String], expected: &str) -> Result<(), String> {
    let rendered = actual.join(", ");
    if rendered == expected {
        Ok(())
    } else {
        Err(format!("expected {what} {expected:?}, got {rendered:?}"))
    }
}

#[then(regex = r#"^the resolution fails mentioning "([^"]+)" and the config file$"#)]
fn resolution_fails_mentioning(world: &mut BehaviourWorld, needle: String) -> Result<(), String> {
    let res = world.result.as_ref().ok_or("no failure captured")?;
    if res.exit_code != 0 && res.output.contains(&needle) && res.output.contains("config.yaml") {
        Ok(())
    } else {
        Err(format!(
            "expected a failure naming {needle:?} and config.yaml, got code {} (output: {:?})",
            res.exit_code, res.output
        ))
    }
}
