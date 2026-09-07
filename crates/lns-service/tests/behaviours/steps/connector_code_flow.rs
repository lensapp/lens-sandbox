use cucumber::{given, then};

use super::connector_code::rig;
use crate::world::BehaviourWorld;

#[given(regex = r#"^its component asks for the fields "([^"]+)" and "([^"]+)"$"#)]
fn its_component_asks_for_the_fields(w: &mut BehaviourWorld, first: String, second: String) {
    rig(w).its_component_is("picking");
    rig(w).labels_the_scenario_names = vec![first, second];
}

// no-op: which field this component marks secret is part of the same ask, and the Then reads it.
#[given(regex = r#"^its component marks "([^"]+)" secret and "([^"]+)" not$"#)]
fn its_component_marks_one_secret(_w: &mut BehaviourWorld, _secret: String, _plain: String) {}

#[given(regex = r#"^its component shows the user something and waits$"#)]
fn its_component_shows_something_and_waits(w: &mut BehaviourWorld) {
    rig(w).its_component_is("showing");
}

#[given(regex = r#"^its component returns done from what it was handed$"#)]
fn its_component_returns_done(w: &mut BehaviourWorld) {
    rig(w).its_component_is("granting");
}

#[then(regex = r#"^the card asks for exactly those two fields, in that order$"#)]
fn the_card_asks_for_exactly_those_fields(w: &mut BehaviourWorld) {
    let labels: Vec<String> = rig(w)
        .asked_first()
        .into_iter()
        .map(|field| field.label)
        .collect();
    assert_eq!(
        labels,
        rig(w).labels_the_scenario_names,
        "lns renders what the component asked for, in the order it asked"
    );
}

#[then(regex = r#"^"([^"]+)" is marked secret and "([^"]+)" is not$"#)]
fn one_field_is_secret_and_the_other_is_not(w: &mut BehaviourWorld, secret: String, plain: String) {
    let asked = rig(w).asked_first();
    let secrecy = |label: &str| {
        asked
            .iter()
            .find(|field| field.label == label)
            .unwrap_or_else(|| panic!("no field labelled {label}, got {asked:?}"))
            .secret
    };
    assert!(secrecy(&secret), "{secret} is never echoed");
    assert!(!secrecy(&plain), "{plain} is not secret, so it is shown");
}

#[then(regex = r#"^the card shows what the component asked to display$"#)]
fn the_card_shows_the_message(w: &mut BehaviourWorld) {
    let shown = rig(w).message_shown();
    assert!(
        shown.contains("auth.some-provider.example/device") && shown.contains("WDJB-MJHT"),
        "a device code is the whole of the round, so the card has to carry it: {shown}"
    );
}

#[then(regex = r#"^lns resumes the component only once the user has pressed on$"#)]
fn lns_resumes_only_once_the_user_pressed_on(w: &mut BehaviourWorld) {
    let name = rig(w).last_declared_name().expect("a connector");
    assert!(
        rig(w).connections_of(&name).is_empty(),
        "the component is still waiting, so nothing is connected yet"
    );
    rig(w).answer("");
    assert!(
        rig(w).is_connected() && !rig(w).connections_of(&name).is_empty(),
        "pressing on is what resumes it, and the component then finishes"
    );
}

#[then(regex = r#"^the connection is made$"#)]
fn the_connection_is_made(w: &mut BehaviourWorld) {
    let name = rig(w).last_declared_name().expect("a connector");
    assert_eq!(rig(w).error, None, "nothing refused this connect");
    assert!(
        rig(w).is_connected() && !rig(w).connections_of(&name).is_empty(),
        "a component that is done leaves a connection behind"
    );
}

#[then(regex = r#"^no field is asked for$"#)]
fn no_field_is_asked_for(w: &mut BehaviourWorld) {
    assert!(
        rig(w).fields_asked_for().is_empty(),
        "this component collects nothing, so the card asks for nothing"
    );
}
