use super::super::browser::Callback;
use super::*;
use crate::connector::mechanism::traits::{Entropy, Exec, Http, Recorder};
use crate::connector::mechanism::{Bounds, CallError, HttpRequest, HttpResponse};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub(crate) struct Fake {
    pub(crate) replies: Mutex<VecDeque<serde_json::Value>>,
    pub(crate) requests: Mutex<Vec<HttpRequest>>,
    pub(crate) opened: Mutex<Vec<String>>,
    pub(crate) callback: Mutex<Option<String>>,
    denied: std::sync::atomic::AtomicBool,
    authorization_error: std::sync::atomic::AtomicBool,
    fail_open: std::sync::atomic::AtomicBool,
    fail_prepare: std::sync::atomic::AtomicBool,
    failure: Mutex<Option<CallError>>,
    status: Mutex<Option<u16>>,
    entropy_length: Mutex<Option<usize>>,
    pub(crate) interleave: Mutex<Option<Box<dyn Fn() + Send + Sync>>>,
    state: Mutex<String>,
    pub(crate) canceled: Mutex<Vec<String>>,
}
impl Http for Fake {
    fn fetch(&self, r: &HttpRequest, _: std::time::Duration) -> Result<HttpResponse, CallError> {
        self.requests.lock().unwrap().push(r.clone());
        if let Some(action) = self.interleave.lock().unwrap().take() {
            action();
        }
        if let Some(error) = self.failure.lock().unwrap().take() {
            return Err(error);
        }
        Ok(HttpResponse {
            status: self.status.lock().unwrap().unwrap_or(200),
            headers: vec![],
            body: self
                .replies
                .lock()
                .unwrap()
                .pop_front()
                .expect("a scripted response")
                .to_string()
                .into_bytes(),
        })
    }
}
impl Exec for Fake {
    fn run(
        &self,
        _: &[String],
        _: std::time::Duration,
    ) -> Result<crate::connector::mechanism::ExecOutput, CallError> {
        panic!("native OAuth must not execute a program through the Wasm host")
    }
}
impl Entropy for Fake {
    fn bytes(&self, n: u32) -> Vec<u8> {
        vec![42; self.entropy_length.lock().unwrap().unwrap_or(n as usize)]
    }
}
impl Recorder for Fake {
    fn reached(&self, _: &str, _: &str, _: bool) {}
    fn ran(&self, _: &str, _: &str, _: bool) {
        panic!("no execution")
    }
    fn renewed(&self, _: &str, _: &str, _: bool) {}
    fn elided(&self, _: &str, _: u32) {}
}
impl Browser for Fake {
    fn prepare(
        &self,
        _: &str,
        path: &str,
        port: Option<u16>,
        state: &str,
        _: u64,
    ) -> Result<(String, String)> {
        if self.fail_prepare.load(std::sync::atomic::Ordering::Relaxed) {
            bail!("callback port is occupied");
        }
        *self.state.lock().unwrap() = state.into();
        Ok((
            "handle".into(),
            format!("http://127.0.0.1:{}{path}", port.unwrap_or(40000)),
        ))
    }
    fn open(&self, url: &str) -> Result<()> {
        self.opened.lock().unwrap().push(url.into());
        if self.fail_open.load(std::sync::atomic::Ordering::Relaxed) {
            bail!("browser could not open");
        }
        Ok(())
    }
    fn poll(&self, _: &str, _: &str, _: u64) -> Result<Callback> {
        if self
            .authorization_error
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            return Ok(Callback::Failed);
        }
        if self.denied.load(std::sync::atomic::Ordering::Relaxed) {
            return Ok(Callback::Denied);
        }
        Ok(self
            .callback
            .lock()
            .unwrap()
            .take()
            .map_or(Callback::Ignore, Callback::Code))
    }
    fn cancel(&self, handle: &str) {
        self.canceled.lock().unwrap().push(handle.into());
    }
    fn sweep(&self, _: u64) {}
}
fn setup(config: OAuth, replies: Vec<serde_json::Value>) -> (Native, Host, Arc<Fake>) {
    let fake = Arc::new(Fake::default());
    *fake.replies.lock().unwrap() = replies.into();
    let host = Host::new(
        "provider",
        Bounds {
            hosts: vec!["auth.example".into()],
            exec: false,
            call_seconds: 30,
            session_seconds: 900,
        },
        fake.clone(),
        fake.clone(),
        fake.clone(),
        fake.clone(),
    );
    (
        Native {
            config,
            browser: fake.clone(),
        },
        host,
        fake,
    )
}
fn device() -> OAuth {
    OAuth::Device {
        client_id: "public-id".into(),
        token_endpoint: "https://auth.example/token".into(),
        device_authorization_endpoint: "https://auth.example/device".into(),
        verification_hosts: vec!["auth.example".into()],
        scope_options: vec![lns_artifact::connector::oauth::ScopeOption {
            name: "read-only".into(),
            label: "Read only".into(),
            scopes: vec!["read".into()],
        }],
    }
}
fn pending(advance: Advance) -> Pending {
    match advance {
        Advance::Pending(p) => p,
        _ => panic!("still pending"),
    }
}
#[test]
fn device_polls_automatically_at_the_provider_interval_and_keeps_device_code_private() {
    let (native, host, fake) = setup(
        device(),
        vec![
            serde_json::json!({"device_code":"private-device","user_code":"ABCD","verification_uri":"https://auth.example/verify","expires_in":900}),
            serde_json::json!({"error":"authorization_pending"}),
            serde_json::json!({"error":"slow_down"}),
            serde_json::json!({"access_token":"access","token_type":"Bearer","refresh_token":"refresh"}),
        ],
    );
    let start = selected(&native, &host, 0);
    assert!(fake.requests.lock().unwrap().is_empty());
    let first = pending(native.advance(&host, &start.state, 0).unwrap());
    assert_eq!(first.next_at_millis, 5000);
    assert!(!format!("{:?}", first).contains("private-device"));
    let early = pending(native.advance(&host, &first.state, 4999).unwrap());
    assert_eq!(fake.requests.lock().unwrap().len(), 1);
    let waiting = pending(native.advance(&host, &early.state, 5000).unwrap());
    assert_eq!(waiting.next_at_millis, 10000);
    let slower = pending(native.advance(&host, &waiting.state, 10000).unwrap());
    assert_eq!(slower.next_at_millis, 20000);
    let Advance::Done(outcome) = native.advance(&host, &slower.state, 20000).unwrap() else {
        panic!("connected")
    };
    assert_eq!(outcome.values.len(), 1);
    assert_eq!(
        outcome.oauth.unwrap().refresh_token.as_deref(),
        Some("refresh")
    );
    assert_eq!(
        fake.opened.lock().unwrap().as_slice(),
        ["https://auth.example/verify"]
    );
}
#[test]
fn authorization_code_uses_matching_pkce_and_the_exact_configured_callback() {
    use base64::Engine;
    use lns_artifact::connector::oauth::Redirect;
    use sha2::Digest;
    let config = OAuth::AuthorizationCode {
        client_id: "public-id".into(),
        token_endpoint: "https://auth.example/token".into(),
        authorization_endpoint: "https://auth.example/authorize".into(),
        redirect: Redirect::Loopback {
            path: "/oauth/callback".into(),
            port: Some(53682),
        },
        scope_options: vec![lns_artifact::connector::oauth::ScopeOption {
            name: "read-only".into(),
            label: "Read only".into(),
            scopes: vec!["read".into()],
        }],
    };
    let (native, host, fake) = setup(
        config,
        vec![serde_json::json!({"access_token":"access","token_type":"Bearer"})],
    );
    let start = selected(&native, &host, 0);
    let waiting = pending(native.advance(&host, &start.state, 0).unwrap());
    let url = reqwest::Url::parse(&fake.opened.lock().unwrap()[0]).unwrap();
    let params: std::collections::BTreeMap<_, _> = url
        .query_pairs()
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    assert_eq!(params["response_type"], "code");
    assert_eq!(params["code_challenge_method"], "S256");
    assert_eq!(
        params["redirect_uri"],
        "http://127.0.0.1:53682/oauth/callback"
    );
    *fake.callback.lock().unwrap() = Some("one-time-code".into());
    let Advance::Done(outcome) = native.advance(&host, &waiting.state, 1000).unwrap() else {
        panic!("connected")
    };
    let requests = fake.requests.lock().unwrap();
    let body: std::collections::BTreeMap<_, _> = form_urlencoded::parse(&requests[0].body)
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    assert_eq!(body["redirect_uri"], params["redirect_uri"]);
    assert_eq!(body["client_id"], "public-id");
    assert_eq!(body["code"], "one-time-code");
    assert_eq!(
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(sha2::Sha256::digest(body["code_verifier"].as_bytes())),
        params["code_challenge"]
    );
    assert!(outcome.oauth.unwrap().refresh_token.is_none());
    assert!(!body.contains_key("client_secret"));
}

