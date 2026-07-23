use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::connectors::Connector;

/// A connector pulled from an OCI registry, wrapped with the provenance needed to verify and re-check it: the reference as given, the resolved digests, when it was pulled, and the definition itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PulledConnector {
    pub source: String,
    pub manifest_digest: String,
    pub config_digest: String,
    pub pulled_at: String,
    pub definition: Connector,
}

/// The local cache of registry-pulled connectors (`~/.lns-pulled-connectors.yaml`), the third catalog source after bundled and user; not `deny_unknown_fields` because it is local state that later versions extend additively.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PulledCatalog {
    #[serde(default)]
    pub connectors: Vec<PulledConnector>,
}

impl PulledCatalog {
    /// Validates every pulled definition on load — the least-trusted, network-derived source gets at least the same load-time scrutiny as the user catalog, so a bad definition degrades to warn-and-ignore rather than reaching the merge.
    pub fn load_or_default(path: &Path) -> io::Result<Self> {
        match fs::read_to_string(path) {
            Ok(text) => {
                let catalog: PulledCatalog = serde_yaml::from_str(&text)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                crate::connectors::Catalog {
                    connectors: catalog
                        .connectors
                        .iter()
                        .map(|c| c.definition.clone())
                        .collect(),
                }
                .validate()
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                Ok(catalog)
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e),
        }
    }

    pub fn save_atomic(&self, path: &Path) -> io::Result<()> {
        let yaml = serde_yaml::to_string(self)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("yaml.tmp");
        fs::write(&tmp, yaml)?;
        fs::rename(&tmp, path)?;
        Ok(())
    }

    /// The stored config-blob digest for a pulled connector id — the fingerprint a per-workload grant's `definitionDigest` is later checked against so a redefined pull re-prompts.
    pub fn digest_for(&self, id: &str) -> Option<&str> {
        self.connectors
            .iter()
            .find(|c| c.definition.id == id)
            .map(|c| c.config_digest.as_str())
    }

    /// Insert or replace the pulled connector sharing this entry's id, returning the entry it replaced.
    pub fn upsert(&mut self, entry: PulledConnector) -> Option<PulledConnector> {
        let previous = self
            .connectors
            .iter()
            .position(|c| c.definition.id == entry.definition.id)
            .map(|i| self.connectors.remove(i));
        self.connectors.push(entry);
        previous
    }

    /// Remove the pulled connector with `id`, reporting whether one was present.
    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.connectors.len();
        self.connectors.retain(|c| c.definition.id != id);
        self.connectors.len() != before
    }
}

