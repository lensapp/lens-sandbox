use std::sync::Mutex;

use super::*;
use crate::connector::mechanism::host::Host;
use crate::connector::mechanism::tests::Parts;
use crate::connector::mechanism::traits::{Mechanism, Prepared};
use crate::connector::mechanism::{Field, Outcome};
use crate::connector::session::InMemorySessions;

/// A mechanism that answers from a script, so a driver test says which shape it is driving.
struct Scripted {
    turns: Mutex<Vec<Step>>,
    resumed_with: Mutex<Vec<(Vec<u8>, Answers)>>,
}

impl Scripted {
    fn answering(turns: Vec<Step>) -> Self {
        Self {
            turns: Mutex::new(turns),
            resumed_with: Mutex::new(Vec::new()),
        }
    }

    fn next(&self) -> Step {
        let mut turns = self.turns.lock().expect("script lock");
        assert!(
            !turns.is_empty(),
            "the driver asked for a turn the script does not have"
        );
        turns.remove(0)
    }
}

impl Mechanism for Scripted {
    fn connect(&self, _host: &Host, _now_millis: u64) -> Result<Step> {
        Ok(self.next())
    }

    fn resume(
        &self,
        _host: &Host,
        state: &[u8],
        answers: &Answers,
        _now_millis: u64,
    ) -> Result<Step> {
        self.resumed_with
            .lock()
            .expect("script lock")
            .push((state.to_vec(), answers.clone()));
        Ok(self.next())
    }

    fn refresh(&self, _host: &Host, _values: &Answers, _now_millis: u64) -> Result<Outcome> {
        anyhow::bail!("not part of a connect")
    }

    fn revoke(&self, _host: &Host, _values: &Answers, _now_millis: u64) -> Result<()> {
        Ok(())
    }
}

/// Hands out one scripted mechanism, and records the component bytes it was given.
struct OneMechanism {
    scripted: std::sync::Arc<Scripted>,
    parts: Parts,
    given: Mutex<Vec<Option<Vec<u8>>>>,
}

impl OneMechanism {
    fn answering(turns: Vec<Step>) -> Self {
        Self {
            scripted: std::sync::Arc::new(Scripted::answering(turns)),
            parts: Parts::new(),
            given: Mutex::new(Vec::new()),
        }
    }
}

impl Mechanisms for OneMechanism {
    fn for_method(
        &self,
        connector: &str,
        _method: &Method,
        component: Option<Vec<u8>>,
    ) -> Result<Prepared> {
        self.given.lock().expect("given lock").push(component);
        let _ = connector;
        Ok(Prepared {
            mechanism: Box::new(Handed(std::sync::Arc::clone(&self.scripted))),
            host: self
                .parts
                .host(crate::connector::mechanism::Bounds::default()),
        })
    }
}

/// The same scripted mechanism, handed out as many times as the driver asks for one.
struct Handed(std::sync::Arc<Scripted>);

impl Mechanism for Handed {
    fn connect(&self, host: &Host, now_millis: u64) -> Result<Step> {
        self.0.connect(host, now_millis)
    }

    fn resume(&self, host: &Host, state: &[u8], answers: &Answers, now: u64) -> Result<Step> {
        self.0.resume(host, state, answers, now)
    }

    fn refresh(&self, host: &Host, values: &Answers, now: u64) -> Result<Outcome> {
        self.0.refresh(host, values, now)
    }

    fn revoke(&self, host: &Host, values: &Answers, now: u64) -> Result<()> {
        self.0.revoke(host, values, now)
    }
}

fn asking(name: &str, secret: bool) -> Step {
    Step::Ask {
        message: "open the picker".to_string(),
        fields: vec![Field {
            name: name.to_string(),
            label: name.to_string(),
            secret,
        }],
        state: b"held".to_vec(),
    }
}

fn done(values: &[(&str, &str)], authority: &[&str]) -> Step {
    Step::Done(Outcome {
        values: values
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect(),
        authority: authority.iter().map(|s| (*s).to_string()).collect(),
        expires_at_millis: None,
    })
}