fn code() -> OAuth {
    OAuth::AuthorizationCode {
        client_id: "public-id".into(),
        token_endpoint: "https://auth.example/token".into(),
        authorization_endpoint: "https://auth.example/authorize".into(),
        redirect: Redirect::Loopback {
            path: "/callback".into(),
            port: None,
        },
        scope_options: vec![lns_artifact::connector::oauth::ScopeOption {
            name: "read-only".into(),
            label: "Read only".into(),
            scopes: vec![],
        }],
    }
}
fn device_response() -> serde_json::Value {
    serde_json::json!({"device_code":"private-device","user_code":"ABCD","verification_uri":"https://auth.example/verify","verification_uri_complete":"https://auth.example/verify?user_code=ABCD","expires_in":120,"interval":2})
}
#[test]
fn device_denial_expiry_and_timeouts_have_distinct_bounded_transitions() {
    for (error, expected) in [
        ("access_denied", "denied"),
        ("expired_token", "expired"),
        ("invalid_request", "refused"),
    ] {
        let (native, host, _) = setup(
            device(),
            vec![device_response(), serde_json::json!({"error":error})],
        );
        let start = selected(&native, &host, 0);
        let waiting = pending(native.advance(&host, &start.state, 0).unwrap());
        assert_eq!(waiting.next_at_millis, 2000);
        let Err(error) = native.advance(&host, &waiting.state, 2000) else {
            panic!("terminal provider response")
        };
        assert!(error.to_string().contains(expected));
    }
    let (native, host, fake) = setup(device(), vec![device_response()]);
    let start = selected(&native, &host, 0);
    let waiting = pending(native.advance(&host, &start.state, 0).unwrap());
    assert_eq!(
        fake.opened.lock().unwrap()[0],
        "https://auth.example/verify?user_code=ABCD"
    );
    *fake.failure.lock().unwrap() = Some(CallError::Failed("timeout with sensitive URL".into()));
    let retry = pending(native.advance(&host, &waiting.state, 2000).unwrap());
    assert_eq!(retry.next_at_millis, 6000);
    assert!(matches!(
        native
            .advance(&host, &retry.state, 120000)
            .err()
            .unwrap()
            .downcast_ref::<FlowError>(),
        Some(FlowError::Expired)
    ));
    *fake.failure.lock().unwrap() = Some(CallError::Refused("refused sensitive URL".into()));
    let Err(error) = native.advance(&host, &retry.state, 6000) else {
        panic!("host refused")
    };
    assert!(!error.to_string().contains("sensitive"));
}
#[test]
fn device_responses_cannot_open_untrusted_or_malformed_browser_destinations() {
    for (key, value) in [
        (
            "verification_uri",
            serde_json::json!("https://evil.example/verify"),
        ),
        (
            "verification_uri_complete",
            serde_json::json!("https://evil.example/verify"),
        ),
        (
            "verification_uri_complete",
            serde_json::json!("http://auth.example/verify"),
        ),
        (
            "verification_uri_complete",
            serde_json::json!("https://auth.example:8443/verify"),
        ),
        (
            "verification_uri_complete",
            serde_json::json!("https://auth.example/verify#hidden"),
        ),
        ("device_code", serde_json::json!("")),
        ("user_code", serde_json::json!("hidden\ncode")),
        ("user_code", serde_json::json!("x".repeat(257))),
        ("expires_in", serde_json::json!(0)),
        ("expires_in", serde_json::json!(u64::MAX)),
        ("interval", serde_json::json!("5")),
    ] {
        let mut response = device_response();
        response[key] = value;
        let (native, host, fake) = setup(device(), vec![response]);
        let start = selected(&native, &host, 0);
        assert!(native.advance(&host, &start.state, 0).is_err(), "{key}");
        assert!(fake.opened.lock().unwrap().is_empty());
    }
    let mut response = device_response();
    response.as_object_mut().unwrap().remove("expires_in");
    let (native, host, _) = setup(device(), vec![response]);
    let start = selected(&native, &host, 0);
    assert!(native.advance(&host, &start.state, 0).is_err());
}
#[test]
fn browser_failures_denial_and_deadline_release_callbacks_without_token_exchange() {
    use std::sync::atomic::Ordering::Relaxed;
    for reason in [
        "bind",
        "open",
        "denied",
        "expired",
        "cancel",
        "provider-error",
    ] {
        let (native, host, fake) = setup(code(), vec![]);
        fake.fail_prepare.store(reason == "bind", Relaxed);
        fake.fail_open.store(reason == "open", Relaxed);
        let start = selected(&native, &host, 0);
        let advanced = native.advance(&host, &start.state, 0);
        if matches!(reason, "bind" | "open") {
            assert!(advanced.is_err());
        } else {
            let waiting = pending(advanced.unwrap());
            assert!(
                matches!(waiting.progress,OAuthProgress::WaitingForBrowser{ref redirect_uri,..} if redirect_uri=="http://127.0.0.1:40000/callback")
            );
            let ignored = pending(native.advance(&host, &waiting.state, 1000).unwrap());
            assert_eq!(ignored.next_at_millis, 2000);
            native.open_browser(&waiting.state).unwrap();
            if reason == "cancel" {
                native.cancel(&waiting.state);
            } else {
                fake.denied.store(reason == "denied", Relaxed);
                fake.authorization_error
                    .store(reason == "provider-error", Relaxed);
                assert!(
                    native
                        .advance(
                            &host,
                            &ignored.state,
                            if reason == "expired" { 900000 } else { 2000 }
                        )
                        .is_err()
                );
            }
        }
        assert_eq!(
            fake.canceled.lock().unwrap().len(),
            usize::from(reason != "bind"),
            "{reason}"
        );
        assert!(fake.requests.lock().unwrap().is_empty());
    }
}
#[test]
fn renewal_binds_registration_retains_rotates_and_never_starts_interaction() {
    let (native, host, fake) = setup(
        device(),
        vec![
            serde_json::json!({"access_token":"one","token_type":"Bearer","refresh_token":"rotated","expires_in":60}),
            serde_json::json!({"access_token":"two","token_type":"Bearer","scope":""}),
            serde_json::json!({"error":"invalid_grant"}),
        ],
    );
    let mut held = crate::connector::store::Connection {
        method: "sign-in".into(),
        authority: crate::connector::store::Authority::of(["read"]),
        values: Default::default(),
        expires_at_millis: Some(100),
        oauth: Some(OAuthState {
            refresh_token: Some("original".into()),
            ..native.binding()
        }),
        generation: 7,
    };
    let first = native.refresh(&host, &held, 0).unwrap();
    assert_eq!(first.expires_at_millis, Some(60000));
    assert_eq!(first.authority, held.authority.0);
    assert_eq!(
        first.oauth.as_ref().unwrap().refresh_token.as_deref(),
        Some("rotated")
    );
    held.oauth = first.oauth;
    let second = native.refresh(&host, &held, 10).unwrap();
    assert!(second.authority.is_empty());
    assert!(second.expires_at_millis.is_none());
    assert_eq!(
        second.oauth.as_ref().unwrap().refresh_token.as_deref(),
        Some("rotated")
    );
    assert!(matches!(
        native
            .refresh(&host, &held, 20)
            .unwrap_err()
            .downcast_ref::<token::TokenError>(),
        Some(token::TokenError::Reconnect)
    ));
    for change in [
        "client",
        "endpoint",
        "kind",
        "invalid",
        "missing-token",
        "missing-state",
    ] {
        let mut changed = held.clone();
        let binding = changed.oauth.as_mut().unwrap();
        match change {
            "client" => binding.client_id = "another".into(),
            "endpoint" => binding.token_endpoint = "https://evil.example/token".into(),
            "kind" => binding.kind = "oauth_authorization_code".into(),
            "invalid" => binding.reconnect_required = true,
            "missing-token" => binding.refresh_token = None,
            _ => changed.oauth = None,
        }
        assert!(native.refresh(&host, &changed, 0).is_err());
    }
    assert_eq!(fake.requests.lock().unwrap().len(), 3);
    assert!(fake.opened.lock().unwrap().is_empty());
    assert!(fake.state.lock().unwrap().is_empty());
    let requests = fake.requests.lock().unwrap();
    let fields: std::collections::BTreeMap<_, _> =
        form_urlencoded::parse(&requests[1].body).collect();
    assert_eq!(fields["refresh_token"], "rotated");
    assert!(!fields.contains_key("scope"));
}

