use cucumber::{given, then, when};

use crate::approval_rig::Scripted;
use crate::world::BehaviourWorld;
use lns_service::approval_flow::session::{ConnectAsk, DecisionOutcome, PendingPrompt};

const CARD: &str = "req-api.some-provider.example";

fn script(w: &mut BehaviourWorld, step: Scripted) {
    w.approval().rounds.push(step);
}

/// The card as it was last drawn, which is the only place a round is visible from outside.
fn drawn(w: &mut BehaviourWorld) -> PendingPrompt {
    w.approval()
        .notifier
        .presented
        .lock()
        .expect("presented lock")
        .last()
        .cloned()
        .expect("the card was drawn at least once")
}

fn round(w: &mut BehaviourWorld) -> ConnectAsk {
    drawn(w).connect.expect("the card carries a round")
}

#[given(regex = r#"^a run holds an offer for "([^"]+)" serving "([^"]+)"$"#)]
fn a_run_holds_an_offer(w: &mut BehaviourWorld, name: String, host: String) {
    w.approval().offer_connector(&name, &host);
}

#[given(regex = r#"^the workload reaches "([^"]+)"$"#)]
fn the_workload_reaches(w: &mut BehaviourWorld, host: String) {
    w.approval().reach(&host);
}

#[given(regex = r#"^the mechanism (?:then )?asks for "([^"]+)" saying "([^"]+)"$"#)]
fn the_mechanism_asks_for(w: &mut BehaviourWorld, field: String, message: String) {
    script(
        w,
        Scripted::Asks {
            message,
            fields: vec![field],
        },
    );
}

#[given(regex = r#"^the mechanism (?:then )?asks for nothing saying "([^"]+)"$"#)]
fn the_mechanism_asks_for_nothing(w: &mut BehaviourWorld, message: String) {
    script(
        w,
        Scripted::Asks {
            message,
            fields: Vec::new(),
        },
    );
}

#[given(regex = r#"^the mechanism then finishes the connection "([^"]+)"$"#)]
fn the_mechanism_finishes(w: &mut BehaviourWorld, connection: String) {
    script(w, Scripted::Connected(connection));
}

#[given(regex = r#"^the mechanism then fails saying "([^"]+)"$"#)]
fn the_mechanism_fails(w: &mut BehaviourWorld, reason: String) {
    script(w, Scripted::Failed(reason));
}

#[given(regex = r#"^the workload also reaches "([^"]+)"$"#)]
fn the_workload_also_reaches(w: &mut BehaviourWorld, host: String) {
    w.approval().reach(&host);
}

#[when(regex = r#"^the developer declines the connector on the other card$"#)]
fn the_developer_declines_on_the_other_card(w: &mut BehaviourWorld) {
    assert_eq!(
        w.approval()
            .session
            .decline_offer("req-gist.some-provider.example"),
        DecisionOutcome::Resolved
    );
}

#[given(regex = r#"^the round runs out before it is answered$"#)]
fn the_round_runs_out(w: &mut BehaviourWorld) {
    w.approval().rounds.run_out_next();
}

#[when(regex = r#"^the developer starts connecting with "([^"]+)"$"#)]
fn the_developer_starts_connecting(w: &mut BehaviourWorld, method: String) {
    assert_eq!(
        w.approval().session.begin_connect(CARD, &method, &method),
        DecisionOutcome::Resolved
    );
}

#[when(regex = r#"^the developer answers the round with "([^"]+)" as "([^"]+)"$"#)]
fn the_developer_answers_with(w: &mut BehaviourWorld, field: String, value: String) {
    let values = lns_ipc::SecretValues(std::collections::BTreeMap::from([(field, value)]));
    assert_eq!(
        w.approval().session.answer_connect(CARD, values),
        DecisionOutcome::Resolved
    );
}

#[when(regex = r#"^the developer answers the round with nothing$"#)]
fn the_developer_answers_with_nothing(w: &mut BehaviourWorld) {
    assert_eq!(
        w.approval()
            .session
            .answer_connect(CARD, lns_ipc::SecretValues::default()),
        DecisionOutcome::Resolved
    );
}

