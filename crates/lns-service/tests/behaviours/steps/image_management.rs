use cucumber::{given, then, when};

use crate::image_rig::ImageRig;
use crate::world::BehaviourWorld;

#[given(expr = "image {string} is cached with layer {string} of {int} bytes")]
async fn image_cached(w: &mut BehaviourWorld, reference: String, digest: String, size: u64) {
    w.image().seed_layer(&reference, &digest, size).await;
}

#[given(expr = "image {string} also has layer {string} of {int} bytes")]
async fn image_extra_layer(w: &mut BehaviourWorld, reference: String, digest: String, size: u64) {
    w.image().seed_layer(&reference, &digest, size).await;
}

#[given(expr = "a live run uses image {string}")]
fn live_run_uses(w: &mut BehaviourWorld, reference: String) {
    w.image().hold(&reference);
}

#[given(expr = "the image index cannot be written")]
fn index_cannot_be_written(w: &mut BehaviourWorld) {
    w.image().fail_index_writes();
}

#[when(expr = "image {string} with layer {string} of {int} bytes is pulled")]
async fn image_pulled(w: &mut BehaviourWorld, reference: String, digest: String, size: u64) {
    w.image().pull(&reference, &digest, size).await;
}

#[when(expr = "the images are listed")]
async fn list_images(w: &mut BehaviourWorld) {
    w.image().list().await;
}

#[when(expr = "image {string} is removed")]
async fn remove_image(w: &mut BehaviourWorld, reference: String) {
    w.image().remove(&reference).await;
}

#[when(expr = "the images are pruned")]
async fn prune_images(w: &mut BehaviourWorld) {
    w.image().prune().await;
}

fn listing(rig: &ImageRig) -> &[lns_ipc::ImageInfo] {
    rig.last_list.as_deref().expect("a listing was taken")
}

fn listed<'a>(rig: &'a ImageRig, reference: &str) -> &'a lns_ipc::ImageInfo {
    listing(rig)
        .iter()
        .find(|i| i.reference == reference)
        .unwrap_or_else(|| panic!("image {reference:?} not in listing {:?}", rig.last_list))
}

#[then(expr = "the image listing is empty")]
fn listing_empty(w: &mut BehaviourWorld) {
    let rig = w.image();
    assert!(listing(rig).is_empty(), "got {:?}", rig.last_list);
}

#[then(expr = "the image listing reports {string} at {int} bytes across {int} layers")]
fn listing_reports(w: &mut BehaviourWorld, reference: String, size: u64, layers: u32) {
    let rig = w.image();
    let info = listed(rig, &reference);
    assert_eq!(info.size_bytes, size);
    assert_eq!(info.layers, layers);
    assert!(
        info.digest.starts_with("sha256:"),
        "got digest {:?}",
        info.digest
    );
}

#[then(expr = "the image listing names {string} as in use by the holding run")]
fn listing_in_use(w: &mut BehaviourWorld, reference: String) {
    let rig = w.image();
    let holder = rig.holder_run_id.clone().expect("a holder run id");
    assert_eq!(listed(rig, &reference).in_use_by, Some(holder));
}

#[then(expr = "the image listing names {string} as idle")]
fn listing_idle(w: &mut BehaviourWorld, reference: String) {
    let rig = w.image();
    assert_eq!(listed(rig, &reference).in_use_by, None);
}

#[then(expr = "the pull succeeds reporting {string} at {int} bytes")]
fn pull_succeeds(w: &mut BehaviourWorld, reference: String, size: u64) {
    let rig = w.image();
    assert!(
        rig.last_error.is_none(),
        "unexpected pull error: {:?}",
        rig.last_error
    );
    let info = rig.last_pull.as_ref().expect("a pull result");
    assert_eq!(info.reference, reference);
    assert_eq!(info.size_bytes, size);
}

#[then(expr = "the pull is refused because the index could not be written")]
fn pull_refused_index_write(w: &mut BehaviourWorld) {
    let err = w.image().last_error.clone().expect("a pull error");
    assert!(err.contains("writing image record"), "got: {err}");
    assert!(
        w.image().last_pull.is_none(),
        "a failed pull must not report a result"
    );
}

#[then(expr = "the image record for {string} is gone from the cache")]
async fn record_gone(w: &mut BehaviourWorld, reference: String) {
    let rig = w.image();
    assert!(
        !rig.record_in_index(&reference).await,
        "record for {reference:?} should be gone"
    );
}

#[then(expr = "the image record for {string} remains in the cache")]
async fn record_remains(w: &mut BehaviourWorld, reference: String) {
    let rig = w.image();
    assert!(
        rig.record_in_index(&reference).await,
        "record for {reference:?} should remain"
    );
}

#[then(expr = "layer {string} is gone from the layer cache")]
fn layer_gone(w: &mut BehaviourWorld, digest: String) {
    assert!(
        !w.image().caches.has_layer(&digest),
        "layer {digest:?} should be gone"
    );
}

#[then(expr = "layer {string} remains in the layer cache")]
fn layer_remains(w: &mut BehaviourWorld, digest: String) {
    assert!(
        w.image().caches.has_layer(&digest),
        "layer {digest:?} should remain"
    );
}

#[then(expr = "the removal reports {int} reclaimed bytes")]
fn removal_reclaimed(w: &mut BehaviourWorld, bytes: u64) {
    let removed = w.image().last_removed.clone().expect("a removal result");
    assert_eq!(removed.reclaimed_bytes, bytes);
}

#[then(expr = "the request is refused because the image is in use")]
fn refused_in_use(w: &mut BehaviourWorld) {
    let err = w.image().last_error.clone().expect("an error");
    assert!(err.contains("in use by run "), "got: {err}");
}

#[then(expr = "the request is refused because there is no such image")]
fn refused_no_such_image(w: &mut BehaviourWorld) {
    let err = w.image().last_error.clone().expect("an error");
    assert!(err.contains("no such image"), "got: {err}");
}

#[then(expr = "the prune removes only image {string}")]
fn prune_removes_only(w: &mut BehaviourWorld, reference: String) {
    let report = w.image().last_prune.clone().expect("a prune report");
    assert_eq!(report.removed, vec![reference]);
}

#[then(expr = "the image prune reports {int} reclaimed bytes")]
fn prune_reclaimed(w: &mut BehaviourWorld, bytes: u64) {
    let report = w.image().last_prune.clone().expect("a prune report");
    assert_eq!(report.reclaimed_bytes, bytes);
}

#[then(expr = "the prune removes no images")]
fn prune_removes_nothing(w: &mut BehaviourWorld) {
    let report = w.image().last_prune.clone().expect("a prune report");
    assert!(report.removed.is_empty(), "got {:?}", report.removed);
    assert_eq!(report.reclaimed_bytes, 0);
}
