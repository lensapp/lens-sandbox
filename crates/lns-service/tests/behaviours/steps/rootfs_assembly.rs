use crate::assembly_rig::AssemblyRig;
use crate::world::BehaviourWorld;
use cucumber::{given, then, when};
use lns_service::composefs::descriptor::{DescriptorBuilder, DescriptorRequest};
use lns_service::content_store::ContentStore;

#[given(regex = r"^an uncached image with layers of (\d+) and (\d+) bytes$")]
fn uncached_image(world: &mut BehaviourWorld, first: usize, second: usize) {
    world.assembly = Some(AssemblyRig::with_layer_sizes(&[first, second]));
}

#[when("the rootfs is assembled with a recording progress sink")]
fn assemble_with_recording_sink(world: &mut BehaviourWorld) {
    let rig = world.assembly.as_mut().expect("assembly rig");
    let events = rig.events.clone();
    let sink = move |current: u64, total: u64| events.lock().unwrap().push((current, total));
    let store = ContentStore::new(rig.dir.path().join("content"));
    let builder = DescriptorBuilder::new(rig.dir.path());
    let digests = rig.layer_digests();
    let built = builder
        .build(
            &store,
            &DescriptorRequest {
                layer_digests: &digests,
                layers: &rig.layers,
                runtime_layer: None,
            },
            &sink,
        )
        .expect("descriptor build");
    rig.built = Some(built);
}

#[then(regex = r"^the sink first observes (\d+) of (\d+) bytes$")]
fn sink_first_observes(world: &mut BehaviourWorld, current: u64, total: u64) {
    assert_eq!(observed(world).first(), Some(&(current, total)));
}

#[then(regex = r"^the sink observes (\d+) of (\d+) bytes after the first layer$")]
fn sink_observes_after_first_layer(world: &mut BehaviourWorld, current: u64, total: u64) {
    assert_eq!(observed(world).get(1), Some(&(current, total)));
}

#[then(regex = r"^the sink observes (\d+) of (\d+) bytes after the last layer$")]
fn sink_observes_after_last_layer(world: &mut BehaviourWorld, current: u64, total: u64) {
    assert_eq!(observed(world).last(), Some(&(current, total)));
}

fn observed(world: &BehaviourWorld) -> Vec<(u64, u64)> {
    let rig = world.assembly.as_ref().expect("assembly rig");
    rig.events.lock().unwrap().clone()
}
