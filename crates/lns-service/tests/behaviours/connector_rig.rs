use std::collections::BTreeMap;

use lns_policy::decision_store::JsonDecisionStore;
use lns_service::connector::dir::ConnectorDir;
use lns_service::connector::store::{
    Connection, ConnectorStore, GrantHolder, Installed, RunDecision,
};

/// Layer 2 forbids the network, a subprocess and a real clock — not only their use, so the rig does not hold the capability at all.
struct ReachesNothing;

impl lns_service::connector::mechanism::traits::Http for ReachesNothing {
    fn fetch(
        &self,
        _request: &lns_service::connector::mechanism::HttpRequest,
        _within: std::time::Duration,
    ) -> Result<
        lns_service::connector::mechanism::HttpResponse,
        lns_service::connector::mechanism::CallError,
    > {
        Err(lns_service::connector::mechanism::CallError::Refused(
            "this rig reaches nothing".to_string(),
        ))
    }
}

impl lns_service::connector::mechanism::traits::Exec for ReachesNothing {
    fn run(
        &self,
        _argv: &[String],
        _within: std::time::Duration,
    ) -> Result<
        lns_service::connector::mechanism::ExecOutput,
        lns_service::connector::mechanism::CallError,
    > {
        Err(lns_service::connector::mechanism::CallError::Refused(
            "this rig starts nothing".to_string(),
        ))
    }
}

impl lns_service::connector::mechanism::traits::Entropy for ReachesNothing {
    fn bytes(&self, count: u32) -> Vec<u8> {
        vec![0; count as usize]
    }
}

impl lns_service::connector::mechanism::traits::Recorder for ReachesNothing {
    // no-op: what a mechanism reached is a Layer 2 concern of the ledger feature, not of this rig.
    fn reached(&self, _connector: &str, _host: &str, _refused: bool) {}

    // no-op: as above.
    fn ran(&self, _connector: &str, _program: &str, _refused: bool) {}
}

/// Every scenario names a run by a short id, so the rig speaks the one holder kind it needs.
fn holder(run: &str) -> GrantHolder {
    GrantHolder::Run(run.to_string())
}

/// The machine a connector scenario installs onto, plus the document it is building up.
pub struct ConnectorRig {
    _tmp: tempfile::TempDir,
    dir: ConnectorDir,
    values: JsonDecisionStore<Connection>,
    grants: JsonDecisionStore<RunDecision>,

    name: String,
    serves: Vec<String>,
    methods: Vec<serde_json::Value>,

    mechanisms: lns_service::connector::mechanism::real::RealMechanisms,
    sessions: lns_service::connector::session::InMemorySessions,

    pub error: Option<String>,
    pub listed: Vec<Installed>,
    /// What a run's boot counted this machine's connectors would write; `None` is a holder with no boot behind it.
    pub counted: Option<Vec<String>>,
}

impl std::fmt::Debug for ConnectorRig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectorRig")
            .field("name", &self.name)
            .field("error", &self.error)
            .finish()
    }
}

impl Default for ConnectorRig {
    fn default() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        Self {
            dir: ConnectorDir::new(tmp.path().join("connectors")),
            values: JsonDecisionStore::new(tmp.path().join("connector-values.json")),
            grants: JsonDecisionStore::new(tmp.path().join("connector-grants.json")),
            _tmp: tmp,
            mechanisms: lns_service::connector::mechanism::real::RealMechanisms::lending(
                std::sync::Arc::new(ReachesNothing),
                std::sync::Arc::new(ReachesNothing),
                std::sync::Arc::new(ReachesNothing),
                std::sync::Arc::new(ReachesNothing),
            )
            .expect("the component runtime starts"),
            sessions: lns_service::connector::session::InMemorySessions::default(),
            name: String::new(),
            serves: Vec::new(),
            methods: Vec::new(),
            error: None,
            listed: Vec::new(),
            counted: None,
        }
    }
}

