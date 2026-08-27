use crate::runner::run_one_shot;
use crate::steps::run_lifecycle::fresh_handle;
use crate::world::BehaviourWorld;
use cucumber::{given, then, when};
use lns_ipc::Request;
use lns_service::run_registry;

fn clear_name(name: &str) {
    if let Ok(id) = run_registry::resolve(name) {
        run_registry::deregister(&id);
    }
}

fn do_register(world: &mut BehaviourWorld, requested: Option<String>) {
    let document = world
        .naming_document
        .as_deref()
        .and_then(|d| lns_service::run::document_name(Some(d)));
    let id = run_registry::allocate_run_id();
    match run_registry::register_named(
        id.clone(),
        requested,
        document.as_deref(),
        fresh_handle("some-image", lns_ipc::RunConfig::default()),
    ) {
        Ok(name) => {
            world.naming_run = Some(id);
            world.naming_name = Some(name);
            world.naming_error = None;
        }
        Err(message) => world.naming_error = Some(message),
    }
}

#[when("a run is registered without a name")]
async fn register_without_name(world: &mut BehaviourWorld) {
    do_register(world, None);
}

#[when(regex = r#"^a run is registered with the name "([^"]+)"$"#)]
async fn register_with_name(world: &mut BehaviourWorld, name: String) {
    do_register(world, Some(name));
}

#[given(regex = r#"^a registered run named "([^"]+)"$"#)]
async fn given_registered_named(world: &mut BehaviourWorld, name: String) {
    clear_name(&name);
    do_register(world, Some(name));
}

#[given(regex = r#"^a registered run named "([^"]+)" that has already exited$"#)]
async fn given_registered_named_exited(world: &mut BehaviourWorld, name: String) {
    clear_name(&name);
    do_register(world, Some(name));
    if let Some(id) = &world.naming_run {
        run_registry::set_exit_code(id, 0);
    }
}

#[given(regex = r#"^a document that declares the name "([^"]+)"$"#)]
async fn given_document_named(world: &mut BehaviourWorld, name: String) {
    world.naming_document = Some(format!(
        r#"{{"apiVersion":"lns.run/v1","kind":"sandbox","name":"{name}","spec":{{"image":"x:1"}}}}"#
    ));
}

#[when("a second run of the same document is registered without a name")]
async fn register_second_without_name(world: &mut BehaviourWorld) {
    world.naming_first_name = world.naming_name.clone();
    do_register(world, None);
}

#[then(regex = r#"^the run's name starts with "([^"]+)"$"#)]
fn then_name_starts_with(world: &mut BehaviourWorld, prefix: String) -> Result<(), String> {
    let name = world.naming_name.as_deref().ok_or("no name assigned")?;
    if name.starts_with(&prefix) {
        Ok(())
    } else {
        Err(format!("name {name:?} does not start with {prefix:?}"))
    }
}

#[then(regex = r#"^the name is "([^"]+)" followed by one more word$"#)]
fn then_one_more_word(world: &mut BehaviourWorld, prefix: String) -> Result<(), String> {
    let name = world.naming_name.as_deref().ok_or("no name assigned")?;
    match name.strip_prefix(&format!("{prefix}-")) {
        Some(word) if !word.is_empty() && word.chars().all(|c| c.is_ascii_lowercase()) => Ok(()),
        _ => Err(format!("name {name:?} is not {prefix:?} plus one word")),
    }
}

#[then(regex = r#"^the run's name is not "([^"]+)"$"#)]
fn then_name_is_not(world: &mut BehaviourWorld, taken: String) -> Result<(), String> {
    let name = world.naming_name.as_deref().ok_or("no name assigned")?;
    if name == taken {
        Err(format!("the generator handed out the held name {taken:?}"))
    } else {
        Ok(())
    }
}

#[then("the two runs have different names")]
fn then_two_runs_differ(world: &mut BehaviourWorld) -> Result<(), String> {
    then_auto_name_differs(world)
}

#[given("a registered run whose name is the generator's first pick")]
async fn given_auto_named(world: &mut BehaviourWorld) {
    do_register(world, None);
    world.naming_first_name = world.naming_name.clone();
}

#[then("the run is assigned a non-empty name")]
fn then_non_empty_name(world: &mut BehaviourWorld) -> Result<(), String> {
    match world.naming_name.as_deref() {
        Some(n) if !n.is_empty() => Ok(()),
        other => Err(format!("expected a non-empty name, got {other:?}")),
    }
}

#[then("the assigned name is not all hex")]
fn then_not_all_hex(world: &mut BehaviourWorld) -> Result<(), String> {
    let name = world.naming_name.as_deref().ok_or("no name assigned")?;
    if name
        .bytes()
        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        Err(format!("name {name:?} is all lowercase hex"))
    } else {
        Ok(())
    }
}

