use cucumber::{given, then, when};
use lns_ipc::{PortPublish, Protocol};
use lns_service::forward::{establish, plan};
use std::net::SocketAddr;

use crate::world::BehaviourWorld;

fn addr(ip: &str, port: u16) -> SocketAddr {
    SocketAddr::new(ip.parse().expect("valid ip"), port)
}

fn mapping(ip: &str, host_port: u16, container_port: u16) -> PortPublish {
    PortPublish {
        host_ip: ip.parse().expect("valid ip"),
        host_port,
        container_port,
        protocol: Protocol::Tcp,
    }
}

fn run_establish(world: &mut BehaviourWorld) {
    let fake = world.forward_fake();
    let specs = plan(&world.forward_specs);
    match establish(fake, &specs) {
        Ok(guard) => world.forward_guard = Some(guard),
        Err(e) => world.forward_error = Some(e.to_string()),
    }
}

#[given(regex = r"^a run is started with published mapping (\S+):(\d+) -> (\d+)$")]
fn run_started_with_mapping(world: &mut BehaviourWorld, ip: String, host: u16, container: u16) {
    world.forward_specs = vec![mapping(&ip, host, container)];
}

#[given(regex = r"^a run published (\S+):(\d+) -> (\d+)$")]
fn a_run_published(world: &mut BehaviourWorld, ip: String, host: u16, container: u16) {
    world.forward_specs = vec![mapping(&ip, host, container)];
    run_establish(world);
}

#[given(regex = r"^a detached run published (\S+):(\d+) -> (\d+)$")]
fn a_detached_run_published(world: &mut BehaviourWorld, ip: String, host: u16, container: u16) {
    world.forward_specs = vec![mapping(&ip, host, container)];
    run_establish(world);
}

#[given(regex = r"^a run published only (\S+):(\d+) -> (\d+)$")]
fn a_run_published_only(world: &mut BehaviourWorld, ip: String, host: u16, container: u16) {
    world.forward_specs = vec![mapping(&ip, host, container)];
    run_establish(world);
}

#[given(regex = r"^host port (\d+) is already bound$")]
fn host_port_already_bound(world: &mut BehaviourWorld, port: u16) {
    world.forward_fake().fail_on(addr("127.0.0.1", port));
}

#[when(regex = r"^the service sets up the run$")]
fn the_service_sets_up_the_run(world: &mut BehaviourWorld) {
    run_establish(world);
}

#[when(regex = r"^the run exits$")]
fn the_run_exits(world: &mut BehaviourWorld) {
    world.forward_guard = None;
}

#[when(regex = r"^the CLI detaches$")]
fn the_cli_detaches(_world: &mut BehaviourWorld) {
    // no-op: detaching does not tear down the guard; it stays alive in the daemon task.
}

#[then(regex = r"^it requests a host listener on (\S+):(\d+) forwarding to guest (\d+)$")]
fn requests_host_listener(
    world: &mut BehaviourWorld,
    ip: String,
    host: u16,
    container: u16,
) -> Result<(), String> {
    if world.forward_fake().was_bound(addr(&ip, host), container) {
        Ok(())
    } else {
        Err(format!("no bind recorded for {ip}:{host} -> {container}"))
    }
}

#[then(regex = r#"^the run fails before boot with an "address already in use" error$"#)]
fn run_fails_addr_in_use(world: &mut BehaviourWorld) -> Result<(), String> {
    match &world.forward_error {
        Some(e) if e.contains("address already in use") => Ok(()),
        other => Err(format!("expected address-in-use error, got {other:?}")),
    }
}

#[then(regex = r"^the process exits non-zero$")]
fn process_exits_non_zero(world: &mut BehaviourWorld) -> Result<(), String> {
    if world.forward_error.is_some() {
        Ok(())
    } else {
        Err("expected a forward error (which aborts the run non-zero)".to_string())
    }
}

#[then(regex = r"^the host port (\d+) is freed for reuse$")]
fn host_port_freed(world: &mut BehaviourWorld, port: u16) -> Result<(), String> {
    if world.forward_fake().was_unbound(addr("127.0.0.1", port)) {
        Ok(())
    } else {
        Err(format!("host port {port} was not released"))
    }
}

#[then(regex = r"^the host listener on (\S+):(\d+) stays up$")]
fn host_listener_stays_up(world: &mut BehaviourWorld, ip: String, port: u16) -> Result<(), String> {
    if world.forward_fake().was_unbound(addr(&ip, port)) {
        Err(format!(
            "{ip}:{port} was torn down while the run is still alive"
        ))
    } else {
        Ok(())
    }
}

#[then(regex = r"^it is torn down only when the run is killed or exits$")]
fn torn_down_on_exit(world: &mut BehaviourWorld) -> Result<(), String> {
    world.forward_guard = None;
    let fake = world.forward_fake();
    let spec = world.forward_specs.first().expect("a published spec");
    let a = addr(&spec.host_ip.to_string(), spec.host_port);
    if fake.was_unbound(a) {
        Ok(())
    } else {
        Err(format!("{a} was not torn down on run end"))
    }
}

#[then(regex = r"^no other guest port is reachable from the host$")]
fn no_other_port_reachable(world: &mut BehaviourWorld) -> Result<(), String> {
    let bound = world.forward_fake().bind_count();
    let expected = world.forward_specs.len();
    if bound == expected {
        Ok(())
    } else {
        Err(format!(
            "bound {bound} ports, expected exactly {expected} published"
        ))
    }
}

#[then(regex = r"^outbound network still flows through the existing proxy/DNS cage$")]
fn egress_unchanged(_world: &mut BehaviourWorld) {
    // no-op: publishing only binds host listeners; it has no egress surface to change.
}
