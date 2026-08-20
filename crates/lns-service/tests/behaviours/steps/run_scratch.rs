use cucumber::{given, then, when};

use crate::world::BehaviourWorld;

fn last_exited(w: &mut BehaviourWorld) -> String {
    w.scratch()
        .exited
        .last()
        .expect("an exited run was staged")
        .clone()
}

#[given("a run has exited")]
#[given("an exited run with a scratch dir")]
#[given("a scratch dir whose run is not in the registry")]
async fn an_exited_run(w: &mut BehaviourWorld) {
    w.scratch().add_exited_run();
}

#[given("two exited runs with scratch dirs")]
async fn two_exited_runs(w: &mut BehaviourWorld) {
    w.scratch().add_exited_run();
    w.scratch().add_exited_run();
}

#[given("a running run with a scratch dir")]
async fn a_running_run(w: &mut BehaviourWorld) {
    w.scratch().add_running_run();
}

#[given("that run's audit chain exists in the data dir")]
async fn an_audit_chain(w: &mut BehaviourWorld) {
    let id = last_exited(w);
    w.scratch().add_audit_chain(&id);
}

#[when("the service reclaims the run")]
async fn reclaim_the_run(w: &mut BehaviourWorld) {
    let id = last_exited(w);
    w.scratch().reclaim_upper(&id);
}

#[when("the exited run is removed")]
async fn remove_the_run(w: &mut BehaviourWorld) {
    let id = last_exited(w);
    w.scratch().remove(&id);
}

#[when("the exited runs are pruned")]
async fn prune_the_runs(w: &mut BehaviourWorld) {
    w.scratch().prune_exited();
}

#[when("the startup sweep runs")]
async fn run_the_sweep(w: &mut BehaviourWorld) {
    w.scratch().sweep();
}

#[then("the run's upper.img no longer exists")]
async fn upper_gone(w: &mut BehaviourWorld) {
    let id = last_exited(w);
    let upper = w.scratch().run_dir(&id).join("upper.img");
    assert!(!upper.exists(), "upper.img must be reclaimed: {upper:?}");
}

#[then("the run's console.log still exists")]
async fn console_kept(w: &mut BehaviourWorld) {
    let id = last_exited(w);
    let console = w.scratch().run_dir(&id).join("console.log");
    assert!(
        console.exists(),
        "console.log stays for post-mortem: {console:?}"
    );
}

#[then("the run's scratch dir no longer exists")]
#[then("that scratch dir is removed")]
async fn dir_gone(w: &mut BehaviourWorld) {
    let id = last_exited(w);
    let dir = w.scratch().run_dir(&id);
    assert!(!dir.exists(), "scratch dir must be reclaimed: {dir:?}");
}

#[then("both scratch dirs are gone")]
async fn both_dirs_gone(w: &mut BehaviourWorld) {
    let ids = w.scratch().exited.clone();
    assert_eq!(ids.len(), 2, "the scenario staged two exited runs");
    for id in ids {
        let dir = w.scratch().run_dir(&id);
        assert!(!dir.exists(), "scratch dir must be reclaimed: {dir:?}");
    }
}

#[then("the prune reports the reclaimed bytes")]
async fn prune_reports_bytes(w: &mut BehaviourWorld) {
    let reclaimed = w.scratch().reclaimed.expect("a prune ran");
    assert!(
        reclaimed > 0,
        "reclaimed bytes must be reported, got {reclaimed}"
    );
}

#[then("only the exited run's scratch dir is gone")]
async fn only_exited_gone(w: &mut BehaviourWorld) {
    let exited = last_exited(w);
    let running = w
        .scratch()
        .running
        .last()
        .expect("a running run was staged")
        .clone();
    let exited_dir = w.scratch().run_dir(&exited);
    let running_dir = w.scratch().run_dir(&running);
    assert!(!exited_dir.exists(), "exited scratch must be reclaimed");
    assert!(running_dir.exists(), "a running run keeps its scratch dir");
}

#[then("that run's audit chain still exists")]
async fn audit_kept(w: &mut BehaviourWorld) {
    let id = last_exited(w);
    let audit = w.scratch().audit_path(&id);
    assert!(
        audit.exists(),
        "the audit chain must outlive the scratch dir"
    );
}