#[then(regex = r#"^the card shows the round asking for "([^"]+)"$"#)]
fn the_card_shows_the_round(w: &mut BehaviourWorld, field: String) {
    let ask = round(w);
    assert_eq!(
        ask.fields.iter().map(|f| &f.name).collect::<Vec<_>>(),
        [&field],
        "the card asks for what the mechanism asked for, not what the document declares"
    );
}

#[then(regex = r#"^the card shows a round with no fields$"#)]
fn the_card_shows_an_empty_round(w: &mut BehaviourWorld) {
    assert!(
        round(w).fields.is_empty(),
        "a round may show something and wait for the developer to press on"
    );
}

#[then(regex = r#"^the card attributes "([^"]+)" to "([^"]+)"$"#)]
fn the_card_attributes(w: &mut BehaviourWorld, message: String, connector: String) {
    let ask = round(w);
    assert_eq!(ask.message, message);
    assert_eq!(ask.connector, connector);
    assert!(
        ask.from_code,
        "these words arrived after every check the document got, so the card says whose they are"
    );
}

#[then(regex = r#"^the developer is told "([^"]+)"$"#)]
fn the_developer_is_told(w: &mut BehaviourWorld, message: String) {
    let informed = w
        .approval()
        .notifier
        .informed
        .lock()
        .expect("informed lock");
    assert!(
        informed.iter().any(|said| said.contains(&message)),
        "expected the card to say {message:?}, got {informed:?}"
    );
}

#[then(regex = r#"^the card carries no round$"#)]
fn the_card_carries_no_round(w: &mut BehaviourWorld) {
    assert!(
        drawn(w).connect.is_none(),
        "a sign-in that ended leaves the card asking what it asked before"
    );
}

#[then(regex = r#"^the mechanism was given "([^"]+)" for "([^"]+)"$"#)]
fn the_mechanism_was_given(w: &mut BehaviourWorld, value: String, field: String) {
    let answered = w.approval().rounds.answered.lock().expect("answered lock");
    assert_eq!(
        answered.last().and_then(|round| round.get(&field)),
        Some(&value)
    );
}

#[then(regex = r#"^the mechanism's round was abandoned$"#)]
fn the_round_was_abandoned(w: &mut BehaviourWorld) {
    assert_eq!(
        *w.approval()
            .rounds
            .abandoned
            .lock()
            .expect("abandoned lock"),
        ["round-1".to_string()],
        "a round nobody will answer is dropped rather than left holding what was typed"
    );
}

#[then(regex = r#"^the run holds a grant of "([^"]+)" through the connection "([^"]+)"$"#)]
fn the_run_holds_a_grant_through(w: &mut BehaviourWorld, name: String, connection: String) {
    let entry = w
        .approval()
        .entry_for(&name)
        .expect("the run keeps what it decided");
    assert_eq!(
        entry.state,
        lns_service::approval_flow::entries::EntryState::Granted
    );
    assert_eq!(
        w.approval()
            .granting
            .granted_through
            .lock()
            .expect("granted lock")
            .last(),
        Some(&Some(connection)),
        "a grant naming no connection injects no credential, so the sign-in would have been for nothing"
    );
}

#[then(regex = r#"^the run holds no grant of "([^"]+)"$"#)]
fn the_run_holds_no_grant(w: &mut BehaviourWorld, name: String) {
    let state = w.approval().entry_for(&name).map(|entry| entry.state);
    assert_ne!(
        state,
        Some(lns_service::approval_flow::entries::EntryState::Granted),
        "a sign-in that did not finish grants nothing"
    );
}

#[then(regex = r#"^the card is gone$"#)]
fn the_card_is_gone(w: &mut BehaviourWorld) {
    assert!(
        w.approval().session.offer_named("some-provider").is_none(),
        "a granted offer is dropped, so the destination it held is no longer a question"
    );
}

#[then(regex = r#"^the workload's request is still held$"#)]
fn the_request_is_still_held(w: &mut BehaviourWorld) {
    assert!(
        w.approval().session.offer_named("some-provider").is_some(),
        "a sign-in that failed leaves the offer standing"
    );
}
