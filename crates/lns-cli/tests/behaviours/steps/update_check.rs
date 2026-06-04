use crate::world::BehaviourWorld;
use cucumber::{given, then, when};
use lns_cli::update_check;
use lns_ipc::UpdateStatus;

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

fn out_str(world: &BehaviourWorld) -> String {
    String::from_utf8(world.uc.out.clone()).unwrap()
}

fn mark(world: &mut BehaviourWorld, latest: &str, min_secure: Option<&str>) {
    world.uc.reader.status = Some(UpdateStatus {
        latest: latest.to_string(),
        min_secure_version: min_secure.map(str::to_string),
        checked_at_unix: 0,
    });
}

#[given("the service has marked a newer version available")]
fn marked_newer(world: &mut BehaviourWorld) {
    mark(world, "99.0.0", None);
}

#[given("the service has marked a security update for the running version")]
fn marked_security(world: &mut BehaviourWorld) {
    mark(world, "99.0.0", Some("98.0.0"));
}

#[given("the service has marked the running version as latest")]
fn marked_current(world: &mut BehaviourWorld) {
    mark(world, CURRENT_VERSION, None);
}

#[given("the service has not marked any version yet")]
fn marked_nothing(world: &mut BehaviourWorld) {
    world.uc.reader.status = None;
}

#[when("the user runs an lns command")]
fn user_runs_command(world: &mut BehaviourWorld) {
    update_check::announce(&world.uc.reader, CURRENT_VERSION, &mut world.uc.out).unwrap();
}

#[when("the user runs the update check in dry-run mode")]
fn dry_run_mode(world: &mut BehaviourWorld) {
    update_check::dry_run(&world.uc.reader, &mut world.uc.out).unwrap();
}

#[then(r#"lns tells the user to run "lns update""#)]
fn tells_run_update(world: &mut BehaviourWorld) -> Result<(), String> {
    let s = out_str(world);
    if s.contains("lns update") {
        Ok(())
    } else {
        Err(format!("no `lns update` hint: {s:?}"))
    }
}

#[then("lns does not contact the network or download anything")]
fn no_network(world: &mut BehaviourWorld) -> Result<(), String> {
    let s = out_str(world);
    if !s.contains("Downloading") && !s.contains("Updated to") {
        Ok(())
    } else {
        Err(format!("output implies a network action: {s:?}"))
    }
}

#[then(r#"the user is told to run "lns update" because a security update is available"#)]
fn told_security(world: &mut BehaviourWorld) -> Result<(), String> {
    let s = out_str(world);
    if s.contains("lns update") && s.to_lowercase().contains("security") {
        Ok(())
    } else {
        Err(format!("no security update message: {s:?}"))
    }
}

#[then("lns prints no update message")]
fn no_update_message(world: &mut BehaviourWorld) -> Result<(), String> {
    let s = out_str(world);
    if !s.contains("lns update") {
        Ok(())
    } else {
        Err(format!("expected silence, got: {s:?}"))
    }
}

#[then("lns prints the payload — install ID, version, and OS/arch — and contacts nothing")]
fn prints_payload(world: &mut BehaviourWorld) -> Result<(), String> {
    let s = out_str(world);
    if s.contains("install_id")
        && s.contains(CURRENT_VERSION)
        && s.contains(std::env::consts::OS)
        && s.contains(std::env::consts::ARCH)
    {
        Ok(())
    } else {
        Err(format!("dry-run payload incomplete: {s:?}"))
    }
}
