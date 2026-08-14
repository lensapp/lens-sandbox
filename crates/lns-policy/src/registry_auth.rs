//! Registry logins live in `~/.lns-registry-auth.json`, not `lns-local-mixin.yaml`, to keep the shareable policy file free of per-machine secrets.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryCredential {
    pub username: String,
    pub secret: String,
}

pub type RegistryAuthFile = HashMap<String, RegistryCredential>;

pub trait RegistryAuthStore: Send + Sync {
    fn load(&self) -> io::Result<RegistryAuthFile>;
    fn save(&self, state: &RegistryAuthFile) -> io::Result<()>;
}

/// Falls back to `./.lns-registry-auth.json` when `HOME` is unset rather than panicking.
pub fn default_registry_auth_path() -> PathBuf {
    if let Some(p) = std::env::var_os("LNS_REGISTRY_AUTH_PATH") {
        return PathBuf::from(p);
    }
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".lns-registry-auth.json"))
        .unwrap_or_else(|| PathBuf::from(".lns-registry-auth.json"))
}

pub struct JsonFileRegistryAuthStore {
    pub path: PathBuf,
}

impl JsonFileRegistryAuthStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl RegistryAuthStore for JsonFileRegistryAuthStore {
    fn load(&self) -> io::Result<RegistryAuthFile> {
        match fs::read_to_string(&self.path) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(HashMap::new()),
            Err(e) => Err(e),
        }
    }

    fn save(&self, state: &RegistryAuthFile) -> io::Result<()> {
        let json = serde_json::to_string_pretty(state)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        crate::secure_file::write_json_secret_atomic(&self.path, json.as_bytes())
    }
}

const DOCKER_HUB_CANONICAL: &str = "docker.io";

/// Folds the Docker Hub aliases onto the single key `oci_client::Reference::registry()` resolves to, so a login and the later pull look up the same entry.
pub fn canonical_registry(host: &str) -> String {
    let lowered = host.trim().trim_end_matches('/').to_ascii_lowercase();
    match lowered.as_str() {
        ""
        | "docker.io"
        | "index.docker.io"
        | "registry-1.docker.io"
        | "registry.hub.docker.com" => DOCKER_HUB_CANONICAL.to_string(),
        _ => lowered,
    }
}

/// A registry must be a bare `host[:port]` — the value we can canonicalize and compare against a parsed reference's registry.
pub fn validate_registry_host(host: &str) -> Result<(), String> {
    if host.is_empty() {
        return Err("registry host must not be empty".to_string());
    }
    if host.contains("://") {
        return Err(format!(
            "registry {host:?} must be a bare host, not a URL (drop the scheme)"
        ));
    }
    if host.contains('/') {
        return Err(format!(
            "registry {host:?} must be a bare host[:port], not a path"
        ));
    }
    if host.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(format!("registry {host:?} must not contain whitespace"));
    }
    Ok(())
}