/// Honors `$LNS_PULLED_CONNECTORS_PATH`, else `$HOME/.lns-pulled-connectors.yaml`, else `./.lns-pulled-connectors.yaml`.
pub fn default_pulled_connectors_path() -> PathBuf {
    if let Some(p) = std::env::var_os("LNS_PULLED_CONNECTORS_PATH") {
        return PathBuf::from(p);
    }
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".lns-pulled-connectors.yaml"))
        .unwrap_or_else(|| PathBuf::from(".lns-pulled-connectors.yaml"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connectors::{AuthKind, ConnectorRoute, CredentialAuth};
    use crate::providers::{InjectionDef, InjectionKind};

    fn sample_definition(id: &str) -> Connector {
        Connector {
            id: id.into(),
            name: None,
            auth_kind: AuthKind::Credential,
            routes: vec![ConnectorRoute {
                match_pattern: format!("api.{id}.example"),
                transport: None,
                scheme: None,
                tls_terminate: false,
                rules: Vec::new(),
            }],
            credential: Some(CredentialAuth {
                env_var: "SOME_TOKEN".into(),
                placeholder: format!("{id}-LNSPLACEHOLDER0000"),
                injections: vec![InjectionDef {
                    kind: InjectionKind::BearerHeader,
                    domain: format!("api.{id}.example"),
                    header: None,
                }],
            }),
            oauth: None,
            token_fallback: None,
        }
    }

    fn sample_pulled(id: &str, config_digest: &str) -> PulledConnector {
        PulledConnector {
            source: format!("registry.lns.run/connectors/{id}:0.1.0"),
            manifest_digest: format!("sha256:{}", "a".repeat(64)),
            config_digest: config_digest.into(),
            pulled_at: "2026-07-23T10:00:00Z".into(),
            definition: sample_definition(id),
        }
    }

    #[test]
    fn load_or_default_returns_empty_when_the_file_is_missing() {
        let dir = tempfile::TempDir::new().unwrap();
        let catalog = PulledCatalog::load_or_default(&dir.path().join("absent.yaml")).unwrap();
        assert!(catalog.connectors.is_empty());
    }

    #[test]
    fn save_atomic_round_trips_through_load_or_default() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir
            .path()
            .join("nested")
            .join(".lns-pulled-connectors.yaml");
        let mut catalog = PulledCatalog::default();
        catalog.upsert(sample_pulled(
            "some-provider",
            &format!("sha256:{}", "b".repeat(64)),
        ));
        catalog.save_atomic(&path).unwrap();
        assert_eq!(PulledCatalog::load_or_default(&path).unwrap(), catalog);
    }

    #[test]
    fn save_atomic_leaves_no_tmp_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(".lns-pulled-connectors.yaml");
        PulledCatalog::default().save_atomic(&path).unwrap();
        assert!(!path.with_extension("yaml.tmp").exists());
    }

    #[test]
    fn load_or_default_surfaces_a_non_not_found_io_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("is-a-dir");
        fs::create_dir(&path).unwrap();
        let err = PulledCatalog::load_or_default(&path).unwrap_err();
        assert_ne!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn load_or_default_surfaces_malformed_yaml_as_invalid_data() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("broken.yaml");
        fs::write(&path, "connectors: not-a-list\n").unwrap();
        let err = PulledCatalog::load_or_default(&path).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn load_or_default_rejects_a_pulled_definition_with_upstream_transport() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(".lns-pulled-connectors.yaml");
        fs::write(
            &path,
            "connectors:\n  - source: registry.lns.run/connectors/some-provider:0.1.0\n    manifestDigest: sha256:aaa\n    configDigest: sha256:bbb\n    pulledAt: 2026-07-23T10:00:00Z\n    definition:\n      id: some-provider\n      authKind: credential\n      routes:\n        - match: api.some-provider.example\n          transport: upstream\n      credential:\n        envVar: SOME_TOKEN\n        placeholder: some-provider-LNSPLACEHOLDER\n",
        )
        .unwrap();
        let err = PulledCatalog::load_or_default(&path).unwrap_err();
        assert_eq!(
            err.kind(),
            io::ErrorKind::InvalidData,
            "a network-pulled definition must get the same load-time validation as the user catalog"
        );
    }

    #[test]
    fn load_or_default_rejects_a_pulled_definition_missing_its_auth_block() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(".lns-pulled-connectors.yaml");
        fs::write(
            &path,
            "connectors:\n  - source: registry.lns.run/connectors/some-provider:0.1.0\n    manifestDigest: sha256:aaa\n    configDigest: sha256:bbb\n    pulledAt: 2026-07-23T10:00:00Z\n    definition:\n      id: some-provider\n      authKind: credential\n",
        )
        .unwrap();
        let err = PulledCatalog::load_or_default(&path).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn digest_for_returns_the_config_digest_or_none() {
        let mut catalog = PulledCatalog::default();
        let digest = format!("sha256:{}", "c".repeat(64));
        catalog.upsert(sample_pulled("some-provider", &digest));
        assert_eq!(catalog.digest_for("some-provider"), Some(digest.as_str()));
        assert_eq!(catalog.digest_for("absent"), None);
    }

    #[test]
    fn upsert_appends_a_new_id_and_replaces_a_matching_one() {
        let mut catalog = PulledCatalog::default();
        assert!(
            catalog
                .upsert(sample_pulled("some-provider", "sha256:old"))
                .is_none(),
            "a first insert replaces nothing"
        );
        let previous = catalog
            .upsert(sample_pulled("some-provider", "sha256:new"))
            .expect("re-pulling the same id replaces the prior entry");
        assert_eq!(previous.config_digest, "sha256:old");
        assert_eq!(catalog.connectors.len(), 1, "an id is stored once");
        assert_eq!(catalog.digest_for("some-provider"), Some("sha256:new"));
    }

    #[test]
    fn remove_reports_whether_an_entry_was_present() {
        let mut catalog = PulledCatalog::default();
        catalog.upsert(sample_pulled("some-provider", "sha256:x"));
        assert!(catalog.remove("some-provider"));
        assert!(catalog.connectors.is_empty());
        assert!(
            !catalog.remove("some-provider"),
            "a second remove is a no-op"
        );
    }

    #[test]
    #[serial_test::serial(env)]
    fn default_path_uses_the_override_when_set() {
        use crate::test_env::EnvVarGuard;
        let _g1 = EnvVarGuard::set("LNS_PULLED_CONNECTORS_PATH", "/tmp/custom-pulled.yaml");
        let _g2 = EnvVarGuard::set("HOME", "/tmp/home-should-be-ignored");
        assert_eq!(
            default_pulled_connectors_path(),
            PathBuf::from("/tmp/custom-pulled.yaml")
        );
    }

    #[test]
    #[serial_test::serial(env)]
    fn default_path_falls_back_to_the_home_dotfile() {
        use crate::test_env::EnvVarGuard;
        let _g1 = EnvVarGuard::unset("LNS_PULLED_CONNECTORS_PATH");
        let _g2 = EnvVarGuard::set("HOME", "/home/dev");
        assert_eq!(
            default_pulled_connectors_path(),
            PathBuf::from("/home/dev/.lns-pulled-connectors.yaml")
        );
    }

    #[test]
    #[serial_test::serial(env)]
    fn default_path_falls_back_to_cwd_when_home_unset() {
        use crate::test_env::EnvVarGuard;
        let _g1 = EnvVarGuard::unset("LNS_PULLED_CONNECTORS_PATH");
        let _g2 = EnvVarGuard::unset("HOME");
        assert_eq!(
            default_pulled_connectors_path(),
            PathBuf::from(".lns-pulled-connectors.yaml")
        );
    }
}
