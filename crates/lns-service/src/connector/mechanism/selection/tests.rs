use super::*;

fn engine() -> RealMechanisms {
    RealMechanisms::new().expect("the component runtime starts")
}

fn method(auth: serde_json::Value) -> Method {
    serde_json::from_value(serde_json::json!({ "name": "sign-in", "auth": auth }))
        .expect("a method reads")
}

#[test]
fn a_method_carrying_code_runs_the_component_the_install_kept() {
    let carrying = method(serde_json::json!({
        "kind": "code",
        "component": "./sign-in.wasm",
        "outputs": ["access_token"],
        "hosts": ["auth.some-provider.example"],
    }));
    let bytes = std::fs::read(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/asking.wasm"
    ))
    .expect("a committed fixture component");

    let prepared = engine()
        .for_method("some-provider", &carrying, Some(bytes))
        .expect("the component compiles");

    assert_eq!(
        prepared.host.bounds().hosts,
        ["auth.some-provider.example".to_string()],
        "the bounds come from the method that declared the mechanism"
    );
}

#[test]
fn a_method_carrying_code_this_machine_did_not_keep_names_the_reinstall() {
    let carrying = method(serde_json::json!({
        "kind": "code",
        "component": "./sign-in.wasm",
        "outputs": ["access_token"],
    }));

    let Err(err) = engine().for_method("some-provider", &carrying, None) else {
        panic!("a method names an implementation this machine does not hold");
    };

    assert!(format!("{err:#}").contains("reinstall"), "{err:#}");
}

#[test]
fn a_method_lns_implements_runs_lnss_own_mechanism_and_reaches_nothing() {
    let pasting = method(serde_json::json!({ "kind": "token" }));

    let prepared = engine()
        .for_method("some-provider", &pasting, None)
        .expect("lns implements this one");

    assert_eq!(prepared.host.bounds(), &Bounds::default());
}

#[test]
fn a_method_with_nothing_to_authenticate_has_no_mechanism_to_run() {
    let open: Method =
        serde_json::from_value(serde_json::json!({ "name": "open" })).expect("a method reads");

    let Err(err) = engine().for_method("some-provider", &open, None) else {
        panic!("there is nothing to authenticate with");
    };

    assert!(format!("{err:#}").contains("no authentication"), "{err:#}");
}

#[test]
fn native_mechanism_selection_does_not_initialize_wasmtime() {
    let native = engine();
    native
        .for_method(
            "provider",
            &method(serde_json::json!({"kind":"token"})),
            None,
        )
        .unwrap();
    for auth in [
        serde_json::json!({"kind":"oauth_device","clientId":"id","deviceAuthorizationEndpoint":"https://auth.example/device","tokenEndpoint":"https://auth.example/token","verificationHosts":["auth.example"]}),
        serde_json::json!({"kind":"oauth_authorization_code","clientId":"id","authorizationEndpoint":"https://auth.example/authorize","tokenEndpoint":"https://auth.example/token","redirect":{"kind":"loopback"}}),
    ] {
        let prepared = native.for_method("provider", &method(auth), None).unwrap();
        assert!(prepared.mechanism.native().is_some());
        assert_eq!(prepared.host.bounds().call_seconds, 30);
    }
    assert!(
        !native.runtime_initialized(),
        "native authentication must not initialize Wasmtime"
    );
}

#[test]
fn unknown_authentication_cannot_fall_back_to_collecting_a_token() {
    let native = engine();
    let Err(error) = native.for_method(
        "provider",
        &method(serde_json::json!({"kind":"future_oauth_kind"})),
        None,
    ) else {
        panic!("unknown mechanism")
    };
    assert!(error.to_string().contains("newer lns"));
    assert!(!native.runtime_initialized());
}
