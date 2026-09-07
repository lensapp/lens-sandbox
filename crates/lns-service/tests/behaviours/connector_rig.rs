use std::collections::BTreeMap;

use lns_policy::decision_store::JsonDecisionStore;
use lns_service::connector::dir::ConnectorDir;
use lns_service::connector::store::{
    Connection, ConnectorStore, GrantHolder, Installed, RunDecision,
};

/// Layer 2 stamps a record without a real clock, so every entry this rig writes carries the same moment.
struct Stopped;

impl lns_service::clock::Clock for Stopped {
    fn now_unix(&self) -> u64 {
        1_780_000_000
    }
}

/// What lns lends a component here: a network and a subprocess Layer 2 forbids, answered by the rig, and the durable ledger production writes, at this rig's own paths.
pub struct Lent {
    ledger: std::path::PathBuf,
    anchor: std::path::PathBuf,
    pub fetched: std::sync::Mutex<Vec<String>>,
    pub started: std::sync::Mutex<Vec<Vec<String>>>,
}

impl Lent {
    fn new(under: &std::path::Path) -> Self {
        Self {
            ledger: under.join("ledger.jsonl"),
            anchor: under.join("ledger.anchor"),
            fetched: std::sync::Mutex::new(Vec::new()),
            started: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// The durable ledger as `lns audit` would merge it (§7.1).
    pub fn ledger(&self) -> Vec<lns_audit::Row> {
        if !self.ledger.exists() {
            return Vec::new();
        }
        lns_audit::stream_ledger(&self.ledger)
            .expect("read the ledger this rig wrote")
            .map(|event| lns_audit::read(&event.expect("one ledger event")).expect("one row"))
            .collect()
    }

    /// The entry production builds, at this rig's own paths, so what it says is production's to decide and only where it lands is the rig's.
    fn wrote(&self, connector: &str, verb: &str, target: &str, refused: bool) {
        lns_service::ledger::append_machine_event_at(
            &self.ledger,
            &self.anchor,
            lns_service::ledger::mechanism_event(&Stopped, connector, verb, target, refused),
        )
        .expect("write the entry this call leaves behind");
    }
}

impl lns_service::connector::mechanism::traits::Http for Lent {
    fn fetch(
        &self,
        request: &lns_service::connector::mechanism::HttpRequest,
        _within: std::time::Duration,
    ) -> Result<
        lns_service::connector::mechanism::HttpResponse,
        lns_service::connector::mechanism::CallError,
    > {
        self.fetched
            .lock()
            .expect("http lock")
            .push(request.url.clone());
        Ok(lns_service::connector::mechanism::HttpResponse {
            status: 200,
            headers: Vec::new(),
            body: b"ok".to_vec(),
        })
    }
}

impl lns_service::connector::mechanism::traits::Exec for Lent {
    fn run(
        &self,
        argv: &[String],
        _within: std::time::Duration,
    ) -> Result<
        lns_service::connector::mechanism::ExecOutput,
        lns_service::connector::mechanism::CallError,
    > {
        self.started.lock().expect("exec lock").push(argv.to_vec());
        Ok(lns_service::connector::mechanism::ExecOutput {
            code: 0,
            stdout: b"ran".to_vec(),
            stderr: Vec::new(),
        })
    }
}

impl lns_service::connector::mechanism::traits::Entropy for Lent {
    fn bytes(&self, count: u32) -> Vec<u8> {
        vec![0; count as usize]
    }
}

impl lns_service::connector::mechanism::traits::Recorder for Lent {
    fn reached(&self, connector: &str, host: &str, refused: bool) {
        self.wrote(connector, "reached", host, refused);
    }

    fn ran(&self, connector: &str, program: &str, refused: bool) {
        self.wrote(connector, "ran", program, refused);
    }

    fn renewed(&self, connector: &str, target: &str, refused: bool) {
        self.wrote(connector, "renewed", target, refused);
    }

    fn elided(&self, connector: &str, after: u32) {
        self.wrote(
            connector,
            "elided",
            &format!("the rest of this call after {after} entries"),
            false,
        );
    }
}

/// The machine's mechanisms, counting how many times one was made ready to call, because a press may move that count and nothing a workload does may (§3.2.6).
pub struct Counted {
    inner: lns_service::connector::mechanism::real::RealMechanisms,
    calls: std::sync::atomic::AtomicUsize,
}

impl Counted {
    pub fn calls(&self) -> usize {
        self.calls.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl lns_service::connector::mechanism::traits::Mechanisms for Counted {
    fn for_method(
        &self,
        connector: &str,
        method: &lns_artifact::connector::Method,
        component: Option<Vec<u8>>,
    ) -> anyhow::Result<lns_service::connector::mechanism::traits::Prepared> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.inner.for_method(connector, method, component)
    }
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
    /// Where each `code` method's component sits on disk, in the order the document declares them (§7.1).
    components: Vec<std::path::PathBuf>,

    pub mechanisms: Counted,
    sessions: lns_service::connector::session::InMemorySessions,
    schedule: lns_service::connector::refresh::Schedule,
    pub lent: std::sync::Arc<Lent>,

    /// The connect this rig is part-way through, so a scenario can answer what a component asked for.
    pub asking: Option<lns_service::connector::connect::Connecting>,
    /// What the first round asked for, kept because a later round answers it and the scenario still reads it.
    first_ask: Option<(String, Vec<lns_service::connector::mechanism::Field>)>,
    /// What the user types into every field a mechanism asks for.
    answer_with: String,
    /// Whether this machine could start one at all; a press must still be answered where it could not.
    holds_a_component_runtime: bool,
    /// The component bytes the first install captured, which an edit to the file afterwards must not reach.
    pub bytes_captured_at_install: Option<Vec<u8>>,
    /// The digest this machine recorded for them, which a reinstall of changed bytes must move.
    pub digest_at_install: Option<String>,
    /// The field labels a scenario says the component asks for, so the words in the feature are the ones checked.
    pub labels_the_scenario_names: Vec<String>,
    /// What each run granted, so a reinstall these scenarios ask for does not read as an update nobody consented to.
    granted: Vec<(String, String)>,
    /// How many times a mechanism had been made ready before the workload's request, so the request itself can be shown to move nothing.
    pub mechanism_calls_before_the_request: usize,
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
        let lent = std::sync::Arc::new(Lent::new(tmp.path()));
        Self {
            dir: ConnectorDir::new(tmp.path().join("connectors")),
            values: JsonDecisionStore::new(tmp.path().join("connector-values.json")),
            grants: JsonDecisionStore::new(tmp.path().join("connector-grants.json")),
            _tmp: tmp,
            mechanisms: Counted {
                inner: lns_service::connector::mechanism::real::RealMechanisms::lending(
                    lent.clone(),
                    lent.clone(),
                    lent.clone(),
                    lent.clone(),
                )
                .expect("the component runtime starts"),
                calls: std::sync::atomic::AtomicUsize::new(0),
            },
            sessions: lns_service::connector::session::InMemorySessions::default(),
            schedule: Default::default(),
            lent,
            name: String::new(),
            serves: Vec::new(),
            methods: Vec::new(),
            components: Vec::new(),
            asking: None,
            first_ask: None,
            answer_with: "pasted".to_string(),
            holds_a_component_runtime: true,
            bytes_captured_at_install: None,
            digest_at_install: None,
            labels_the_scenario_names: Vec::new(),
            granted: Vec::new(),
            mechanism_calls_before_the_request: 0,
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
        self.components.clear();
    }

    pub fn add_method(&mut self, method: serde_json::Value) {
        self.methods.push(method);
    }

    /// A `code` method and the component it connects with, written to a real file, because what an install captures and what is on disk afterwards are the difference §7.1 turns on.
    pub fn add_code_method(&mut self, method: serde_json::Value, fixture: &str) {
        self.methods.push(method);
        self.components
            .push(self.component_path(self.components.len()));
        self.the_component_changes(fixture);
    }

    /// Writes new bytes to the file the method already names, the way editing it on disk does.
    pub fn the_component_changes(&mut self, fixture: &str) {
        let path = self
            .components
            .last()
            .cloned()
            .expect("a code method must be described before its component changes");
        std::fs::write(path, component(fixture)).expect("write the component this method names");
    }

    fn component_path(&self, index: usize) -> std::path::PathBuf {
        self._tmp.path().join(format!("component-{index}.wasm"))
    }

    /// What is on disk right now, which an install captures and a later edit does not reach back into.
    pub fn components(&self) -> Vec<Vec<u8>> {
        self.components
            .iter()
            .map(|path| std::fs::read(path).expect("read the component on disk"))
            .collect()
    }

    /// The bytes this machine kept for the one component it holds, read back through the store.
    pub fn component_captured(&self) -> Vec<u8> {
        self.store()
            .component(&self.name, 0)
            .expect("this machine captured a component")
    }

    pub fn carries_a_component(&self) -> bool {
        !self.components.is_empty()
    }

    /// What a user who answers an ask with nothing gives a mechanism lns implements, which is the way that one fails.
    pub fn answers_nothing(&mut self) {
        self.answer_with = String::new();
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

    /// The digest stands for every byte captured, so it moves whenever the document or a component does (§7.1).
    pub fn digest(&self) -> String {
        let mut covered = self.document();
        for component in self.components() {
            covered.extend_from_slice(&component);
        }
        format!("sha256:{:016x}", fxhash(&covered))
    }

    pub fn install(&mut self) {
        let digest = self.digest();
        let document = self.document();
        let components = self.components();
        self.error = self
            .store()
            .install(&digest, &document, &[], &components)
            .err()
            .map(|e| format!("{e:#}"));
    }

    /// Installs the way `lns connector install` does, so the refusals that depend on where the bytes came from are the ones under test (§3.2.6).
    pub async fn install_from(&mut self, operand: &str) {
        let source = OneConnector {
            digest: self.digest(),
            document: self.document(),
            components: self.components(),
        };
        self.error = lns_service::connector::handler::install(&self.store(), &source, operand)
            .await
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

    pub fn grant(&mut self, run: &str, name: &str, method: &str) {
        self.remember_the_grant(run, method);
        self.record_the_grant(run, name, method);
    }

    /// One entry per run and method, so swapping a component twice does not re-record the same grant twice.
    fn remember_the_grant(&mut self, run: &str, method: &str) {
        let held = (run.to_string(), method.to_string());
        if !self.granted.contains(&held) {
            self.granted.push(held);
        }
    }

    fn record_the_grant(&self, run: &str, name: &str, method: &str) {
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

    fn driver(&self, now_millis: u64) -> lns_service::connector::connect::Driver<'_> {
        lns_service::connector::connect::Driver {
            store: self.store(),
            mechanisms: &self.mechanisms,
            sessions: &self.sessions,
            now_millis,
        }
    }

    /// Begins a connect and keeps whatever it answered, so a scenario can read the ask the component decided on.
    pub fn begin(&mut self, method: &str) {
        let name = self.name.clone();
        match self.driver(0).begin(&name, method, "work") {
            Ok(connecting) => {
                self.error = match &connecting {
                    lns_service::connector::connect::Connecting::Failed(why) => Some(why.clone()),
                    _ => None,
                };
                if let lns_service::connector::connect::Connecting::Asks {
                    message, fields, ..
                } = &connecting
                {
                    self.first_ask = Some((message.clone(), fields.clone()));
                }
                self.asking = Some(connecting);
            }
            Err(e) => {
                self.error = Some(format!("{e:#}"));
                self.asking = None;
            }
        }
    }

    /// Answers the ask this rig is holding with what the user types.
    pub fn answer_held(&mut self) {
        let held = self.answer_with.clone();
        self.answer(&held);
    }

    /// Answers the ask this rig is holding, with a value for each field it named.
    pub fn answer(&mut self, value: &str) {
        let lns_service::connector::connect::Connecting::Asks {
            session, fields, ..
        } = self.asking.as_ref().expect("nothing has asked anything")
        else {
            panic!("this connect is not asking, so there is nothing to answer");
        };
        let (session, values) = (
            session.clone(),
            fields
                .iter()
                .map(|field| (field.name.clone(), value.to_string()))
                .collect(),
        );
        match self.driver(0).answer(&session, values) {
            Ok(turn) => {
                self.error = match &turn.connecting {
                    lns_service::connector::connect::Connecting::Failed(why) => Some(why.clone()),
                    _ => None,
                };
                self.asking = Some(turn.connecting);
            }
            Err(e) => {
                self.error = Some(format!("{e:#}"));
                self.asking = None;
            }
        }
    }

    /// What the component asked for this round, or nothing where it asked for nothing.
    pub fn fields_asked_for(&self) -> Vec<lns_service::connector::mechanism::Field> {
        match self.asking.as_ref() {
            Some(lns_service::connector::connect::Connecting::Asks { fields, .. }) => {
                fields.clone()
            }
            _ => Vec::new(),
        }
    }

    /// What the first round asked for, which is what the card drew even where a later round has answered it.
    pub fn asked_first(&self) -> Vec<lns_service::connector::mechanism::Field> {
        self.first_ask
            .as_ref()
            .map(|(_, fields)| fields.clone())
            .unwrap_or_default()
    }

    pub fn message_shown(&self) -> String {
        self.first_ask
            .as_ref()
            .map(|(message, _)| message.clone())
            .unwrap_or_default()
    }

    /// Swaps the component and installs it again, so what runs next is the new one. The grant a run already holds is left bound to the bytes it consented to, which is why only a scenario that reads no grant may use this.
    pub fn the_component_becomes(&mut self, fixture: &str) {
        self.the_component_changes(fixture);
        self.install();
        assert_eq!(self.error, None, "the swapped component installs");
    }

    /// Names the component this connector carries, before anything has connected with it. Not an edit to an installed connector: the digest stands, so no grant is disturbed.
    pub fn its_component_is(&mut self, fixture: &str) {
        assert!(
            self.granted.is_empty(),
            "which component a scenario describes is decided before a run grants it; use a reinstall to describe an edit"
        );
        self.the_component_changes(fixture);
        self.install();
        assert_eq!(self.error, None, "the component this method names installs");
    }

    pub fn is_connected(&self) -> bool {
        matches!(
            self.asking,
            Some(lns_service::connector::connect::Connecting::Connected(_))
        )
    }

    /// One pass of the schedule lns keeps, over this rig's one connector (§3.2.6).
    pub fn refresh_at(&mut self, now_millis: u64) -> Vec<String> {
        let name = self.name.clone();
        let renewed = lns_service::connector::refresh::once(
            &self.store(),
            &self.mechanisms,
            self.lent.as_ref(),
            &self.schedule,
            &name,
            now_millis,
        );
        match renewed {
            Ok(renewed) => {
                self.error = None;
                renewed
            }
            Err(e) => {
                self.error = Some(format!("{e:#}"));
                Vec::new()
            }
        }
    }

    /// Which connections the schedule would reach at this moment, which is what a floor bounds.
    pub fn due_at(&self, now_millis: u64) -> Vec<String> {
        let name = self.name.clone();
        self.schedule
            .due(&name, &self.connections_of(&name), now_millis)
            .into_iter()
            .map(|(label, _)| label)
            .collect()
    }

    pub fn disconnect(&mut self) {
        let name = self.name.clone();
        let mechanisms = self.holds_a_component_runtime.then_some(
            &self.mechanisms as &dyn lns_service::connector::mechanism::traits::Mechanisms,
        );
        self.error =
            lns_service::connector::handler::disconnect(&self.store(), mechanisms, &name, None, 0)
                .err()
                .map(|e| format!("{e:#}"));
    }

    /// Stands the machine up without one, the way a host whose component runtime will not start meets a press.
    pub fn no_component_runtime_starts(&mut self) {
        self.holds_a_component_runtime = false;
    }

    pub fn holds_a_component_runtime(&self) -> bool {
        self.holds_a_component_runtime
    }

    /// The whole of what a workload's request can cause: lns reads what the run already granted, and decides from that alone (§3.2.4).
    pub fn what_a_request_reads(&self, run: &str, at: u64) -> Vec<String> {
        lns_service::connector::handler::granted_supply(&self.store(), &holder(run), at)
            .expect("read what this run granted")
            .into_keys()
            .collect()
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
        self.remember_the_grant(run, method);
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

    /// Marks the one connection this rig holds as running out at `at`, the way a mechanism reporting an expiry does.
    pub fn the_connection_runs_out_at(&mut self, at: u64) {
        let store = self.store();
        let name = self.name.clone();
        let held = store.connections_of(&name).expect("connections");
        for (label, mut connection) in held {
            connection.expires_at_millis = Some(at);
            store
                .record_authentication(&name, &label, connection)
                .expect("record the expiry");
        }
    }

    /// What the boundary is armed with for one variable, read through the same path a starting run reads (§7.1). `at` is the moment the run reads it, which decides whether a connection's values have run out (§4.1).
    pub fn supplied_to(&self, run: &str, variable: &str) -> Option<String> {
        self.supplied_to_at(run, variable, 0)
    }

    pub fn supplied_to_at(&self, run: &str, variable: &str, at: u64) -> Option<String> {
        lns_service::connector::handler::granted_supply(&self.store(), &holder(run), at)
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

    /// What one granted method writes into the guest at that path, read the same way a starting run reads it (§7.1).
    pub fn file_written_to(&self, run: &str, path: &str) -> Option<String> {
        lns_service::connector::handler::granted_supply(&self.store(), &holder(run), 0)
            .expect("read what this run granted")
            .values()
            .flat_map(|payload| payload.files.clone())
            .find(|file| file.path == path)
            .map(|file| match file.content {
                lns_service::approval_flow::protocol::WireFileContent::Content(text) => text,
                other => panic!("this fileset is inline text, not {other:?}"),
            })
    }

    pub fn connections_of(&self, name: &str) -> BTreeMap<String, Connection> {
        self.store()
            .connections_of(name)
            .expect("read the connections")
    }
}

/// Built from `tests/fixtures/mechanism` and committed beside it, so this suite needs no wasm toolchain.
fn component(fixture: &str) -> Vec<u8> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/");
    std::fs::read(format!("{path}{fixture}.wasm")).expect("a committed fixture component")
}

/// The bytes one operand names, answered by the rig rather than a registry, so the refusals that depend on which form named them are the ones under test.
struct OneConnector {
    digest: String,
    document: Vec<u8>,
    components: Vec<Vec<u8>>,
}

impl lns_service::connector::source::ConnectorSource for OneConnector {
    async fn fetch(
        &self,
        _source: &lns_service::connector::source::Source,
    ) -> anyhow::Result<lns_service::connector::source::FetchedConnector> {
        Ok(lns_service::connector::source::FetchedConnector {
            digest: self.digest.clone(),
            document: self.document.clone(),
            filesets: Vec::new(),
            components: self.components.clone(),
        })
    }
}

/// A short stable digest for a scenario's bytes; nothing here verifies content-addressing, only that one document keeps one digest.
fn fxhash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |acc, b| {
        (acc ^ u64::from(*b)).wrapping_mul(0x100000001b3)
    })
}
