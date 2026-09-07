use std::collections::BTreeMap;
use std::sync::Mutex;

use super::*;
use crate::connector::connect::tests::{CODE_DOCUMENT, Rig};
use crate::connector::mechanism::host::Host;
use crate::connector::mechanism::tests::Parts;
use crate::connector::mechanism::traits::{Mechanism, Prepared};
use crate::connector::mechanism::{Answers, Bounds, Outcome, Step};

fn connection(expires_at_millis: Option<u64>) -> Connection {
    Connection {
        method: "sign-in".to_string(),
        authority: Authority::default(),
        values: [("access_token".to_string(), "old".to_string())].into(),
        expires_at_millis,
    }
}

/// Records what a renewal did, so a unit test never reaches this machine's own ledger.
#[derive(Default)]
struct Wrote(Mutex<Vec<(String, bool)>>);

impl crate::connector::mechanism::traits::Recorder for Wrote {
    // no-op: an outbound call is the mechanism host's to record, not this pass's.
    fn reached(&self, _connector: &str, _host: &str, _refused: bool) {}

    // no-op: as above.
    fn ran(&self, _connector: &str, _program: &str, _refused: bool) {}

    fn renewed(&self, connector: &str, target: &str, refused: bool) {
        self.0
            .lock()
            .expect("wrote lock")
            .push((format!("{connector} {target}"), refused));
    }

    // no-op: a ceiling on what one call writes down is the mechanism host's, and this pass makes no component call of its own.
    fn elided(&self, _connector: &str, _after: u32) {}
}

fn labels(due: &[(String, Connection)]) -> Vec<String> {
    due.iter().map(|(label, _)| label.clone()).collect()
}

fn held(entries: &[(&str, Option<u64>)]) -> BTreeMap<String, Connection> {
    entries
        .iter()
        .map(|(label, expiry)| ((*label).to_string(), connection(*expiry)))
        .collect()
}

#[test]
fn a_connection_reporting_no_expiry_is_refreshed_on_no_schedule_at_all() {
    // A value with no stated lifetime is one lns has no reason to believe has ended (§3.2.6).
    let due = Schedule::default().due("some-provider", &held(&[("work", None)]), 1_000_000_000);

    assert!(due.is_empty());
}

#[test]
fn a_connection_running_out_soon_is_due_and_one_running_out_much_later_is_not() {
    let now = 1_000_000_000;
    let soon = now + AHEAD_MILLIS - 1;
    let later = now + AHEAD_MILLIS + 1;

    let due = labels(&Schedule::default().due(
        "some-provider",
        &held(&[("soon", Some(soon)), ("later", Some(later))]),
        now,
    ));

    assert_eq!(due, ["soon"]);
}

#[test]
fn a_connection_already_run_out_is_still_due_so_a_reconnect_is_not_the_only_way_back() {
    let now = 1_000_000_000;

    let due =
        labels(&Schedule::default().due("some-provider", &held(&[("work", Some(now - 1))]), now));

    assert_eq!(due, ["work"]);
}

#[test]
fn a_mechanism_reporting_a_one_second_lifetime_cannot_produce_a_hot_loop() {
    // The floor wins over a short reported expiry: it disarms on time, but it is never a wake-up (§3.2.6).
    let now = 1_000_000_000;
    let schedule = Schedule::default();
    let held = held(&[("work", Some(now + 1_000))]);
    schedule.tried("some-provider", Of::Connection("work"), now - 1);

    let due = schedule.due("some-provider", &held, now);

    assert!(due.is_empty(), "it was tried a millisecond ago");
    assert_eq!(
        labels(&schedule.due("some-provider", &held, now + FLOOR_MILLIS)),
        ["work"],
        "and it is due again once the floor has passed"
    );
}

#[test]
fn one_connectors_connection_does_not_starve_another_connectors_by_sharing_a_label() {
    // Two connectors commonly hold a connection named for the same method, and a schedule keyed by label alone would let the first seen starve the second forever.
    let now = 1_000_000_000;
    let schedule = Schedule::default();
    let held = held(&[("token", Some(now + 1_000))]);

    schedule.tried("alpha", Of::Connection("token"), now);

    assert!(schedule.due("alpha", &held, now).is_empty());
    assert_eq!(
        labels(&schedule.due("beta", &held, now)),
        ["token"],
        "beta was never tried, so its floor has not started"
    );
}

