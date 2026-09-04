use std::time::Instant;

use cucumber::{given, then, when};
use lns_policy::{Policy, Verdict};
use lns_service::approval_flow::entries::{
    Entry, EntryKind, EntryState, EntryStore, FileEntryStore,
};
use lns_service::approval_flow::offline;
use lns_service::approval_flow::protocol::{Decision, RequestPending, Treatment};
use lns_service::approval_flow::session::{Answer, AnswerOutcome};

use crate::approval_rig::ApprovalRig;
use crate::world::BehaviourWorld;

const STOPPED_RUN: &str = "aa01";

fn pending(host: &str) -> RequestPending {
    RequestPending {
        id: format!("req-{host}"),
        host: host.to_string(),
        action: format!("CONNECT {host}:443"),
        reason: "policy-ambiguous".into(),
        treatment: Treatment::Inspected,
    }
}

fn state_named(words: &str) -> EntryState {
    match words {
        "undecided" => EntryState::Undecided,
        "withdrawn" => EntryState::Withdrawn,
        "always allowed" => EntryState::AlwaysAllowed,
        "always denied" => EntryState::AlwaysDenied,
        "granted" => EntryState::Granted,
        other => panic!("no entry state is called {other:?}"),
    }
}

fn answer_named(words: &str) -> Answer {
    match words {
        "always allow" => Answer::AlwaysAllow,
        "always deny" => Answer::AlwaysDeny,
        "ask again" => Answer::AskAgain,
        other => panic!("no answer is called {other:?}"),
    }
}

fn assert_state(world: &mut BehaviourWorld, subject: &str, want: EntryState) {
    let rig = world.approval();
    let entry = rig
        .entry_for(subject)
        .unwrap_or_else(|| panic!("nothing was asked about {subject:?}: {:?}", rig.entries()));
    assert_eq!(
        entry.state, want,
        "the run's approvals list {subject:?} as {:?}",
        entry.state
    );
}

/// What the stopped-sandbox scenarios answer against: a run directory with no session behind it.
fn stopped(world: &mut BehaviourWorld) -> &mut crate::world::StoppedRunRig {
    world.stopped_run()
}

#[given(regex = r#"^the sandbox is named "([^"]+)"$"#)]
fn given_sandbox_named(world: &mut BehaviourWorld, name: String) {
    world.approval = Some(ApprovalRig::for_run(Some(name)));
}

#[given(regex = r#"^a workload has an open approval card for "([^"]+)"$"#)]
fn given_open_card_for(world: &mut BehaviourWorld, host: String) {
    let rig = world.approval();
    rig.session.submit_pending(pending(&host), Instant::now());
}

#[given(regex = r#"^the run's approvals list "([^"]+)" as undecided$"#)]
fn given_listed_undecided(world: &mut BehaviourWorld, host: String) {
    let rig = world.approval();
    rig.session.submit_pending(pending(&host), Instant::now());
    rig.session.dismiss_request(&format!("req-{host}"));
    assert_state(world, &host, EntryState::Undecided);
}

#[given(regex = r#"^the run's approvals list "([^"]+)" as always allowed$"#)]
fn given_listed_always_allowed(world: &mut BehaviourWorld, host: String) {
    let rig = world.approval();
    Policy::default()
        .save_atomic(&rig.policy_path)
        .expect("save initial policy");
    rig.session.submit_pending(pending(&host), Instant::now());
    rig.session
        .record_decision(&format!("req-{host}"), Decision::AllowAlways);
    assert_state(world, &host, EntryState::AlwaysAllowed);
}

#[given("the run's approvals hold a notice that the rule could not be persisted")]
#[then("the run's approvals hold a notice that the rule could not be persisted")]
fn notice_is_listed(world: &mut BehaviourWorld) {
    let rig = world.approval();
    if notice_of(rig).is_none() {
        rig.store.break_next_save();
        rig.session
            .submit_pending(pending("api.linear.app"), Instant::now());
        rig.session
            .record_decision("req-api.linear.app", Decision::AllowAlways);
    }
    let rig = world.approval();
    let notice = notice_of(rig).expect("a notice");
    assert!(
        notice.subject().contains("not persisted"),
        "the notice must say the rule did not land, got {:?}",
        notice.subject()
    );
}

fn notice_of(rig: &ApprovalRig) -> Option<Entry> {
    rig.entries()
        .into_iter()
        .find(|entry| matches!(entry.kind, EntryKind::Notice { .. }))
}

#[given(regex = r#"^a stopped sandbox whose approvals list "([^"]+)" as undecided$"#)]
fn given_stopped_sandbox_with_entry(world: &mut BehaviourWorld, host: String) {
    let rig = stopped(world);
    let entry = Entry::new(
        Some(STOPPED_RUN.to_string()),
        EntryKind::Destination {
            destination: host.clone(),
            action: format!("CONNECT {host}:443"),
            raw: false,
        },
        EntryState::Undecided,
    );
    let path = lns_service::cache::approvals_path(rig.home.path(), STOPPED_RUN);
    std::fs::create_dir_all(path.parent().expect("run dir")).expect("run dir");
    FileEntryStore::new(path).record(entry.clone());
    rig.entry_id = Some(entry.id);
}

