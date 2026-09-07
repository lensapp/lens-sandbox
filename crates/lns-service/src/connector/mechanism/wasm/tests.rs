use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::*;
use crate::connector::mechanism::Bounds;
use crate::connector::mechanism::tests::Parts;

/// Built from `tests/fixtures/mechanism` and committed beside it, so this suite needs no wasm toolchain.
fn fixture(name: &str) -> Vec<u8> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/");
    std::fs::read(format!("{path}{name}.wasm")).expect("a committed fixture component")
}

fn compiled(name: &str) -> (Runtime, Component) {
    let runtime = Runtime::new().expect("the component runtime starts");
    let component = runtime
        .compile(&fixture(name))
        .expect("a fixture component compiles");
    (runtime, component)
}

fn reaching(hosts: &[&str]) -> Bounds {
    Bounds {
        hosts: hosts.iter().map(|h| (*h).to_string()).collect(),
        call_seconds: 30,
        session_seconds: 900,
        exec: false,
    }
}

#[test]
fn a_component_reaches_a_host_its_method_declares_and_gets_what_came_back() {
    let (_runtime, component) = compiled("fetching");
    let parts = Parts::new();

    let step = component
        .connect(&parts.host(reaching(&["auth.some-provider.example"])), 0)
        .expect("the component answers");

    let Step::Done(outcome) = step else {
        panic!("a reachable host finishes the connect, not {step:?}");
    };
    assert_eq!(
        outcome.values.get("access_token").map(String::as_str),
        Some("ok")
    );
    assert_eq!(
        outcome.authority,
        std::collections::BTreeSet::from(["read".to_string()])
    );
}

#[test]
fn a_call_beginning_gives_the_component_the_whole_of_what_one_call_may_write_down() {
    // The ceiling is per call, and a `Host` that carried a spent counter into the next one would silently record nothing of it.
    let (_runtime, component) = compiled("fetching");
    let parts = Parts::new();
    let host = parts.host(reaching(&["auth.some-provider.example"]));
    for _ in 0..crate::connector::mechanism::host::MAX_RECORDED_ENTRIES_PER_CALL + 1 {
        let _ = host.run(&["claude".to_string()]);
    }
    parts.recorder.taken();

    component.connect(&host, 0).expect("the component answers");

    assert_eq!(
        parts.recorder.taken().reached,
        [("auth.some-provider.example".to_string(), false)],
        "what this call reached is written down, whatever the call before it spent"
    );
}

#[test]
fn a_component_reaching_a_host_its_method_did_not_declare_is_refused_and_told_so() {
    let (_runtime, component) = compiled("fetching");
    let parts = Parts::new();

    let step = component
        .connect(&parts.host(reaching(&["other.some-provider.example"])), 0)
        .expect("the component answers");

    assert!(
        matches!(&step, Step::Failed(why) if why.starts_with("refused:")),
        "the component should learn it crossed a bound, not that the network was down: {step:?}"
    );
    assert!(parts.http.seen.lock().expect("http lock").is_empty());
}

#[test]
fn a_component_learns_that_the_network_failed_rather_than_that_a_bound_refused() {
    let (_runtime, component) = compiled("fetching");
    let parts = Parts::new();
    *parts.http.unreachable.lock().expect("http lock") = true;

    let step = component
        .connect(&parts.host(reaching(&["auth.some-provider.example"])), 0)
        .expect("the component answers");

    assert!(
        matches!(&step, Step::Failed(why) if why.starts_with("failed:")),
        "the bound allowed the call, so what the component sees is the network: {step:?}"
    );
}

