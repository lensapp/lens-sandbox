use cucumber::then;

use super::connector_code::rig;
use crate::world::BehaviourWorld;

#[then(regex = r#"^the durable ledger records the call, naming "([^"]+)" and that host$"#)]
fn the_ledger_records_the_call(w: &mut BehaviourWorld, connector: String) {
    let row = one_row(w);
    assert!(
        row.detail.contains(&connector) && row.detail.contains("auth.some-provider.example"),
        "the entry names who reached where: {}",
        row.detail
    );
    assert!(row.detail.contains("reached"), "got: {}", row.detail);
}

#[then(regex = r#"^no run's audit chain records it$"#)]
fn no_runs_chain_records_it(w: &mut BehaviourWorld) {
    assert_eq!(
        one_row(w).run,
        "",
        "a component reaches a host on the machine's behalf, so no run could account for it (§7.1)"
    );
}

#[then(regex = r#"^the durable ledger records the call as refused$"#)]
fn the_ledger_records_the_call_as_refused(w: &mut BehaviourWorld) {
    let row = one_row(w);
    assert!(
        row.detail.contains("refused") && row.detail.contains("other.some-provider.example"),
        "a bound that was crossed is written down as crossed: {}",
        row.detail
    );
}

#[then(regex = r#"^the durable ledger records the program it started$"#)]
fn the_ledger_records_the_program(w: &mut BehaviourWorld) {
    let row = one_row(w);
    assert!(
        row.detail.contains("ran") && row.detail.contains("claude"),
        "the entry names the program, because nothing else will ever record it: {}",
        row.detail
    );
}

#[then(regex = r#"^the durable ledger records the renewal$"#)]
fn the_ledger_records_the_renewal(w: &mut BehaviourWorld) {
    let renewals = rows(w)
        .into_iter()
        .filter(|row| row.detail.contains("renewed"))
        .collect::<Vec<_>>();
    assert_eq!(
        renewals.len(),
        1,
        "a renewal nobody watched is invisible unless it is written down, got {:?}",
        rows(w)
    );
    assert!(
        renewals[0].detail.contains("some-provider"),
        "got: {}",
        renewals[0].detail
    );
}

#[then(regex = r#"^the entry names the schedule rather than a user action$"#)]
fn the_entry_names_the_schedule(w: &mut BehaviourWorld) {
    let renewal = rows(w)
        .into_iter()
        .find(|row| row.detail.contains("renewed"))
        .expect("a renewal was recorded");
    assert_eq!(
        renewal.run, "",
        "lns renewed it on its own schedule, so no run and no press is behind the entry"
    );
}

#[then(regex = r#"^the durable ledger records nothing about the connect$"#)]
fn the_ledger_records_nothing_about_the_connect(w: &mut BehaviourWorld) {
    let all = rows(w);
    assert!(
        all.is_empty(),
        "a connection is the machine's standing state, not a decision any chain accounts for (cli-spec §3.6), got {all:?}"
    );
}

fn rows(w: &mut BehaviourWorld) -> Vec<lns_audit::Row> {
    rig(w).lent.ledger()
}

fn one_row(w: &mut BehaviourWorld) -> lns_audit::Row {
    let all = rows(w);
    assert_eq!(all.len(), 1, "one call leaves one entry, got {all:?}");
    all.into_iter().next().expect("one row")
}