pub(crate) const CODE_DOCUMENT: &str = r#"{"apiVersion":"lns.run/v1","kind":"connector","name":"some-provider","spec":{"serves":["api.some-provider.example"],"methods":[{"name":"sign-in","auth":{"kind":"code","component":"./sign-in.wasm","outputs":["access_token"]},"credentials":[{"envVar":"SOME_TOKEN","placeholder":"some_LNSPLACEHOLDER0000000000","field":"access_token"}]}]}}"#;

/// The installed set and the two decision files, in memory, so a driver test says nothing about disks.
#[derive(Default)]
struct FakeSet {
    entries: Mutex<Vec<crate::connector::store::Installed>>,
    components: Mutex<Vec<Vec<u8>>>,
}

impl crate::connector::store::InstalledSet for FakeSet {
    fn list(&self) -> std::io::Result<Vec<crate::connector::store::Installed>> {
        Ok(self.entries.lock().expect("set lock").clone())
    }

    fn put(
        &self,
        name: &str,
        digest: &str,
        document: &[u8],
        _filesets: &[Vec<u8>],
        components: &[Vec<u8>],
    ) -> std::io::Result<()> {
        *self.components.lock().expect("set lock") = components.to_vec();
        let mut entries = self.entries.lock().expect("set lock");
        entries.retain(|held| held.name != name);
        entries.push(crate::connector::store::Installed {
            name: name.to_string(),
            digest: digest.to_string(),
            document: document.to_vec(),
        });
        Ok(())
    }

    fn fileset_layer(&self, _name: &str, _index: usize) -> std::io::Result<Vec<u8>> {
        Err(std::io::Error::other("this rig packs no filesets"))
    }

    fn component(&self, _name: &str, index: usize) -> std::io::Result<Vec<u8>> {
        self.components
            .lock()
            .expect("set lock")
            .get(index)
            .cloned()
            .ok_or_else(|| std::io::Error::other("no such component"))
    }

    fn remove(&self, _name: &str) -> std::io::Result<bool> {
        Ok(false)
    }
}

pub(crate) struct FakeMap<T> {
    state: Mutex<lns_policy::decision_store::DecisionFile<T>>,
    pub(crate) fail_save: Mutex<bool>,
}

impl<T> Default for FakeMap<T> {
    fn default() -> Self {
        Self {
            state: Mutex::new(lns_policy::decision_store::DecisionFile::new()),
            fail_save: Mutex::new(false),
        }
    }
}

impl<T: Clone + Send + Sync> lns_policy::decision_store::DecisionStore<T> for FakeMap<T> {
    fn load(&self) -> std::io::Result<lns_policy::decision_store::DecisionFile<T>> {
        Ok(self.state.lock().expect("map lock").clone())
    }

    fn save(&self, state: &lns_policy::decision_store::DecisionFile<T>) -> std::io::Result<()> {
        if *self.fail_save.lock().expect("map lock") {
            return Err(std::io::Error::other("disk full"));
        }
        *self.state.lock().expect("map lock") = state.clone();
        Ok(())
    }
}

pub(crate) struct Rig {
    set: FakeSet,
    pub(crate) values: FakeMap<Connection>,
    grants: FakeMap<crate::connector::store::RunDecision>,
    sessions: InMemorySessions,
}

impl Rig {
    pub(crate) fn holding(document: &str, component: Option<&[u8]>) -> Self {
        let rig = Self {
            set: FakeSet::default(),
            values: FakeMap::default(),
            grants: FakeMap::default(),
            sessions: InMemorySessions::default(),
        };
        rig.installs("sha256:abc", document, component);
        rig
    }

    pub(crate) fn installs(&self, digest: &str, document: &str, component: Option<&[u8]>) {
        self.store()
            .install(
                digest,
                document.as_bytes(),
                &[],
                &component
                    .map(<[u8]>::to_vec)
                    .into_iter()
                    .collect::<Vec<_>>(),
            )
            .expect("a connector this machine holds");
    }

    pub(crate) fn store(&self) -> ConnectorStore<'_> {
        ConnectorStore::new(&self.set, &self.values, &self.grants)
    }
}

