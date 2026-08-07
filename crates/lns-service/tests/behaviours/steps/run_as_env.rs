use crate::world::BehaviourWorld;
use cucumber::{given, then};

#[given(regex = r#"^the definition declares env "([^=]+)=([^"]*)"$"#)]
fn definition_declares_env(world: &mut BehaviourWorld, key: String, value: String) {
    world.definition_env.push(format!("{key}={value}"));
}

#[then(regex = r#"^the supervised workload env pins the declared (HOME|USER) to "([^"]*)"$"#)]
fn pins_declared(world: &mut BehaviourWorld, key: String, value: String) -> Result<(), String> {
    let entry = format!("LENS_SANDBOX_WORKLOAD_{key}={value}");
    let env = composed(world)?;
    if env.contains(&entry) {
        Ok(())
    } else {
        Err(format!("expected {entry:?} in {env:?}"))
    }
}

#[then(regex = r"^the supervised workload env pins no declared HOME$")]
fn pins_no_declared_home(world: &mut BehaviourWorld) -> Result<(), String> {
    let env = composed(world)?;
    match env
        .iter()
        .find(|e| e.starts_with("LENS_SANDBOX_WORKLOAD_HOME="))
    {
        Some(entry) => Err(format!("unexpected {entry:?} in {env:?}")),
        None => Ok(()),
    }
}

fn composed(world: &BehaviourWorld) -> Result<&Vec<String>, String> {
    world
        .composed_env
        .as_ref()
        .map(|c| &c.env)
        .ok_or_else(|| "no workload env was composed".to_string())
}