#[test]
fn an_oversized_authorization_session_releases_its_callback_before_opening_a_browser() {
    let mut config = code();
    if let OAuth::AuthorizationCode { scope_options, .. } = &mut config {
        scope_options[0].scopes = vec!["#".repeat(256); 100];
    }
    let (native, host, fake) = setup(config, vec![]);
    let start = selected(&native, &host, 0);
    assert!(native.advance(&host, &start.state, 0).is_err());
    assert_eq!(fake.canceled.lock().unwrap().as_slice(), ["handle"]);
    assert!(fake.opened.lock().unwrap().is_empty());
}

#[test]
fn native_oauth_refuses_legacy_answer_and_refresh_calls_and_disconnects_locally() {
    use crate::connector::mechanism::traits::Mechanism;
    let (native, host, fake) = setup(device(), vec![]);
    let mechanism: &dyn Mechanism = &native;
    assert!(mechanism.connect(&host, 0).is_err());
    assert!(
        mechanism
            .resume(&host, b"state", &Default::default(), 0)
            .is_err()
    );
    assert!(matches!(
        mechanism
            .refresh(&host, &Default::default(), 0)
            .unwrap_err()
            .downcast_ref::<token::TokenError>(),
        Some(token::TokenError::Reconnect)
    ));
    mechanism.revoke(&host, &Default::default(), 0).unwrap();
    assert!(fake.requests.lock().unwrap().is_empty());
    assert!(fake.opened.lock().unwrap().is_empty());
}