#[given(regex = r#"^the developer grants the connector "([^"]+)" to the run$"#)]
fn given_connector_granted(world: &mut BehaviourWorld, name: String) {
    let rig = world.approval();
    rig.grant_connector(&name);
}

#[given(regex = r#"^the workload holds two requests to "([^"]+)"$"#)]
fn given_two_requests(world: &mut BehaviourWorld, host: String) {
    let rig = world.approval();
    for nth in ["first", "second"] {
        rig.session.submit_pending(
            RequestPending {
                id: format!("req-{nth}-{host}"),
                host: host.clone(),
                action: format!("CONNECT {host}:443"),
                reason: "policy-ambiguous".into(),
                treatment: Treatment::Inspected,
            },
            Instant::now(),
        );
    }
}

#[when(regex = r#"^the developer picks "([^"]+)" on the first$"#)]
fn when_developer_picks_on_the_first(world: &mut BehaviourWorld, choice: String) {
    assert_eq!(choice, "always allow", "no other verdict is scripted here");
    let rig = world.approval();
    rig.session
        .record_decision("req-first-api.linear.app", Decision::AllowAlways);
}

#[when("the second request times out")]
fn when_second_times_out(world: &mut BehaviourWorld) {
    let rig = world.approval();
    let past_the_deadline = Instant::now() + rig.timeout + std::time::Duration::from_secs(1);
    assert_eq!(
        rig.session.tick_timeouts(past_the_deadline),
        1,
        "the second request is the only one left to sweep"
    );
}

#[when("lns-service restarts")]
fn when_service_restarts(world: &mut BehaviourWorld) {
    world.approval().restart();
}

#[when(regex = r#"^the developer answers "([^"]+)" on that entry$"#)]
fn when_developer_answers(world: &mut BehaviourWorld, words: String) {
    let answer = answer_named(&words);
    if let Some(rig) = world.stopped.as_ref() {
        let id = rig.entry_id.clone().expect("a stopped entry");
        let outcome = offline::answer(rig.home.path(), STOPPED_RUN, &id, answer);
        world.stopped_run().outcome = Some(outcome);
        return;
    }
    let rig = world.approval();
    let id = rig
        .entries()
        .into_iter()
        .find(|entry| entry.is_answerable())
        .expect("an answerable entry")
        .id;
    let outcome = rig.session.answer_entry(&id, answer);
    assert!(
        matches!(outcome, AnswerOutcome::Recorded(_)),
        "the answer must be recorded, got {outcome:?}"
    );
}

#[when("the developer reads that entry")]
fn when_developer_reads(_world: &mut BehaviourWorld) {
    // no-op: reading an entry changes nothing; the assertions that follow say what it offers.
}

#[when("the developer removes the sandbox")]
fn when_sandbox_removed(world: &mut BehaviourWorld) {
    let rig = stopped(world);
    assert!(
        lns_service::run::reclaim_run_dir(
            &lns_service::run::RealRemoveDir,
            rig.home.path(),
            STOPPED_RUN,
        ),
        "the run directory must be reclaimed"
    );
}

#[then(regex = r#"^the run's approvals list "([^"]+)" as (.+)$"#)]
#[then(regex = r#"^the run's approvals still list "([^"]+)" as (.+)$"#)]
fn then_listed_as(world: &mut BehaviourWorld, host: String, words: String) {
    assert_state(world, &host, state_named(&words));
}

#[then(regex = r#"^the run's approvals still list that destination as (.+)$"#)]
fn then_that_destination_listed_as(world: &mut BehaviourWorld, words: String) {
    assert_state(world, "api.linear.app", state_named(&words));
}

#[then(regex = r#"^the entry for "([^"]+)" names the sandbox "([^"]+)"$"#)]
fn then_entry_names_sandbox(world: &mut BehaviourWorld, host: String, sandbox: String) {
    let rig = world.approval();
    let entry = rig.entry_for(&host).expect("an entry");
    assert_eq!(entry.sandbox.as_deref(), Some(sandbox.as_str()));
}

#[then("that notice offers no verdict")]
fn then_notice_offers_no_verdict(world: &mut BehaviourWorld) {
    let rig = world.approval();
    let notice = notice_of(rig).expect("a notice");
    assert!(
        !notice.is_answerable(),
        "a notice asks nothing, so it takes no answer"
    );
}

#[then("that entry offers no verdict")]
fn then_connector_offers_no_verdict(world: &mut BehaviourWorld) {
    let rig = world.approval();
    let connector = rig.entry_for("linear").expect("the connector entry");
    assert!(
        !connector.is_answerable(),
        "a connector is decided through `lns connector`, not here"
    );
}

#[then("the run directory holds no approvals")]
fn then_run_dir_empty(world: &mut BehaviourWorld) {
    let rig = stopped(world);
    assert!(
        offline::list(rig.home.path(), STOPPED_RUN).is_empty(),
        "removing the sandbox takes its approvals with it"
    );
}

