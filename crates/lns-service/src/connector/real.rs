//! Production wiring: the three connector stores at the paths cli-spec §9 names,
//! and the registry-or-machine resolver behind them.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use lns_ipc::Response;
use lns_policy::decision_store::JsonDecisionStore;

use super::handler;
use super::registry::RegistryConnectors;
use super::store::{ConnectorStore, Profile, ProjectDecision};

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
