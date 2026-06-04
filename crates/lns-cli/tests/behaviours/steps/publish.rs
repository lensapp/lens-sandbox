use crate::world::BehaviourWorld;
use cucumber::then;

#[then(regex = r"^a Ports line shows `([^`]+)`$")]
fn ports_line_shows(world: &mut BehaviourWorld, expected: String) -> Result<(), String> {
    if world.summary_output.contains(&expected) {
        Ok(())
    } else {
        Err(format!(
            "expected Ports line {expected:?} in:\n{}",
            world.summary_output
        ))
    }
}

#[then(regex = r"^the run summary marks the mapping as exposed beyond this machine$")]
fn summary_marks_exposed(world: &mut BehaviourWorld) -> Result<(), String> {
    if world.summary_output.contains("exposed beyond this machine") {
        Ok(())
    } else {
        Err(format!("no exposure note in:\n{}", world.summary_output))
    }
}

fn failure_output(world: &BehaviourWorld) -> Result<&str, String> {
    let res = world.result.as_ref().ok_or("no CLI run captured")?;
    if res.exit_code == 0 {
        return Err("command unexpectedly succeeded".to_string());
    }
    Ok(&res.output)
}

#[then(regex = r"^the command fails with an error that udp publishing is not yet supported$")]
fn fails_udp_not_supported(world: &mut BehaviourWorld) -> Result<(), String> {
    let output = failure_output(world)?;
    if output.contains("udp publishing is not yet supported") {
        Ok(())
    } else {
        Err(format!("expected udp-unsupported error, got: {output:?}"))
    }
}

#[then(regex = r"^the command fails with a parse error naming the bad spec$")]
fn fails_with_parse_error(world: &mut BehaviourWorld) -> Result<(), String> {
    let output = failure_output(world)?;
    if output.contains("notaport") {
        Ok(())
    } else {
        Err(format!(
            "expected parse error naming the bad spec, got: {output:?}"
        ))
    }
}