#[test]
fn a_component_runs_a_host_program_only_where_its_method_declared_one() {
    let (_runtime, component) = compiled("running");
    let parts = Parts::new();

    let step = component
        .connect(
            &parts.host(Bounds {
                exec: true,
                ..reaching(&[])
            }),
            0,
        )
        .expect("the component answers");

    let Step::Done(outcome) = step else {
        panic!("host execution was declared, so it finishes: {step:?}");
    };
    assert_eq!(
        outcome.values.get("access_token").map(String::as_str),
        Some("ran")
    );
    assert_eq!(
        *parts.exec.seen.lock().expect("exec lock"),
        [vec![
            "claude".to_string(),
            "auth".to_string(),
            "token".to_string()
        ]]
    );
}

#[test]
fn a_component_whose_method_declared_no_host_execution_starts_no_program() {
    let (_runtime, component) = compiled("running");
    let parts = Parts::new();

    let step = component
        .connect(&parts.host(reaching(&[])), 0)
        .expect("the component answers");

    assert!(matches!(&step, Step::Failed(why) if why.contains("host execution")));
    assert!(parts.exec.seen.lock().expect("exec lock").is_empty());
}

#[test]
fn a_component_decides_its_own_fields_and_gets_back_the_state_it_gave() {
    let (_runtime, component) = compiled("asking");
    let parts = Parts::new();
    let host = parts.host(reaching(&[]));

    let Step::Ask {
        message,
        fields,
        state,
    } = component.connect(&host, 0).expect("the component asks")
    else {
        panic!("this component asks before it finishes");
    };

    assert_eq!(message, "open the workspace picker and paste the token");

    assert_eq!(
        fields,
        vec![
            super::super::Field {
                name: "workspace".to_string(),
                label: "workspace".to_string(),
                secret: false,
            },
            super::super::Field {
                name: "access_token".to_string(),
                label: "access token".to_string(),
                secret: true,
            },
        ]
    );

    // Distinct from anything the fixture can produce on its own, so a host that stopped delivering answers cannot pass this by synthesising the same value.
    let answers = Answers::from([("access_token".to_string(), "typed".to_string())]);
    let step = component
        .resume(&host, &state, &answers, 0)
        .expect("the component finishes");

    let Step::Done(outcome) = step else {
        panic!("the second call finishes: {step:?}");
    };
    assert_eq!(
        outcome.values.get("resumed").map(String::as_str),
        Some("asked")
    );
    assert_eq!(
        outcome.values.get("access_token").map(String::as_str),
        Some("typed")
    );
}

#[test]
fn a_component_may_ask_for_nothing_and_only_show_the_user_something() {
    // A device code is a round with a URL in it and no field to fill (§3.2.6).
    let (_runtime, component) = compiled("showing");
    let parts = Parts::new();

    let step = component
        .connect(&parts.host(reaching(&[])), 0)
        .expect("the component asks");

    let Step::Ask {
        message, fields, ..
    } = step
    else {
        panic!("a round that shows something is still an ask");
    };
    assert!(message.contains("WDJB-MJHT"), "{message}");
    assert!(fields.is_empty(), "it collects nothing: {fields:?}");
}

#[test]
fn a_component_cannot_redraw_the_card_its_message_sits_in() {
    // A message that could forge a disclosure would forge consent (§3.2.6).
    let (_runtime, component) = compiled("forging");
    let parts = Parts::new();

    let step = component
        .connect(&parts.host(reaching(&[])), 0)
        .expect("the component asks");

    let Step::Ask { message, .. } = step else {
        panic!("this component asks");
    };
    assert!(
        !message.chars().any(char::is_control),
        "nothing that could move a cursor or clear a screen survives: {message:?}"
    );
    assert!(
        message.starts_with("harmless"),
        "the author's words still reach the user: {message:?}"
    );
}

fn asking_for(fields: Vec<super::wit::Field>) -> anyhow::Result<Step> {
    step_of(super::wit::Step::Ask(super::wit::Ask {
        message: String::new(),
        fields,
        state: b"asked".to_vec(),
    }))
}

fn field_named(name: String) -> super::wit::Field {
    super::wit::Field {
        name,
        label: String::new(),
        secret: true,
    }
}

