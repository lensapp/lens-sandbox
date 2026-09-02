use cucumber::{then, when};

use crate::volume_rig::VolumeRig;
use crate::world::BehaviourWorld;
use lns_service::volume_store::VOLUME_DEFAULT_SIZE_BYTES;

#[when(expr = "the volumes are listed")]
async fn list_volumes(w: &mut BehaviourWorld) {
    w.volume().list().await;
}

#[when(expr = "volume {string} is created")]
async fn create_volume(w: &mut BehaviourWorld, name: String) {
    w.volume().create(&name).await;
}

#[when(expr = "volume {string} is inspected")]
async fn inspect_volume(w: &mut BehaviourWorld, name: String) {
    w.volume().inspect(&name).await;
}

#[when(expr = "volume {string} is removed")]
async fn remove_volume(w: &mut BehaviourWorld, name: String) {
    w.volume().remove(&name).await;
}

#[when(expr = "the volumes are pruned")]
async fn prune_volumes(w: &mut BehaviourWorld) {
    w.volume().prune(false).await;
}

#[when(expr = "the volumes are pruned as a dry run")]
async fn prune_volumes_dry(w: &mut BehaviourWorld) {
    w.volume().prune(true).await;
}

fn listing(rig: &VolumeRig) -> &[lns_ipc::VolumeInfo] {
    rig.last_list.as_deref().expect("a listing was taken")
}

fn listed<'a>(rig: &'a VolumeRig, name: &str) -> &'a lns_ipc::VolumeInfo {
    listing(rig)
        .iter()
        .find(|v| v.name == name)
        .unwrap_or_else(|| panic!("volume {name:?} not in listing {:?}", rig.last_list))
}

#[then(expr = "the listing is empty")]
async fn listing_empty(w: &mut BehaviourWorld) {
    let rig = w.volume();
    assert!(listing(rig).is_empty(), "got {:?}", rig.last_list);
}

#[then(expr = "the listing names {string} as in use by the holding run")]
async fn listing_in_use(w: &mut BehaviourWorld, name: String) {
    let rig = w.volume();
    let holder = rig.holder_name.clone().expect("a holder sandbox name");
    assert_eq!(listed(rig, &name).in_use_by, vec![holder]);
}

#[then(expr = "the listing names {string} as in use by {string} and {string}")]
async fn listing_in_use_by_both(
    w: &mut BehaviourWorld,
    name: String,
    first: String,
    second: String,
) {
    let rig = w.volume();
    let mut expected = vec![first, second];
    expected.sort();
    assert_eq!(listed(rig, &name).in_use_by, expected);
}

#[then(expr = "the listing names {string} as idle")]
async fn listing_idle(w: &mut BehaviourWorld, name: String) {
    let rig = w.volume();
    assert!(
        listed(rig, &name).in_use_by.is_empty(),
        "expected no holder, got {:?}",
        listed(rig, &name).in_use_by
    );
}

#[then(expr = "the operation succeeds")]
async fn operation_succeeds(w: &mut BehaviourWorld) {
    let rig = w.volume();
    assert!(
        rig.last_error.is_none(),
        "unexpected error: {:?}",
        rig.last_error
    );
}

fn inspected(rig: &VolumeRig, name: &str) -> lns_ipc::VolumeInfo {
    let info = rig.last_inspect.clone().expect("an inspection result");
    assert_eq!(info.name, name);
    info
}

#[then(expr = "the inspection reports {string} as idle")]
async fn inspection_idle(w: &mut BehaviourWorld, name: String) {
    let rig = w.volume();
    assert!(inspected(rig, &name).in_use_by.is_empty());
}

#[then(expr = "the inspection reports {string} as in use by the holding run")]
async fn inspection_in_use(w: &mut BehaviourWorld, name: String) {
    let rig = w.volume();
    let holder = rig.holder_name.clone().expect("a holder sandbox name");
    assert_eq!(inspected(rig, &name).in_use_by, vec![holder]);
}