/// Answers one renewal from a script, so a pass test says what came back.
struct Renewing {
    outcome: Mutex<Option<Result<Outcome>>>,
    parts: Parts,
}

impl Renewing {
    fn answering(outcome: Result<Outcome>) -> Self {
        Self {
            outcome: Mutex::new(Some(outcome)),
            parts: Parts::new(),
        }
    }
}

impl Mechanisms for Renewing {
    fn for_method(
        &self,
        _connector: &str,
        _method: &lns_artifact::connector::Method,
        _component: Option<Vec<u8>>,
    ) -> Result<Prepared> {
        Ok(Prepared {
            mechanism: Box::new(Answered(
                self.outcome
                    .lock()
                    .expect("script lock")
                    .take()
                    .unwrap_or_else(|| anyhow::bail!("the script has one renewal in it")),
            )),
            host: self.parts.host(Bounds::default()),
        })
    }
}

struct Answered(Result<Outcome>);

impl Mechanism for Answered {
    fn connect(&self, _host: &Host, _now: u64) -> Result<Step> {
        anyhow::bail!("not part of a refresh")
    }

    fn resume(&self, _host: &Host, _s: &[u8], _a: &Answers, _now: u64) -> Result<Step> {
        anyhow::bail!("not part of a refresh")
    }

    fn refresh(&self, _host: &Host, _values: &Answers, _now: u64) -> Result<Outcome> {
        match &self.0 {
            Ok(outcome) => Ok(outcome.clone()),
            Err(e) => anyhow::bail!("{e:#}"),
        }
    }

    fn revoke(&self, _host: &Host, _values: &Answers, _now: u64) -> Result<()> {
        Ok(())
    }
}

fn holding(expires_at_millis: Option<u64>) -> Rig {
    let rig = Rig::holding(CODE_DOCUMENT, Some(b"the mechanism"));
    rig.store()
        .record_authentication(
            "some-provider",
            "work",
            Connection {
                method: "sign-in".to_string(),
                authority: Authority::of(["repo:read"]),
                values: [("access_token".to_string(), "old".to_string())].into(),
                expires_at_millis,
            },
        )
        .expect("a connection this machine holds");
    rig
}

fn only_connection(rig: &Rig) -> Connection {
    rig.store()
        .connections_of("some-provider")
        .expect("connections")
        .remove("work")
        .expect("the one this rig holds")
}

#[test]
fn a_renewal_replaces_the_values_and_the_expiry_it_came_with() {
    let rig = holding(Some(1_000));
    let mechanisms = Renewing::answering(Ok(Outcome {
        values: [("access_token".to_string(), "fresh".to_string())].into(),
        authority: std::collections::BTreeSet::new(),
        expires_at_millis: Some(9_000_000),
    }));
    let wrote = Wrote::default();
    let schedule = Schedule::default();

    let renewed = once(
        &rig.store(),
        &mechanisms,
        &wrote,
        &schedule,
        "some-provider",
        1_000,
    )
    .expect("the pass runs");

    assert_eq!(renewed, ["work"]);
    let held = only_connection(&rig);
    assert_eq!(
        held.values.get("access_token").map(String::as_str),
        Some("fresh")
    );
    assert_eq!(held.expires_at_millis, Some(9_000_000));
    assert_eq!(
        held.authority.0,
        ["repo:read".to_string()].into(),
        "a renewal that omits scopes carries forward the ones the connection already held"
    );
}

