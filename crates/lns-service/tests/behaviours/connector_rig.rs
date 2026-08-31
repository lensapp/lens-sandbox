use std::collections::BTreeMap;

use lns_policy::decision_store::JsonDecisionStore;
use lns_service::connector::dir::ConnectorDir;
use lns_service::connector::store::{Connection, ConnectorStore, Installed, RunDecision};

/// The machine a connector scenario installs onto, plus the document it is building up.
pub struct ConnectorRig {
    _tmp: tempfile::TempDir,
    dir: ConnectorDir,
    values: JsonDecisionStore<Connection>,
    grants: JsonDecisionStore<RunDecision>,

    name: String,
    serves: Vec<String>,
    methods: Vec<serde_json::Value>,

    pub error: Option<String>,
    pub listed: Vec<Installed>,
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
            name: String::new(),
            serves: Vec::new(),
            methods: Vec::new(),
            error: None,
            listed: Vec::new(),
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
            .install(&digest, &document, &[])
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
        self.store()
            .decide(
                run,
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
        lns_service::connector::handler::offerable(&self.store(), run)
            .expect("read what this run is offered")
            .into_iter()
            .map(|offer| offer.name)
            .collect()
    }

    pub fn granted_method(&self, run: &str, name: &str) -> Option<String> {
        match self.store().decision(run, name).expect("read the decision") {
            Some(RunDecision::Granted { method, .. }) => Some(method),
            _ => None,
        }
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