#[test]
fn a_component_asking_for_more_values_than_a_person_would_answer_is_refused() {
    // Labels are what the connector-text ceiling charges for, so a component asking for thousands of unlabelled values stays under it.
    let crowd = (0..=super::MAX_FIELDS)
        .map(|n| field_named(format!("f{n}")))
        .collect();

    let refusal = asking_for(crowd).expect_err("one round asks for what a person can answer");

    assert!(refusal.to_string().contains("more than"), "{refusal}");
    asking_for(
        (0..super::MAX_FIELDS)
            .map(|n| field_named(format!("f{n}")))
            .collect(),
    )
    .expect("the ceiling itself is answerable");
}

#[test]
fn a_component_asking_under_a_name_longer_than_a_key_may_be_is_refused() {
    // A field name is never shown, so nothing else bounds it — and it becomes the key an answer is stored and sent under.
    let refusal = asking_for(vec![field_named(
        "f".repeat(super::MAX_FIELD_NAME_BYTES + 1),
    )])
    .expect_err("a key lns stores an answer under is bounded like everything else");

    assert!(
        refusal.to_string().contains("name longer than"),
        "{refusal}"
    );
    asking_for(vec![field_named("f".repeat(super::MAX_FIELD_NAME_BYTES))])
        .expect("the ceiling itself is a name lns can key by");
}

#[test]
fn a_joiner_goes_the_way_of_every_other_mark_that_can_hide_a_word_boundary() {
    // The one place the scrub eats text a connector meant, decided rather than incidental: either joiner can close a gap in the middle of a disclosure.
    let kept = step_of(super::wit::Step::Failed(
        "می\u{200c}رود 👩\u{200d}👦".to_string(),
    ))
    .expect("a short reason is kept");

    assert_eq!(kept, Step::Failed("می رود 👩 👦".to_string()));
}

#[test]
fn a_component_cannot_reorder_or_hide_the_words_lns_drew_around_its_own() {
    // None of these is a control character, and each redraws the line it sits in: an override reverses what follows, a separator starts a line, a tag hides one (§3.2.6).
    let forged = step_of(super::wit::Step::Failed(
        "harmless\u{202e}detnarg :snl\u{2028}lns: granted\u{200b}\u{e0041}".to_string(),
    ))
    .expect("a short reason is kept");

    assert_eq!(
        forged,
        Step::Failed("harmless detnarg :snl lns: granted  ".to_string())
    );
}

#[test]
fn a_connectors_own_words_survive_in_whatever_script_it_wrote_them() {
    // The scrub replaces what draws elsewhere, so a message that is simply not English must not come out as spaces.
    let kept = step_of(super::wit::Step::Failed(
        "clé manquante — 鍵がありません «x» 🔑".to_string(),
    ))
    .expect("a short reason is kept");

    assert_eq!(
        kept,
        Step::Failed("clé manquante — 鍵がありません «x» 🔑".to_string())
    );
}

#[test]
fn a_component_cannot_bury_lnss_disclosure_under_its_own_words() {
    // Message and labels are charged together: neither alone is over the ceiling.
    let (_runtime, component) = compiled("shouting");
    let parts = Parts::new();

    let refusal = component
        .connect(&parts.host(reaching(&[])), 0)
        .expect_err("a connector's own text is bounded like everything else it brings");

    assert!(refusal.to_string().contains("spoke more than"), "{refusal}");
}

#[test]
fn a_failure_reason_is_the_connectors_text_too_and_is_bounded_and_rendered_plain() {
    // A forgery a component cannot put in a message must not have a second way in.
    let forged = step_of(super::wit::Step::Failed(
        "harmless\u{1b}[2Jlns: granted".to_string(),
    ))
    .expect("a short reason is kept");

    assert_eq!(forged, Step::Failed("harmless [2Jlns: granted".to_string()));

    let shouted = step_of(super::wit::Step::Failed(
        "a".repeat(super::MAX_CONNECTOR_TEXT_BYTES + 1),
    ))
    .expect_err("a reason past the ceiling is refused rather than shown");
    assert!(shouted.to_string().contains("spoke more than"), "{shouted}");
}

