use crate::world::{BehaviourWorld, ResolvedRunView};
use cucumber::{then, when};
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

#[when(regex = r"^the user resolves `lns run ([^`]+)` against the configured defaults$")]
fn resolve_run_against_defaults(world: &mut BehaviourWorld, image_and_flags: String) {
    let path = config_path(world);
    let mut argv = vec!["lns".to_string(), "run".to_string()];
    argv.extend(image_and_flags.split_whitespace().map(str::to_string));
    let args: RunArgs = parse_args(&argv).expect("argv must parse against the CLI grammar");
    let defaults = config::load_run_defaults(&path).expect("gap-filler defaults load");
    let resolved = config::apply_run_defaults(args, defaults);
    world.resolved_run = Some(ResolvedRunView {
        summary: format_summary(
            &resolved,
            &Policy::default(),
            Path::new("./lns-policy.yaml"),
            &PolicySource::FoundInCwd,
        ),
        ..Default::default()
    });
}

#[then(regex = r#"^the run summary shows "([^"]+)"$"#)]
fn run_summary_shows(world: &mut BehaviourWorld, needle: String) -> Result<(), String> {
    let view = world
        .resolved_run
        .as_ref()
        .ok_or("no resolved run captured")?;
    if view.summary.contains(&needle) {
        Ok(())
    } else {
        Err(format!(
            "expected summary to contain {needle:?}, got:\n{}",
            view.summary
        ))
    }
}
