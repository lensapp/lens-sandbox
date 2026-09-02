use cucumber::{given, then, when};

use crate::world::BehaviourWorld;

#[given(expr = "no volume named {string} exists in the store")]
async fn no_volume(_w: &mut BehaviourWorld, _name: String) {
    // no-op: a fresh rig starts with an empty store
}

#[given(expr = "volume {string} already exists in the store")]
async fn volume_exists(w: &mut BehaviourWorld, name: String) {
    w.volume().preexisting_image(&name);
}

#[given(expr = "a live run holds volume {string}")]
async fn live_run_holds(w: &mut BehaviourWorld, name: String) {
    w.volume().hold(&name).await;
}

#[given(expr = "the live sandbox {string} holds volume {string}")]
async fn live_sandbox_holds(w: &mut BehaviourWorld, sandbox: String, name: String) {
    w.volume().hold_as(&sandbox, &name).await;
}

#[given(expr = "the stopped sandbox {string} declares volume {string}")]
async fn stopped_sandbox_declares(w: &mut BehaviourWorld, sandbox: String, name: String) {
    w.volume().declare(&sandbox, &name);
}

#[given(expr = "run {string} declares volume {string} in a record this build will not run")]
async fn damaged_run_declares(w: &mut BehaviourWorld, run_id: String, name: String) {
    w.volume().damaged_record_declaring(&run_id, &name);
}

#[given(expr = "the record of run {string} cannot be read")]
async fn unreadable_record(w: &mut BehaviourWorld, run_id: String) {
    w.volume().unreadable_record(&run_id);
}

#[given(expr = "the sandbox {string} is removed")]
async fn sandbox_removed(w: &mut BehaviourWorld, sandbox: String) {
    w.volume().forget_sandbox(&sandbox);
}

#[given(expr = "a run held volume {string} and has since ended")]
async fn held_then_ended(w: &mut BehaviourWorld, name: String) {
    let rig = w.volume();
    rig.hold(&name).await;
    rig.release_held();
}

#[when(expr = "a run requests volume {string} at {string}")]
async fn request(w: &mut BehaviourWorld, name: String, target: String) {
    w.volume().request(&name, &target, false).await;
}

#[when(expr = "a run requests volume {string} at {string} read-only")]
async fn request_ro(w: &mut BehaviourWorld, name: String, target: String) {
    w.volume().request(&name, &target, true).await;
}

#[when(expr = "a run requests volume {string} at both {string} and {string}")]
async fn request_two_paths(w: &mut BehaviourWorld, name: String, a: String, b: String) {
    w.volume().request_paths(&name, &[&a, &b], false).await;
}

#[given(expr = "volume {string} already exists in the store holding {word}")]
async fn volume_exists_sized(w: &mut BehaviourWorld, name: String, size: String) {
    let bytes = gib(&size);
    w.volume().preexisting_image_sized(&name, bytes);
}

#[when(expr = "a run requests volume {string} at {string} sized {word}")]
async fn request_sized(w: &mut BehaviourWorld, name: String, target: String, size: String) {
    let bytes = gib(&size);
    w.volume().request_sized(&name, &target, bytes).await;
}

#[when(
    expr = "a run requests volume {string} at {string} sized {word} and at {string} sized {word}"
)]
async fn request_two_sizes(
    w: &mut BehaviourWorld,
    name: String,
    a: String,
    a_size: String,
    b: String,
    b_size: String,
) {
    let (first, second) = (gib(&a_size), gib(&b_size));
    w.volume()
        .request_mounts(&[
            (&name, &a, false, Some(first)),
            (&name, &b, false, Some(second)),
        ])
        .await;
}

#[then(expr = "the backing image for {string} holds {word}")]
async fn image_holds(w: &mut BehaviourWorld, name: String, size: String) {
    let want = gib(&size);
    let got = w.volume().image_size(&name);
    assert_eq!(
        got,
        Some(want),
        "volume {name} should hold {size}, not {got:?} bytes"
    );
}

fn gib(spec: &str) -> u64 {
    let digits = spec.trim_end_matches("Gi");
    digits.parse::<u64>().expect("a size in GiB") << 30
}

#[when(expr = "a volume {string} at {string} is recorded in the audit chain")]
async fn record_audit(w: &mut BehaviourWorld, name: String, target: String) {
    w.volume().record_attach(&name, &target);
}

#[then(expr = "a backing image for {string} is created in the store")]
async fn image_created(w: &mut BehaviourWorld, name: String) {
    let rig = w.volume();
    let want = rig.image_path(&name);
    assert!(
        rig.fs.created().contains(&want),
        "expected {} to be created; created={:?}",
        want.display(),
        rig.fs.created()
    );
}