impl ConnectorRig {
    pub fn store(&self) -> ConnectorStore<'_> {
        ConnectorStore::new(&self.dir, &self.values, &self.grants)
    }

    /// Starts a fresh document, so a scenario that installs twice describes the second one from scratch.
    pub fn declare(&mut self, name: &str, serves: &str) {
        self.name = name.to_string();
        self.serves = vec![serves.to_string()];
        self.methods.clear();
    }

    pub fn add_method(&mut self, method: serde_json::Value) {
        self.methods.push(method);
    }

    pub fn document(&self) -> Vec<u8> {
        serde_json::json!({
            "apiVersion": "lns.run/v1",
            "kind": "connector",
            "name": self.name,
            "spec": { "serves": self.serves, "methods": self.methods() },
        })
        .to_string()
        .into_bytes()
    }

    /// Every connector declares a method, so a scenario that only cares about `serves` gets a payload-free one rather than a line of noise per scenario.
    fn methods(&self) -> Vec<serde_json::Value> {
        if self.methods.is_empty() {
            return vec![serde_json::json!({
                "name": "token",
                "auth": { "kind": "token" },
            })];
        }
        self.methods.clone()
    }

    /// The digest stands for the bytes, so it moves whenever the document does.
    pub fn digest(&self) -> String {
        format!("sha256:{:016x}", fxhash(&self.document()))
    }

    pub fn install(&mut self) {
        let digest = self.digest();
        let document = self.document();
        self.error = self
            .store()
            .install(&digest, &document, &[], &[])
            .err()
            .map(|e| format!("{e:#}"));
    }

    /// The connector this scenario last described, which is the one its later steps mean.
    pub fn last_declared_name(&self) -> Option<String> {
        (!self.name.is_empty()).then(|| self.name.clone())
    }

    pub fn installed_named(&self, name: &str) -> Option<Installed> {
        self.store()
            .installed()
            .expect("read the installed set")
            .into_iter()
            .find(|entry| entry.name == name)
    }

    pub fn methods_of(&self, name: &str) -> Vec<lns_artifact::connector::Method> {
        let entry = self.installed_named(name).expect("installed");
        lns_artifact::connector::parse(&entry.document)
            .expect("a stored document parses")
            .spec
            .methods
    }

    pub fn grant(&self, run: &str, name: &str, method: &str) {
        let holder = holder(run);
        self.store()
            .decide(
                &holder,
                name,
                RunDecision::Granted {
                    digest: self.digest(),
                    method: method.to_string(),
                    connection: None,
                    authority: Default::default(),
                },
            )
            .expect("record the grant");
    }

    /// What one run is still offered, through the same path the card reads (§3.2.1).
    pub fn offered_to(&self, run: &str) -> Vec<String> {
        lns_service::connector::handler::offerable(&self.store(), &holder(run))
            .expect("read what this run is offered")
            .into_iter()
            .map(|offer| offer.name)
            .collect()
    }

    pub fn granted_method(&self, run: &str, name: &str) -> Option<String> {
        match self
            .store()
            .decision(&holder(run), name)
            .expect("read the decision")
        {
            Some(RunDecision::Granted { method, .. }) => Some(method),
            _ => None,
        }
    }

    /// Connects through the real driver and the real mechanism, because a connect that stored a connection itself would prove nothing about what produces one.
    pub fn connect(&mut self, method: &str, value: &str) {
        let name = self.name.clone();
        let values = self
            .asks_of(&name, method)
            .into_iter()
            .map(|ask| (ask, value.to_string()))
            .collect();
        let driver = lns_service::connector::connect::Driver {
            store: self.store(),
            mechanisms: &self.mechanisms,
            sessions: &self.sessions,
            now_millis: 0,
        };
        self.error = match driver.with_values(&name, method, "work", values) {
            Ok(lns_service::connector::connect::Connecting::Connected(_)) => None,
            Ok(turn) => Some(format!("{turn:?}")),
            Err(e) => Some(format!("{e:#}")),
        };
    }

    fn asks_of(&self, name: &str, method: &str) -> Vec<String> {
        lns_service::connector::handler::list(&self.store())
            .expect("list the installed set")
            .into_iter()
            .find(|connector| connector.name == name)
            .unwrap_or_else(|| panic!("{name} is not installed"))
            .methods
            .into_iter()
            .find(|view| view.name == method)
            .unwrap_or_else(|| panic!("{name} declares no method {method}"))
            .asks
    }

    pub fn grant_through_the_connection(&mut self, run: &str, method: &str) {
        let name = self.name.clone();
        self.error = lns_service::connector::handler::grant(
            &self.store(),
            &name,
            &holder(run),
            method,
            Some("work"),
            self.counted.as_deref(),
        )
        .err()
        .map(|e| format!("{e:#}"));
    }

    /// What the boundary is armed with for one variable, read through the same path a starting run reads (§7.1).
    pub fn supplied_to(&self, run: &str, variable: &str) -> Option<String> {
        lns_service::connector::handler::granted_supply(&self.store(), &holder(run))
            .expect("read what this run granted")
            .values()
            .flat_map(|payload| payload.credentials.clone())
            .find(|credential| credential.env_var.as_deref() == Some(variable))
            .and_then(|credential| {
                credential
                    .injections
                    .first()
                    .map(|injection| injection.value().to_string())
            })
    }

    pub fn connections_of(&self, name: &str) -> BTreeMap<String, Connection> {
        self.store()
            .connections_of(name)
            .expect("read the connections")
    }
}

/// A short stable digest for a scenario's bytes; nothing here verifies content-addressing, only that one document keeps one digest.
fn fxhash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |acc, b| {
        (acc ^ u64::from(*b)).wrapping_mul(0x100000001b3)
    })
}
