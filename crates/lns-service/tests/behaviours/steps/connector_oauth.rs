use crate::oauth_rig::OAuthRig;
use crate::world::BehaviourWorld;
use cucumber::{given, then, when};

#[given(expr = "a public client using native {string} OAuth")]
fn registered(w: &mut BehaviourWorld, flow: String) {
    w.oauth = Some(OAuthRig::new(&flow));
}
#[when("the user starts native authorization")]
fn start(w: &mut BehaviourWorld) {
    let rig = w.oauth.as_mut().unwrap();
    rig.waiting = Some(rig.native.start(&rig.host, rig.now).unwrap());
}
#[then("native authorization has made no provider request yet")]
fn no_request(w: &mut BehaviourWorld) {
    assert!(
        w.oauth
            .as_ref()
            .unwrap()
            .fake
            .requests
            .lock()
            .unwrap()
            .is_empty()
    );
}
#[when("the service advances native authorization")]
fn advance(w: &mut BehaviourWorld) {
    w.oauth.as_mut().unwrap().advance();
}
#[then("LNS presents structured authorization progress")]
fn progress(w: &mut BehaviourWorld) {
    let progress = &w.oauth.as_ref().unwrap().waiting.as_ref().unwrap().progress;
    assert!(matches!(
        progress,
        lns_ipc::OAuthProgress::DeviceAuthorization { .. }
            | lns_ipc::OAuthProgress::WaitingForBrowser { .. }
    ));
    assert!(!format!("{progress:?}").contains("private-device"));
}
#[when("the provider authorizes the native OAuth operation")]
fn authorize(w: &mut BehaviourWorld) {
    w.oauth.as_mut().unwrap().authorize();
}
#[then("native authorization supplies one public access token and private renewal state")]
fn connected(w: &mut BehaviourWorld) {
    let result = w.oauth.as_ref().unwrap().outcome.as_ref().unwrap();
    assert_eq!(
        result.values.keys().map(String::as_str).collect::<Vec<_>>(),
        ["access_token"]
    );
    assert_eq!(
        result.oauth.as_ref().unwrap().refresh_token.as_deref(),
        Some("refresh")
    );
    assert_eq!(
        result
            .authority
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["read"]
    );
}

#[then("the native connector installs without a component or an account connection")]
fn install(w: &mut BehaviourWorld) {
    let rig = w.oauth.as_ref().unwrap();
    let view = rig.install();
    assert!(view.connections.is_empty());
    assert!(view.methods[0].offerable);
    assert!(view.methods[0].oauth.is_some());
    assert!(!view.methods[0].carries_code);
    assert!(rig.fake.requests.lock().unwrap().is_empty());
    assert!(rig.fake.opened.lock().unwrap().is_empty());
}

#[when(expr = "the user chooses native permission preset {string}")]
fn select(w: &mut BehaviourWorld, name: String) {
    let rig = w.oauth.as_mut().unwrap();
    let waiting = rig.waiting.as_ref().unwrap();
    rig.waiting = Some(rig.native.select(&waiting.state, &name, rig.now).unwrap());
}

#[then(expr = "the native provider is asked for scopes {string}")]
fn requested(w: &mut BehaviourWorld, expected: String) {
    let rig = w.oauth.as_ref().unwrap();
    let requests = rig.fake.requests.lock().unwrap();
    let params: std::collections::BTreeMap<String, String> = if let Some(request) = requests.first()
    {
        form_urlencoded::parse(&request.body)
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect()
    } else {
        reqwest::Url::parse(&rig.fake.opened.lock().unwrap()[0])
            .unwrap()
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect()
    };
    assert_eq!(params["scope"], expected);
}

#[then(expr = "native authorization records authority {string}")]
fn authority(w: &mut BehaviourWorld, expected: String) {
    assert_eq!(
        w.oauth
            .as_ref()
            .unwrap()
            .outcome
            .as_ref()
            .unwrap()
            .authority,
        expected.split(' ').map(str::to_string).collect()
    );
}

#[given("the host cannot open a local browser")]
fn no_browser(w: &mut BehaviourWorld) {
    w.oauth
        .as_ref()
        .unwrap()
        .fake
        .browser_unavailable
        .store(true, std::sync::atomic::Ordering::Relaxed);
}
