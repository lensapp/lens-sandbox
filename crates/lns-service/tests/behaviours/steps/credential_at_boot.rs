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

#[then(regex = r#"^the error points at `lns integration connect ([^`]+)`$"#)]
fn error_points_at_connect(w: &mut BehaviourWorld, id: String) -> Result<(), String> {
    let error = launch_error(w)?;
    if error.contains(&format!("`lns integration connect {id}`")) {
        Ok(())
    } else {
        Err(format!(
            "expected the error to point at `lns integration connect {id}`, got: {error}"
        ))
    }
}