/// The stored credential for `registry`, looked up under its canonical key.
pub fn credential_for<'a>(
    file: &'a RegistryAuthFile,
    registry: &str,
) -> Option<&'a RegistryCredential> {
    file.get(&canonical_registry(registry))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> RegistryAuthFile {
        let mut m = RegistryAuthFile::new();
        m.insert(
            "ghcr.io".into(),
            RegistryCredential {
                username: "octocat".into(),
                secret: "ghp_real_token".into(),
            },
        );
        m
    }

    #[test]
    fn credential_round_trips_through_json() {
        let cred = RegistryCredential {
            username: "u".into(),
            secret: "s".into(),
        };
        let s = serde_json::to_string(&cred).unwrap();
        let back: RegistryCredential = serde_json::from_str(&s).unwrap();
        assert_eq!(cred, back);
    }

    #[test]
    fn load_returns_empty_state_when_file_missing() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = JsonFileRegistryAuthStore::new(dir.path().join("never.json"));
        assert!(store.load().unwrap().is_empty());
    }

    #[test]
    fn load_returns_invalid_data_for_malformed_json() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("auth.json");
        fs::write(&path, "{ not json").unwrap();
        let err = JsonFileRegistryAuthStore::new(path).load().unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn load_propagates_non_not_found_io_errors() {
        let dir = tempfile::TempDir::new().unwrap();
        let err = JsonFileRegistryAuthStore::new(dir.path().to_path_buf())
            .load()
            .unwrap_err();
        assert_ne!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn save_then_load_round_trips_full_state() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = JsonFileRegistryAuthStore::new(dir.path().join("auth.json"));
        let original = sample();
        store.save(&original).unwrap();
        assert_eq!(store.load().unwrap(), original);
    }

    #[test]
    fn canonical_registry_folds_docker_hub_aliases_to_docker_io() {
        for alias in [
            "",
            "docker.io",
            "index.docker.io",
            "registry-1.docker.io",
            "registry.hub.docker.com",
            "DOCKER.IO",
            "docker.io/",
        ] {
            assert_eq!(canonical_registry(alias), "docker.io", "alias {alias:?}");
        }
    }

    #[test]
    fn canonical_registry_lowercases_and_trims_other_hosts() {
        assert_eq!(canonical_registry(" GHCR.IO "), "ghcr.io");
        assert_eq!(canonical_registry("localhost:5000"), "localhost:5000");
        assert_eq!(
            canonical_registry("registry.example.test"),
            "registry.example.test"
        );
    }

    #[test]
    fn validate_registry_host_accepts_bare_hosts_and_ports() {
        validate_registry_host("ghcr.io").unwrap();
        validate_registry_host("localhost:5000").unwrap();
        validate_registry_host("registry.example.test").unwrap();
    }

    #[test]
    fn validate_registry_host_rejects_empty_urls_paths_and_whitespace() {
        assert!(validate_registry_host("").unwrap_err().contains("empty"));
        assert!(
            validate_registry_host("https://ghcr.io")
                .unwrap_err()
                .contains("URL")
        );
        assert!(
            validate_registry_host("ghcr.io/owner")
                .unwrap_err()
                .contains("path")
        );
        assert!(
            validate_registry_host("ghcr io")
                .unwrap_err()
                .contains("whitespace")
        );
    }

    #[test]
    fn credential_for_looks_up_under_the_canonical_key() {
        let mut file = RegistryAuthFile::new();
        file.insert(
            "docker.io".into(),
            RegistryCredential {
                username: "hubuser".into(),
                secret: "hubsecret".into(),
            },
        );
        // A pull reference resolves Docker Hub to its own registry alias; the lookup must still hit the stored docker.io entry.
        assert_eq!(
            credential_for(&file, "index.docker.io").unwrap().username,
            "hubuser"
        );
        assert!(credential_for(&file, "ghcr.io").is_none());
    }

    #[test]
    #[serial_test::serial(env)]
    fn default_registry_auth_path_uses_override_when_set() {
        use crate::test_env::EnvVarGuard;
        let _g1 = EnvVarGuard::set("LNS_REGISTRY_AUTH_PATH", "/tmp/custom-auth.json");
        let _g2 = EnvVarGuard::set("HOME", "/tmp/home-should-be-ignored");
        assert_eq!(
            default_registry_auth_path(),
            PathBuf::from("/tmp/custom-auth.json")
        );
    }

    #[test]
    #[serial_test::serial(env)]
    fn default_registry_auth_path_falls_back_to_home_dotfile() {
        use crate::test_env::EnvVarGuard;
        let _g1 = EnvVarGuard::unset("LNS_REGISTRY_AUTH_PATH");
        let _g2 = EnvVarGuard::set("HOME", "/home/dev");
        assert_eq!(
            default_registry_auth_path(),
            PathBuf::from("/home/dev/.lns-registry-auth.json")
        );
    }

    #[test]
    #[serial_test::serial(env)]
    fn default_registry_auth_path_falls_back_to_cwd_when_home_unset() {
        use crate::test_env::EnvVarGuard;
        let _g1 = EnvVarGuard::unset("LNS_REGISTRY_AUTH_PATH");
        let _g2 = EnvVarGuard::unset("HOME");
        assert_eq!(
            default_registry_auth_path(),
            PathBuf::from(".lns-registry-auth.json")
        );
    }
}
