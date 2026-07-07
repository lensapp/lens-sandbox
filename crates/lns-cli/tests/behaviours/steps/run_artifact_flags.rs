use crate::world::BehaviourWorld;
use cucumber::then;

fn summary_contains(world: &BehaviourWorld, needle: &str) -> Result<(), String> {
    if world.summary_output.contains(needle) {
        Ok(())
    } else {
        Err(format!(
            "expected {needle:?} in summary:\n{}",
            world.summary_output
        ))
    }
}

#[then(regex = r#"^the summary lists the override "([^"]+)"$"#)]
fn lists_override(world: &mut BehaviourWorld, reference: String) -> Result<(), String> {
    summary_contains(world, &reference)
}

#[then(regex = r"^the summary states signature verification is skipped$")]
fn states_skipped(world: &mut BehaviourWorld) -> Result<(), String> {
    summary_contains(world, "verification skipped")
}

#[then(regex = r#"^the summary names the policy source "([^"]+)"$"#)]
fn names_policy_source(world: &mut BehaviourWorld, reference: String) -> Result<(), String> {
    summary_contains(world, &reference)
}