#[then(expr = "the refusal does not tell the user to remove a sandbox")]
async fn refusal_does_not_name_a_sandbox(w: &mut BehaviourWorld) {
    let err = w.volume().last_error.clone().expect("an error");
    assert!(
        !err.contains("remove the sandbox"),
        "no `lns rm` can clear this claim, so the refusal must not send the user there: {err}"
    );
}

#[then(expr = "the prune reports {string} as failed, naming run {string}")]
async fn prune_reports_failure(w: &mut BehaviourWorld, volume: String, run_id: String) {
    let report = w.volume().last_prune.clone().expect("a prune report");
    let failure = report
        .failed
        .iter()
        .find(|f| f.name == volume)
        .unwrap_or_else(|| panic!("no failure for {volume:?} in {:?}", report.failed));
    assert!(
        failure.error.contains(&run_id) && failure.error.contains("restart the service"),
        "a skipped volume must say why and name the remedy: {}",
        failure.error
    );
}

#[then(expr = "the refusal names the sandbox {string}")]
async fn refusal_names_sandbox(w: &mut BehaviourWorld, sandbox: String) {
    let err = w.volume().last_error.clone().expect("an error");
    assert!(
        err.contains(&sandbox),
        "refusal must name the sandbox to remove first; got: {err}"
    );
}

#[then(expr = "the inspection reports the volume's size and disk usage")]
async fn inspection_size(w: &mut BehaviourWorld) {
    let info = w.volume().last_inspect.clone().expect("an inspection");
    assert_eq!(info.size_bytes, VOLUME_DEFAULT_SIZE_BYTES);
    assert_eq!(info.disk_bytes, crate::volume_rig::FAKE_ALLOCATED_BYTES);
}

#[then(expr = "the request is refused because there is no such volume")]
async fn refused_no_such_volume(w: &mut BehaviourWorld) {
    let err = w.volume().last_error.clone().expect("an error");
    assert!(err.contains("no such volume"), "got: {err}");
}

#[then(expr = "the backing image for {string} is gone from the store")]
async fn image_gone(w: &mut BehaviourWorld, name: String) {
    let rig = w.volume();
    assert!(
        !rig.image_in_store(&name),
        "backing image for {name:?} should be gone"
    );
}

#[then(expr = "the backing image for {string} remains in the store")]
async fn image_remains(w: &mut BehaviourWorld, name: String) {
    let rig = w.volume();
    assert!(
        rig.image_in_store(&name),
        "backing image for {name:?} should remain"
    );
}

#[then(expr = "the prune removes {string} and {string}")]
async fn prune_removes_both(w: &mut BehaviourWorld, a: String, b: String) {
    let report = w.volume().last_prune.clone().expect("a prune report");
    assert!(
        report.removed.contains(&a) && report.removed.contains(&b),
        "got {:?}",
        report.removed
    );
    assert_eq!(report.removed.len(), 2, "got {:?}", report.removed);
}

#[then(expr = "the prune removes only {string}")]
async fn prune_removes_only(w: &mut BehaviourWorld, name: String) {
    let report = w.volume().last_prune.clone().expect("a prune report");
    assert_eq!(report.removed, vec![name]);
}

#[then(expr = "the prune reports the reclaimed space")]
async fn prune_reports_reclaimed(w: &mut BehaviourWorld) {
    let report = w.volume().last_prune.clone().expect("a prune report");
    assert_eq!(
        report.reclaimed_bytes,
        report.removed.len() as u64 * crate::volume_rig::FAKE_ALLOCATED_BYTES
    );
}

#[then(expr = "the prune removes nothing")]
async fn prune_removes_nothing(w: &mut BehaviourWorld) {
    let report = w.volume().last_prune.clone().expect("a prune report");
    assert!(report.removed.is_empty(), "got {:?}", report.removed);
    assert_eq!(report.reclaimed_bytes, 0);
}