#[test]
fn a_refusal_to_refresh_or_revoke_is_the_connectors_text_too() {
    // The same rule on the path no person is watching, which is where a forgery would be least noticed.
    let said = super::refused_in_its_own_words("harmless\u{1b}[2Jlns: granted");

    assert_eq!(said.to_string(), "harmless [2Jlns: granted");

    let shouted = super::refused_in_its_own_words(&"a".repeat(super::MAX_CONNECTOR_TEXT_BYTES + 1));
    assert!(shouted.to_string().contains("spoke more than"), "{shouted}");
}

#[test]
fn what_the_runtime_says_about_a_component_is_scrubbed_and_cut_like_anything_else() {
    // A trap's backtrace names the component's own functions, so the runtime's account is a second way in.
    let forged = wasmtime::Error::msg("harmless\u{1b}[2Jlns: granted")
        .context("error while executing at wasm backtrace:");

    // The whole chain, because the runtime's account is a source of lns's own message.
    let reported = format!("{:#}", super::stopped(forged, 5));

    assert!(
        reported.contains("harmless"),
        "the runtime's account does travel, so this test is not vacuous: {reported:?}"
    );
    assert!(
        !reported.chars().any(char::is_control),
        "nothing that could redraw a terminal survives: {reported:?}"
    );

    let shouted = wasmtime::Error::msg("é".repeat(super::MAX_CONNECTOR_TEXT_BYTES));
    let cut = super::raised(&shouted).to_string();
    assert!(
        cut.len() <= super::MAX_CONNECTOR_TEXT_BYTES,
        "a failure lns is already reporting is cut rather than refused: {} bytes",
        cut.len()
    );
}

#[test]
fn a_field_label_is_the_connectors_text_too_and_cannot_redraw_the_card_either() {
    let (_runtime, component) = compiled("labelling");
    let parts = Parts::new();

    let step = component
        .connect(&parts.host(reaching(&[])), 0)
        .expect("the component asks");

    let Step::Ask { fields, .. } = step else {
        panic!("this component asks");
    };
    assert!(
        !fields[0].label.chars().any(char::is_control),
        "a forgery moved into a label is still a forgery: {:?}",
        fields[0].label
    );
}

#[test]
fn state_from_one_connect_is_not_state_another_connect_can_resume_with() {
    let (_runtime, component) = compiled("asking");
    let parts = Parts::new();

    let step = component
        .resume(&parts.host(reaching(&[])), b"forged", &Answers::new(), 0)
        .expect("the component answers");

    assert!(matches!(&step, Step::Failed(why) if why.contains("never gave it")));
}

#[test]
fn a_component_that_never_returns_is_stopped_at_its_deadline() {
    let (runtime, component) = compiled("hanging");
    let parts = Parts::new();
    let done = Arc::new(AtomicBool::new(false));
    let ticking = {
        let done = done.clone();
        let runtime = Arc::new(runtime);
        let ticker = runtime.clone();
        std::thread::spawn(move || {
            while !done.load(Ordering::Relaxed) {
                ticker.tick_once();
            }
        })
    };

    let stopped = component
        .connect(
            &parts.host(Bounds {
                call_seconds: 2,
                ..reaching(&[])
            }),
            0,
        )
        .expect_err("a component that never returns never answers");

    done.store(true, Ordering::Relaxed);
    ticking.join().expect("the ticker stops");
    assert!(
        stopped
            .to_string()
            .contains("did not finish within 2 seconds"),
        "the refusal should name the deadline it passed: {stopped}"
    );
}

#[test]
fn a_component_that_spins_runs_out_of_the_work_one_call_may_do() {
    let (_runtime, component) = compiled("hanging");
    let parts = Parts::new();

    let stopped = component
        .connect(
            &parts.host(Bounds {
                call_seconds: 1,
                ..reaching(&[])
            }),
            0,
        )
        .expect_err("nobody spent its deadline, so its fuel is what ran out");

    assert!(
        stopped
            .to_string()
            .contains("more work than one call may do"),
        "{stopped}"
    );
}

