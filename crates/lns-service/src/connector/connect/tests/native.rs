use super::*;
use crate::connector::mechanism::oauth::flow::tests::Fake;
use crate::connector::mechanism::real::RealMechanisms;
use std::sync::Arc;

fn document() -> String {
    serde_json::json!({"apiVersion":"lns.run/v1","kind":"connector","name":"some-provider","spec":{"serves":["api.example"],"methods":[{"name":"sign-in","auth":{"kind":"oauth_device","scopeOptions":[{"name":"read-only","label":"Read only","scopes":["read"]}],"clientId":"public-id","deviceAuthorizationEndpoint":"https://auth.example/device","tokenEndpoint":"https://auth.example/token","verificationHosts":["auth.example"]},"credentials":[{"envVar":"TOKEN","placeholder":"LNSPLACEHOLDER0000000000","injections":[{"kind":"bearer_header","domain":"api.example"}]}]}]}}).to_string()
}
fn machine() -> (Rig, RealMechanisms, Arc<Fake>) {
    let fake = Arc::new(Fake::default());
    *fake.replies.lock().unwrap()=vec![
        serde_json::json!({"device_code":"private-device","user_code":"ABCD","verification_uri":"https://auth.example/verify","expires_in":900}),
        serde_json::json!({"access_token":"access","token_type":"Bearer","refresh_token":"refresh","expires_in":3600}),
    ].into();
    let mechanisms =
        RealMechanisms::lending(fake.clone(), fake.clone(), fake.clone(), fake.clone())
            .unwrap()
            .with_browser(fake.clone());
    (Rig::holding(&document(), None), mechanisms, fake)
}
fn at<'a>(rig: &'a Rig, mechanisms: &'a RealMechanisms, now: u64) -> Driver<'a> {
    Driver {
        store: rig.store(),
        mechanisms,
        sessions: &rig.sessions,
        now_millis: now,
    }
}
fn begin(rig: &Rig, mechanisms: &RealMechanisms) -> String {
    let Connecting::Pending { session, .. } = at(rig, mechanisms, 0)
        .begin("some-provider", "sign-in", "work")
        .unwrap()
    else {
        panic!("pending")
    };
    at(rig, mechanisms, 0)
        .answer(
            &session,
            [("scopeOption".into(), "read-only".into())].into(),
        )
        .unwrap();
    session
}
#[test]
fn native_connect_status_and_private_persistence_share_the_connector_lifecycle() {
    let (rig, mechanisms, fake) = machine();
    let handle = begin(&rig, &mechanisms);
    assert!(fake.requests.lock().unwrap().is_empty());
    at(&rig, &mechanisms, 0)
        .advance_native(&handle, || 0)
        .unwrap();
    for _ in 0..10 {
        assert!(matches!(
            at(&rig, &mechanisms, 1)
                .native_status(&handle)
                .unwrap()
                .connecting,
            Connecting::Pending { .. }
        ));
    }
    assert_eq!(fake.requests.lock().unwrap().len(), 1);
    at(&rig, &mechanisms, 5000)
        .advance_native(&handle, || 5000)
        .unwrap();
    assert!(matches!(
        at(&rig, &mechanisms, 5000)
            .native_status(&handle)
            .unwrap()
            .connecting,
        Connecting::Connected(_)
    ));
    let held = rig
        .store()
        .connections_of("some-provider")
        .unwrap()
        .remove("work")
        .unwrap();
    assert_eq!(
        held.values,
        Answers::from([("access_token".into(), "access".into())])
    );
    assert_eq!(
        held.oauth.as_ref().unwrap().refresh_token.as_deref(),
        Some("refresh")
    );
    assert!(held.generation > 0);
    let restored: Connection = serde_json::from_slice(&serde_json::to_vec(&held).unwrap()).unwrap();
    assert_eq!(held, restored);
    let view = crate::connector::handler::list(&rig.store()).unwrap();
    let public = serde_json::to_string(&view).unwrap();
    assert!(!public.contains("refresh"));
    assert!(!public.contains("private-device"));
    assert!(view[0].methods[0].oauth.is_some());
    assert!(!view[0].methods[0].carries_code);
}
#[test]
fn cancellation_and_deadline_reject_results_after_the_network_returns() {
    for expired in [false, true] {
        let (rig, mechanisms, _) = machine();
        let handle = begin(&rig, &mechanisms);
        at(&rig, &mechanisms, 0)
            .advance_native(&handle, || 0)
            .unwrap();
        at(&rig, &mechanisms, 5000)
            .advance_native(&handle, || {
                if !expired {
                    at(&rig, &mechanisms, 5001).abandon_handle(&handle);
                }
                if expired { 900_000 } else { 5001 }
            })
            .unwrap();
        assert!(
            rig.store()
                .connections_of("some-provider")
                .unwrap()
                .is_empty()
        );
        at(&rig, &mechanisms, 900_000).sweep_native();
        assert!(matches!(
            at(&rig, &mechanisms, 900_000)
                .native_status(&handle)
                .unwrap()
                .connecting,
            Connecting::Pending {
                progress: lns_ipc::OAuthProgress::Canceled | lns_ipc::OAuthProgress::Expired,
                ..
            }
        ));
    }
}
#[test]
fn a_changed_connector_or_connection_cannot_take_a_late_native_result() {
    for change in ["install", "disconnect", "supersede", "write-failure"] {
        let (rig, mechanisms, _) = machine();
        let handle = begin(&rig, &mechanisms);
        at(&rig, &mechanisms, 0)
            .advance_native(&handle, || 0)
            .unwrap();
        at(&rig, &mechanisms, 5000)
            .advance_native(&handle, || {
                match change {
                    "install" => rig.installs("sha256:new", &document(), None),
                    "disconnect" => {
                        at(&rig, &mechanisms, 5001).cancel_connector("some-provider", Some("work"))
                    }
                    "supersede" => {
                        begin(&rig, &mechanisms);
                    }
                    _ => *rig.values.fail_save.lock().unwrap() = true,
                }
                5001
            })
            .unwrap();
        assert!(
            rig.store()
                .connections_of("some-provider")
                .unwrap()
                .is_empty(),
            "{change}"
        );
    }
}
#[test]
fn noninteractive_native_auth_never_starts_browser_or_provider_work() {
    let (rig, mechanisms, fake) = machine();
    assert!(matches!(
        at(&rig, &mechanisms, 0)
            .with_values("some-provider", "sign-in", "work", Answers::new())
            .unwrap(),
        Connecting::Failed(_)
    ));
    assert!(fake.requests.lock().unwrap().is_empty());
    assert!(fake.opened.lock().unwrap().is_empty());
}

