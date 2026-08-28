//! Production wiring: the three connector stores at the paths cli-spec §9 names,
//! and the registry-or-machine resolver behind them.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use lns_ipc::Response;
use lns_policy::decision_store::JsonDecisionStore;

use super::handler;
use super::registry::RegistryConnectors;
use super::store::{ConnectorStore, Profile, ProjectDecision};
use crate::approval_flow::protocol::GrantedPayload;

/// One machine verb, named without the wire type so the dispatcher hands over no store of its own.
pub enum Call {
    Install(String),
    Uninstall(String),
    List,
    Connect {
        name: String,
        method: String,
        profile: String,
        values: std::collections::BTreeMap<String, String>,
    },
    Disconnect {
        name: String,
        profile: Option<String>,
    },
    Grant {
        name: String,
        project_dir: String,
        method: String,
        profile: Option<String>,
    },
    Forget {
        name: String,
        project_dir: String,
    },
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

/// The connectors a run in this directory can be offered (§3.2.1). Unreadable connector state offers none, because a run must still launch beside it.
pub fn offers_for_project(project_dir: &Path) -> Vec<lns_ipc::ConnectorView> {
    match read_offers(project_dir) {
        Ok(patterns) => patterns,
        Err(e) => {
            crate::log::warn!(
                "cannot read this machine's connectors, so this run is offered none: {e:#}"
            );
            Vec::new()
        }
    }
}

fn read_offers(project_dir: &Path) -> Result<Vec<lns_ipc::ConnectorView>> {
    with_project_store(project_dir, handler::offerable)
}

/// The connector store as the approval session reaches it: every method opens the three stores itself, because a card outlives no lock.
pub struct RealConnectorPort {
    project_dir: String,
}

impl RealConnectorPort {
    pub fn new(project_dir: String) -> Self {
        Self { project_dir }
    }

    fn with_store<T>(&self, f: impl FnOnce(&ConnectorStore<'_>) -> Result<T>) -> Result<T, String> {
        let paths = Paths::resolve().map_err(|e| format!("{e:#}"))?;
        let installed = super::dir::ConnectorDir::new(paths.connectors);
        let values: JsonDecisionStore<Profile> = JsonDecisionStore::new(paths.values);
        let grants: JsonDecisionStore<ProjectDecision> = JsonDecisionStore::new(paths.grants);
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
            Ok(handler::connect(store, name, method, label, values.0)?.invalidated)
        })
    }

    fn grant(
        &self,
        name: &str,
        digest: &str,
        method: &str,
        profile: Option<&str>,
    ) -> Result<crate::approval_flow::protocol::GrantedPayload, String> {
        self.with_store(|store| {
            handler::grant_disclosed(store, name, digest, &self.project_dir, method, profile)
        })
    }

    fn decline(&self, name: &str) -> Result<(), String> {
        self.with_store(|store| {
            store.decide(&self.project_dir, name, ProjectDecision::Declined)?;
            Ok(())
        })
    }
}

/// What the grants this project already made supply to a starting run, by connector (§7.1). Unreadable connector state supplies nothing, as it offers nothing.
pub fn granted_supply_for(project_dir: &Path) -> BTreeMap<String, GrantedPayload> {
    match read_granted_supply(project_dir) {
        Ok(supply) => supply,
        Err(e) => {
            crate::log::warn!(
                "cannot read what this project granted, so this run starts without it: {e:#}"
            );
            BTreeMap::new()
        }
    }
}

fn read_granted_supply(project_dir: &Path) -> Result<BTreeMap<String, GrantedPayload>> {
    with_project_store(project_dir, |store, dir| {
        handler::granted_supply(store, dir)
    })
}

