use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use cucumber::{given, then, when};
use lns_service::connector::mechanism::host::Host;
use lns_service::connector::mechanism::traits::{
    Browser, BrowserSession, Entropy, Exec, Http, Mechanism, Recorder,
};
use lns_service::connector::mechanism::wasm::Runtime;
use lns_service::connector::mechanism::{
    Answers, Bounds, CallError, ExecOutput, HttpRequest, HttpResponse, Outcome, Step,
};

use crate::world::BehaviourWorld;

const METADATA: &str = r#"{"issuer":"https://mcp.linear.app","authorization_endpoint":"https://mcp.linear.app/authorize","token_endpoint":"https://mcp.linear.app/token","registration_endpoint":"https://mcp.linear.app/register"}"#;
const TOKEN: &str = r#"{"access_token":"linear-test-access","refresh_token":"linear-test-refresh","token_type":"Bearer","expires_in":3600,"scope":"read"}"#;

#[derive(Default, Debug)]
pub struct LinearRig {
    ports: Arc<Ports>,
    outcome: Option<Outcome>,
    failure: Option<String>,
}

#[derive(Default, Debug)]
struct Ports {
    replies: Mutex<VecDeque<(u16, String)>>,
    sent: Mutex<Vec<HttpRequest>>,
    opened: Mutex<Vec<String>>,
}

impl Http for Ports {
    fn fetch(
        &self,
        request: &HttpRequest,
        _within: std::time::Duration,
    ) -> Result<HttpResponse, CallError> {
        self.sent.lock().unwrap().push(request.clone());
        let (status, body) = self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("a scripted response for each request");
        Ok(HttpResponse {
            status,
            body: body.into_bytes(),
            headers: vec![],
        })
    }
}

impl Browser for Ports {
    fn prepare(
        &self,
        _connector: &str,
        _within: std::time::Duration,
    ) -> Result<BrowserSession, CallError> {
        Ok(BrowserSession {
            handle: "browser-handle".into(),
            redirect_uri: "http://127.0.0.1:54321/callback".into(),
            state: "expected-state".into(),
        })
    }
    fn open(&self, _connector: &str, handle: &str, url: &str) -> Result<(), CallError> {
        assert_eq!(handle, "browser-handle");
        let url = reqwest::Url::parse(url).unwrap();
        let pairs: std::collections::BTreeMap<_, _> = url.query_pairs().collect();
        assert_eq!(
            pairs.get("state").map(|v| v.as_ref()),
            Some("expected-state")
        );
        assert_eq!(
            pairs.get("resource").map(|v| v.as_ref()),
            Some("https://mcp.linear.app/mcp")
        );
        assert_eq!(
            pairs.get("code_challenge_method").map(|v| v.as_ref()),
            Some("S256")
        );
        self.opened.lock().unwrap().push(url.to_string());
        Ok(())
    }
    fn poll(&self, _connector: &str, handle: &str) -> Result<Option<String>, CallError> {
        assert_eq!(handle, "browser-handle");
        Ok(Some("authorization-code".into()))
    }
}

impl Entropy for Ports {
    fn bytes(&self, count: u32) -> Vec<u8> {
        vec![42; count as usize]
    }
}
impl Exec for Ports {
    fn run(&self, _argv: &[String], _within: std::time::Duration) -> Result<ExecOutput, CallError> {
        panic!("this connector never executes host programs")
    }
}
impl Recorder for Ports {
    // no-op: this scenario observes HTTP and browser activity directly.
    fn reached(&self, _connector: &str, _host: &str, _refused: bool) {}
    // no-op: host execution is refused before the fake executor could run.
    fn ran(&self, _connector: &str, _program: &str, _refused: bool) {}
    // no-op: these scenarios drive the mechanism rather than the renewal scheduler.
    fn renewed(&self, _connector: &str, _target: &str, _refused: bool) {}
    // no-op: these scenarios do not exercise the ledger's record ceiling.
    fn elided(&self, _connector: &str, _after: u32) {}
}

fn rig(w: &mut BehaviourWorld) -> &mut LinearRig {
    w.linear_auth.as_mut().expect("Linear scenario")
}