#[test]
fn corrupt_state_entropy_failure_and_request_bounds_never_reach_a_provider() {
    let (native, host, fake) = setup(code(), vec![]);
    for bytes in [b"corrupt".to_vec(), vec![b'x'; token::MAX_BODY + 1]] {
        assert!(native.advance(&host, &bytes, 0).is_err());
        native.cancel(&bytes);
    }
    assert!(native.start(&host, u64::MAX).is_err());
    *fake.entropy_length.lock().unwrap() = Some(31);
    assert!(native.start(&host, 0).is_err());
    *fake.entropy_length.lock().unwrap() = None;
    let start = selected(&native, &host, 0);
    *fake.entropy_length.lock().unwrap() = Some(31);
    assert!(native.advance(&host, &start.state, 0).is_err());
    assert!(
        fetch(
            &host,
            "https://auth.example/token",
            &[("refresh_token", &"a".repeat(token::MAX_BODY + 1))]
        )
        .is_err()
    );
    assert!(fake.requests.lock().unwrap().is_empty());
    assert!(fake.opened.lock().unwrap().is_empty());
    assert!(fake.state.lock().unwrap().is_empty());
}

#[test]
fn a_pending_error_cannot_hide_a_redirect_or_server_failure_status() {
    let (native, host, fake) = setup(
        device(),
        vec![
            device_response(),
            serde_json::json!({"error":"authorization_pending"}),
        ],
    );
    let start = selected(&native, &host, 0);
    let waiting = pending(native.advance(&host, &start.state, 0).unwrap());
    *fake.status.lock().unwrap() = Some(302);
    let Err(error) = native.advance(&host, &waiting.state, 2000) else {
        panic!("a redirect is not pending authorization")
    };
    assert!(error.to_string().contains("invalid polling status"));
}