fn driver<'a>(rig: &'a Rig, mechanisms: &'a OneMechanism, now_millis: u64) -> Driver<'a> {
    Driver {
        store: rig.store(),
        mechanisms,
        sessions: &rig.sessions,
        now_millis,
    }
}

#[test]
fn a_connect_asks_what_the_mechanism_asks_for_and_says_whose_words_they_are() {
    let rig = Rig::holding(CODE_DOCUMENT, Some(b"the mechanism"));
    let mechanisms = OneMechanism::answering(vec![asking("access_token", true)]);

    let turn = driver(&rig, &mechanisms, 0)
        .begin("some-provider", "sign-in", "work")
        .expect("the mechanism answers");

    let Connecting::Asks {
        session,
        message,
        fields,
        from_code,
    } = turn
    else {
        panic!("this mechanism asks: {turn:?}");
    };
    assert!(!session.is_empty());
    assert!(
        from_code,
        "these words are a component's, so whatever shows them says so"
    );
    assert_eq!(message, "open the picker");
    assert_eq!(fields[0].name, "access_token");
    assert!(fields[0].secret);
    assert_eq!(
        *mechanisms.given.lock().expect("given lock"),
        [Some(b"the mechanism".to_vec())],
        "the component the install kept is what the mechanism is built from"
    );
}

#[test]
fn answering_hands_back_the_state_the_mechanism_gave_and_finishes_the_connect() {
    let rig = Rig::holding(CODE_DOCUMENT, Some(b"the mechanism"));
    let mechanisms = OneMechanism::answering(vec![
        asking("access_token", true),
        done(&[("access_token", "abc")], &["repo:read"]),
    ]);
    let Connecting::Asks { session, .. } = driver(&rig, &mechanisms, 0)
        .begin("some-provider", "sign-in", "work")
        .expect("the mechanism asks")
    else {
        panic!("this mechanism asks first");
    };

    let turn = driver(&rig, &mechanisms, 1_000)
        .answer(
            &session,
            Answers::from([("access_token".to_string(), "abc".to_string())]),
        )
        .expect("the mechanism finishes");

    assert_eq!(turn.connector, "some-provider");
    assert!(
        matches!(turn.connecting, Connecting::Connected(_)),
        "{:?}",
        turn.connecting
    );
    assert_eq!(
        mechanisms.scripted.resumed_with.lock().expect("lock")[0].0,
        b"held".to_vec(),
        "the state handed back is the state the mechanism gave, verbatim"
    );
    let held = rig
        .store()
        .connections_of("some-provider")
        .expect("connections");
    assert_eq!(
        held["work"].values,
        [("access_token".to_string(), "abc".to_string())].into()
    );
    assert_eq!(
        held["work"].authority.0,
        ["repo:read".to_string()].into(),
        "what the mechanism reported is what the connection records"
    );
}

#[test]
fn a_connection_keeps_only_what_the_method_says_it_produces() {
    // A mechanism returning more returned something its document never declared, and no credential could draw on it.
    let rig = Rig::holding(CODE_DOCUMENT, Some(b"the mechanism"));
    let mechanisms = OneMechanism::answering(vec![done(
        &[("access_token", "abc"), ("smuggled", "not declared")],
        &[],
    )]);

    driver(&rig, &mechanisms, 0)
        .begin("some-provider", "sign-in", "work")
        .expect("the mechanism finishes on its first turn");

    let held = rig
        .store()
        .connections_of("some-provider")
        .expect("connections");
    assert_eq!(
        held["work"].values.keys().collect::<Vec<_>>(),
        ["access_token"]
    );
}

#[test]
fn a_mechanism_that_finishes_without_what_it_declared_it_produces_stores_nothing() {
    // Honouring `outputs` in one direction only stores a connection reported as connected whose credential can never arm, and the user meets that as a request the destination rejects.
    let rig = Rig::holding(CODE_DOCUMENT, Some(b"the mechanism"));
    let mechanisms = OneMechanism::answering(vec![done(&[], &["repo:read"])]);

    let err = driver(&rig, &mechanisms, 0)
        .begin("some-provider", "sign-in", "work")
        .expect_err("a connection with a hole in it is not one this machine will hold");

    let rendered = format!("{err:#}");
    assert!(rendered.contains("access_token"), "{rendered}");
    assert!(
        rig.store()
            .connections_of("some-provider")
            .expect("connections")
            .is_empty(),
        "nothing is kept from an answer that did not honour the document"
    );
}