#[test]
fn uninstalling_a_pending_browser_operation_releases_its_original_listener() {
    let (rig, mechanisms, fake) = machine();
    let mut code: serde_json::Value = serde_json::from_str(&document()).unwrap();
    code["spec"]["methods"][0]["auth"] = serde_json::json!({"kind":"oauth_authorization_code","scopeOptions":[{"name":"read-only","label":"Read only","scopes":["read"]}],"clientId":"public-id","authorizationEndpoint":"https://auth.example/authorize","tokenEndpoint":"https://auth.example/token","redirect":{"kind":"loopback","port":53682}});
    rig.installs("sha256:code", &code.to_string(), None);
    let handle = begin(&rig, &mechanisms);
    at(&rig, &mechanisms, 0)
        .advance_native(&handle, || 0)
        .unwrap();
    rig.store().uninstall("some-provider").unwrap();
    at(&rig, &mechanisms, 1).abandon_handle(&handle);
    assert_eq!(fake.canceled.lock().unwrap().as_slice(), ["handle"]);
}

fn connected(rig: &Rig, mechanisms: &RealMechanisms) -> Connection {
    let handle = begin(rig, mechanisms);
    at(rig, mechanisms, 0)
        .advance_native(&handle, || 0)
        .unwrap();
    at(rig, mechanisms, 5000)
        .advance_native(&handle, || 5000)
        .unwrap();
    rig.store()
        .connections_of("some-provider")
        .unwrap()
        .remove("work")
        .unwrap()
}

