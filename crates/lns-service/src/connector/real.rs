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
}

impl RealConnectorPort {
    pub fn new(run_id: String) -> Self {
        Self {
            holder: GrantHolder::Run(run_id),
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
        self.with_store(|store| {
            handler::grant_disclosed(store, name, digest, &self.holder, method, connection)
        })
    }

    fn decline(&self, name: &str) -> Result<(), String> {
        self.with_store(|store| {
            store.decide(&self.holder, name, RunDecision::Declined)?;
            Ok(())
        })
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

/// The names an unclaimed reservation waits for. Unreadable state reserves none: a collision is a confusion, and the discard is what keeps it from being a leak (§3.2.4).
pub fn reserved_names() -> std::collections::BTreeSet<String> {
    with_stores(|store| Ok(store.reserved_names()?)).unwrap_or_else(|e| {
        crate::log::warn!("cannot read which names are reserved: {e:#}");
        Default::default()
    })
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
            Ok(Response::ConnectorForgotten {
                had_decision: handler::forget(&store, &name, &holder)?,
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
    #[serial(env)]
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
    #[serial(env)]
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
    #[serial(env)]
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
    #[serial(env)]
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
    #[serial(env)]
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

    fn reconnect_reporting_admin(home: &Path) -> Vec<String> {
        use crate::approval_flow::session::ConnectorPort;
        RealConnectorPort::new(RUN.to_string())
            .connect(
                "some-provider",
                "token",
                "work",
                lns_ipc::SecretValues(
                    [("SOME_TOKEN".to_string(), "sk-live".to_string())]
                        .into_iter()
                        .collect(),
                ),
            )
            .unwrap_or_else(|e| panic!("re-authenticating against {}: {e}", home.display()))
    }

    #[test]
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
    #[serial(env)]
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
    #[serial(env)]
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
    #[serial(env)]
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
    #[serial(env)]
    fn connector_state_this_build_cannot_reach_reserves_nothing_and_takes_nothing() {
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", "relative/dir");
        assert!(reserved_names().is_empty());
        assert_eq!(
            take_reservations_for("reviewer", RUN, true, Some("reviewer")),
            Reservations::Unreadable
        );
    }

    #[test]
    #[serial(env)]
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
    #[serial(env)]
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
    #[serial(env)]
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
    #[serial(env)]
    fn a_card_connects_then_grants_and_the_guest_is_given_what_the_method_supplies() {
        use crate::approval_flow::session::ConnectorPort;
        let home = tempfile::tempdir().expect("tempdir");
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", home.path());
        install_connectable(home.path());
        let port = RealConnectorPort::new(RUN.to_string());

        let invalidated = port
            .connect(
                "some-provider",
                "token",
                "work",
                lns_ipc::SecretValues(
                    [("SOME_TOKEN".to_string(), "sk-live".to_string())]
                        .into_iter()
                        .collect(),
                ),
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
    #[serial(env)]
    fn a_run_that_granted_yesterday_gets_what_it_granted_today() {
        // §7.1: the grant is recorded once and supplies every later run. Reading only the decision would leave the run with the destination closed and the credential absent.
        use crate::approval_flow::session::ConnectorPort;
        let home = tempfile::tempdir().expect("tempdir");
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", home.path());
        install_connectable(home.path());
        let port = RealConnectorPort::new(RUN.to_string());
        port.connect(
            "some-provider",
            "token",
            "work",
            lns_ipc::SecretValues(
                [("SOME_TOKEN".to_string(), "sk-live".to_string())]
                    .into_iter()
                    .collect(),
            ),
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

    #[test]
    #[serial(env)]
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
    #[serial(env)]
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
    #[serial(env)]
    fn a_run_that_declined_is_supplied_nothing() {
        use crate::approval_flow::session::ConnectorPort;
        let home = tempfile::tempdir().expect("tempdir");
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", home.path());
        install_connectable(home.path());
        RealConnectorPort::new(RUN.to_string())
            .decline("some-provider")
            .expect("decline");

        assert!(granted_supply_for(RUN).is_empty());
    }

    #[test]
    #[serial(env)]
    fn connector_state_that_cannot_be_read_supplies_nothing_rather_than_failing_the_run() {
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", "relative/dir");
        assert!(granted_supply_for(RUN).is_empty());
    }

    #[test]
    #[serial(env)]
    fn a_card_grant_against_bytes_that_changed_is_refused() {
        use crate::approval_flow::session::ConnectorPort;
        let home = tempfile::tempdir().expect("tempdir");
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", home.path());
        install_connectable(home.path());

        let refused = RealConnectorPort::new(RUN.to_string())
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
    #[serial(env)]
    fn a_card_decline_is_the_standing_no_the_next_run_reads() {
        use crate::approval_flow::session::ConnectorPort;
        let home = tempfile::tempdir().expect("tempdir");
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", home.path());
        install_one(home.path(), "api.some-provider.example");

        RealConnectorPort::new(RUN.to_string())
            .decline("some-provider")
            .expect("decline");

        assert!(offers_for_run(RUN).is_empty());
        assert!(
            !offers_for_run(OTHER_RUN).is_empty(),
            "the no is this run's, not the machine's"
        );
    }

    #[test]
    #[serial(env)]
    fn a_card_answer_that_cannot_reach_the_store_says_so_rather_than_panicking() {
        use crate::approval_flow::session::ConnectorPort;
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", "relative/dir");
        let port = RealConnectorPort::new(RUN.to_string());

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
