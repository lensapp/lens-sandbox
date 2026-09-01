//! Production wiring: the three connector stores at the paths cli-spec §9 names,
//! and the registry-or-machine resolver behind them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use lns_ipc::Response;
use lns_policy::decision_store::JsonDecisionStore;

use super::handler;
use super::registry::RegistryConnectors;
use super::store::{Connection, ConnectorStore, GrantHolder, RunDecision};
use crate::approval_flow::protocol::GrantedPayload;

/// One machine verb, named without the wire type so the dispatcher hands over no store of its own.
pub enum Call {
    Install(String),
    Uninstall(String),
    List,
    Connect {
        name: String,
        method: String,
        connection: String,
        values: std::collections::BTreeMap<String, String>,
    },
    Disconnect {
        name: String,
        connection: Option<String>,
    },
    Grant {
        name: String,
        run: String,
        method: String,
        connection: Option<String>,
    },
    Forget {
        name: String,
        run: String,
    },
}

/// Who a `--run` handle names: the run it resolves to, or — where it is a name no run holds — the reservation waiting for it (§3.2.4). The service decides this because only the service holds the registry.
fn holder_of(handle: &str) -> Result<GrantHolder> {
    match crate::run_registry::resolve(handle) {
        Ok(id) => Ok(GrantHolder::Run(id)),
        // An id, or a prefix of one, either resolves or is nothing; only a name can name a run that does not exist yet.
        Err(crate::run_registry::ResolveError::Unknown { .. })
            if lns_ipc::validate_run_name(handle).is_ok() =>
        {
            Ok(GrantHolder::Reservation(handle.to_string()))
        }
        Err(e) => Err(anyhow::anyhow!("{e}")),
    }
}

/// The holders an authentication invalidated the grant of, named as the user typed them. A run the machine no longer holds is reported by its short id rather than dropped.
fn named_holders(holders: Vec<GrantHolder>) -> Vec<String> {
    holders
        .into_iter()
        .map(|holder| match holder {
            GrantHolder::Run(id) => crate::run_registry::name_of(&id)
                .unwrap_or_else(|| lns_ipc::short_run_id(&id).to_string()),
            GrantHolder::Reservation(name) => name,
        })
        .collect()
}

/// The three paths a connector store is kept at, resolved together so one missing home refuses all three rather than half.
#[derive(Debug)]
struct Paths {
    connectors: PathBuf,
    values: PathBuf,
    grants: PathBuf,
}

impl Paths {
    fn resolve() -> Result<Self> {
        Ok(Self {
            connectors: lns_ipc::connectors_root().context("locating the connector store")?,
            values: lns_ipc::connector_values_path().context("locating the connector values")?,
            grants: lns_ipc::connector_grants_path().context("locating the connector grants")?,
        })
    }
}

/// The connectors one run can be offered (§3.2.1). Unreadable connector state offers none, because a run must still launch beside it.
pub fn offers_for_run(run_id: &str) -> Vec<lns_ipc::ConnectorView> {
    match read_offers(&GrantHolder::Run(run_id.to_string())) {
        Ok(patterns) => patterns,
        Err(e) => {
            crate::log::warn!(
                "cannot read this machine's connectors, so this run is offered none: {e:#}"
            );
            Vec::new()
        }
    }
}

fn read_offers(holder: &GrantHolder) -> Result<Vec<lns_ipc::ConnectorView>> {
    with_run_store(holder, handler::offerable)
}

/// The connector store as the approval session reaches it: every method opens the three stores itself, because a card outlives no lock.
pub struct RealConnectorPort {
    holder: GrantHolder,
    microvm: String,
}

impl RealConnectorPort {
    pub fn new(run_id: String, microvm: String) -> Self {
        Self {
            holder: GrantHolder::Run(run_id),
            microvm,
        }
    }

    fn with_store<T>(&self, f: impl FnOnce(&ConnectorStore<'_>) -> Result<T>) -> Result<T, String> {
        let paths = Paths::resolve().map_err(|e| format!("{e:#}"))?;
        let installed = super::dir::ConnectorDir::new(paths.connectors);
        let values: JsonDecisionStore<Connection> = JsonDecisionStore::new(paths.values);
        let grants: JsonDecisionStore<RunDecision> = JsonDecisionStore::new(paths.grants);
        f(&ConnectorStore::new(&installed, &values, &grants)).map_err(|e| format!("{e:#}"))
    }
}

impl crate::approval_flow::session::ConnectorPort for RealConnectorPort {
    fn connect(
        &self,
        name: &str,
        method: &str,
        label: &str,
        values: lns_ipc::SecretValues,
    ) -> Result<Vec<String>, String> {
        self.with_store(|store| {
            Ok(named_holders(
                handler::connect(store, name, method, label, values.0)?.invalidated,
            ))
        })
    }

    fn grant(
        &self,
        name: &str,
        digest: &str,
        method: &str,
        connection: Option<&str>,
    ) -> Result<crate::approval_flow::protocol::GrantedPayload, String> {
        let (granted, payload) = self.with_store(|store| {
            handler::grant_disclosed(store, name, digest, &self.holder, method, connection)
        })?;
        // A grant that changed nothing decided nothing, and a line for it would let the chain count re-runs of a command as decisions.
        if !granted.unchanged {
            record_decision(&self.holder, &self.microvm, granted_event(name, &granted));
        }
        Ok(payload)
    }

