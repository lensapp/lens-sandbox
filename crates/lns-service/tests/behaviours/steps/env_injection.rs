use crate::world::BehaviourWorld;
use cucumber::{given, then, when};
use lns_service::workload_env::run_workload_env;

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

#[when(regex = r#"^the user runs `([^`]*)`$"#)]
fn when_user_runs(world: &mut BehaviourWorld, cmd: String) {
    // The run displaces these before its environment travels, so the audit chain records what the user supplied rather than what was typed.
    let typed = user_env_from(&cmd);
    let (user_env, refused) = lns_service::workload_env::without_what_a_grant_fills(
        &typed,
        &world.connectors,
        lns_service::workload_env::source_among(&typed),
    );
    world.refused_env = refused;
    world.connectors.left_unarmed = lns_service::connector::claims::left_to_a_connector(
        &world.declared_credentials,
        &world.installed_connectors,
        &world.connectors.filled,
        None,
    );
    let image_env = world.image_env.clone();
    world.composed_env = Some(run_workload_env(
        image_env.as_deref(),
        &user_env,
        None,
        None,
        &Default::default(),
        &world.connectors,
    ));
    let from_the_image = composed(world)
        .map(|c| c.refused.clone())
        .unwrap_or_default();
    // One answer over every source, the way the run reports it: warning per source would name a variable twice.
    world.refused_env = lns_service::workload_env::one_refusal_per_variable(
        std::mem::take(&mut world.refused_env)
            .into_iter()
            .chain(from_the_image),
    );
    world.user_env = user_env;
}

#[given(regex = r#"^the sandbox declares the credential (\S+)$"#)]
fn the_sandbox_declares(world: &mut BehaviourWorld, key: String) {
    world.declared_credentials.push(lns_spec::Credential {
        env_var: Some(key),
        placeholder: "the_declarations_own_LNSPLACEHOLDER".to_string(),
        field: None,
        injections: Vec::new(),
    });
}

#[given(regex = r#"^the installed connector "([^"]+)" claims (\S+) with the placeholder (\S+)$"#)]
fn the_installed_connector_claims(
    world: &mut BehaviourWorld,
    connector: String,
    key: String,
    placeholder: String,
) {
    // A real document, so the scenario exercises the rule rather than the harness's idea of it.
    let document = serde_json::json!({
        "apiVersion": "lns.run/v1",
        "kind": "connector",
        "name": connector,
        "spec": {
            "serves": [format!("api.{connector}.example")],
            "methods": [{
                "name": "token",
                "auth": { "kind": "token" },
                "credentials": [{ "envVar": key, "placeholder": placeholder }],
            }],
        },
    });
    world.installed_connectors.push(
        lns_artifact::connector::parse(document.to_string().as_bytes()).expect("a valid connector"),
    );
}

#[then(regex = r#"^the run is told nothing about (\S+)$"#)]
fn then_the_run_is_told_nothing(world: &mut BehaviourWorld, key: String) -> Result<(), String> {
    match world.refused_env.iter().find(|refused| refused.key == key) {
        Some(refused) => Err(format!(
            "nothing displaced {key}, so a remedy for it names something nobody did: {}",
            lns_service::workload_env::refusal_warning(refused)
        )),
        None => Ok(()),
    }
}

#[then(regex = r#"^the workload's environment carries no (\S+) entry$"#)]
fn then_env_carries_no_entry(world: &mut BehaviourWorld, key: String) -> Result<(), String> {
    let env = &composed(world)?.env;
    match env
        .iter()
        .find(|kv| kv.split_once('=').is_some_and(|(k, _)| k == key))
    {
        Some(found) => Err(format!(
            "no sandbox declared {key}, so {found:?} must not be set"
        )),
        None => Ok(()),
    }
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

#[then(regex = r#"^the audit entry for the run records (\S+) set to "([^"]*)"$"#)]
fn then_audit_records(
    world: &mut BehaviourWorld,
    key: String,
    value: String,
) -> Result<(), String> {
    let env = lns_service::workload_env::injected_env(&world.user_env)
        .ok_or("no run_env audit event was built")?;
    match env.get(&key).and_then(|v| v.as_str()) {
        Some(v) if v == value => Ok(()),
        other => Err(format!("audit env[{key}] = {other:?}, want {value:?}")),
    }
}

#[given(regex = r#"^the connector "([^"]+)" fills (\S+) with the placeholder (\S+) for this run$"#)]
fn the_connector_fills(
    world: &mut BehaviourWorld,
    connector: String,
    key: String,
    placeholder: String,
) {
    world.connectors.filled.insert(
        key,
        lns_service::workload_env::Filled {
            connector,
            placeholder,
        },
    );
}

#[then(regex = r#"^the run is told "([^"]+)" fills (\S+)$"#)]
fn then_the_run_is_told(
    world: &mut BehaviourWorld,
    connector: String,
    key: String,
) -> Result<(), String> {
    let refused = world.refused_env.clone();
    match refused.iter().find(|refused| refused.key == key) {
        Some(refused) => {
            let warning = lns_service::workload_env::refusal_warning(refused);
            if warning.contains(&connector) {
                Ok(())
            } else {
                Err(format!(
                    "the warning does not name the connector: {warning}"
                ))
            }
        }
        None => Err(format!("{key} was dropped silently, refused: {refused:?}")),
    }
}

#[then(regex = r#"^the audit entry for the run records no (\S+) entry$"#)]
fn then_audit_records_no_entry(world: &mut BehaviourWorld, key: String) -> Result<(), String> {
    match lns_service::workload_env::injected_env(&world.user_env)
        .and_then(|env| env.get(&key).cloned())
    {
        Some(value) => Err(format!(
            "the chain says {key} entered the sandbox as {value:?}, and it did not"
        )),
        None => Ok(()),
    }
}

#[then(regex = r#"^the run names (\S+) once$"#)]
fn then_the_run_names_once(world: &mut BehaviourWorld, key: String) -> Result<(), String> {
    let named = world
        .refused_env
        .iter()
        .filter(|refused| refused.key == key)
        .count();
    match named {
        1 => Ok(()),
        other => Err(format!(
            "{key} was named {other} times, so the reader is given {other} remedies for one variable"
        )),
    }
}