#[then(r#"the entry offers "always allow", "always deny", and "ask again""#)]
fn then_entry_offers_the_three(world: &mut BehaviourWorld) {
    let rig = world.approval();
    let id = rig
        .entries()
        .into_iter()
        .find(|entry| entry.is_answerable())
        .expect("an answerable entry")
        .id;
    for (answer, want) in [
        (Answer::AlwaysDeny, EntryState::AlwaysDenied),
        (Answer::AskAgain, EntryState::Undecided),
        (Answer::AlwaysAllow, EntryState::AlwaysAllowed),
    ] {
        assert_eq!(
            rig.session.answer_entry(&id, answer),
            AnswerOutcome::Recorded(want),
            "the entry must take {answer:?}"
        );
    }
}

#[then("the entry offers no once verdict")]
fn then_no_once_verdict(world: &mut BehaviourWorld) {
    let rig = world.approval();
    assert_eq!(
        rig.session
            .record_decision("req-api.linear.app", Decision::AllowOnce),
        lns_service::approval_flow::session::DecisionOutcome::UnknownId,
        "a once verdict answers a held request, and this one has gone"
    );
}

#[then(regex = r#"^"([^"]+)" contains a deny rule for "([^"]+)"$"#)]
fn then_file_holds_deny(world: &mut BehaviourWorld, _file: String, host: String) {
    assert_file_verdict(world, &host, Some(Verdict::Deny));
}

#[then(regex = r#"^"([^"]+)" contains no allow rule for "([^"]+)"$"#)]
fn then_file_holds_no_allow(world: &mut BehaviourWorld, _file: String, host: String) {
    let held = file_verdicts(world, &host);
    assert!(
        !held.contains(&Verdict::Allow),
        "an allow rule for {host:?} must be gone, got {held:?}"
    );
}

#[then(regex = r#"^"([^"]+)" contains no rule for "([^"]+)"$"#)]
fn then_file_holds_nothing(world: &mut BehaviourWorld, _file: String, host: String) {
    assert_file_verdict(world, &host, None);
}

#[then(regex = r#"^the running policy contains no rule for "([^"]+)"$"#)]
fn then_running_policy_holds_nothing(world: &mut BehaviourWorld, host: String) {
    let rig = world.approval();
    let held: Vec<Verdict> = rig
        .session
        .current_policy()
        .network
        .egress
        .http
        .iter()
        .filter(|rule| rule.match_pattern == host)
        .map(|rule| rule.verdict)
        .collect();
    assert!(
        held.is_empty(),
        "the running policy must ask about {host:?} again, got {held:?}"
    );
}

#[then(regex = r#"^that sandbox's "([^"]+)" contains a new allow rule for "([^"]+)"$"#)]
fn then_stopped_file_holds_allow(world: &mut BehaviourWorld, _file: String, host: String) {
    let rig = stopped(world);
    let decided = Policy::load_or_default(&lns_service::cache::decisions_path(
        rig.home.path(),
        STOPPED_RUN,
    ))
    .expect("read the stopped run's decisions");
    let rule = decided
        .network
        .egress
        .http
        .iter()
        .find(|rule| rule.match_pattern == host)
        .unwrap_or_else(|| panic!("no rule for {host:?} in {:?}", decided.network.egress.http));
    assert_eq!(rule.verdict, Verdict::Allow);
}

#[then("the sandbox stays stopped")]
fn then_sandbox_stays_stopped(world: &mut BehaviourWorld) {
    let rig = stopped(world);
    assert!(
        !lns_service::run_registry::is_live(STOPPED_RUN),
        "answering an entry must not boot the sandbox"
    );
    assert!(rig.outcome.is_some(), "the answer was recorded");
}

#[then(regex = r#"^the run's approvals list the connector "([^"]+)" as granted$"#)]
fn then_connector_listed(world: &mut BehaviourWorld, name: String) {
    let rig = world.approval();
    let entry = rig
        .entry_for(&name)
        .unwrap_or_else(|| panic!("the connector {name:?} is not listed: {:?}", rig.entries()));
    assert_eq!(entry.state, EntryState::Granted);
    assert!(!entry.is_answerable());
}

fn file_verdicts(world: &mut BehaviourWorld, host: &str) -> Vec<Verdict> {
    let rig = world.approval();
    Policy::load_or_default(&rig.policy_path)
        .expect("read the decisions file")
        .network
        .egress
        .http
        .iter()
        .filter(|rule| rule.match_pattern == host)
        .map(|rule| rule.verdict)
        .collect()
}

fn assert_file_verdict(world: &mut BehaviourWorld, host: &str, want: Option<Verdict>) {
    let held = file_verdicts(world, host);
    match want {
        Some(verdict) => assert_eq!(
            held,
            vec![verdict],
            "the file must hold one {verdict:?} rule"
        ),
        None => assert!(held.is_empty(), "the file must hold no rule, got {held:?}"),
    }
}