/// Opens the three stores against the folded project directory a grant is keyed by.
fn with_project_store<T>(
    project_dir: &Path,
    f: impl FnOnce(&ConnectorStore<'_>, &str) -> Result<T>,
) -> Result<T> {
    let folded = lns_artifact::sandbox::fold_path(project_dir);
    let dir = folded
        .to_str()
        .with_context(|| format!("project directory {} is not utf-8", folded.display()))?;
    let paths = Paths::resolve()?;
    let installed = super::dir::ConnectorDir::new(paths.connectors);
    let values: JsonDecisionStore<Profile> = JsonDecisionStore::new(paths.values);
    let grants: JsonDecisionStore<ProjectDecision> = JsonDecisionStore::new(paths.grants);
    f(&ConnectorStore::new(&installed, &values, &grants), dir)
}

pub async fn answer(call: Call) -> Result<Response> {
    let paths = Paths::resolve()?;
    let installed = super::dir::ConnectorDir::new(paths.connectors);
    let values: JsonDecisionStore<Profile> = JsonDecisionStore::new(paths.values);
    let grants: JsonDecisionStore<ProjectDecision> = JsonDecisionStore::new(paths.grants);
    let store = ConnectorStore::new(&installed, &values, &grants);
    match call {
        Call::Install(source) => Ok(Response::ConnectorInstalled {
            connector: handler::install(&store, &RegistryConnectors, &source).await?,
        }),
        Call::Uninstall(name) => Ok(match handler::uninstall(&store, &name)? {
            Some(dropped_profiles) => Response::ConnectorUninstalled {
                name,
                dropped_profiles,
            },
            None => Response::ConnectorUnknown { name },
        }),
        Call::List => Ok(Response::ConnectorList {
            connectors: handler::list(&store)?,
        }),
        Call::Connect {
            name,
            method,
            profile,
            values,
        } => {
            let connected = handler::connect(&store, &name, &method, &profile, values)?;
            Ok(Response::ConnectorConnected {
                name,
                profile: connected.profile,
                invalidated: connected.invalidated,
            })
        }
        Call::Disconnect { name, profile } => Ok(Response::ConnectorDisconnected {
            dropped: handler::disconnect(&store, &name, profile.as_deref())?,
            name,
        }),
        Call::Grant {
            name,
            project_dir,
            method,
            profile,
        } => {
            let granted = handler::grant(&store, &name, &project_dir, &method, profile.as_deref())?;
            Ok(Response::ConnectorGranted {
                name,
                method: granted.method,
                profile: granted.profile,
                displaced: granted.displaced,
                unchanged: granted.unchanged,
            })
        }
        Call::Forget { name, project_dir } => Ok(Response::ConnectorForgotten {
            had_decision: handler::forget(&store, &name, &project_dir)?,
            name,
        }),
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
            "apiVersion: lns.run/v1\nkind: connector\nname: some-provider\nspec:\n  serves: [api.some-provider.example]\n  methods:\n    - name: token\n      auth:\n        kind: token\n      filesets:\n        - path: ./seed\n          guestPath: /home/agent/.some-provider\n",
        )
        .unwrap();
        std::fs::create_dir(dir.path().join("seed")).unwrap();
        std::fs::write(dir.path().join("seed").join("notes.md"), b"hello").unwrap();

        let fetched = super::super::source::read_local(&RealSnapshotFs, read_document, dir.path())
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
    fn decline(home: &Path, dir: &str) {
        let installed = super::super::dir::ConnectorDir::new(home.join("connectors"));
        let values: JsonDecisionStore<Profile> =
            JsonDecisionStore::new(home.join("connector-values.json"));
        let grants: JsonDecisionStore<ProjectDecision> =
            JsonDecisionStore::new(home.join("connector-grants.json"));
        ConnectorStore::new(&installed, &values, &grants)
            .decide(dir, "some-provider", ProjectDecision::Declined)
            .expect("record a decline");
    }

    #[test]
    #[serial(env)]
    fn a_run_holds_what_an_undecided_connector_on_this_machine_serves() {
        let home = tempfile::tempdir().expect("tempdir");
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", home.path());
        install_one(home.path(), "api.some-provider.example");
        assert_eq!(
            served_by(offers_for_project(Path::new("/work"))),
            ["api.some-provider.example"]
        );
    }

    #[test]
    #[serial(env)]
    fn the_project_directory_is_folded_the_way_a_grant_keys_it() {
        // The decision `lns connector grant` writes is keyed by a folded path; reading an unfolded one here would make every grant invisible and re-offer what the project already decided.
        let home = tempfile::tempdir().expect("tempdir");
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", home.path());
        install_one(home.path(), "api.some-provider.example");
        decline(home.path(), "/work");
        assert!(
            offers_for_project(Path::new("/work/sub/..")).is_empty(),
            "/work/sub/.. is /work, so this project has already declined"
        );
        assert_eq!(
            served_by(offers_for_project(Path::new("/elsewhere"))),
            ["api.some-provider.example"],
            "the control: an empty answer above must come from the decline, not from state this test failed to write"
        );
    }

    #[test]
    #[serial(env)]
    fn connector_state_this_build_cannot_reach_holds_nothing_rather_than_failing_the_run() {
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", "relative/dir");
        assert!(offers_for_project(Path::new("/work")).is_empty());
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

    #[test]
    #[serial(env)]
    fn a_card_connects_then_grants_and_the_guest_is_given_what_the_method_supplies() {
        use crate::approval_flow::session::ConnectorPort;
        let home = tempfile::tempdir().expect("tempdir");
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", home.path());
        install_connectable(home.path());
        let port = RealConnectorPort::new("/work".to_string());

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
        assert!(invalidated.is_empty(), "no project granted anything yet");

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
            offers_for_project(Path::new("/work")).is_empty(),
            "the project decided, so nothing it serves is offered again"
        );
    }

    #[test]
    #[serial(env)]
    fn a_project_that_granted_yesterday_gets_what_it_granted_today() {
        // §7.1: the grant is recorded once and supplies every later run. Reading only the decision would leave the run with the destination closed and the credential absent.
        use crate::approval_flow::session::ConnectorPort;
        let home = tempfile::tempdir().expect("tempdir");
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", home.path());
        install_connectable(home.path());
        let port = RealConnectorPort::new("/work".to_string());
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

        let supplied = granted_supply_for(Path::new("/work"));
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
            "the profile the grant named is the one that arms it"
        );
        assert!(
            granted_supply_for(Path::new("/elsewhere")).is_empty(),
            "another project granted nothing, so it gets nothing"
        );
    }

    #[test]
    #[serial(env)]
    fn a_project_that_granted_two_connectors_is_supplied_both() {
        // Stopping at the first recorded grant would leave the second connector's destination closed, chosen by whichever the store listed first.
        let home = tempfile::tempdir().expect("tempdir");
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", home.path());
        install_connectable(home.path());
        install_named(home.path(), "other-provider", "api.other-provider.example");
        with_project_store(Path::new("/work"), |store, dir| {
            for name in ["some-provider", "other-provider"] {
                store.decide(
                    dir,
                    name,
                    ProjectDecision::Granted {
                        digest: "sha256:abc".to_string(),
                        method: "token".to_string(),
                        profile: None,
                        authority: Default::default(),
                    },
                )?;
            }
            Ok(())
        })
        .expect("record both grants");

        let supplied = granted_supply_for(Path::new("/work"));

        assert_eq!(
            supplied.keys().collect::<Vec<_>>(),
            ["other-provider", "some-provider"],
            "both grants are this project's"
        );
    }

    #[test]
    #[serial(env)]
    fn a_grant_made_against_bytes_that_have_since_changed_supplies_nothing() {
        // The connector was updated after the grant, so what it now opens is not what the project consented to.
        let home = tempfile::tempdir().expect("tempdir");
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", home.path());
        install_connectable(home.path());
        with_project_store(Path::new("/work"), |store, dir| {
            store.decide(
                dir,
                "some-provider",
                ProjectDecision::Granted {
                    digest: "sha256:the-version-they-agreed-to".to_string(),
                    method: "token".to_string(),
                    profile: None,
                    authority: Default::default(),
                },
            )?;
            Ok(())
        })
        .expect("record a grant against older bytes");

        assert!(granted_supply_for(Path::new("/work")).is_empty());
    }

    #[test]
    #[serial(env)]
    fn a_project_that_declined_is_supplied_nothing() {
        use crate::approval_flow::session::ConnectorPort;
        let home = tempfile::tempdir().expect("tempdir");
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", home.path());
        install_connectable(home.path());
        RealConnectorPort::new("/work".to_string())
            .decline("some-provider")
            .expect("decline");

        assert!(granted_supply_for(Path::new("/work")).is_empty());
    }

    #[test]
    #[serial(env)]
    fn connector_state_that_cannot_be_read_supplies_nothing_rather_than_failing_the_run() {
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", "relative/dir");
        assert!(granted_supply_for(Path::new("/work")).is_empty());
    }

    #[test]
    #[serial(env)]
    fn a_card_grant_against_bytes_that_changed_is_refused() {
        use crate::approval_flow::session::ConnectorPort;
        let home = tempfile::tempdir().expect("tempdir");
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", home.path());
        install_connectable(home.path());

        let refused = RealConnectorPort::new("/work".to_string())
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

        RealConnectorPort::new("/work".to_string())
            .decline("some-provider")
            .expect("decline");

        assert!(offers_for_project(Path::new("/work")).is_empty());
        assert!(
            !offers_for_project(Path::new("/elsewhere")).is_empty(),
            "the no is this project's, not the machine's"
        );
    }

    #[test]
    #[serial(env)]
    fn a_card_answer_that_cannot_reach_the_store_says_so_rather_than_panicking() {
        use crate::approval_flow::session::ConnectorPort;
        let _guard = crate::test_env::EnvVarGuard::set("LNS_HOME", "relative/dir");
        let port = RealConnectorPort::new("/work".to_string());

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