#[then(expr = "no backing image is created")]
async fn no_image_created(w: &mut BehaviourWorld) {
    let rig = w.volume();
    assert!(
        rig.fs.created().is_empty(),
        "no image should have been created; created={:?}",
        rig.fs.created()
    );
}

#[then(expr = "the spec attaches {string} at {string}")]
async fn spec_attaches(w: &mut BehaviourWorld, name: String, target: String) {
    let rig = w.volume();
    let att = rig
        .attachment(&name)
        .unwrap_or_else(|| panic!("no attachment for {name}; have {:?}", rig.last_attachments));
    assert_eq!(att.target, target);
}

#[then(expr = "the spec attaches {string} at both {string} and {string}")]
async fn spec_attaches_both(w: &mut BehaviourWorld, name: String, a: String, b: String) {
    let targets = w.volume().attachment_targets(&name);
    assert!(
        targets.contains(&a) && targets.contains(&b),
        "got {targets:?}"
    );
}

#[then(expr = "the backing image for {string} is created exactly once")]
async fn image_created_once(w: &mut BehaviourWorld, name: String) {
    assert_eq!(w.volume().created_count(&name), 1);
}

#[then(expr = "that attachment is writable")]
async fn attachment_writable(w: &mut BehaviourWorld) {
    let rig = w.volume();
    let att = rig.last_attachments.first().expect("an attachment");
    assert!(!att.read_only, "attachment must be writable");
}

#[then(expr = "the spec marks the {string} attachment read-only")]
async fn spec_marks_ro(w: &mut BehaviourWorld, name: String) {
    let rig = w.volume();
    let att = rig.attachment(&name).expect("attachment present");
    assert!(att.read_only, "attachment must be read-only");
}

#[then(expr = "the request is refused")]
async fn refused(w: &mut BehaviourWorld) {
    assert!(
        w.volume().last_error.is_some(),
        "expected the request to be refused"
    );
}

#[then(expr = "the refusal tells the user to repair run {string} and restart the service")]
async fn refusal_names_repair(w: &mut BehaviourWorld, run_id: String) {
    let err = w.volume().last_error.clone().expect("an error");
    assert!(err.contains(&run_id), "refusal must name the run: {err}");
    assert!(
        err.contains("restart the service"),
        "a repaired record is only re-read at boot, so the refusal must say so: {err}"
    );
}

#[then(expr = "the request is refused because the volume is in use")]
async fn refused_in_use(w: &mut BehaviourWorld) {
    let err = w.volume().last_error.clone().expect("an error");
    assert!(err.contains("in use by"), "got: {err}");
}

#[then(expr = "the first run's hold on {string} is unaffected")]
async fn hold_unaffected(w: &mut BehaviourWorld, name: String) {
    let rig = w.volume();
    let holder = rig.holder_run_id.clone().expect("a holder run id");
    let again = lns_service::volume_store::acquire_with(
        &rig.fs,
        &rig.registry,
        &rig.store_root,
        &name,
        "deadbeef00000000000000000000aa99",
        lns_service::volume_store::VOLUME_DEFAULT_SIZE_BYTES,
    )
    .await;
    assert!(
        again.is_err(),
        "the original hold must still be in force, but a fresh acquire succeeded"
    );
    let _ = holder;
}

#[then(expr = "the request succeeds")]
async fn request_succeeds(w: &mut BehaviourWorld) {
    let rig = w.volume();
    assert!(
        rig.last_error.is_none(),
        "unexpected error: {:?}",
        rig.last_error
    );
    assert!(!rig.last_attachments.is_empty(), "expected an attachment");
}

#[then(expr = "the request is refused with a volume-name validation error")]
async fn refused_validation(w: &mut BehaviourWorld) {
    let err = w.volume().last_error.clone().expect("an error");
    assert!(err.contains("invalid volume name"), "got: {err}");
}

#[then(expr = "no path outside the store is touched")]
async fn no_path_outside(w: &mut BehaviourWorld) {
    let outside = w.volume().touched_outside_store();
    assert!(
        outside.is_empty(),
        "touched paths outside the store: {outside:?}"
    );
}

#[then(expr = "the audit chain records the volume name {string} and target {string}")]
async fn audit_records(w: &mut BehaviourWorld, name: String, target: String) {
    let content = w.volume().audit_contents();
    assert!(content.contains("\"class_uid\":1001"), "OCSF: {content}");
    assert!(
        content.contains(&format!("\"lns_name\":\"{name}\"")),
        "{content}"
    );
    assert!(
        content.contains(&format!("\"lns_target\":\"{target}\"")),
        "{content}"
    );
}