    fn decline(&self, name: &str) -> Result<(), String> {
        self.with_store(|store| {
            store.decide(&self.holder, name, RunDecision::Declined)?;
            Ok(())
        })?;
        record_decision(&self.holder, &self.microvm, declined_event(name));
        Ok(())
    }
}

/// The ledger line one connector decision leaves. A connection is the machine's and no run holds it, so connecting writes none: no run's timeline could account for it (cli-spec §3.6).
fn record_decision(holder: &GrantHolder, microvm: &str, event: lns_ipc::LedgerEvent) {
    let record = lns_ipc::LedgerRecord {
        ts: crate::ledger::now_rfc3339(&crate::clock::RealClock),
        // A reservation is held by a name no run answers to yet, so it belongs to no run's timeline until one takes it.
        run: match holder {
            GrantHolder::Run(id) => id.clone(),
            GrantHolder::Reservation(_) => String::new(),
        },
        microvm: microvm.to_string(),
        event,
    };
    if let Err(e) = crate::ledger::append_ledger_record(&record) {
        crate::log::warn!("could not record this connector decision: {e:#}");
    }
}

/// The name the run answers to, so a line the CLI writes carries the same identity as one a run's own recorder stamps.
fn microvm_of(holder: &GrantHolder) -> String {
    match holder {
        GrantHolder::Run(id) => crate::run_registry::name_of(id).unwrap_or_default(),
        GrantHolder::Reservation(_) => String::new(),
    }
}

fn granted_event(name: &str, granted: &handler::Granted) -> lns_ipc::LedgerEvent {
    lns_ipc::LedgerEvent::Connector {
        connector: name.to_string(),
        verb: lns_ipc::ConnectorVerb::Granted,
        method: Some(granted.method.clone()),
        connection: granted.connection.clone(),
        digest: Some(granted.digest.clone()),
    }
}

fn declined_event(name: &str) -> lns_ipc::LedgerEvent {
    connector_event(name, lns_ipc::ConnectorVerb::Declined)
}

fn forgotten_event(name: &str) -> lns_ipc::LedgerEvent {
    connector_event(name, lns_ipc::ConnectorVerb::Forgotten)
}

/// A decline answers for every version of the bytes and a forget clears whatever was there, so neither names a method or a digest.
fn connector_event(name: &str, verb: lns_ipc::ConnectorVerb) -> lns_ipc::LedgerEvent {
    lns_ipc::LedgerEvent::Connector {
        connector: name.to_string(),
        verb,
        method: None,
        connection: None,
        digest: None,
    }
}

/// What one run creation did with the reservations waiting for the name it was given.
#[derive(Debug, PartialEq, Eq)]
pub enum Reservations {
    /// The run the user named took them, and holds their grants from now on.
    Taken(usize),
    /// A generated name got them, so they were dropped: the name belongs to a run that did not consent to them (§3.2.4).
    Discarded(usize),
    /// A restart, which carries its name and its run id over and so has nothing to take.
    Untouched,
    /// The store could not be read, so this run starts un-granted and the card asks.
    Unreadable,
}

/// §3.2.4's take rule, applied to one run creation: only a run the user named takes a reservation, and only as it is created.
pub fn take_reservations_for(
    assigned_name: &str,
    run_id: &str,
    fresh: bool,
    requested_name: Option<&str>,
) -> Reservations {
    if !fresh {
        return Reservations::Untouched;
    }
    let named_by_the_user = requested_name.is_some();
    let taken = with_stores(|store| {
        Ok(if named_by_the_user {
            store.claim_reservations(assigned_name, run_id)?
        } else {
            store.discard_reservations(assigned_name)?
        })
    });
    match taken {
        Err(e) => {
            crate::log::warn!(
                "cannot read what was reserved for {assigned_name}, so this run starts without it: {e:#}"
            );
            Reservations::Unreadable
        }
        Ok(taken) if named_by_the_user => Reservations::Taken(taken),
        Ok(0) => Reservations::Discarded(0),
        Ok(dropped) => {
            // A grant the user recorded is being deleted, and no card asked; saying nothing would make it look like it never landed.
            crate::log::warn!(
                "dropped {dropped} grant(s) reserved for {assigned_name}: that name went to a run you did not name, so nothing consented to them"
            );
            Reservations::Discarded(dropped)
        }
    }
}

/// What one removed run decided, removed with it — and where the store is unreadable, left for a later sweep, because a row nobody can read answers for nobody (§8.4).
pub fn forget_what_a_run_decided(run_id: &str) -> usize {
    with_stores(|store| Ok(store.forget_run(run_id)?)).unwrap_or_else(|e| {
        crate::log::warn!("cannot drop what run {run_id} decided: {e:#}");
        0
    })
}

/// The rows of runs this machine no longer records, dropped at boot — hygiene, because a run id is never reused and so a missed row is dead weight rather than consent (§7.1).
pub fn forget_runs_except(recorded: &std::collections::BTreeSet<String>) -> usize {
    with_stores(|store| Ok(store.forget_runs_except(recorded)?)).unwrap_or_else(|e| {
        crate::log::warn!("cannot sweep the grants of runs this machine no longer records: {e:#}");
        0
    })
}

/// The names an unclaimed reservation waits for. Unreadable state reserves none: a collision is a confusion, and the discard is what keeps it from being a leak (§3.2.4).
pub fn reserved_names() -> std::collections::BTreeSet<String> {
    with_stores(|store| Ok(store.reserved_names()?)).unwrap_or_else(|e| {
        crate::log::warn!("cannot read which names are reserved: {e:#}");
        Default::default()
    })
}

/// Records a grant the way a card would, so another module's test can stand up the state an exec reads without reaching into the store's shape.
#[cfg(test)]
pub fn record_a_grant_for_a_test(run_id: &str, name: &str, digest: &str) {
    with_run_store(&GrantHolder::Run(run_id.to_string()), |store, holder| {
        store.decide(
            holder,
            name,
            RunDecision::Granted {
                digest: digest.to_string(),
                method: "token".to_string(),
                connection: None,
                authority: Default::default(),
            },
        )?;
        Ok(())
    })
    .expect("record a grant");
}

fn with_stores<T>(f: impl FnOnce(&ConnectorStore<'_>) -> Result<T>) -> Result<T> {
    let paths = Paths::resolve()?;
    let installed = super::dir::ConnectorDir::new(paths.connectors);
    let values: JsonDecisionStore<Connection> = JsonDecisionStore::new(paths.values);
    let grants: JsonDecisionStore<RunDecision> = JsonDecisionStore::new(paths.grants);
    f(&ConnectorStore::new(&installed, &values, &grants))
}

/// What the grants this run already made supply to it as it starts, by connector (§7.1). Unreadable connector state supplies nothing, as it offers nothing.
pub fn granted_supply_for(run_id: &str) -> BTreeMap<String, GrantedPayload> {
    match read_granted_supply(&GrantHolder::Run(run_id.to_string())) {
        Ok(supply) => supply,
        Err(e) => {
            crate::log::warn!("cannot read what this run granted, so it starts without it: {e:#}");
            BTreeMap::new()
        }
    }
}

/// Each variable a granted method fills for this run, by the connector that fills it: the workload reads a placeholder there and the boundary substitutes the real value, so no other source may set it (§3.2.4).
pub fn variables_a_grant_fills(run_id: &str) -> BTreeMap<String, String> {
    granted_supply_for(run_id)
        .into_iter()
        .flat_map(|(connector, supply)| {
            supply
                .credentials
                .into_iter()
                .filter_map(move |credential| Some((credential.env_var?, connector.clone())))
        })
        .collect()
}

fn read_granted_supply(holder: &GrantHolder) -> Result<BTreeMap<String, GrantedPayload>> {
    with_run_store(holder, handler::granted_supply)
}

/// Opens the three stores against the holder a grant is keyed by.
fn with_run_store<T>(
    holder: &GrantHolder,
    f: impl FnOnce(&ConnectorStore<'_>, &GrantHolder) -> Result<T>,
) -> Result<T> {
    with_stores(|store| f(store, holder))
}

pub async fn answer(call: Call) -> Result<Response> {
    let paths = Paths::resolve()?;
    let installed = super::dir::ConnectorDir::new(paths.connectors);
    let values: JsonDecisionStore<Connection> = JsonDecisionStore::new(paths.values);
    let grants: JsonDecisionStore<RunDecision> = JsonDecisionStore::new(paths.grants);
    let store = ConnectorStore::new(&installed, &values, &grants);
    match call {
        Call::Install(source) => Ok(Response::ConnectorInstalled {
            connector: handler::install(&store, &RegistryConnectors, &source).await?,
        }),
        Call::Uninstall(name) => Ok(match handler::uninstall(&store, &name)? {
            Some(dropped_connections) => Response::ConnectorUninstalled {
                name,
                dropped_connections,
            },
            None => Response::ConnectorUnknown { name },
        }),
        Call::List => Ok(Response::ConnectorList {
            connectors: handler::list(&store)?,
        }),
        Call::Connect {
            name,
            method,
            connection,
            values,
        } => {
            let connected = handler::connect(&store, &name, &method, &connection, values)?;
            Ok(Response::ConnectorConnected {
                name,
                connection: connected.connection,
                invalidated: named_holders(connected.invalidated),
            })
        }
        Call::Disconnect { name, connection } => Ok(Response::ConnectorDisconnected {
            dropped: handler::disconnect(&store, &name, connection.as_deref())?,
            name,
        }),
        Call::Grant {
            name,
            run,
            method,
            connection,
        } => {
            let holder = holder_of(&run)?;
            let granted = handler::grant(&store, &name, &holder, &method, connection.as_deref())?;
            if !granted.unchanged {
                record_decision(
                    &holder,
                    &microvm_of(&holder),
                    granted_event(&name, &granted),
                );
            }
            Ok(Response::ConnectorGranted {
                name,
                method: granted.method,
                connection: granted.connection,
                displaced: granted.displaced,
                unchanged: granted.unchanged,
                reserved: matches!(holder, GrantHolder::Reservation(_)),
            })
        }
        Call::Forget { name, run } => {
            let holder = holder_of(&run)?;
            let had_decision = handler::forget(&store, &name, &holder)?;
            if had_decision {
                record_decision(&holder, &microvm_of(&holder), forgotten_event(&name));
            }
            Ok(Response::ConnectorForgotten {
                had_decision,
                reserved: matches!(holder, GrantHolder::Reservation(_)),
                name,
            })
        }
    }
}

pub(super) fn read_document(path: &Path) -> Result<Vec<u8>> {
    let yaml =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let value: serde_json::Value =
        serde_yaml::from_str(&yaml).with_context(|| format!("parsing {}", path.display()))?;
    serde_json::to_vec(&value).context("normalizing the connector to json")
}

pub(super) struct RealSnapshotFs;

impl lns_artifact::walk::SnapshotFs for RealSnapshotFs {
    fn read_limited(&self, path: &Path, max_bytes: u64) -> std::io::Result<Vec<u8>> {
        lns_artifact::walk::real_read_limited(path, max_bytes)
    }

    fn dir_entries(&self, dir: &Path) -> std::io::Result<Vec<lns_artifact::walk::DirEntry>> {
        lns_artifact::walk::real_dir_entries(dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    const RUN: &str = "1a2b3c4d0000000000000000000000aa";
    const OTHER_RUN: &str = "9f8e7d6c0000000000000000000000bb";

    #[test]
    #[serial(env, global_runs)]
    fn the_three_stores_are_wired_to_the_paths_the_cli_specification_names() {
        // Construction reaches no disk, so this pins the wiring rather than the I/O.
        let home = tempfile::tempdir().expect("tempdir");
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", home.path());
        let paths = Paths::resolve().expect("a home resolves");
        assert_eq!(paths.connectors, home.path().join("connectors"));
        assert_eq!(paths.values, home.path().join("connector-values.json"));
        assert_eq!(paths.grants, home.path().join("connector-grants.json"));
    }

    #[test]
    #[serial(env, global_runs)]
    fn no_resolvable_home_refuses_rather_than_writing_beside_the_working_directory() {
        let _lns = crate::test_env::EnvVarGuard::set("LNS_HOME", "relative/dir");
        let err = Paths::resolve().expect_err("a relative home must be refused");
        assert!(
            err.to_string().contains("locating the connector store"),
            "the first store to resolve is the one that reports, so the message names it: {err:#}"
        );
    }

    #[test]
    fn a_local_connector_resolves_off_the_real_filesystem_including_its_fileset() {
        // The service reads a directory through the one shared snapshot implementation, so this pins that path end to end rather than the delegation alone.
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("lns.yaml"),
            "apiVersion: lns.run/v1\nkind: connector\nname: some-provider\nspec:\n  serves: [api.some-provider.example]\n  methods:\n    - name: token\n      auth:\n        kind: token\n      filesets:\n        - path: ./seed\n          guestPath: ~/.some-provider\n",
        )
        .unwrap();
        std::fs::create_dir(dir.path().join("seed")).unwrap();
        std::fs::write(dir.path().join("seed").join("notes.md"), b"hello").unwrap();

        let fetched = super::super::source::read_local(
            &RealSnapshotFs,
            read_document,
            &dir.path().join("lns.yaml"),
        )
        .expect("a real connector directory resolves");

        assert!(fetched.digest.starts_with("sha256:"), "{}", fetched.digest);
        assert!(
            String::from_utf8_lossy(&fetched.document).contains("some-provider"),
            "the document travels verbatim"
        );
    }

    #[test]
    fn reading_a_document_that_is_not_there_names_the_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let err = read_document(&dir.path().join("lns.yaml")).unwrap_err();
        assert!(format!("{err:#}").contains("lns.yaml"), "{err:#}");
    }

    #[test]
    fn a_document_that_is_not_yaml_names_the_path_it_could_not_parse() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("lns.yaml");
        std::fs::write(&path, "\tnot: [yaml").unwrap();
        let err = read_document(&path).unwrap_err();
        assert!(format!("{err:#}").contains("parsing"), "{err:#}");
    }

    fn served_by(offers: Vec<lns_ipc::ConnectorView>) -> Vec<String> {
        offers.into_iter().flat_map(|offer| offer.serves).collect()
    }

    fn install_one(home: &Path, serves: &str) {
        install_named(home, "some-provider", serves);
    }

    fn install_named(home: &Path, name: &str, serves: &str) {
        let dir = home.join("connectors").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("document.json"),
            serde_json::json!({
                "apiVersion": "lns.run/v1",
                "kind": "connector",
                "name": name,
                "spec": {
                    "serves": [serves],
                    "methods": [{ "name": "token", "auth": { "kind": "token" } }],
                },
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(dir.join("digest"), "sha256:abc").unwrap();
    }

    fn install_filling(home: &Path, name: &str, env_var: &str) {
        install_with_credential(
            home,
            name,
            serde_json::json!({
                "envVar": env_var,
                "placeholder": format!("{name}-LNSPLACEHOLDER000"),
            }),
        );
    }

    /// A method §4.1 allows: a credential that exists only to be injected on the wire, so it sets no variable and narrows no environment.
    fn install_filling_nothing(home: &Path, name: &str) {
        install_with_credential(
            home,
            name,
            serde_json::json!({ "placeholder": format!("{name}-LNSPLACEHOLDER000") }),
        );
    }

    fn install_with_credential(home: &Path, name: &str, credential: serde_json::Value) {
        let dir = home.join("connectors").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("document.json"),
            serde_json::json!({
                "apiVersion": "lns.run/v1",
                "kind": "connector",
                "name": name,
                "spec": {
                    "serves": [format!("api.{name}.example")],
                    "methods": [{
                        "name": "token",
                        "auth": { "kind": "token" },
                        "credentials": [credential],
                    }],
                },
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(dir.join("digest"), "sha256:abc").unwrap();
    }

    fn grant_recorded(run_id: &str, name: &str) {
        with_run_store(&GrantHolder::Run(run_id.to_string()), |store, holder| {
            store.decide(
                holder,
                name,
                RunDecision::Granted {
                    digest: "sha256:abc".to_string(),
                    method: "token".to_string(),
                    connection: None,
                    authority: Default::default(),
                },
            )?;
            Ok(())
        })
        .expect("record a grant");
    }

    /// Through the store rather than hand-written json, so the file this reads back is the shape a real decline writes.
    fn decline(home: &Path, run_id: &str) {
        let holder = GrantHolder::Run(run_id.to_string());
        let installed = super::super::dir::ConnectorDir::new(home.join("connectors"));
        let values: JsonDecisionStore<Connection> =
            JsonDecisionStore::new(home.join("connector-values.json"));
        let grants: JsonDecisionStore<RunDecision> =
            JsonDecisionStore::new(home.join("connector-grants.json"));
        ConnectorStore::new(&installed, &values, &grants)
            .decide(&holder, "some-provider", RunDecision::Declined)
            .expect("record a decline");
    }

    #[test]
    #[serial(env, global_runs)]
    fn a_run_holds_what_an_undecided_connector_on_this_machine_serves() {
        let home = tempfile::tempdir().expect("tempdir");
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", home.path());
        install_one(home.path(), "api.some-provider.example");
        assert_eq!(
            served_by(offers_for_run(RUN)),
            ["api.some-provider.example"]
        );
    }

    #[test]
    #[serial(env, global_runs)]
    fn one_runs_decline_reaches_that_run_and_no_other() {
        // §7.1: a decision is keyed by the run, so a second run — in the same directory or any other — is asked for itself.
        let home = tempfile::tempdir().expect("tempdir");
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", home.path());
        install_one(home.path(), "api.some-provider.example");
        decline(home.path(), RUN);
        assert!(offers_for_run(RUN).is_empty(), "this run has declined");
        assert_eq!(
            served_by(offers_for_run(OTHER_RUN)),
            ["api.some-provider.example"],
            "another run decided nothing, so it is still offered"
        );
    }

    #[test]
    #[serial(env, global_runs)]
    fn connector_state_this_build_cannot_reach_holds_nothing_rather_than_failing_the_run() {
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", "relative/dir");
        assert!(offers_for_run(RUN).is_empty());
    }

    fn install_connectable(home: &Path) {
        let dir = home.join("connectors").join("some-provider");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("document.json"),
            serde_json::json!({
                "apiVersion": "lns.run/v1",
                "kind": "connector",
                "name": "some-provider",
                "spec": {
                    "serves": ["api.some-provider.example"],
                    "methods": [{
                        "name": "token",
                        "auth": { "kind": "token" },
                        "egress": { "http": [{ "match": "api.some-provider.example", "verdict": "allow" }] },
                        "credentials": [{
                            "envVar": "SOME_TOKEN",
                            "placeholder": "some-provider-LNSPLACEHOLDER00",
                            "injections": [{ "kind": "bearer_header", "domain": "api.some-provider.example" }],
                        }],
                    }],
                },
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(dir.join("digest"), "sha256:abc").unwrap();
    }

    /// The grant an earlier start of `run_id` would have recorded, naming connection `work` at the authority given.
    fn grant_naming_work(
        home: &Path,
        holder: &GrantHolder,
        authority: crate::connector::store::Authority,
    ) {
        let installed = super::super::dir::ConnectorDir::new(home.join("connectors"));
        let values: JsonDecisionStore<Connection> =
            JsonDecisionStore::new(home.join("connector-values.json"));
        let grants: JsonDecisionStore<RunDecision> =
            JsonDecisionStore::new(home.join("connector-grants.json"));
        ConnectorStore::new(&installed, &values, &grants)
            .decide(
                holder,
                "some-provider",
                RunDecision::Granted {
                    digest: "sha256:abc".to_string(),
                    method: "token".to_string(),
                    connection: Some("work".to_string()),
                    authority,
                },
            )
            .expect("record a grant");
    }

    /// One value under every key the method's own view asks for, so a test cannot connect under a key nothing reads back.
    fn asked_for(name: &str, method: &str) -> std::collections::BTreeMap<String, String> {
        with_stores(handler::list)
            .expect("list the installed set")
            .into_iter()
            .find(|connector| connector.name == name)
            .unwrap_or_else(|| panic!("{name} is not installed"))
            .methods
            .into_iter()
            .find(|view| view.name == method)
            .unwrap_or_else(|| panic!("{name} declares no method {method}"))
            .asks
            .into_iter()
            .map(|ask| (ask, "sk-live".to_string()))
            .collect()
    }

    fn reconnect_reporting_admin(home: &Path) -> Vec<String> {
        use crate::approval_flow::session::ConnectorPort;
        RealConnectorPort::new(RUN.to_string(), "calm-finch".to_string())
            .connect(
                "some-provider",
                "token",
                "work",
                lns_ipc::SecretValues(asked_for("some-provider", "token")),
            )
            .unwrap_or_else(|e| panic!("re-authenticating against {}: {e}", home.display()))
    }

    #[test]
    #[serial_test::serial(env, global_runs)]
    fn a_name_no_run_holds_is_a_reservation_and_an_unresolvable_id_is_an_error() {
        // §2.4: a name is never all lowercase hex, so an id that resolves to nothing cannot be read as a name a run may later take.
        assert_eq!(
            holder_of("revieweer").expect("a name no run holds"),
            GrantHolder::Reservation("revieweer".to_string())
        );
        let err = holder_of("1a2b").expect_err("an id prefix that resolves to nothing");
        assert!(err.to_string().contains("no such run"), "{err}");
    }

    #[tokio::test]
    #[serial_test::serial(env, global_runs)]
    async fn a_handle_that_resolves_names_the_run_it_resolved_to() {
        let (handle, _cancel) = crate::run_registry::test_handle();
        crate::run_registry::register_named(RUN.to_string(), Some("reviewer".into()), handle)
            .expect("register");
        let resolved = holder_of("reviewer");
        crate::run_registry::deregister(RUN);
        assert_eq!(
            resolved.expect("reviewer resolves"),
            GrantHolder::Run(RUN.to_string()),
            "the store never sees a name for a run that exists"
        );
    }

    #[test]
    #[serial(env, global_runs)]
    fn a_run_the_user_named_takes_what_was_reserved_and_a_generated_name_discards_it() {
        let home = tempfile::tempdir().expect("tempdir");
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", home.path());
        install_connectable(home.path());
        let reserved = GrantHolder::Reservation("reviewer".to_string());
        grant_naming_work(
            home.path(),
            &reserved,
            crate::connector::store::Authority::default(),
        );

        assert_eq!(reserved_names(), ["reviewer".to_string()].into());
        assert_eq!(
            take_reservations_for("reviewer", RUN, true, Some("reviewer")),
            Reservations::Taken(1)
        );
        assert!(reserved_names().is_empty(), "consumed once");
        assert!(
            granted_supply_for(RUN).contains_key("some-provider"),
            "the run it was reserved for is supplied on this very start"
        );

        grant_naming_work(
            home.path(),
            &reserved,
            crate::connector::store::Authority::default(),
        );
        assert_eq!(
            take_reservations_for("reviewer", OTHER_RUN, true, None),
            Reservations::Discarded(1)
        );
        assert!(
            reserved_names().is_empty(),
            "a generated name discards rather than leaving it for a later run"
        );
        assert!(
            !granted_supply_for(OTHER_RUN).contains_key("some-provider"),
            "discarding is not a quiet claim"
        );
    }

    #[test]
    #[serial(env, global_runs)]
    fn a_generated_name_no_reservation_waits_for_drops_nothing_and_says_nothing() {
        // The ordinary case: most runs are auto-named and no reservation is waiting, so the discard must be silent.
        let home = tempfile::tempdir().expect("tempdir");
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", home.path());
        install_connectable(home.path());
        assert_eq!(
            take_reservations_for("amber_otter", RUN, true, None),
            Reservations::Discarded(0)
        );
    }

    #[test]
    #[serial(env, global_runs)]
    fn removing_a_run_removes_what_it_granted_so_the_next_run_of_that_name_is_asked() {
        // §8.4: the grant goes with the run, and the name is then free of it.
        let home = tempfile::tempdir().expect("tempdir");
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", home.path());
        install_connectable(home.path());
        grant_naming_work(
            home.path(),
            &GrantHolder::Run(RUN.to_string()),
            crate::connector::store::Authority::default(),
        );
        assert!(granted_supply_for(RUN).contains_key("some-provider"));

        assert_eq!(forget_what_a_run_decided(RUN), 1);

        assert!(granted_supply_for(RUN).is_empty());
        assert_eq!(
            served_by(offers_for_run(RUN)),
            ["api.some-provider.example"],
            "with nothing decided, the connector is offered again"
        );
    }

    #[test]
    #[serial(env, global_runs)]
    fn the_boot_sweep_keeps_a_recorded_run_and_drops_the_rest() {
        let home = tempfile::tempdir().expect("tempdir");
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", home.path());
        install_connectable(home.path());
        for run_id in [RUN, OTHER_RUN] {
            grant_naming_work(
                home.path(),
                &GrantHolder::Run(run_id.to_string()),
                crate::connector::store::Authority::default(),
            );
        }

        assert_eq!(forget_runs_except(&[RUN.to_string()].into()), 1);

        assert!(granted_supply_for(RUN).contains_key("some-provider"));
        assert!(granted_supply_for(OTHER_RUN).is_empty());
    }

    #[test]
    #[serial(env, global_runs)]
    fn connector_state_this_build_cannot_reach_forgets_nothing_rather_than_failing_a_removal() {
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", "relative/dir");
        assert_eq!(forget_what_a_run_decided(RUN), 0);
        assert_eq!(forget_runs_except(&Default::default()), 0);
    }

    #[test]
    #[serial(env, global_runs)]
    fn a_restart_takes_nothing_because_it_carries_its_own_run_id() {
        // §3.2.4: a rename or a restart is not the creation of a run, so a reservation for that name stays waiting.
        let home = tempfile::tempdir().expect("tempdir");
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", home.path());
        install_connectable(home.path());
        grant_naming_work(
            home.path(),
            &GrantHolder::Reservation("reviewer".to_string()),
            crate::connector::store::Authority::default(),
        );

        assert_eq!(
            take_reservations_for("reviewer", RUN, false, Some("reviewer")),
            Reservations::Untouched
        );
        assert_eq!(reserved_names(), ["reviewer".to_string()].into());
    }

    #[test]
    #[serial(env, global_runs)]
    fn connector_state_this_build_cannot_reach_reserves_nothing_and_takes_nothing() {
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", "relative/dir");
        assert!(reserved_names().is_empty());
        assert_eq!(
            take_reservations_for("reviewer", RUN, true, Some("reviewer")),
            Reservations::Unreadable
        );
    }

    #[test]
    #[serial(env, global_runs)]
    fn an_invalidated_reservation_is_reported_by_the_name_it_waits_for() {
        // A reservation has no run to look up, so it is its own name — never a run id run through the registry.
        let home = tempfile::tempdir().expect("tempdir");
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", home.path());
        install_connectable(home.path());
        grant_naming_work(
            home.path(),
            &GrantHolder::Reservation("revieweer".to_string()),
            crate::connector::store::Authority::of(["repo:read"]),
        );

        assert_eq!(reconnect_reporting_admin(home.path()), ["revieweer"]);
    }

    #[tokio::test]
    #[serial(env, global_runs)]
    async fn an_invalidated_grant_is_reported_by_the_name_its_run_holds() {
        // A store key carries the NUL separator, so reporting one would print it at the user.
        let home = tempfile::tempdir().expect("tempdir");
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", home.path());
        install_connectable(home.path());
        grant_naming_work(
            home.path(),
            &GrantHolder::Run(RUN.to_string()),
            crate::connector::store::Authority::of(["repo:read"]),
        );
        let (handle, _cancel) = crate::run_registry::test_handle();
        crate::run_registry::register_named(RUN.to_string(), Some("reviewer".into()), handle)
            .expect("register");

        let invalidated = reconnect_reporting_admin(home.path());

        crate::run_registry::deregister(RUN);
        assert_eq!(invalidated, ["reviewer"]);
        assert!(
            !invalidated[0].contains('\0'),
            "a store key must never reach the user"
        );
    }

    #[test]
    #[serial(env, global_runs)]
    fn an_invalidated_grant_of_a_run_this_machine_no_longer_holds_is_reported_by_its_short_id() {
        // The run was removed between the grant and the re-authentication; naming it by id still tells the user which decision went.
        let home = tempfile::tempdir().expect("tempdir");
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", home.path());
        install_connectable(home.path());
        grant_naming_work(
            home.path(),
            &GrantHolder::Run(RUN.to_string()),
            crate::connector::store::Authority::of(["repo:read"]),
        );

        assert_eq!(
            reconnect_reporting_admin(home.path()),
            [lns_ipc::short_run_id(RUN)]
        );
    }

    #[test]
    #[serial(env, global_runs)]
    fn a_card_connects_then_grants_and_the_guest_is_given_what_the_method_supplies() {
        use crate::approval_flow::session::ConnectorPort;
        let home = tempfile::tempdir().expect("tempdir");
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", home.path());
        install_connectable(home.path());
        let port = RealConnectorPort::new(RUN.to_string(), "calm-finch".to_string());

        let invalidated = port
            .connect(
                "some-provider",
                "token",
                "work",
                lns_ipc::SecretValues(asked_for("some-provider", "token")),
            )
            .expect("connect");
        assert!(invalidated.is_empty(), "no run granted anything yet");

        let payload = port
            .grant("some-provider", "sha256:abc", "token", Some("work"))
            .expect("grant");

        assert_eq!(
            payload.egress.network.egress.http[0].match_pattern,
            "api.some-provider.example"
        );
        assert_eq!(
            payload.credentials[0].injections[0].value(),
            "Bearer sk-live",
            "the value typed into the card is what the boundary substitutes"
        );
        assert!(
            offers_for_run(RUN).is_empty(),
            "this run decided, so nothing it serves is offered again"
        );
    }

    #[test]
    #[serial(env, global_runs)]
    fn a_run_that_granted_yesterday_gets_what_it_granted_today() {
        // §7.1: the grant is recorded once and supplies every later run. Reading only the decision would leave the run with the destination closed and the credential absent.
        use crate::approval_flow::session::ConnectorPort;
        let home = tempfile::tempdir().expect("tempdir");
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", home.path());
        install_connectable(home.path());
        let port = RealConnectorPort::new(RUN.to_string(), "calm-finch".to_string());
        port.connect(
            "some-provider",
            "token",
            "work",
            lns_ipc::SecretValues(asked_for("some-provider", "token")),
        )
        .expect("connect");
        port.grant("some-provider", "sha256:abc", "token", Some("work"))
            .expect("grant");

        let supplied = granted_supply_for(RUN);
        let supply = supplied
            .get("some-provider")
            .expect("the recorded grant supplies");

        assert_eq!(
            supply.egress.network.egress.http[0].match_pattern,
            "api.some-provider.example"
        );
        assert_eq!(
            supply.credentials[0].injections[0].value(),
            "Bearer sk-live",
            "the connection the grant named is the one that arms it"
        );
        assert!(
            granted_supply_for(OTHER_RUN).is_empty(),
            "another run granted nothing, so it gets nothing"
        );
    }

    /// The one connection the grant tests settle on, so the grant has an account behind the method that authenticates.
    fn connect_work() {
        use crate::approval_flow::session::ConnectorPort;
        RealConnectorPort::new(RUN.to_string(), "calm-finch".to_string())
            .connect(
                "some-provider",
                "token",
                "work",
                lns_ipc::SecretValues(asked_for("some-provider", "token")),
            )
            .expect("connect");
    }

    /// A run the registry answers for, because a `--run` handle is resolved before any decision is recorded against it. Deregisters itself, so a run one test needs is not a run another one finds.
    struct RegisteredRun {
        run_id: String,
        _cancel: tokio::sync::oneshot::Receiver<i32>,
    }

    impl Drop for RegisteredRun {
        fn drop(&mut self) {
            crate::run_registry::deregister(&self.run_id);
        }
    }

    fn a_registered_run(run_id: &str) -> RegisteredRun {
        let (handle, cancel) = crate::run_registry::test_handle();
        crate::run_registry::register_named(run_id.to_string(), None, handle).expect("register");
        RegisteredRun {
            run_id: run_id.to_string(),
            _cancel: cancel,
        }
    }

    /// Every ledger line this machine holds, read back the way `lns audit` reads it.
    fn recorded() -> Vec<lns_audit::Row> {
        let path = lns_ipc::connection_ledger().expect("the ledger path");
        if !path.exists() {
            return Vec::new();
        }
        lns_audit::stream_ledger(&path)
            .expect("stream the ledger")
            .map(|event| lns_audit::read(&event.expect("a ledger line")).expect("a readable row"))
            .collect()
    }

    #[tokio::test]
    #[serial(env, global_runs)]
    async fn a_grant_is_written_to_the_chain_under_the_run_that_holds_it() {
        // §3.6: the ledger holds the connector decisions, and a grant opens destinations and arms a credential — the decision the chain most needs.
        let home = tempfile::tempdir().expect("tempdir");
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", home.path());
        install_connectable(home.path());
        let _run = a_registered_run(RUN);
        connect_work();

        answer(Call::Grant {
            name: "some-provider".to_string(),
            run: RUN.to_string(),
            method: "token".to_string(),
            connection: None,
        })
        .await
        .expect("grant");

        let rows = recorded();
        assert_eq!(rows.len(), 1, "one decision, one line: {rows:?}");
        assert_eq!(rows[0].kind, "connector");
        assert_eq!(
            rows[0].run, RUN,
            "the run that holds the grant owns the line"
        );
        assert_eq!(rows[0].connector.as_deref(), Some("some-provider"));
        assert_eq!(rows[0].detail, "granted some-provider token as work");
    }

    #[tokio::test]
    #[serial(env, global_runs)]
    async fn granting_again_what_a_run_already_grants_writes_no_second_line() {
        // cli-spec §3.3 makes a repeat an exit-1 non-answer, so a line for it would let the chain count re-runs of a command as decisions.
        let home = tempfile::tempdir().expect("tempdir");
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", home.path());
        install_connectable(home.path());
        let _run = a_registered_run(RUN);
        connect_work();
        let again = || {
            answer(Call::Grant {
                name: "some-provider".to_string(),
                run: RUN.to_string(),
                method: "token".to_string(),
                connection: None,
            })
        };

        again().await.expect("grant");
        again().await.expect("grant the same thing again");

        assert_eq!(
            recorded().len(),
            1,
            "one decision was made, however many times it was asked for"
        );
    }

    #[test]
    #[serial(env, global_runs)]
    fn a_decision_whose_line_cannot_be_written_still_stands_and_says_so() {
        // The chain is the weaker of the two: losing a line understates history, and refusing the grant over it would let a full disk take the user's answer away.
        use crate::approval_flow::session::ConnectorPort;
        let home = tempfile::tempdir().expect("tempdir");
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", home.path());
        install_connectable(home.path());
        connect_work();
        std::fs::create_dir_all(lns_ipc::connection_ledger().expect("the ledger path"))
            .expect("a ledger path nothing can append to");
        let port = RealConnectorPort::new(RUN.to_string(), "calm-finch".to_string());

        let mut granted = None;
        let said = crate::test_env::captured_messages(|| {
            granted = Some(port.grant("some-provider", "sha256:abc", "token", Some("work")));
        });

        assert!(
            granted.expect("the grant was answered").is_ok(),
            "a lost audit line must not take the user's answer with it"
        );
        assert!(
            said.iter().any(|m| m.contains("connector decision")),
            "the operator has to learn the record was lost: {said:?}"
        );
    }

    #[tokio::test]
    #[serial(env, global_runs)]
    async fn a_forget_is_written_and_forgetting_nothing_writes_nothing() {
        // A line for a forget that cleared nothing would say a decision was retracted where none was held.
        let home = tempfile::tempdir().expect("tempdir");
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", home.path());
        install_connectable(home.path());
        let _run = a_registered_run(RUN);

        answer(Call::Forget {
            name: "some-provider".to_string(),
            run: RUN.to_string(),
        })
        .await
        .expect("forget nothing");
        assert!(
            recorded().is_empty(),
            "nothing was cleared, so nothing happened"
        );

        grant_recorded(RUN, "some-provider");
        answer(Call::Forget {
            name: "some-provider".to_string(),
            run: RUN.to_string(),
        })
        .await
        .expect("forget");

        let rows = recorded();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].detail, "forgot some-provider");
        assert_eq!(rows[0].run, RUN);
    }

    #[tokio::test]
    #[serial(env, global_runs)]
    async fn a_grant_reserved_for_a_name_no_run_holds_belongs_to_no_run_s_timeline() {
        // §3.2.4: the reservation waits for a run that does not exist, so no run id could own the line and `lns audit <RUN>` must not claim it.
        let home = tempfile::tempdir().expect("tempdir");
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", home.path());
        install_connectable(home.path());
        connect_work();

        answer(Call::Grant {
            name: "some-provider".to_string(),
            run: "audit-reservation".to_string(),
            method: "token".to_string(),
            connection: None,
        })
        .await
        .expect("reserve a grant");

        let rows = recorded();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].detail, "granted some-provider token as work");
        assert_eq!(rows[0].run, "", "no run holds it yet");
    }

    #[test]
    #[serial(env, global_runs)]
    fn a_card_grant_and_a_card_decline_are_both_written_to_the_chain() {
        use crate::approval_flow::session::ConnectorPort;
        let home = tempfile::tempdir().expect("tempdir");
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", home.path());
        install_connectable(home.path());
        let port = RealConnectorPort::new(RUN.to_string(), "calm-finch".to_string());
        port.connect(
            "some-provider",
            "token",
            "work",
            lns_ipc::SecretValues(asked_for("some-provider", "token")),
        )
        .expect("connect");
        assert!(
            recorded().is_empty(),
            "a connection is the machine's and no run holds it, so no run's timeline accounts for it"
        );

        port.grant("some-provider", "sha256:abc", "token", Some("work"))
            .expect("grant");
        RealConnectorPort::new(OTHER_RUN.to_string(), "calm-finch".to_string())
            .decline("some-provider")
            .expect("decline");

        let rows = recorded();
        assert_eq!(
            rows.iter()
                .map(|row| (row.detail.as_str(), row.run.as_str()))
                .collect::<Vec<_>>(),
            [
                ("granted some-provider token as work", RUN),
                ("declined some-provider", OTHER_RUN),
            ],
            "the card's answer is the run's decision, and the account it settled on is part of it"
        );
    }

    #[test]
    #[serial(env, global_runs)]
    fn the_variables_a_grant_fills_are_the_ones_no_other_source_may_set() {
        // A variable missing from this map is one a `-e` can shadow, putting a real secret where the boundary substitutes a placeholder.
        let home = tempfile::tempdir().expect("tempdir");
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", home.path());
        install_connectable(home.path());
        install_filling(home.path(), "other-provider", "OTHER_TOKEN");
        install_filling_nothing(home.path(), "wire-only");
        for name in ["some-provider", "other-provider", "wire-only"] {
            grant_recorded(RUN, name);
        }

        assert_eq!(
            variables_a_grant_fills(RUN),
            [
                ("OTHER_TOKEN".to_string(), "other-provider".to_string()),
                ("SOME_TOKEN".to_string(), "some-provider".to_string()),
            ]
            .into(),
            "every granted connector contributes, and a credential with no envVar sets no variable"
        );
        assert!(
            variables_a_grant_fills(OTHER_RUN).is_empty(),
            "a grant belongs to one run, so another run's environment is not narrowed by it"
        );
    }

    #[test]
    #[serial(env, global_runs)]
    fn a_run_that_granted_two_connectors_is_supplied_both() {
        // Stopping at the first recorded grant would leave the second connector's destination closed, chosen by whichever the store listed first.
        let home = tempfile::tempdir().expect("tempdir");
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", home.path());
        install_connectable(home.path());
        install_named(home.path(), "other-provider", "api.other-provider.example");
        with_run_store(&GrantHolder::Run(RUN.to_string()), |store, holder| {
            for name in ["some-provider", "other-provider"] {
                store.decide(
                    holder,
                    name,
                    RunDecision::Granted {
                        digest: "sha256:abc".to_string(),
                        method: "token".to_string(),
                        connection: None,
                        authority: Default::default(),
                    },
                )?;
            }
            Ok(())
        })
        .expect("record both grants");

        let supplied = granted_supply_for(RUN);

        assert_eq!(
            supplied.keys().collect::<Vec<_>>(),
            ["other-provider", "some-provider"],
            "both grants are this run's"
        );
    }

    #[test]
    #[serial(env, global_runs)]
    fn a_grant_made_against_bytes_that_have_since_changed_supplies_nothing() {
        // The connector was updated after the grant, so what it now opens is not what the run consented to.
        let home = tempfile::tempdir().expect("tempdir");
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", home.path());
        install_connectable(home.path());
        with_run_store(&GrantHolder::Run(RUN.to_string()), |store, holder| {
            store.decide(
                holder,
                "some-provider",
                RunDecision::Granted {
                    digest: "sha256:the-version-they-agreed-to".to_string(),
                    method: "token".to_string(),
                    connection: None,
                    authority: Default::default(),
                },
            )?;
            Ok(())
        })
        .expect("record a grant against older bytes");

        assert!(granted_supply_for(RUN).is_empty());
    }

    #[test]
    #[serial(env, global_runs)]
    fn a_run_that_declined_is_supplied_nothing() {
        use crate::approval_flow::session::ConnectorPort;
        let home = tempfile::tempdir().expect("tempdir");
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", home.path());
        install_connectable(home.path());
        RealConnectorPort::new(RUN.to_string(), "calm-finch".to_string())
            .decline("some-provider")
            .expect("decline");

        assert!(granted_supply_for(RUN).is_empty());
    }

    #[test]
    #[serial(env, global_runs)]
    fn connector_state_that_cannot_be_read_supplies_nothing_rather_than_failing_the_run() {
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", "relative/dir");
        assert!(granted_supply_for(RUN).is_empty());
    }

    #[test]
    #[serial(env, global_runs)]
    fn a_card_grant_against_bytes_that_changed_is_refused() {
        use crate::approval_flow::session::ConnectorPort;
        let home = tempfile::tempdir().expect("tempdir");
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", home.path());
        install_connectable(home.path());

        let refused = RealConnectorPort::new(RUN.to_string(), "calm-finch".to_string())
            .grant(
                "some-provider",
                "sha256:what-the-card-showed",
                "token",
                None,
            )
            .expect_err("the installed bytes are not the ones disclosed");

        assert!(refused.contains("replaced"), "{refused}");
    }

    #[test]
    #[serial(env, global_runs)]
    fn a_card_decline_is_the_standing_no_the_next_run_reads() {
        use crate::approval_flow::session::ConnectorPort;
        let home = tempfile::tempdir().expect("tempdir");
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", home.path());
        install_one(home.path(), "api.some-provider.example");

        RealConnectorPort::new(RUN.to_string(), "calm-finch".to_string())
            .decline("some-provider")
            .expect("decline");

        assert!(offers_for_run(RUN).is_empty());
        assert!(
            !offers_for_run(OTHER_RUN).is_empty(),
            "the no is this run's, not the machine's"
        );
    }

    #[test]
    #[serial(env, global_runs)]
    fn a_card_answer_that_cannot_reach_the_store_says_so_rather_than_panicking() {
        use crate::approval_flow::session::ConnectorPort;
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", "relative/dir");
        let port = RealConnectorPort::new(RUN.to_string(), "calm-finch".to_string());

        assert!(
            port.decline("some-provider")
                .expect_err("no home resolves")
                .contains("locating the connector store")
        );
        assert!(
            port.grant("some-provider", "sha256:abc", "token", None)
                .is_err()
        );
        assert!(
            port.connect(
                "some-provider",
                "token",
                "work",
                lns_ipc::SecretValues::default()
            )
            .is_err()
        );
    }

    #[test]
    fn a_yaml_document_is_normalised_to_the_json_the_parser_reads() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("lns.yaml");
        std::fs::write(&path, "apiVersion: lns.run/v1\nkind: connector\n").unwrap();
        let json = read_document(&path).expect("read");
        let value: serde_json::Value = serde_json::from_slice(&json).expect("json");
        assert_eq!(value["kind"], "connector");
    }
}