#[test]
fn a_mechanism_that_fails_stores_nothing_and_leaves_the_offer_standing() {
    let rig = Rig::holding(CODE_DOCUMENT, Some(b"the mechanism"));
    let mechanisms =
        OneMechanism::answering(vec![Step::Failed("the provider said no".to_string())]);

    let turn = driver(&rig, &mechanisms, 0)
        .begin("some-provider", "sign-in", "work")
        .expect("a failed connect is an answer, not a fault");

    assert_eq!(turn, Connecting::Failed("the provider said no".to_string()));
    assert!(
        rig.store()
            .connections_of("some-provider")
            .expect("connections")
            .is_empty()
    );
}

#[test]
fn a_session_this_machine_did_not_mint_is_not_one_it_will_resume() {
    let rig = Rig::holding(CODE_DOCUMENT, Some(b"the mechanism"));
    let mechanisms = OneMechanism::answering(vec![]);

    let err = driver(&rig, &mechanisms, 0)
        .answer("some-provider/sign-in/99", Answers::new())
        .unwrap_err();

    assert!(format!("{err:#}").contains("no longer open"), "{err:#}");
}

#[test]
fn a_session_past_its_deadline_is_dropped_rather_than_resumed() {
    // sessionSeconds bounds the whole exchange, and a person who walked away leaves secret state behind.
    let rig = Rig::holding(CODE_DOCUMENT, Some(b"the mechanism"));
    let mechanisms = OneMechanism::answering(vec![asking("access_token", true)]);
    let Connecting::Asks { session, .. } = driver(&rig, &mechanisms, 0)
        .begin("some-provider", "sign-in", "work")
        .expect("the mechanism asks")
    else {
        panic!("this mechanism asks first");
    };

    let err = driver(&rig, &mechanisms, 901_000)
        .answer(&session, Answers::new())
        .unwrap_err();

    assert!(format!("{err:#}").contains("no longer open"), "{err:#}");
}

#[test]
fn the_whole_exchange_runs_out_rather_than_each_round_of_it() {
    // A deadline recomputed per round would let an exchange run for any length of time in 900-second hops (§3.2.6).
    let rig = Rig::holding(CODE_DOCUMENT, Some(b"the mechanism"));
    let mechanisms = OneMechanism::answering(vec![
        asking("workspace", false),
        asking("access_token", true),
    ]);
    let Connecting::Asks { session, .. } = driver(&rig, &mechanisms, 0)
        .begin("some-provider", "sign-in", "work")
        .expect("the mechanism asks")
    else {
        panic!("this mechanism asks first");
    };
    let Connecting::Asks { session, .. } = driver(&rig, &mechanisms, 890_000)
        .answer(
            &session,
            Answers::from([("workspace".to_string(), "acme".to_string())]),
        )
        .expect("it is still inside the exchange")
        .connecting
    else {
        panic!("this mechanism asks twice");
    };

    let err = driver(&rig, &mechanisms, 1_000_000)
        .answer(&session, Answers::new())
        .unwrap_err();

    assert!(
        format!("{err:#}").contains("no longer open"),
        "the exchange began at 0 and the whole of it is bounded: {err:#}"
    );
}

#[test]
fn a_reinstall_mid_exchange_is_a_different_implementation_and_will_not_be_resumed() {
    // The state and the answers belong to the bytes the connect began against (§3.2.6).
    let rig = Rig::holding(CODE_DOCUMENT, Some(b"the mechanism"));
    let mechanisms = OneMechanism::answering(vec![asking("access_token", true)]);
    let Connecting::Asks { session, .. } = driver(&rig, &mechanisms, 0)
        .begin("some-provider", "sign-in", "work")
        .expect("the mechanism asks")
    else {
        panic!("this mechanism asks first");
    };

    rig.installs("sha256:def", CODE_DOCUMENT, Some(b"other bytes entirely"));

    let err = driver(&rig, &mechanisms, 0)
        .answer(
            &session,
            Answers::from([("access_token".to_string(), "abc".to_string())]),
        )
        .unwrap_err();

    assert!(format!("{err:#}").contains("no longer open"), "{err:#}");
}

