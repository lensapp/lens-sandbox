use cucumber::{given, then};
use lns_policy::credentials::CredentialEntry;

use crate::world::BehaviourWorld;

fn definition_with_slot(name: &str, env: &str, required: bool) -> String {
    format!(
        r#"{{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{{"name":"hermes"}},"spec":{{"image":"ghcr.io/team/base:1","credentials":[{{"name":"{name}","env":"{env}","required":{required}}}]}}}}"#
    )
}

#[given(
    regex = r#"^the sandbox definition requires a credential slot for "([^"]+)" injected as "([^"]+)"$"#
)]
fn definition_requires_slot(w: &mut BehaviourWorld, name: String, env: String) {
    let rig = w.declared.get_or_insert_with(Default::default);
    rig.definition = Some(definition_with_slot(&name, &env, true));
}

#[given(
    regex = r#"^the sandbox definition declares a credential slot for "([^"]+)" injected as "([^"]+)"$"#
)]
fn definition_declares_slot(w: &mut BehaviourWorld, name: String, env: String) {
    let rig = w.declared.get_or_insert_with(Default::default);
    rig.definition = Some(definition_with_slot(&name, &env, false));
}

#[given(regex = r#"^the per-machine credential store has no entry for "([^"]+)"$"#)]
fn store_has_no_entry(w: &mut BehaviourWorld, id: String) {
    let rig = w.declared.get_or_insert_with(Default::default);
    rig.store.remove(&id);
}

#[given(regex = r#"^the per-machine credential store has a deny entry for "([^"]+)"$"#)]
fn store_has_deny_entry(w: &mut BehaviourWorld, id: String) {
    let rig = w.declared.get_or_insert_with(Default::default);
    rig.store.insert(id, CredentialEntry::Deny);
}

#[given(regex = r#"^the per-machine credential store has a stored value for "([^"]+)"$"#)]
fn store_has_stored_value(w: &mut BehaviourWorld, id: String) {
    let rig = w.declared.get_or_insert_with(Default::default);
    rig.store.insert(
        id,
        CredentialEntry::Stored {
            value: "some-secret".into(),
        },
    );
}

#[given(regex = r#"^the per-machine credential store has a host-detect entry for "([^"]+)"$"#)]
fn store_has_host_detect_entry(w: &mut BehaviourWorld, id: String) {
    let rig = w.declared.get_or_insert_with(Default::default);
    rig.store.insert(id, CredentialEntry::HostDetect);
}

fn launch_error(w: &BehaviourWorld) -> Result<&str, String> {
    w.declared
        .as_ref()
        .and_then(|r| r.error.as_deref())
        .ok_or_else(|| "no launch error was recorded".to_string())
}

#[then(regex = r#"^the error names the injection target "([^"]+)"$"#)]
fn error_names_injection_target(w: &mut BehaviourWorld, env: String) -> Result<(), String> {
    let error = launch_error(w)?;
    if error.contains(&format!("injected as {env}")) {
        Ok(())
    } else {
        Err(format!(
            "expected the error to name the injection target {env}, got: {error}"
        ))
    }
}

#[then("the error says the credential was denied on this machine")]
fn error_says_denied(w: &mut BehaviourWorld) -> Result<(), String> {
    let error = launch_error(w)?;
    if !error.contains("you have denied it") {
        return Err(format!(
            "expected a denied-on-this-machine error, got: {error}"
        ));
    }
    if error.contains("no value is bound") {
        return Err(format!("a deny must not read as never-bound: {error}"));
    }
    Ok(())
}

#[then(regex = r#"^the error points at `lns connector connect ([^`]+)`$"#)]
fn error_points_at_connect(w: &mut BehaviourWorld, id: String) -> Result<(), String> {
    let error = launch_error(w)?;
    if error.contains(&format!("`lns connector connect {id}`")) {
        Ok(())
    } else {
        Err(format!(
            "expected the error to point at `lns connector connect {id}`, got: {error}"
        ))
    }
}

fn armed_provider_for_env<'a>(
    w: &'a BehaviourWorld,
    env: &str,
) -> Result<&'a (String, String, String), String> {
    let rig = w.declared.as_ref().ok_or("no launch happened")?;
    if let Some(err) = &rig.error {
        return Err(format!("the launch failed: {err}"));
    }
    rig.providers
        .iter()
        .find(|(_, var, _)| var == env)
        .ok_or_else(|| format!("no provider seeds {env}; armed: {:?}", rig.providers))
}

#[then(regex = r#"^the workload's environment contains the placeholder under "([^"]+)"$"#)]
fn env_contains_placeholder_under(w: &mut BehaviourWorld, env: String) -> Result<(), String> {
    let (_, _, placeholder) = armed_provider_for_env(w, &env)?;
    if placeholder.is_empty() {
        return Err(format!("{env} was seeded without a placeholder"));
    }
    Ok(())
}

#[then("no value-decision prompt is shown before the workload starts")]
fn no_value_decision_prompt(w: &mut BehaviourWorld) -> Result<(), String> {
    let rig = w.declared.as_ref().ok_or("no launch happened")?;
    if let Some(err) = &rig.error {
        return Err(format!("the launch failed: {err}"));
    }
    if let Some(pending) = &rig.pending {
        return Err(format!(
            "the launch blocked on a prompt for {}",
            pending.connector
        ));
    }
    if rig.running_policy.is_none() {
        return Err("the workload never started".to_string());
    }
    Ok(())
}

#[then(regex = r#"^the workload sees only a placeholder in "([^"]+)"$"#)]
fn workload_sees_only_placeholder(w: &mut BehaviourWorld, env: String) -> Result<(), String> {
    let (_, _, placeholder) = armed_provider_for_env(w, &env)?.clone();
    if !lns_policy::providers::is_self_identifying(&placeholder) {
        return Err(format!(
            "the seeded value must be a self-identifying placeholder, got {placeholder}"
        ));
    }
    let rig = w.declared.as_ref().ok_or("no launch happened")?;
    let stores_real_value = rig.store.values().any(|entry| {
        matches!(entry, CredentialEntry::Stored { value } if placeholder.contains(value.as_str()))
    });
    if stores_real_value {
        return Err(format!(
            "the placeholder leaks the stored value: {placeholder}"
        ));
    }
    Ok(())
}

#[then("the real value is substituted at the boundary")]
fn real_value_stays_at_boundary(w: &mut BehaviourWorld) -> Result<(), String> {
    let rig = w.declared.as_ref().ok_or("no launch happened")?;
    let stored: Vec<&str> = rig
        .store
        .values()
        .filter_map(|entry| match entry {
            CredentialEntry::Stored { value } => Some(value.as_str()),
            _ => None,
        })
        .collect();
    if stored.is_empty() {
        return Err("the scenario bound no stored value to check against".to_string());
    }
    for (id, env, placeholder) in &rig.providers {
        for value in &stored {
            if placeholder.contains(value) || env.contains(value) || id.contains(value) {
                return Err(format!(
                    "the real value reached the workload plan via provider {id}"
                ));
            }
        }
    }
    Ok(())
}