#[test]
fn authorization_waits_for_an_explicit_permission_choice() {
    let (native, host, fake) = setup(device(), vec![]);
    let waiting = native.start(&host, 0).unwrap();
    assert_eq!(
        serde_json::to_value(&waiting.progress).unwrap()["kind"],
        "selecting_scopes"
    );
    assert!(fake.requests.lock().unwrap().is_empty());
    assert!(fake.opened.lock().unwrap().is_empty());
}

fn selected(native: &Native, host: &Host, now: u64) -> Pending {
    native
        .select(&native.start(host, now).unwrap().state, "read-only", now)
        .unwrap()
}

#[test]
fn choosing_provider_defaults_omits_the_scope_parameter() {
    for mut config in [device(), code()] {
        match &mut config {
            OAuth::Device { scope_options, .. }
            | OAuth::AuthorizationCode { scope_options, .. } => scope_options[0].scopes.clear(),
        }
        let (native, host, fake) = setup(
            config,
            vec![
                serde_json::json!({"device_code":"private","user_code":"ABCD","verification_uri":"https://auth.example/verify","expires_in":900}),
            ],
        );
        let start = selected(&native, &host, 0);
        native.advance(&host, &start.state, 0).unwrap();
        for request in fake.requests.lock().unwrap().iter() {
            assert!(
                !form_urlencoded::parse(&request.body).any(|(key, _)| key == "scope"),
                "provider defaults must omit scope"
            );
        }
        for url in fake.opened.lock().unwrap().iter() {
            assert!(
                !reqwest::Url::parse(url)
                    .unwrap()
                    .query_pairs()
                    .any(|(key, _)| key == "scope")
            );
        }
    }
}