#[then("the assigned name is two words joined by a hyphen")]
fn then_two_hyphenated_words(world: &mut BehaviourWorld) -> Result<(), String> {
    let name = world.naming_name.as_deref().ok_or("no name assigned")?;
    match name.split_once('-') {
        Some((first, second))
            if !first.is_empty()
                && !second.is_empty()
                && !second.contains('-')
                && name.chars().all(|c| c.is_ascii_lowercase() || c == '-') =>
        {
            Ok(())
        }
        _ => Err(format!(
            "name {name:?} is not two hyphenated lowercase words"
        )),
    }
}

#[then(regex = r#"^the run's name is "([^"]+)"$"#)]
fn then_run_name_is(world: &mut BehaviourWorld, expected: String) -> Result<(), String> {
    match world.naming_name.as_deref() {
        Some(n) if n == expected => Ok(()),
        other => Err(format!("expected name {expected:?}, got {other:?}")),
    }
}

#[then("registration is refused")]
fn then_registration_refused(world: &mut BehaviourWorld) -> Result<(), String> {
    if world.naming_error.is_some() {
        Ok(())
    } else {
        Err("expected registration to be refused".into())
    }
}

#[then(regex = r#"^the refusal contains "([^"]+)"$"#)]
fn then_refusal_contains(world: &mut BehaviourWorld, needle: String) -> Result<(), String> {
    let err = world.naming_error.as_deref().ok_or("no refusal captured")?;
    if err.contains(&needle) {
        Ok(())
    } else {
        Err(format!(
            "expected refusal to contain {needle:?}, got {err:?}"
        ))
    }
}

#[then("the refusal explains a name must not be all hex")]
fn then_refusal_all_hex(world: &mut BehaviourWorld) -> Result<(), String> {
    let err = world.naming_error.as_deref().ok_or("no refusal captured")?;
    if err.contains("all lowercase hex") {
        Ok(())
    } else {
        Err(format!("expected an all-hex explanation, got {err:?}"))
    }
}

#[then("the auto-assigned name differs from every listed run's name")]
fn then_auto_name_differs(world: &mut BehaviourWorld) -> Result<(), String> {
    let first = world
        .naming_first_name
        .as_deref()
        .ok_or("no first auto name")?;
    let second = world.naming_name.as_deref().ok_or("no second auto name")?;
    if first != second {
        Ok(())
    } else {
        Err(format!("auto names collided: {first:?}"))
    }
}

#[when(regex = r#"^a StopRun request for run "([^"]+)" arrives$"#)]
async fn stop_by_handle(world: &mut BehaviourWorld, run: String) {
    world.response = Some(
        run_one_shot(
            &Request::StopRun {
                run,
                timeout_secs: 1,
            },
            world.started_at(),
        )
        .await,
    );
}

#[when("a StopRun request for that run's numeric id arrives")]
async fn stop_by_numeric_id(world: &mut BehaviourWorld) {
    let id = world
        .naming_run
        .clone()
        .expect("a run must be registered first");
    world.response = Some(
        run_one_shot(
            &Request::StopRun {
                run: id,
                timeout_secs: 1,
            },
            world.started_at(),
        )
        .await,
    );
}

#[when(regex = r#"^a RemoveRun request for run "([^"]+)" arrives$"#)]
async fn remove_by_handle(world: &mut BehaviourWorld, run: String) {
    world.response = Some(
        run_one_shot(
            &Request::RemoveRun { run, force: false },
            world.started_at(),
        )
        .await,
    );
}

#[then(regex = r#"^a run can then be registered with the name "([^"]+)"$"#)]
fn then_can_register_name(world: &mut BehaviourWorld, name: String) -> Result<(), String> {
    let _ = world;
    let id = run_registry::allocate_run_id();
    let result = run_registry::register_named(
        id.clone(),
        Some(name.clone()),
        None,
        fresh_handle("some-image", lns_ipc::RunConfig::default()),
    );
    let outcome = match &result {
        Ok(assigned) if *assigned == name => Ok(()),
        Ok(other) => Err(format!("registered under {other:?}, expected {name:?}")),
        Err(e) => Err(format!("expected reuse to succeed, got {e:?}")),
    };
    run_registry::deregister(&id);
    outcome
}

#[when(regex = r#"^a RenameRun request renames "([^"]+)" to "([^"]+)"$"#)]
async fn rename_request(world: &mut BehaviourWorld, run: String, new_name: String) {
    world.response =
        Some(run_one_shot(&Request::RenameRun { run, new_name }, world.started_at()).await);
}

#[then(regex = r#"^the run resolves by the name "([^"]+)"$"#)]
fn then_resolves_by_name(_world: &mut BehaviourWorld, name: String) -> Result<(), String> {
    run_registry::resolve(&name)
        .map(|_| ())
        .map_err(|e| format!("expected {name:?} to resolve, got {e}"))
}

#[then(regex = r#"^the run no longer resolves by the name "([^"]+)"$"#)]
fn then_not_resolves_by_name(_world: &mut BehaviourWorld, name: String) -> Result<(), String> {
    match run_registry::resolve(&name) {
        Err(_) => Ok(()),
        Ok(id) => Err(format!("expected {name:?} not to resolve, but got id {id}")),
    }
}
