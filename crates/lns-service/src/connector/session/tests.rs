use super::*;

fn asked() -> Session {
    Session {
        connector: "some-provider".to_string(),
        digest: "sha256:abc".to_string(),
        method: "sign-in".to_string(),
        label: "work".to_string(),
        state: b"a device code".to_vec(),
        expires_at_millis: 900_000,
    }
}

#[test]
fn a_debug_of_a_session_cannot_print_the_state_it_holds() {
    // `log::debug!` renders one of these on any path that logs a connect, and a device code is secret material (§3.2.6).
    let rendered = format!("{:?}", asked());

    assert!(!rendered.contains("a device code"), "{rendered}");
    assert!(rendered.contains("redacted"), "{rendered}");
    assert!(
        rendered.contains("some-provider") && rendered.contains("sign-in"),
        "everything that is not secret still has to be diagnosable: {rendered}"
    );
}

#[test]
fn a_connect_nobody_came_back_for_does_not_keep_its_secrets_forever() {
    // Nothing ever calls `take` on an abandoned session, so opening the next one is where it goes (§3.2.6).
    let sessions = InMemorySessions::default();
    let abandoned = sessions.open(asked(), 0);

    sessions.open(asked(), 900_001);

    assert_eq!(
        sessions.open.lock().expect("open lock").len(),
        1,
        "the one still open is the one just opened"
    );
    assert!(sessions.take(&abandoned, 900_001).is_none());
}

#[test]
fn a_sweep_drops_what_nobody_came_back_for_without_waiting_for_the_next_connect() {
    // A tray-resident service may not see another connect for days, and an abandoned one holds a device code.
    let sessions = InMemorySessions::default();
    let abandoned = sessions.open(asked(), 0);

    sessions.sweep(901_000);

    assert!(sessions.open.lock().expect("open lock").is_empty());
    assert!(sessions.take(&abandoned, 901_000).is_none());
}

#[test]
fn a_handle_is_unique_to_the_connect_it_opened() {
    let sessions = InMemorySessions::default();

    let first = sessions.open(asked(), 0);
    let second = sessions.open(asked(), 0);

    assert_ne!(
        first, second,
        "two connects of the same method are two exchanges, and one handle answers one of them"
    );
}