#[test]
fn selected_scopes_survive_configuration_changes_and_cannot_be_selected_twice() {
    let (mut native, host, fake) = setup(
        device(),
        vec![
            serde_json::json!({"device_code":"private","user_code":"ABCD","verification_uri":"https://auth.example/verify","expires_in":900}),
            serde_json::json!({"access_token":"access","token_type":"Bearer"}),
        ],
    );
    let waiting = native.start(&host, 0).unwrap();
    assert!(native.select(&waiting.state, "unknown", 0).is_err());
    assert!(native.select(&waiting.state, "read-only", 900000).is_err());
    assert!(native.select(b"invalid", "read-only", 0).is_err());
    let start = native.select(&waiting.state, "read-only", 0).unwrap();
    assert!(native.select(&start.state, "read-only", 0).is_err());
    if let OAuth::Device { scope_options, .. } = &mut native.config {
        scope_options[0].scopes = vec!["admin".into()];
    }
    let active = pending(native.advance(&host, &start.state, 0).unwrap());
    let body = fake.requests.lock().unwrap()[0].body.clone();
    assert!(form_urlencoded::parse(&body).any(|(k, v)| k == "scope" && v == "read"));
    let Advance::Done(outcome) = native.advance(&host, &active.state, 5000).unwrap() else {
        panic!("connected")
    };
    assert_eq!(outcome.authority, ["read".into()].into());
}