#[test]
fn a_renewal_that_reports_scopes_replaces_the_ones_the_connection_held() {
    // Carry-forward is for what a renewal omits; what it reports is what the connection now records (§3.2.4).
    let rig = holding(Some(1_000));
    let mechanisms = Renewing::answering(Ok(Outcome {
        values: [("access_token".to_string(), "fresh".to_string())].into(),
        authority: std::collections::BTreeSet::from(["repo:write".to_string()]),
        expires_at_millis: Some(9_000_000),
    }));
    let wrote = Wrote::default();
    let schedule = Schedule::default();

    once(
        &rig.store(),
        &mechanisms,
        &wrote,
        &schedule,
        "some-provider",
        1_000,
    )
    .expect("the pass runs");

    assert_eq!(
        only_connection(&rig).authority.0,
        ["repo:write".to_string()].into()
    );
}

#[test]
fn a_renewal_keeps_only_what_the_method_says_it_produces() {
    // A renewal returns the same shape a connect does, so it is filtered by the same rule (§3.2.6).
    let rig = holding(Some(1_000));
    let mechanisms = Renewing::answering(Ok(Outcome {
        values: [
            ("access_token".to_string(), "fresh".to_string()),
            ("id_token".to_string(), "never declared".to_string()),
        ]
        .into(),
        authority: std::collections::BTreeSet::new(),
        expires_at_millis: Some(9_000_000),
    }));
    let wrote = Wrote::default();
    let schedule = Schedule::default();

    once(
        &rig.store(),
        &mechanisms,
        &wrote,
        &schedule,
        "some-provider",
        1_000,
    )
    .expect("the pass runs");

    assert_eq!(
        only_connection(&rig).values.keys().collect::<Vec<_>>(),
        ["access_token"]
    );
}

#[tokio::test]
#[serial_test::serial(env, global_runs)]
async fn a_pass_over_a_connector_holding_no_connection_renews_nothing_and_says_nothing() {
    let home = tempfile::tempdir().expect("tempdir");
    let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", home.path());
    let project = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        project.path().join("lns.yaml"),
        serde_json::json!({
            "apiVersion": "lns.run/v1",
            "kind": "connector",
            "name": "some-provider",
            "spec": {
                "serves": ["api.some-provider.example"],
                "methods": [{ "name": "token", "auth": { "kind": "token" } }],
            },
        })
        .to_string(),
    )
    .expect("write");
    crate::connector::real::answer(crate::connector::real::Call::Install(
        project.path().display().to_string(),
    ))
    .await
    .expect("install");

    let wrote = Wrote::default();
    let said = crate::test_env::captured_messages(|| {
        pass(
            &Renewing::answering(Err(anyhow::anyhow!("nothing should call this"))),
            &wrote,
            &Schedule::default(),
            1_000,
        );
    });

    assert!(
        wrote.0.lock().expect("wrote lock").is_empty(),
        "this connector holds no connection, so there was nothing to renew"
    );
    assert!(said.is_empty(), "and nothing to say about it: {said:?}");
}

#[test]
fn a_renewal_reporting_different_authority_names_the_runs_that_must_decide_again() {
    // Nobody is watching a renewal, so what it invalidated has to be readable afterwards.
    let rig = holding(Some(1_000));
    rig.store()
        .decide(
            &crate::connector::store::GrantHolder::Run("1a2b3c4d".to_string()),
            "some-provider",
            crate::connector::store::RunDecision::Granted {
                digest: "sha256:abc".to_string(),
                method: "sign-in".to_string(),
                connection: Some("work".to_string()),
                authority: Authority::of(["repo:read"]),
            },
        )
        .expect("a run that granted this connection");
    let mechanisms = Renewing::answering(Ok(Outcome {
        values: [("access_token".to_string(), "fresh".to_string())].into(),
        authority: std::collections::BTreeSet::from(["repo:write".to_string()]),
        expires_at_millis: Some(9_000_000),
    }));
    let wrote = Wrote::default();

    let renewed = once(
        &rig.store(),
        &mechanisms,
        &wrote,
        &Schedule::default(),
        "some-provider",
        1_000,
    )
    .expect("the pass runs");

    assert_eq!(renewed, ["work"]);
    assert_eq!(
        *wrote.0.lock().expect("wrote lock"),
        [
            ("some-provider work".to_string(), false),
            (
                "some-provider work (1 run(s) must decide again)".to_string(),
                false
            ),
        ],
        "the grants it dropped are durable state a user must act on, and nobody was watching"
    );
    assert!(
        rig.store()
            .decision(
                &crate::connector::store::GrantHolder::Run("1a2b3c4d".to_string()),
                "some-provider"
            )
            .expect("read the decision")
            .is_none(),
        "a renewal reporting different authority invalidates the grants naming that connection"
    );
}