#[test]
fn a_card_hands_each_round_only_what_that_round_asked_for() {
    let rig = Rig::holding(CODE_DOCUMENT, Some(b"the mechanism"));
    let mechanisms = OneMechanism::answering(vec![
        asking("workspace", false),
        asking("access_token", true),
        done(&[("access_token", "abc")], &[]),
    ]);

    driver(&rig, &mechanisms, 0)
        .with_values(
            "some-provider",
            "sign-in",
            "work",
            Answers::from([
                ("workspace".to_string(), "acme".to_string()),
                ("access_token".to_string(), "abc".to_string()),
            ]),
        )
        .expect("the mechanism finishes");

    let resumed = mechanisms.scripted.resumed_with.lock().expect("lock");
    assert_eq!(
        resumed[1].1,
        Answers::from([("access_token".to_string(), "abc".to_string())]),
        "the second round sees what it asked for and not what the first consumed (§3.2.6)"
    );
}

#[test]
fn a_card_that_gave_up_keeps_nothing_it_had_already_collected() {
    let rig = Rig::holding(CODE_DOCUMENT, Some(b"the mechanism"));
    let mechanisms = OneMechanism::answering(vec![
        asking("workspace", false),
        asking("access_token", true),
    ]);

    let turn = driver(&rig, &mechanisms, 0)
        .with_values(
            "some-provider",
            "sign-in",
            "work",
            Answers::from([("workspace".to_string(), "acme".to_string())]),
        )
        .expect("a mechanism asking for more is an answer");

    assert!(
        matches!(turn, Connecting::Failed(ref why) if why.contains("access_token")),
        "{turn:?}"
    );
    assert!(
        rig.sessions.open.lock().expect("open lock").is_empty(),
        "the round that was collected already is not left holding what was typed"
    );
}

#[test]
fn one_handle_answers_once() {
    let rig = Rig::holding(CODE_DOCUMENT, Some(b"the mechanism"));
    let mechanisms = OneMechanism::answering(vec![
        asking("access_token", true),
        done(&[("access_token", "abc")], &[]),
    ]);
    let Connecting::Asks { session, .. } = driver(&rig, &mechanisms, 0)
        .begin("some-provider", "sign-in", "work")
        .expect("the mechanism asks")
    else {
        panic!("this mechanism asks first");
    };
    let values = Answers::from([("access_token".to_string(), "abc".to_string())]);
    driver(&rig, &mechanisms, 0)
        .answer(&session, values.clone())
        .expect("the first answer finishes it");

    let err = driver(&rig, &mechanisms, 0)
        .answer(&session, values)
        .unwrap_err();

    assert!(format!("{err:#}").contains("no longer open"), "{err:#}");
}

#[test]
fn a_round_the_person_walked_away_from_holds_nothing_and_answers_nobody() {
    let rig = Rig::holding(CODE_DOCUMENT, Some(b"the mechanism"));
    let mechanisms = OneMechanism::answering(vec![asking("access_token", true)]);
    let Connecting::Asks { session, .. } = driver(&rig, &mechanisms, 0)
        .begin("some-provider", "sign-in", "work")
        .expect("the mechanism asks")
    else {
        panic!("this mechanism asks first");
    };

    driver(&rig, &mechanisms, 0).abandon_handle(&session);

    assert!(
        rig.sessions.open.lock().expect("open lock").is_empty(),
        "an abandoned round holds no state, rather than waiting out sessionSeconds with it"
    );
    let err = driver(&rig, &mechanisms, 0)
        .answer(&session, Answers::new())
        .unwrap_err();
    assert!(format!("{err:#}").contains("no longer open"), "{err:#}");
}