#[given("Linear authorizes its direct API and MCP with the same token")]
fn both_interfaces(w: &mut BehaviourWorld) {
    let ports = Arc::new(Ports::default());
    *ports.replies.lock().unwrap() = [
        (200, METADATA),
        (201, r#"{"client_id":"lns-test-client"}"#),
        (200, TOKEN),
        (200, r#"{"data":{"viewer":{"id":"user-id"}}}"#),
        (
            200,
            r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-11-25"}}"#,
        ),
    ]
    .into_iter()
    .map(|(status, body)| (status, body.into()))
    .collect();
    w.linear_auth = Some(LinearRig {
        ports,
        ..Default::default()
    });
}

#[given("Linear's MCP token is refused by its direct API")]
fn mcp_only(w: &mut BehaviourWorld) {
    both_interfaces(w);
    rig(w).ports.replies.lock().unwrap()[3] = (401, "{}".into());
}

fn host(ports: &Arc<Ports>) -> Host {
    Host::new(
        "linear",
        Bounds {
            hosts: vec!["mcp.linear.app".into(), "api.linear.app".into()],
            call_seconds: 30,
            session_seconds: 900,
            exec: false,
        },
        ports.clone(),
        ports.clone(),
        ports.clone(),
        ports.clone(),
    )
    .with_browser(ports.clone())
}

#[when("I finish the Linear connector's browser sign-in")]
fn sign_in(w: &mut BehaviourWorld) {
    let rig = rig(w);
    let runtime = Runtime::new().unwrap();
    let component = runtime
        .compile(include_bytes!(
            "../../../../../connectors/linear/sign-in.wasm"
        ))
        .unwrap();
    let host = host(&rig.ports);
    let Step::Ask { state, .. } = component.connect(&host, 0).unwrap() else {
        panic!("sign-in starts with consent")
    };
    assert!(rig.ports.opened.lock().unwrap().is_empty());
    let Step::Ask { state, .. } = component.resume(&host, &state, &Answers::new(), 0).unwrap()
    else {
        panic!("sign-in waits for the browser")
    };
    match component
        .resume(&host, &state, &Answers::new(), 1000)
        .unwrap()
    {
        Step::Done(outcome) => rig.outcome = Some(outcome),
        Step::Failed(why) => rig.failure = Some(why),
        Step::Ask { .. } => panic!("completed browser authorization must finish"),
    }
}

#[when("the Linear connector renews its credential")]
fn renew(w: &mut BehaviourWorld) {
    let rig = rig(w);
    *rig.ports.replies.lock().unwrap() = [(200, TOKEN.into())].into();
    let runtime = Runtime::new().unwrap();
    let component = runtime
        .compile(include_bytes!(
            "../../../../../connectors/linear/sign-in.wasm"
        ))
        .unwrap();
    rig.outcome = Some(
        component
            .refresh(
                &host(&rig.ports),
                &[
                    ("client_id".into(), "lns-test-client".into()),
                    ("refresh_token".into(), "previous-refresh".into()),
                ]
                .into(),
                1000,
            )
            .unwrap(),
    );
}

#[then("the Linear connection contains renewable credentials")]
fn renewable(w: &mut BehaviourWorld) {
    let rig = rig(w);
    assert_eq!(rig.failure, None);
    let outcome = rig.outcome.as_ref().expect("connected");
    assert_eq!(outcome.values["access_token"], "linear-test-access");
    assert_eq!(outcome.values["refresh_token"], "linear-test-refresh");
    assert_eq!(outcome.values["client_id"], "lns-test-client");
    assert_eq!(outcome.expires_at_millis, Some(3_601_000));
    assert_eq!(outcome.authority, ["read".to_string()].into());
}

#[then("both Linear interfaces were checked with the same token")]
fn same_token(w: &mut BehaviourWorld) {
    let sent = rig(w).ports.sent.lock().unwrap();
    for url in [
        "https://api.linear.app/graphql",
        "https://mcp.linear.app/mcp",
    ] {
        let request = sent
            .iter()
            .find(|r| r.url == url)
            .expect("each interface was checked");
        assert!(
            request
                .headers
                .contains(&("Authorization".into(), "Bearer linear-test-access".into()))
        );
    }
    let exchange = sent
        .iter()
        .find(|r| r.url == "https://mcp.linear.app/token")
        .unwrap();
    let form: std::collections::BTreeMap<_, _> = form_urlencoded::parse(&exchange.body).collect();
    assert_eq!(form["code"], "authorization-code");
    assert_eq!(form["code_verifier"].len(), 43);
}

#[then("the Linear connector refuses to save general API access")]
fn refuses(w: &mut BehaviourWorld) {
    let rig = rig(w);
    assert!(rig.outcome.is_none());
    assert!(
        rig.failure
            .as_ref()
            .expect("refused")
            .contains("The direct API check failed: Linear refused the request (HTTP 401).")
    );
}

#[then("no browser was opened for renewal")]
fn no_browser(w: &mut BehaviourWorld) {
    assert!(rig(w).ports.opened.lock().unwrap().is_empty());
}