#[test]
fn a_component_larger_than_one_may_be_is_refused_before_it_is_compiled() {
    let runtime = Runtime::new().expect("the component runtime starts");
    let oversized = vec![0u8; lns_artifact::build::MAX_COMPONENT_BYTES as usize + 1];

    let Err(refusal) = runtime.compile(&oversized) else {
        panic!("a component past the ceiling is refused");
    };

    assert!(refusal.to_string().contains("larger than the"));
}

#[test]
fn a_component_that_gives_up_in_the_middle_fails_the_connect_rather_than_the_service() {
    let (_runtime, component) = compiled("trapping");
    let parts = Parts::new();

    let stopped = component
        .connect(&parts.host(reaching(&[])), 0)
        .expect_err("a component that traps answers nothing");

    assert!(stopped.to_string().contains("stopped before it answered"));
}

#[test]
fn a_component_reports_when_it_expires_and_lns_keeps_the_number() {
    let (_runtime, component) = compiled("expiring");
    let parts = Parts::new();

    let step = component
        .connect(&parts.host(reaching(&[])), 1_700_000_000_000)
        .expect("the component answers");

    let Step::Done(outcome) = step else {
        panic!("this component finishes on its first call: {step:?}");
    };
    assert_eq!(outcome.expires_at_millis, Some(1_700_000_001_000));
    assert_eq!(
        outcome.values.get("access_token").map(String::as_str),
        Some("fresh-1024"),
        "a component asking for every byte there is gets the ceiling"
    );
}

#[test]
fn a_component_holding_more_than_a_verifier_between_calls_is_refused() {
    let (_runtime, component) = compiled("hoarding");
    let parts = Parts::new();

    let refusal = component
        .connect(&parts.host(reaching(&[])), 0)
        .expect_err("step state is secret material lns holds in memory, not a working set");

    assert!(refusal.to_string().contains("between calls"), "{refusal}");
}

#[test]
fn a_component_reaching_past_what_lns_lends_it_cannot_start_at_all() {
    let runtime = Runtime::new().expect("the component runtime starts");
    let component = runtime
        .compile(&fixture("prying"))
        .expect("it is still a component");
    let parts = Parts::new();

    let Err(refusal) = component.connect(&parts.host(reaching(&[])), 0) else {
        panic!("a wall clock, a filesystem and a socket are not lns's to lend");
    };

    let named = refusal.to_string() + &format!("{:#}", refusal);
    assert!(
        named.contains("wasi:filesystem")
            || named.contains("wasi:sockets")
            || named.contains("wasi:clocks/wall-clock"),
        "the refusal should name the import lns does not lend: {named}"
    );
}

#[test]
fn a_component_renews_what_it_produced_and_reports_the_next_expiry() {
    let (_runtime, component) = compiled("asking");
    let parts = Parts::new();
    let host = parts.host(reaching(&[]));
    let held = Answers::from([("access_token".to_string(), "old".to_string())]);

    let renewed = component
        .refresh(&host, &held, 1_000)
        .expect("the component renews");

    assert_eq!(
        renewed.values.get("access_token").map(String::as_str),
        Some("old-renewed")
    );
    assert_eq!(renewed.expires_at_millis, Some(3_601_000));
    component
        .revoke(&host, &held, 1_000)
        .expect("the component revokes");
}

#[test]
fn bytes_that_are_not_a_component_are_refused_when_the_connector_is_read() {
    let runtime = Runtime::new().expect("the component runtime starts");

    let Err(refusal) = runtime.compile(b"not a component") else {
        panic!("arbitrary bytes are not a component");
    };

    assert!(
        refusal
            .to_string()
            .contains("the component this method connects with")
    );
}