#[test]
fn an_answer_a_round_consumed_is_dropped_rather_than_handed_over_again() {
    let rig = Rig::holding(CODE_DOCUMENT, Some(b"the mechanism"));
    let mechanisms = OneMechanism::answering(vec![
        asking("workspace", false),
        asking("access_token", true),
        done(&[("access_token", "abc")], &[]),
    ]);
    let Connecting::Asks { session, .. } = driver(&rig, &mechanisms, 0)
        .begin("some-provider", "sign-in", "work")
        .expect("the mechanism asks")
    else {
        panic!("this mechanism asks first");
    };
    let Connecting::Asks { session, .. } = driver(&rig, &mechanisms, 0)
        .answer(
            &session,
            Answers::from([("workspace".to_string(), "acme".to_string())]),
        )
        .expect("it asks again")
        .connecting
    else {
        panic!("this mechanism asks twice");
    };

    driver(&rig, &mechanisms, 0)
        .answer(
            &session,
            Answers::from([("access_token".to_string(), "abc".to_string())]),
        )
        .expect("the mechanism finishes");

    let resumed = mechanisms.scripted.resumed_with.lock().expect("lock");
    assert_eq!(
        resumed[1].1,
        Answers::from([("access_token".to_string(), "abc".to_string())]),
        "a password typed in one round is not handed to the component again in the next (§3.2.6)"
    );
}

#[test]
fn a_field_name_lns_could_not_key_an_answer_by_is_refused_before_anyone_is_asked() {
    let rig = Rig::holding(CODE_DOCUMENT, Some(b"the mechanism"));
    let mechanisms = OneMechanism::answering(vec![Step::Ask {
        message: String::new(),
        fields: vec![Field {
            name: "  ".to_string(),
            label: "token".to_string(),
            secret: true,
        }],
        state: Vec::new(),
    }]);

    let err = driver(&rig, &mechanisms, 0)
        .begin("some-provider", "sign-in", "work")
        .unwrap_err();

    assert!(format!("{err:#}").contains("cannot key by"), "{err:#}");
}

const TOKEN_DOCUMENT: &str = r#"{"apiVersion":"lns.run/v1","kind":"connector","name":"some-provider","spec":{"serves":["api.some-provider.example"],"methods":[{"name":"paste","auth":{"kind":"token"},"credentials":[{"envVar":"SOME_TOKEN","placeholder":"some_LNSPLACEHOLDER0000000000"}]},{"name":"open","env":{"SOME_REGION":"eu"}}]}}"#;

#[test]
fn a_card_that_collected_less_than_the_mechanism_asks_for_says_so_rather_than_half_connecting() {
    // The card shows one form and presses once, so a mechanism that wants a second round has nowhere to ask.
    let rig = Rig::holding(CODE_DOCUMENT, Some(b"the mechanism"));
    let mechanisms = OneMechanism::answering(vec![asking("workspace", false)]);

    let turn = driver(&rig, &mechanisms, 0)
        .with_values(
            "some-provider",
            "sign-in",
            "work",
            Answers::from([("access_token".to_string(), "abc".to_string())]),
        )
        .expect("a mechanism asking for more is an answer, not a fault");

    let Connecting::Failed(why) = turn else {
        panic!("nothing was collected for what it asked: {turn:?}");
    };
    assert!(why.contains("workspace"), "{why}");
    assert!(
        rig.store()
            .connections_of("some-provider")
            .expect("connections")
            .is_empty()
    );
}

/// Asks for the same thing forever, which is what a card has nobody to stop.
struct AlwaysAsking(Parts);

impl Mechanisms for AlwaysAsking {
    fn for_method(
        &self,
        _connector: &str,
        _method: &Method,
        _component: Option<Vec<u8>>,
    ) -> Result<Prepared> {
        Ok(Prepared {
            mechanism: Box::new(Insistent),
            host: self.0.host(crate::connector::mechanism::Bounds::default()),
        })
    }
}

struct Insistent;

impl Mechanism for Insistent {
    fn connect(&self, _host: &Host, _now_millis: u64) -> Result<Step> {
        Ok(asking("access_token", true))
    }

    fn resume(&self, _host: &Host, _s: &[u8], _a: &Answers, _now: u64) -> Result<Step> {
        Ok(asking("access_token", true))
    }

