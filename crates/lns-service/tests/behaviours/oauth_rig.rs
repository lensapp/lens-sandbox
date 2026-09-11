use anyhow::Result;
use lns_artifact::connector::oauth::{OAuth, Redirect};
use lns_service::connector::mechanism::host::Host;
use lns_service::connector::mechanism::oauth::browser::Browser;
use lns_service::connector::mechanism::oauth::browser::Callback;
use lns_service::connector::mechanism::oauth::flow::Native;
use lns_service::connector::mechanism::oauth::{Advance, Pending};
use lns_service::connector::mechanism::traits::{Entropy, Exec, Http, Recorder};
use lns_service::connector::mechanism::{Bounds, CallError, HttpRequest, HttpResponse};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub(crate) struct Fake {
    pub(crate) replies: Mutex<VecDeque<serde_json::Value>>,
    pub(crate) requests: Mutex<Vec<HttpRequest>>,
    pub(crate) opened: Mutex<Vec<String>>,
    callback: Mutex<Option<String>>,
    state: Mutex<String>,
    pub(crate) canceled: Mutex<Vec<String>>,
}
impl Http for Fake {
    fn fetch(&self, r: &HttpRequest, _: std::time::Duration) -> Result<HttpResponse, CallError> {
        self.requests.lock().unwrap().push(r.clone());
        Ok(HttpResponse {
            status: 200,
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
    ) -> Result<lns_service::connector::mechanism::ExecOutput, CallError> {
        panic!("native OAuth must not execute a program through the Wasm host")
    }
}
impl Entropy for Fake {
    fn bytes(&self, n: u32) -> Vec<u8> {
        vec![42; n as usize]
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
        *self.state.lock().unwrap() = state.into();
        Ok((
            "handle".into(),
            format!("http://127.0.0.1:{}{path}", port.unwrap_or(40000)),
        ))
    }
    fn open(&self, url: &str) -> Result<()> {
        self.opened.lock().unwrap().push(url.into());
        Ok(())
    }
    fn poll(&self, _: &str, _: &str, _: u64) -> Result<Callback> {
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
        scopes: vec!["read".into()],
    }
}

pub struct OAuthRig {
    pub native: Native,
    pub host: Host,
    pub fake: Arc<Fake>,
    pub waiting: Option<Pending>,
    pub outcome: Option<lns_service::connector::mechanism::Outcome>,
    pub now: u64,
}
impl std::fmt::Debug for OAuthRig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OAuthRig(<redacted>)")
    }
}
impl OAuthRig {
    pub fn new(flow: &str) -> Self {
        let config = match flow {
            "device" => device(),
            "authorization code" => OAuth::AuthorizationCode {
                client_id: "public-id".into(),
                token_endpoint: "https://auth.example/token".into(),
                authorization_endpoint: "https://auth.example/authorize".into(),
                redirect: Redirect::Loopback {
                    path: "/callback".into(),
                    port: Some(53682),
                },
                scopes: vec!["read".into()],
            },
            _ => panic!("unknown test flow"),
        };
        let mut replies = vec![];
        if flow == "device" {
            replies.push(serde_json::json!({"device_code":"private-device","user_code":"ABCD","verification_uri":"https://auth.example/verify","expires_in":900}));
        }
        replies.push(serde_json::json!({"access_token":"access","token_type":"Bearer","refresh_token":"refresh"}));
        let (native, host, fake) = setup(config, replies);
        Self {
            native,
            host,
            fake,
            waiting: None,
            outcome: None,
            now: 0,
        }
    }
    pub fn advance(&mut self) {
        let waiting = self.waiting.take().expect("operation started");
        match self
            .native
            .advance(&self.host, &waiting.state, self.now)
            .unwrap()
        {
            Advance::Pending(next) => self.waiting = Some(next),
            Advance::Done(outcome) => self.outcome = Some(outcome),
        }
    }
    pub fn authorize(&mut self) {
        *self.fake.callback.lock().unwrap() = Some("one-time-code".into());
        self.now = 5000;
    }
}

#[derive(Default)]
struct Installed(Mutex<Vec<lns_service::connector::store::Installed>>);
impl lns_service::connector::store::InstalledSet for Installed {
    fn list(&self) -> std::io::Result<Vec<lns_service::connector::store::Installed>> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn put(
        &self,
        name: &str,
        digest: &str,
        document: &[u8],
        filesets: &[Vec<u8>],
        components: &[Vec<u8>],
    ) -> std::io::Result<()> {
        assert!(filesets.is_empty());
        assert!(components.is_empty());
        self.0
            .lock()
            .unwrap()
            .push(lns_service::connector::store::Installed {
                name: name.into(),
                digest: digest.into(),
                document: document.into(),
            });
        Ok(())
    }
    fn fileset_layer(&self, _: &str, _: usize) -> std::io::Result<Vec<u8>> {
        panic!("these native connectors need no fileset")
    }
    fn component(&self, _: &str, _: usize) -> std::io::Result<Vec<u8>> {
        panic!("native OAuth must not load a component")
    }
    fn remove(&self, _: &str) -> std::io::Result<bool> {
        panic!("this scenario installs a connector")
    }
}
struct Decisions<T>(Mutex<lns_policy::decision_store::DecisionFile<T>>);
impl<T: Clone + Send + Sync> lns_policy::decision_store::DecisionStore<T> for Decisions<T> {
    fn load(&self) -> std::io::Result<lns_policy::decision_store::DecisionFile<T>> {
        Ok(self.0.lock().unwrap().clone())
    }
    fn save(&self, values: &lns_policy::decision_store::DecisionFile<T>) -> std::io::Result<()> {
        *self.0.lock().unwrap() = values.clone();
        Ok(())
    }
}
impl OAuthRig {
    pub fn install(&self) -> lns_ipc::ConnectorView {
        let installed = Installed::default();
        let values = Decisions(Mutex::new(Default::default()));
        let grants = Decisions(Mutex::new(Default::default()));
        let store =
            lns_service::connector::store::ConnectorStore::new(&installed, &values, &grants);
        let document = serde_json::json!({"apiVersion":"lns.run/v1","kind":"connector","name":"native-provider","spec":{"serves":["api.example.test"],"methods":[{"name":"sign-in","auth":self.native.config,"credentials":[{"envVar":"API_TOKEN","placeholder":"LNSPLACEHOLDER0000000000","injections":[{"kind":"bearer_header","domain":"api.example.test"}]}]}]}});
        store
            .install(
                "sha256:fixture",
                &serde_json::to_vec(&document).unwrap(),
                &[],
                &[],
            )
            .unwrap();
        lns_service::connector::handler::list(&store)
            .unwrap()
            .remove(0)
    }
}