#[tokio::test]
#[serial_test::serial(env, global_runs)]
async fn a_machine_that_cannot_read_its_own_connector_state_renews_nothing_and_says_so() {
    // A regular file where the connector directory should be: ENOTDIR is an error the kernel gives every uid.
    let home = tempfile::tempdir().expect("tempdir");
    std::fs::write(home.path().join("connectors"), b"not a directory").expect("write");
    let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", home.path());
    let wrote = Wrote::default();

    let said = crate::test_env::captured_messages(|| {
        pass(
            &Renewing::answering(Err(anyhow::anyhow!("nothing should call this"))),
            &wrote,
            &Schedule::default(),
            1_000,
        );
    });

    assert!(wrote.0.lock().expect("wrote lock").is_empty());
    assert!(
        said.iter()
            .any(|m| m.contains("could not look at what is running out")),
        "an operator is told rather than left with a silent machine: {said:?}"
    );
}

#[tokio::test(start_paused = true)]
#[serial_test::serial(env, global_runs)]
async fn the_loop_looks_again_every_time_it_wakes() {
    // A machine this pass cannot read reports once per floor, so counting the reports counts the wakes.
    let home = tempfile::tempdir().expect("tempdir");
    std::fs::write(home.path().join("connectors"), b"not a directory").expect("write");
    let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", home.path());
    let looked = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let _reports =
        tracing::subscriber::set_default(tracing_subscriber::layer::SubscriberExt::with(
            tracing_subscriber::registry(),
            Counting(std::sync::Arc::clone(&looked)),
        ));
    let passes = || looked.load(std::sync::atomic::Ordering::Relaxed);

    keep(
        &tokio::runtime::Handle::current(),
        std::sync::Arc::new(Renewing::answering(Err(anyhow::anyhow!(
            "this machine cannot be read, so nothing is asked of it"
        )))),
        std::sync::Arc::new(Wrote::default()),
    );

    assert_eq!(passes(), 0, "nothing happens before the first wake");
    // Time is paused, so this hands the loop each wake rather than waiting half a minute for it.
    tokio::time::sleep(LOOK_EVERY + std::time::Duration::from_secs(1)).await;
    assert_eq!(
        passes(),
        1,
        "the first wake looked and said what it could not read"
    );
    tokio::time::sleep(LOOK_EVERY).await;
    assert_eq!(passes(), 2, "and it looks again every time it wakes");
}