    fn refresh(&self, _host: &Host, _v: &Answers, _now: u64) -> Result<Outcome> {
        anyhow::bail!("not part of a connect")
    }

    fn revoke(&self, _host: &Host, _v: &Answers, _now: u64) -> Result<()> {
        Ok(())
    }
}

#[test]
fn a_mechanism_that_keeps_asking_a_card_is_stopped_rather_than_driven_forever() {
    // Nobody is at a card between rounds, so a component that asks again for what it has already been given would wedge the run's approval.
    let rig = Rig::holding(CODE_DOCUMENT, Some(b"the mechanism"));
    let insistent = AlwaysAsking(Parts::new());
    let driver = Driver {
        store: rig.store(),
        mechanisms: &insistent,
        sessions: &rig.sessions,
        now_millis: 0,
    };

    let turn = driver
        .with_values(
            "some-provider",
            "sign-in",
            "work",
            Answers::from([("access_token".to_string(), "abc".to_string())]),
        )
        .expect("it stops rather than hanging");

    let Connecting::Failed(why) = turn else {
        panic!("a mechanism that never finishes never connects: {turn:?}");
    };
    assert!(why.contains("kept asking"), "{why}");
}

#[test]
fn a_card_that_collected_what_the_mechanism_asks_for_finishes_without_a_second_round() {
    let rig = Rig::holding(CODE_DOCUMENT, Some(b"the mechanism"));
    let mechanisms = OneMechanism::answering(vec![
        asking("access_token", true),
        done(&[("access_token", "abc")], &[]),
    ]);

    let turn = driver(&rig, &mechanisms, 0)
        .with_values(
            "some-provider",
            "sign-in",
            "work",
            Answers::from([("access_token".to_string(), "abc".to_string())]),
        )
        .expect("the mechanism finishes");

    assert!(matches!(turn, Connecting::Connected(_)), "{turn:?}");
}

#[test]
fn a_caller_who_cannot_be_asked_again_is_told_what_is_still_wanted() {
    let still_asking = Connecting::Asks {
        session: "some-provider/sign-in/1".to_string(),
        message: String::new(),
        from_code: true,
        fields: vec![Field {
            name: "workspace".to_string(),
            label: "the workspace".to_string(),
            secret: false,
        }],
    };

    let err = still_asking.finished().unwrap_err();

    assert!(format!("{err:#}").contains("the workspace"), "{err:#}");
    assert!(
        Connecting::Failed("the provider said no".to_string())
            .finished()
            .unwrap_err()
            .to_string()
            .contains("the provider said no")
    );
}

#[test]
fn a_mechanism_lns_implements_has_no_author_to_attribute() {
    // §3.2.6 exempts a document's own `label`: it sits in what the card discloses and is checked before the connector installs. Attributing it would say a component wrote words no component wrote.
    let rig = Rig::holding(TOKEN_DOCUMENT, None);
    let mechanisms = OneMechanism::answering(vec![asking("token", true)]);

    let turn = driver(&rig, &mechanisms, 0)
        .begin("some-provider", "paste", "work")
        .expect("the mechanism answers");

    let Connecting::Asks { from_code, .. } = turn else {
        panic!("this mechanism asks: {turn:?}");
    };
    assert!(!from_code);
}

#[test]
fn a_method_with_nothing_to_connect_is_granted_instead() {
    let rig = Rig::holding(TOKEN_DOCUMENT, None);
    let mechanisms = OneMechanism::answering(vec![]);

    let err = driver(&rig, &mechanisms, 0)
        .begin("some-provider", "open", "work")
        .unwrap_err();

    assert!(format!("{err:#}").contains("grant it instead"), "{err:#}");
}

#[test]
fn a_method_that_carries_no_component_is_built_from_none() {
    let rig = Rig::holding(TOKEN_DOCUMENT, None);
    let mechanisms = OneMechanism::answering(vec![done(&[("token", "sk-live")], &[])]);

    driver(&rig, &mechanisms, 0)
        .begin("some-provider", "paste", "work")
        .expect("a mechanism lns implements needs no component");

    assert_eq!(*mechanisms.given.lock().expect("given lock"), [None]);
}
