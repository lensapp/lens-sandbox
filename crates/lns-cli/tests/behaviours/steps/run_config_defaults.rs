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
            lns_cli::run::summary::resolved_size(Default::default(), &resolved),
            &Policy::default(),
            Path::new("./lns-local-mixin.yaml"),
            &PolicySource::Found,
        ),
        mixins: resolved.mixins.clone(),
        ..Default::default()
    });
}

#[then(regex = r#"^the run carries the mixin "([^"]+)"$"#)]
fn run_carries_the_mixin(world: &mut BehaviourWorld, expected: String) -> Result<(), String> {
    let view = world
        .resolved_run
        .as_ref()
        .ok_or("no resolved run captured")?;
    if view.mixins == [expected.clone()] {
        Ok(())
    } else {
        Err(format!(
            "expected the run to carry the mixin {expected:?}, got {:?}",
            view.mixins
        ))
    }
}

#[when(regex = r"^the local run summary is composed against the configured defaults$")]
fn compose_local_summary_against_defaults(world: &mut BehaviourWorld) {
    compose_declared_summary_against_defaults(world, String::new());
}

#[when(
    regex = r#"^the local run summary is composed against the configured defaults with "([^"]+)"$"#
)]
fn compose_local_summary_against_defaults_with(world: &mut BehaviourWorld, flags: String) {
    compose_declared_summary_against_defaults(world, flags);
}

fn compose_declared_summary_against_defaults(world: &mut BehaviourWorld, flags: String) {
    let path = config_path(world);
    let declared = lns_cli::run::declarative::Defaults::from_definition(
        &crate::steps::declarative_run::definition(world),
        Some(crate::world::TEST_HOST),
    );
    let mut argv = vec!["lns".to_string(), "run".to_string()];
    argv.extend(flags.split_whitespace().map(str::to_string));
    argv.push("alpine".to_string());
    let args: RunArgs = parse_args(&argv).expect("argv must parse against the CLI grammar");
    let defaults = config::load_run_defaults(&path).expect("gap-filler defaults load");
    let resolved = config::apply_run_defaults(args, defaults);
    world.resolved_run = Some(ResolvedRunView {
        summary: format_summary(
            &resolved,
            lns_cli::run::summary::resolved_size(declared.size, &resolved),
            &Policy::default(),
            Path::new("./lns-local-mixin.yaml"),
            &PolicySource::Found,
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
