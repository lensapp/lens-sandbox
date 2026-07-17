use crate::runner::CliRun;
use crate::world::BehaviourWorld;
use cucumber::{given, then, when};
use lns_cli::command::parse_args;
use lns_cli::config::{self, ConfigArgs, ConfigCommand, ConfigKey, ConfigKeyArgs, ConfigSetArgs};
use std::path::PathBuf;

fn config_path(world: &mut BehaviourWorld) -> PathBuf {
    if world.cwd.is_none() {
        world.cwd = Some(tempfile::TempDir::new().expect("create tempdir"));
    }
    world.cwd.as_ref().unwrap().path().join("config.yaml")
}

fn run_config(world: &mut BehaviourWorld, cmd: ConfigCommand) -> CliRun {
    let path = config_path(world);
    let mut buf = Vec::<u8>::new();
    match config::run(&cmd, &path, &mut buf) {
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

fn key(name: &str) -> ConfigKey {
    ConfigKey::parse(name).expect("scenario names a known config key")
}

fn set_cmd(name: &str, values: &str) -> ConfigCommand {
    ConfigCommand::Set(ConfigSetArgs {
        key: key(name),
        values: values.split_whitespace().map(str::to_string).collect(),
    })
}

fn get_cmd(name: &str) -> ConfigCommand {
    ConfigCommand::Get(ConfigKeyArgs { key: key(name) })
}

#[given(regex = r#"^the default "([^"]+)" is "([^"]+)"$"#)]
fn default_is(world: &mut BehaviourWorld, name: String, values: String) {
    let run = run_config(world, set_cmd(&name, &values));
    assert_eq!(run.exit_code, 0, "seeding {name} failed: {}", run.output);
}

#[when(regex = r#"^the developer sets the default "([^"]+)" to "([^"]+)"$"#)]
fn sets_default(world: &mut BehaviourWorld, name: String, values: String) {
    world.result = Some(run_config(world, set_cmd(&name, &values)));
}

#[given("a config file that still carries a run.env entry")]
fn config_carries_legacy_env(world: &mut BehaviourWorld) {
    let path = config_path(world);
    std::fs::write(&path, "run:\n  env:\n    - TZ=UTC\n").expect("seed legacy config");
}

#[when(regex = r#"^the user runs config command "([^"]+)"$"#)]
fn run_config_command(world: &mut BehaviourWorld, tail: String) {
    let path = config_path(world);
    let mut argv = vec!["lns".to_string(), "config".to_string()];
    argv.extend(tail.split_whitespace().map(str::to_string));
    world.result = Some(match parse_args::<ConfigArgs, _, _>(&argv) {
        Ok(args) => {
            let mut buf = Vec::<u8>::new();
            match config::run(&args.command, &path, &mut buf) {
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
        Err(e) => CliRun {
            exit_code: e.exit_code(),
            output: e.to_string(),
        },
    });
}

#[then(regex = r#"^the output warns that "([^"]+)" is no longer supported$"#)]
fn output_warns_legacy(world: &mut BehaviourWorld, key: String) -> Result<(), String> {
    let out = &world.result.as_ref().ok_or("no CLI run captured")?.output;
    if out.contains(&key) && out.contains("no longer supported") {
        Ok(())
    } else {
        Err(format!(
            "expected a legacy warning for {key:?}, got: {out:?}"
        ))
    }
}

#[then(regex = r#"^the output does not list "([^"]+)" as an active default$"#)]
fn output_no_active_default(world: &mut BehaviourWorld, key: String) -> Result<(), String> {
    let out = &world.result.as_ref().ok_or("no CLI run captured")?.output;
    let active = format!("{key} = ");
    if out.lines().any(|l| l.starts_with(&active)) {
        Err(format!(
            "{key:?} must not appear as an active default, got: {out:?}"
        ))
    } else {
        Ok(())
    }
}

#[when(regex = r#"^the developer gets the default "([^"]+)"$"#)]
fn gets_default(world: &mut BehaviourWorld, name: String) {
    world.result = Some(run_config(world, get_cmd(&name)));
}

#[when(regex = r#"^the developer unsets the default "([^"]+)"$"#)]
fn unsets_default(world: &mut BehaviourWorld, name: String) {
    let cmd = ConfigCommand::Unset(ConfigKeyArgs { key: key(&name) });
    world.result = Some(run_config(world, cmd));
}

#[when(regex = r"^the developer lists the configured defaults$")]
fn lists_defaults(world: &mut BehaviourWorld) {
    world.result = Some(run_config(world, ConfigCommand::List));
}

#[then(regex = r"^the command reports the default was set$")]
fn reports_set(world: &mut BehaviourWorld) -> Result<(), String> {
    let res = world.result.as_ref().ok_or("no CLI run captured")?;
    if res.exit_code == 0 && res.output.starts_with("Set run.") {
        Ok(())
    } else {
        Err(format!(
            "expected a `Set run.*` confirmation, got code {} (output: {:?})",
            res.exit_code, res.output
        ))
    }
}

#[then(regex = r#"^getting "([^"]+)" prints "([^"]+)"$"#)]
fn getting_prints(world: &mut BehaviourWorld, name: String, value: String) -> Result<(), String> {
    let run = run_config(world, get_cmd(&name));
    if run.exit_code == 0 && run.output.lines().any(|l| l == value) {
        Ok(())
    } else {
        Err(format!(
            "expected get {name} to print {value:?}, got code {} (output: {:?})",
            run.exit_code, run.output
        ))
    }
}

#[then(regex = r#"^getting "([^"]+)" exits 1 with no output$"#)]
fn getting_exits_one(world: &mut BehaviourWorld, name: String) -> Result<(), String> {
    let run = run_config(world, get_cmd(&name));
    expect_silent_exit_one(&run)
}

#[then(regex = r"^the command exits 1 with no output$")]
fn command_exits_one_silently(world: &mut BehaviourWorld) -> Result<(), String> {
    let res = world.result.clone().ok_or("no CLI run captured")?;
    expect_silent_exit_one(&res)
}

fn expect_silent_exit_one(run: &CliRun) -> Result<(), String> {
    if run.exit_code == 1 && run.output.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "expected silent exit 1, got code {} (output: {:?})",
            run.exit_code, run.output
        ))
    }
}

#[then(regex = r#"^the listing shows "([^"]+)"$"#)]
fn listing_shows(world: &mut BehaviourWorld, line: String) -> Result<(), String> {
    let res = world.result.as_ref().ok_or("no CLI run captured")?;
    if res.output.lines().any(|l| l == line) {
        Ok(())
    } else {
        Err(format!(
            "expected listing line {line:?}, got: {:?}",
            res.output
        ))
    }
}

#[then(regex = r"^the output says no defaults are configured$")]
fn no_defaults_configured(world: &mut BehaviourWorld) -> Result<(), String> {
    let res = world.result.as_ref().ok_or("no CLI run captured")?;
    if res.output.starts_with("No defaults set in ") {
        Ok(())
    } else {
        Err(format!(
            "expected a no-defaults notice, got: {:?}",
            res.output
        ))
    }
}

#[then(regex = r#"^the command fails mentioning "([^"]+)"$"#)]
fn command_fails_mentioning(world: &mut BehaviourWorld, needle: String) -> Result<(), String> {
    let res = world.result.as_ref().ok_or("no CLI run captured")?;
    if res.exit_code != 0 && res.output.contains(&needle) {
        Ok(())
    } else {
        Err(format!(
            "expected a failure mentioning {needle:?}, got code {} (output: {:?})",
            res.exit_code, res.output
        ))
    }
}
