use crate::world::BehaviourWorld;
use cucumber::{given, then, when};

#[given(regex = r#"^the definition declares user "([^"]+)"$"#)]
fn definition_declares_user(world: &mut BehaviourWorld, user: String) {
    world.spec_user = Some(user);
}

#[given(regex = r#"^the image declares USER "([^"]+)"$"#)]
fn image_declares_user(world: &mut BehaviourWorld, user: String) {
    world.image_user = Some(user);
}

#[given("the image declares no USER")]
fn image_declares_no_user(world: &mut BehaviourWorld) {
    world.image_user = None;
}

#[when(regex = r"^the run-as user is resolved for `lns run ([^`]+)`$")]
fn resolve_run_as(world: &mut BehaviourWorld, cmd: String) {
    let flag_user = cmd
        .split_whitespace()
        .skip_while(|token| *token != "-u")
        .nth(1)
        .map(str::to_string);
    world.resolved_run_as = Some(lns_service::vm::resolve_run_as(
        flag_user.as_deref(),
        None,
        world.spec_user.as_deref(),
        world.image_user.as_deref(),
    ));
}

#[then(regex = r#"^the workload runs as "([^"]+)"$"#)]
fn runs_as(world: &mut BehaviourWorld, user: String) -> Result<(), String> {
    let run_as = resolved(world)?;
    if run_as.user == user {
        Ok(())
    } else {
        Err(format!("expected user {user:?}, got {:?}", run_as.user))
    }
}

#[then(regex = r#"^the workload runs as "([^"]+)" with uid (\d+)$"#)]
fn runs_as_with_uid(world: &mut BehaviourWorld, user: String, uid: u32) -> Result<(), String> {
    let run_as = resolved(world)?;
    if run_as.user == user && run_as.uid == Some(uid) {
        Ok(())
    } else {
        Err(format!(
            "expected {user:?} uid {uid}, got {:?} uid {:?}",
            run_as.user, run_as.uid
        ))
    }
}

#[then(regex = r#"^the workload runs as "([^"]+)" in group "([^"]+)"$"#)]
fn runs_as_in_group(world: &mut BehaviourWorld, user: String, group: String) -> Result<(), String> {
    let run_as = resolved(world)?;
    if run_as.user == user && run_as.group.as_deref() == Some(group.as_str()) {
        Ok(())
    } else {
        Err(format!(
            "expected {user:?} in {group:?}, got {:?} in {:?}",
            run_as.user, run_as.group
        ))
    }
}

#[then("the workload's uid is left for the guest to resolve")]
fn uid_left_to_the_guest(world: &mut BehaviourWorld) -> Result<(), String> {
    let run_as = resolved(world)?;
    match run_as.uid {
        None => Ok(()),
        Some(uid) => Err(format!(
            "a named user must be resolved from the guest's passwd, not guessed as uid {uid}"
        )),
    }
}

fn resolved(world: &BehaviourWorld) -> Result<&lns_service::vm::RunAs, String> {
    world
        .resolved_run_as
        .as_ref()
        .ok_or_else(|| "no run-as user was resolved".to_string())
}
