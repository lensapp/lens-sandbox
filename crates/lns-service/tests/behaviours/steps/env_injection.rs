use crate::world::BehaviourWorld;
use cucumber::{given, then, when};
use lns_service::workload_env::{refusal_warning, run_workload_env};

fn user_env_from(cmd: &str) -> Vec<String> {
    let toks: Vec<&str> = cmd.split_whitespace().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        if (toks[i] == "-e" || toks[i] == "--env") && i + 1 < toks.len() {
            out.push(toks[i + 1].to_string());
            i += 2;
        } else {
            i += 1;
        }
    }
    out
}

fn composed(world: &BehaviourWorld) -> Result<&lns_service::workload_env::WorkloadEnv, String> {
    world.composed_env.as_ref().ok_or("no composed env".into())
}

#[given(regex = r#"^the image declares ENV (\S+)=(.*)$"#)]
fn given_image_env(world: &mut BehaviourWorld, key: String, value: String) {
    world
        .image_env
        .get_or_insert_with(Vec::new)
        .push(format!("{key}={value}"));
}

#[given(regex = r#"^a connected integration manages the "([^"]+)" credential variable$"#)]
fn given_integration_manages(world: &mut BehaviourWorld, env_var: String) {
    world.managed_vars.push(env_var);
}

#[when(regex = r#"^the user runs `([^`]*)`$"#)]
fn when_user_runs(world: &mut BehaviourWorld, cmd: String) {
    let user_env = user_env_from(&cmd);
    let image_env = world.image_env.clone();
    let managed = world.managed_vars.clone();
    world.composed_env = Some(run_workload_env(
        image_env.as_deref(),
        &user_env,
        None,
        None,
        &managed,
    ));
    world.user_env = user_env;
}

#[then(regex = r#"^the workload's environment contains (\S+) set to "([^"]*)"$"#)]
fn then_env_contains_set_to(
    world: &mut BehaviourWorld,
    key: String,
    value: String,
) -> Result<(), String> {
    let want = format!("{key}={value}");
    let env = &composed(world)?.env;
    if env.contains(&want) {
        Ok(())
    } else {
        Err(format!("expected {want:?} in {env:?}"))
    }
}

#[then(regex = r#"^the workload's environment contains (\S+) and (\S+)$"#)]
fn then_env_contains_both(
    world: &mut BehaviourWorld,
    first: String,
    second: String,
) -> Result<(), String> {
    let env = &composed(world)?.env;
    for want in [first, second] {
        if !env.contains(&want) {
            return Err(format!("expected {want:?} in {env:?}"));
        }
    }
    Ok(())
}

#[then(regex = r#"^the workload's (\S+) override is dropped and refused$"#)]
fn then_managed_var_refused(world: &mut BehaviourWorld, env_var: String) -> Result<(), String> {
    let composed = composed(world)?;
    let prefix = format!("{env_var}=");
    if composed.env.iter().any(|e| e.starts_with(&prefix)) {
        return Err(format!(
            "user {env_var} must not reach the workload env: {:?}",
            composed.env
        ));
    }
    if !composed.refused.contains(&env_var) {
        return Err(format!(
            "expected {env_var} among refused, got {:?}",
            composed.refused
        ));
    }
    Ok(())
}

#[then(
    regex = r#"^a warning states the (\S+) override was refused because it is a managed credential$"#
)]
fn then_warning_explains_refusal(
    world: &mut BehaviourWorld,
    env_var: String,
) -> Result<(), String> {
    let _ = composed(world)?;
    let msg = refusal_warning(&env_var);
    if msg.contains(&env_var) && msg.contains("managed credential") {
        Ok(())
    } else {
        Err(format!("refusal warning unclear: {msg:?}"))
    }
}

#[then("the run still starts")]
fn then_run_still_starts(world: &mut BehaviourWorld) -> Result<(), String> {
    composed(world).map(|_| ())
}

#[then(regex = r#"^the audit entry for the run records (\S+) set to "([^"]*)"$"#)]
fn then_audit_records(
    world: &mut BehaviourWorld,
    key: String,
    value: String,
) -> Result<(), String> {
    let obj = lns_service::workload_env::injected_env_audit(&world.user_env, &[])
        .ok_or("no run_env audit event was built")?;
    let env = obj
        .get("env")
        .and_then(|v| v.as_object())
        .ok_or("audit event missing env object")?;
    match env.get(&key).and_then(|v| v.as_str()) {
        Some(v) if v == value => Ok(()),
        other => Err(format!("audit env[{key}] = {other:?}, want {value:?}")),
    }
}
