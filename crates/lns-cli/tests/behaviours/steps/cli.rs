use crate::runner::run_lns;
use crate::world::BehaviourWorld;
use cucumber::{then, when};

fn split_args(cmd_line: &str) -> Vec<String> {
    let trimmed = match cmd_line.strip_prefix("lns") {
        Some(rest) if rest.is_empty() || rest.starts_with(' ') => rest.trim_start_matches(' '),
        _ => panic!("Layer 2 in-process harness only drives `lns` (got {cmd_line:?})"),
    };
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    for ch in trimmed.chars() {
        match (ch, quote) {
            ('"' | '\'', None) if current.is_empty() => quote = Some(ch),
            (c, Some(q)) if c == q => quote = None,
            (' ', None) => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(ch),
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

#[when(regex = r#"^I run "([^"]*)"$"#)]
async fn i_run(world: &mut BehaviourWorld, cmd_line: String) {
    if let Some(rest) = cmd_line.strip_prefix("lns start ") {
        crate::steps::sandbox_cli::drive_sandbox_command(world, &format!("start {rest}")).await;
        return;
    }
    let parsed = split_args(&cmd_line);
    let args: Vec<&str> = parsed.iter().map(String::as_str).collect();
    world.result = Some(run_lns(&args));
}

#[then(regex = r#"^the exit code is (\d+)$"#)]
fn exit_code_is(world: &mut BehaviourWorld, code: i32) -> Result<(), String> {
    let res = world.result.as_ref().ok_or("no CLI run captured")?;
    if res.exit_code == code {
        Ok(())
    } else {
        Err(format!(
            "expected exit code {code}, got {} (output: {:?})",
            res.exit_code, res.output
        ))
    }
}

#[then(regex = r#"^the output contains "([^"]*)"$"#)]
fn output_contains(world: &mut BehaviourWorld, needle: String) -> Result<(), String> {
    let res = world.result.as_ref().ok_or("no CLI run captured")?;
    if res.output.contains(&needle) {
        Ok(())
    } else {
        Err(format!(
            "expected output to contain {needle:?}, got {:?}",
            res.output
        ))
    }
}

#[when(regex = r#"^the user runs `([^`]*)`$"#)]
fn the_user_runs(world: &mut BehaviourWorld, cmd_line: String) {
    let parsed = split_args(&cmd_line);
    let args: Vec<&str> = parsed.iter().map(String::as_str).collect();
    world.result = Some(run_lns(&args));
}

#[then("the command fails with a parse error naming the bad -e argument")]
fn parse_error_names_env_arg(world: &mut BehaviourWorld) -> Result<(), String> {
    let res = world.result.as_ref().ok_or("no CLI run captured")?;
    if res.exit_code == 2 && res.output.contains("--env") {
        Ok(())
    } else {
        Err(format!(
            "expected exit 2 naming --env, got code {} (output: {:?})",
            res.exit_code, res.output
        ))
    }
}

#[then("the command fails with a parse error requiring KEY=VALUE form")]
fn parse_error_requires_kv_form(world: &mut BehaviourWorld) -> Result<(), String> {
    let res = world.result.as_ref().ok_or("no CLI run captured")?;
    if res.exit_code == 2 && res.output.contains("KEY=VALUE") {
        Ok(())
    } else {
        Err(format!(
            "expected exit 2 requiring KEY=VALUE form, got code {} (output: {:?})",
            res.exit_code, res.output
        ))
    }
}

#[then(regex = r"^the command fails with a parse error naming the (--[a-z-]+) flag$")]
fn parse_error_names_flag(world: &mut BehaviourWorld, flag: String) -> Result<(), String> {
    let res = world.result.as_ref().ok_or("no CLI run captured")?;
    if res.exit_code == 2 && res.output.contains(&flag) {
        Ok(())
    } else {
        Err(format!(
            "expected exit 2 naming {flag}, got code {} (output: {:?})",
            res.exit_code, res.output
        ))
    }
}

#[then(regex = r"^the command succeeds$")]
fn command_succeeds(world: &mut BehaviourWorld) -> Result<(), String> {
    match world.result.as_ref() {
        Some(run) if run.exit_code == 0 => Ok(()),
        Some(run) => Err(format!("exited {}:\n{}", run.exit_code, run.output)),
        None => Err("the command did not run".to_string()),
    }
}

#[then(regex = r"^the command fails with an exit code other than 0$")]
fn command_fails(world: &mut BehaviourWorld) -> Result<(), String> {
    match world.result.as_ref().map(|r| r.exit_code) {
        Some(0) | None => Err("expected a non-zero exit code".to_string()),
        Some(_) => Ok(()),
    }
}

#[then("no run is started")]
fn no_run_is_started(world: &mut BehaviourWorld) -> Result<(), String> {
    let res = world.result.as_ref().ok_or("no CLI run captured")?;
    if res.exit_code != 0 {
        Ok(())
    } else {
        Err(format!(
            "expected a non-zero exit so dispatch never happens, got code {} (output: {:?})",
            res.exit_code, res.output
        ))
    }
}
