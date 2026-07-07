use crate::world::BehaviourWorld;
use cucumber::{given, then, when};
use lns_artifact::spec::{Quantity, Resources};
use lns_service::artifact::resources::{ResourceOverrides, VmSize, resolve_size};

const DEFAULTS: VmSize = VmSize {
    cpus: 1,
    mem_mib: 512,
};

#[given(regex = r#"^a bundle whose sandbox requests (\d+) cpus and (\d+) MiB$"#)]
async fn sandbox_requests(world: &mut BehaviourWorld, cpus: i64, mib: i64) {
    world.resource().bundle = Some(Resources {
        cpu: Some(Quantity::Int(cpus)),
        memory: Some(Quantity::Int(mib)),
    });
}

#[when("the bundle is run with no resource flags")]
async fn run_no_flags(world: &mut BehaviourWorld) {
    let rig = world.resource();
    rig.overrides = ResourceOverrides::default();
    rig.size = Some(resolve_size(rig.bundle.as_ref(), &rig.overrides, DEFAULTS));
}

#[when(regex = r#"^the bundle is run with (\d+) cpus and (\d+) MiB$"#)]
async fn run_with_flags(world: &mut BehaviourWorld, cpus: u8, mib: usize) {
    let rig = world.resource();
    rig.overrides = ResourceOverrides {
        cpus: Some(cpus),
        mem_mib: Some(mib),
    };
    rig.size = Some(resolve_size(rig.bundle.as_ref(), &rig.overrides, DEFAULTS));
}

#[then(regex = r#"^the run is sized at (\d+) cpus and (\d+) MiB$"#)]
async fn run_is_sized(world: &mut BehaviourWorld, cpus: u8, mib: usize) {
    let size = world.resource().size.expect("a run must have been sized");
    assert_eq!(size, VmSize { cpus, mem_mib: mib });
}