#[test]
fn native_scheduler_atomically_rotates_and_disarms_revoked_credentials_without_a_browser() {
    let (rig, mechanisms, fake) = machine();
    let old = connected(&rig, &mechanisms);
    fake.opened.lock().unwrap().clear();
    let schedule = crate::connector::refresh::Schedule::default();
    for (response, now, expired) in [
        (
            serde_json::json!({"access_token":"next","token_type":"Bearer","refresh_token":"rotated","expires_in":3600}),
            3_400_000,
            false,
        ),
        (
            serde_json::json!({"error":"invalid_grant"}),
            6_800_000,
            true,
        ),
    ] {
        fake.replies.lock().unwrap().push_back(response);
        crate::connector::refresh::once(
            &rig.store(),
            &mechanisms,
            fake.as_ref(),
            &schedule,
            "some-provider",
            now,
        )
        .unwrap();
        let held = rig
            .store()
            .connections_of("some-provider")
            .unwrap()
            .remove("work")
            .unwrap();
        assert_eq!(held.has_run_out(now), expired);
        assert!(held.generation > old.generation);
        if expired {
            assert!(held.values.is_empty());
            assert!(held.oauth.unwrap().refresh_token.is_none());
        } else {
            assert_eq!(held.values["access_token"], "next");
            assert_eq!(
                held.oauth.unwrap().refresh_token.as_deref(),
                Some("rotated")
            );
        }
    }
    assert!(fake.opened.lock().unwrap().is_empty());
    let count = fake.requests.lock().unwrap().len();
    crate::connector::refresh::once(
        &rig.store(),
        &mechanisms,
        fake.as_ref(),
        &schedule,
        "some-provider",
        u64::MAX,
    )
    .unwrap();
    assert_eq!(fake.requests.lock().unwrap().len(), count);
}

#[test]
fn native_renewal_preserves_old_credentials_on_transient_or_atomic_store_failure() {
    for failure in ["provider", "store"] {
        let (rig, mechanisms, fake) = machine();
        let old = connected(&rig, &mechanisms);
        fake.replies.lock().unwrap().push_back(if failure=="provider" {serde_json::json!({"error":"temporarily_unavailable"})} else {serde_json::json!({"access_token":"next","token_type":"Bearer","refresh_token":"rotated","expires_in":3600})});
        *rig.values.fail_save.lock().unwrap() = failure == "store";
        let renewed = crate::connector::refresh::once(
            &rig.store(),
            &mechanisms,
            fake.as_ref(),
            &Default::default(),
            "some-provider",
            3_400_000,
        )
        .unwrap();
        assert!(renewed.is_empty());
        assert_eq!(
            rig.store().connections_of("some-provider").unwrap()["work"],
            old
        );
        assert!(old.has_run_out(3_605_000));
    }
}