/// Counts what the loop reported, so a loop that stopped waking is a failed assertion rather than a silent pass.
struct Counting(std::sync::Arc<std::sync::atomic::AtomicUsize>);

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Counting {
    fn on_event(
        &self,
        _event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
}

#[test]
fn a_renewal_that_ran_while_nobody_watched_is_written_down() {
    // It leaves no state behind, so the entry is the only record there will ever be that it happened (§3.2.6).
    let rig = holding(Some(1_000));
    let mechanisms = Renewing::answering(Ok(Outcome {
        values: [("access_token".to_string(), "fresh".to_string())].into(),
        authority: std::collections::BTreeSet::new(),
        expires_at_millis: Some(9_000_000),
    }));
    let wrote = Wrote::default();

    once(
        &rig.store(),
        &mechanisms,
        &wrote,
        &Schedule::default(),
        "some-provider",
        1_000,
    )
    .expect("the pass runs");

    assert_eq!(
        *wrote.0.lock().expect("wrote lock"),
        [("some-provider work".to_string(), false)]
    );
}

#[test]
fn a_renewal_this_machine_could_not_store_is_still_a_renewal_that_ran() {
    // It ran at the provider whether or not this machine kept what came back, and one connection lns could not store must not take the others with it.
    let rig = holding(Some(1_000));
    *rig.values.fail_save.lock().expect("map lock") = true;
    let mechanisms = Renewing::answering(Ok(Outcome {
        values: [("access_token".to_string(), "fresh".to_string())].into(),
        authority: std::collections::BTreeSet::new(),
        expires_at_millis: Some(9_000_000),
    }));
    let wrote = Wrote::default();

    let renewed = once(
        &rig.store(),
        &mechanisms,
        &wrote,
        &Schedule::default(),
        "some-provider",
        1_000,
    )
    .expect("the pass carries on");

    assert!(
        renewed.is_empty(),
        "nothing was kept, so nothing is reported renewed"
    );
    assert_eq!(
        *wrote.0.lock().expect("wrote lock"),
        [("some-provider work".to_string(), false)],
        "but it happened, and the entry is the only record there will ever be"
    );
}

#[test]
fn a_renewal_that_failed_is_written_down_as_refused() {
    let rig = holding(Some(1_000));
    let mechanisms = Renewing::answering(Err(anyhow::anyhow!("the provider is down")));
    let wrote = Wrote::default();

    once(
        &rig.store(),
        &mechanisms,
        &wrote,
        &Schedule::default(),
        "some-provider",
        1_000,
    )
    .expect("a failed renewal is an answer, not a fault");

    assert_eq!(
        *wrote.0.lock().expect("wrote lock"),
        [("some-provider work".to_string(), true)]
    );
}

#[test]
fn a_connector_lns_cannot_read_is_reported_once_rather_than_every_pass() {
    // The loop looks every thirty seconds for the life of the process, so an unreadable document must not warn every time.
    let rig = Rig::holding(CODE_DOCUMENT, Some(b"the mechanism"));
    let mechanisms = Renewing::answering(Err(anyhow::anyhow!("nothing should call this")));
    let wrote = Wrote::default();
    let schedule = Schedule::default();

    // A connector the pass is handed but the store cannot answer for: unreadable and uninstalled fail the same read.
    let first = once(
        &rig.store(),
        &mechanisms,
        &wrote,
        &schedule,
        "absent",
        1_000,
    );
    let again = once(
        &rig.store(),
        &mechanisms,
        &wrote,
        &schedule,
        "absent",
        2_000,
    );

    assert!(first.is_err(), "the first pass says what it could not read");
    assert!(
        again.expect("the second says nothing").is_empty(),
        "and the ones inside the floor stay quiet"
    );
}

#[test]
fn a_renewal_that_fails_leaves_the_connection_as_it_was() {
    // A provider outage must not burn the grant: the connection stands, and its own expiry still disarms it.
    let rig = holding(Some(1_000));
    let mechanisms = Renewing::answering(Err(anyhow::anyhow!("the provider is down")));
    let wrote = Wrote::default();
    let schedule = Schedule::default();

    let renewed = once(
        &rig.store(),
        &mechanisms,
        &wrote,
        &schedule,
        "some-provider",
        1_000,
    )
    .expect("a failed renewal is an answer, not a fault");

    assert!(renewed.is_empty());
    let held = only_connection(&rig);
    assert_eq!(
        held.values.get("access_token").map(String::as_str),
        Some("old")
    );
    assert_eq!(held.expires_at_millis, Some(1_000));
    assert!(
        schedule
            .due(
                "some-provider",
                &rig.store().connections_of("some-provider").unwrap(),
                1_000
            )
            .is_empty(),
        "a failed try still spends the floor, so an outage is not a hot loop either"
    );
}

#[test]
fn a_connection_reporting_no_expiry_is_left_alone_by_the_pass() {
    let rig = holding(None);
    let mechanisms = Renewing::answering(Err(anyhow::anyhow!("nothing should call this")));
    let wrote = Wrote::default();
    let schedule = Schedule::default();

    let renewed = once(
        &rig.store(),
        &mechanisms,
        &wrote,
        &schedule,
        "some-provider",
        1_000_000_000,
    )
    .expect("the pass runs");

    assert!(renewed.is_empty());
}