#[test]
fn native_renewal_network_work_cannot_resurrect_a_disconnected_connection() {
    let (rig, mechanisms, fake) = machine();
    let rig = Arc::new(rig);
    connected(&rig, &mechanisms);
    fake.replies.lock().unwrap().push_back(serde_json::json!({"access_token":"late","token_type":"Bearer","refresh_token":"late-refresh","expires_in":3600}));
    let deleting = rig.clone();
    *fake.interleave.lock().unwrap() = Some(Box::new(move || {
        deleting
            .store()
            .drop_connections("some-provider", Some("work"))
            .unwrap();
    }));
    let renewed = crate::connector::refresh::once(
        &rig.store(),
        &mechanisms,
        fake.as_ref(),
        &Default::default(),
        "some-provider",
        3_400_000,
    )
    .unwrap();
    assert!(renewed.is_empty());
    assert!(
        rig.store()
            .connections_of("some-provider")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_device_token_arriving_after_the_providers_shorter_deadline_is_not_saved() {
    let (rig, mechanisms, fake) = machine();
    fake.replies.lock().unwrap()[0]["expires_in"] = serde_json::json!(6);
    let handle = begin(&rig, &mechanisms);
    at(&rig, &mechanisms, 0)
        .advance_native(&handle, || 0)
        .unwrap();
    at(&rig, &mechanisms, 5000)
        .advance_native(&handle, || 6000)
        .unwrap();
    assert!(
        rig.store()
            .connections_of("some-provider")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn native_browser_actions_and_provider_failures_do_not_reuse_stale_operations() {
    let (rig, mechanisms, fake) = machine();
    let handle = begin(&rig, &mechanisms);
    assert!(
        at(&rig, &mechanisms, 0)
            .native_status(&handle)
            .unwrap()
            .connecting
            .finished()
            .is_err()
    );
    assert!(
        at(&rig, &mechanisms, 0)
            .open_native_browser("unknown")
            .is_err()
    );
    assert!(
        at(&rig, &mechanisms, 0)
            .open_native_browser(&handle)
            .is_err()
    );
    at(&rig, &mechanisms, 0)
        .advance_native(&handle, || 0)
        .unwrap();
    at(&rig, &mechanisms, 1)
        .open_native_browser(&handle)
        .unwrap();
    assert_eq!(fake.opened.lock().unwrap().len(), 2);
    assert!(
        at(&rig, &mechanisms, 900000)
            .open_native_browser(&handle)
            .is_err()
    );
    rig.installs("sha256:changed", &document(), None);
    assert!(
        at(&rig, &mechanisms, 1)
            .open_native_browser(&handle)
            .is_err()
    );
    at(&rig, &mechanisms, 5000)
        .advance_native(&handle, || 5000)
        .unwrap();
    assert!(matches!(
        at(&rig, &mechanisms, 5000)
            .native_status(&handle)
            .unwrap()
            .connecting,
        Connecting::Failed(_)
    ));
    at(&rig, &mechanisms, 5001)
        .advance_native(&handle, || 5001)
        .unwrap();
    assert_eq!(fake.requests.lock().unwrap().len(), 1);
    for error in ["access_denied", "expired_token"] {
        let (rig, mechanisms, fake) = machine();
        fake.replies.lock().unwrap()[1] = serde_json::json!({"error":error});
        let handle = begin(&rig, &mechanisms);
        at(&rig, &mechanisms, 0)
            .advance_native(&handle, || 0)
            .unwrap();
        at(&rig, &mechanisms, 5000)
            .advance_native(&handle, || 5000)
            .unwrap();
        let result = at(&rig, &mechanisms, 5000).native_status(&handle).unwrap();
        if error == "expired_token" {
            assert!(matches!(
                result.connecting,
                Connecting::Pending {
                    progress: lns_ipc::OAuthProgress::Expired,
                    ..
                }
            ));
        } else {
            assert!(
                matches!(result.connecting,Connecting::Failed(ref why) if why.contains("denied"))
            );
        }
    }
}
#[test]
fn cancellation_during_callback_preparation_releases_the_new_listener() {
    let (rig, mechanisms, fake) = machine();
    let mut code: serde_json::Value = serde_json::from_str(&document()).unwrap();
    code["spec"]["methods"][0]["auth"] = serde_json::json!({"kind":"oauth_authorization_code","scopeOptions":[{"name":"read-only","label":"Read only","scopes":["read"]}],"clientId":"public-id","authorizationEndpoint":"https://auth.example/authorize","tokenEndpoint":"https://auth.example/token","redirect":{"kind":"loopback"}});
    rig.installs("sha256:code", &code.to_string(), None);
    let handle = begin(&rig, &mechanisms);
    at(&rig, &mechanisms, 0)
        .advance_native(&handle, || {
            at(&rig, &mechanisms, 1).abandon_handle(&handle);
            1
        })
        .unwrap();
    assert_eq!(fake.canceled.lock().unwrap().as_slice(), ["handle"]);
    assert!(
        rig.store()
            .connections_of("some-provider")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn replacing_native_auth_with_code_requires_reconnect_before_running_that_code() {
    let (rig, mechanisms, fake) = machine();
    connected(&rig, &mechanisms);
    rig.installs("sha256:code", CODE_DOCUMENT, Some(b"not a component"));
    crate::connector::refresh::once(
        &rig.store(),
        &mechanisms,
        fake.as_ref(),
        &Default::default(),
        "some-provider",
        3_400_000,
    )
    .unwrap();
    let held = rig
        .store()
        .connections_of("some-provider")
        .unwrap()
        .remove("work")
        .unwrap();
    assert!(
        held.oauth.unwrap().reconnect_required,
        "a native renewal must not invoke a replacement code mechanism"
    );
    assert!(held.values.is_empty());
}

#[test]
fn cancellation_during_code_exchange_drops_the_token_and_closes_the_callback() {
    let (rig, mechanisms, fake) = machine();
    let mut code: serde_json::Value = serde_json::from_str(&document()).unwrap();
    code["spec"]["methods"][0]["auth"] = serde_json::json!({"kind":"oauth_authorization_code","scopeOptions":[{"name":"read-only","label":"Read only","scopes":["read"]}],"clientId":"public-id","authorizationEndpoint":"https://auth.example/authorize","tokenEndpoint":"https://auth.example/token","redirect":{"kind":"loopback"}});
    rig.installs("sha256:code", &code.to_string(), None);
    fake.replies.lock().unwrap().pop_front();
    let handle = begin(&rig, &mechanisms);
    at(&rig, &mechanisms, 0)
        .advance_native(&handle, || 0)
        .unwrap();
    *fake.callback.lock().unwrap() = Some("one-time-code".into());
    at(&rig, &mechanisms, 1000)
        .advance_native(&handle, || {
            at(&rig, &mechanisms, 1001).abandon_handle(&handle);
            1001
        })
        .unwrap();
    assert!(
        rig.store()
            .connections_of("some-provider")
            .unwrap()
            .is_empty()
    );
    assert!(!fake.canceled.lock().unwrap().is_empty());
    assert_eq!(fake.requests.lock().unwrap().len(), 1);
}

#[test]
fn selecting_permissions_is_explicit_once_and_bound_to_the_live_operation() {
    let (rig, mechanisms, fake) = machine();
    let driver = at(&rig, &mechanisms, 0);
    let Connecting::Pending { session, progress } =
        driver.begin("some-provider", "sign-in", "work").unwrap()
    else {
        panic!("pending")
    };
    assert!(matches!(
        progress,
        lns_ipc::OAuthProgress::SelectingScopes { .. }
    ));
    for _ in 0..10 {
        driver.advance_native(&session, || 0).unwrap();
        driver.native_status(&session).unwrap();
    }
    assert!(rig.sessions.operations().due(899999).is_empty());
    assert!(fake.requests.lock().unwrap().is_empty());
    for answer in [
        Answers::new(),
        [("scopeOption".into(), "invented".into())].into(),
        [
            ("scopeOption".into(), "read-only".into()),
            ("scope".into(), "admin".into()),
        ]
        .into(),
    ] {
        assert!(driver.answer(&session, answer).is_err());
    }
    let answer: Answers = [("scopeOption".into(), "read-only".into())].into();
    driver.answer(&session, answer.clone()).unwrap();
    assert!(driver.answer(&session, answer.clone()).is_err());
    driver.advance_native(&session, || 0).unwrap();
    assert_eq!(fake.requests.lock().unwrap().len(), 1);
    assert!(driver.answer(&session, answer).is_err());
    assert_eq!(fake.requests.lock().unwrap().len(), 1);
}

#[test]
fn canceled_expired_or_changed_connectors_cannot_accept_permission_answers() {
    for cause in ["cancel", "expiry", "changed"] {
        let (rig, mechanisms, fake) = machine();
        let Connecting::Pending { session, .. } = at(&rig, &mechanisms, 0)
            .begin("some-provider", "sign-in", "work")
            .unwrap()
        else {
            panic!("pending")
        };
        let mut now = 0;
        match cause {
            "cancel" => at(&rig, &mechanisms, 0).abandon_handle(&session),
            "expiry" => now = 900000,
            "changed" => {
                rig.store().uninstall("some-provider").unwrap();
            }
            _ => unreachable!(),
        }
        assert!(
            at(&rig, &mechanisms, now)
                .answer(
                    &session,
                    [("scopeOption".into(), "read-only".into())].into()
                )
                .is_err()
        );
        assert!(fake.requests.lock().unwrap().is_empty());
        assert!(fake.opened.lock().unwrap().is_empty());
    }
}

#[test]
fn transient_device_polling_keeps_one_operation_and_cancellation_stops_its_retry() {
    let (rig, mechanisms, fake) = machine();
    let handle = begin(&rig, &mechanisms);
    at(&rig, &mechanisms, 0)
        .advance_native(&handle, || 0)
        .unwrap();
    *fake.status.lock().unwrap() = Some(503);
    *fake.raw_body.lock().unwrap() = Some(b"<html>unavailable</html>".to_vec());
    at(&rig, &mechanisms, 5000)
        .advance_native(&handle, || 5000)
        .unwrap();
    for now in 5001..5010 {
        assert!(matches!(
            at(&rig, &mechanisms, now)
                .native_status(&handle)
                .unwrap()
                .connecting,
            Connecting::Pending { .. }
        ));
        at(&rig, &mechanisms, now)
            .advance_native(&handle, || now)
            .unwrap();
    }
    assert_eq!(fake.requests.lock().unwrap().len(), 2);
    at(&rig, &mechanisms, 6000).abandon_handle(&handle);
    at(&rig, &mechanisms, 15000)
        .advance_native(&handle, || 15000)
        .unwrap();
    assert_eq!(fake.requests.lock().unwrap().len(), 2);
    assert!(
        rig.store()
            .connections_of("some-provider")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn reconciliation_snapshot_exposes_only_the_next_live_granted_expiry() {
    let (rig, mechanisms, _) = machine();
    let held = connected(&rig, &mechanisms);
    let holder = crate::connector::store::GrantHolder::Run("run".into());
    let snapshot =
        crate::connector::handler::granted_supply_snapshot(&rig.store(), &holder, 5000).unwrap();
    assert!(snapshot.expires_at_millis.is_none());
    rig.store()
        .decide(
            &holder,
            "some-provider",
            crate::connector::store::RunDecision::Granted {
                digest: "sha256:abc".into(),
                method: "sign-in".into(),
                connection: Some("work".into()),
                authority: held.authority,
            },
        )
        .unwrap();
    let snapshot =
        crate::connector::handler::granted_supply_snapshot(&rig.store(), &holder, 5000).unwrap();
    assert_eq!(snapshot.expires_at_millis, Some(3605000));
    let snapshot =
        crate::connector::handler::granted_supply_snapshot(&rig.store(), &holder, 3605000).unwrap();
    assert!(snapshot.expires_at_millis.is_none());
    assert_eq!(
        snapshot.payloads["some-provider"].credentials[0].injections[0].value(),
        ""
    );
}
