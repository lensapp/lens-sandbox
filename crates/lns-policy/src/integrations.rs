use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

use crate::providers::InjectionDef;
use crate::{HttpRule, RouteRule, Scheme, Transport, Verdict, is_false};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthKind {
    Credential,
    Oauth,
}

/// A route an integration needs reachable. Verdict is implicitly `allow`; `transport` defaults to direct. Materializes into a full [`RouteRule`] at run time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrationRoute {
    #[serde(rename = "match")]
    pub match_pattern: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transport: Option<Transport>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheme: Option<Scheme>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub tls_terminate: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<HttpRule>,
}

impl IntegrationRoute {
    /// HTTP-level `rules` can only be enforced when the proxy terminates TLS, so declaring them implies termination.
    pub fn to_route_rule(&self) -> RouteRule {
        RouteRule {
            match_pattern: self.match_pattern.clone(),
            verdict: Verdict::Allow,
            transport: self.transport.unwrap_or(Transport::Direct),
            scheme: self.scheme,
            description: None,
            tls_terminate: self.tls_terminate || !self.rules.is_empty(),
            rules: self.rules.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialAuth {
    pub env_var: String,
    pub placeholder: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub injections: Vec<InjectionDef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Integration {
    pub id: String,
    pub auth_kind: AuthKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub routes: Vec<IntegrationRoute>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<CredentialAuth>,
}

impl Integration {
    /// A `credential` integration must carry its `credential:` block; `oauth` is recognized but inert until that work lands.
    pub fn validate(&self) -> Result<(), String> {
        match self.auth_kind {
            AuthKind::Credential if self.credential.is_none() => Err(format!(
                "integration {:?} declares authKind credential but has no `credential:` block",
                self.id
            )),
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    #[serde(default)]
    pub integrations: Vec<Integration>,
}

impl Catalog {
    fn validate(&self) -> Result<(), String> {
        for i in &self.integrations {
            i.validate()?;
        }
        Ok(())
    }

    pub fn load_or_default(path: &Path) -> io::Result<Self> {
        match fs::read_to_string(path) {
            Ok(text) => {
                let catalog: Catalog = serde_yaml::from_str(&text)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                catalog
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
}

const BUNDLED_YAML: &str = include_str!("integrations.yaml");

static BUNDLED: LazyLock<Vec<Integration>> =
    LazyLock::new(|| parse_catalog(BUNDLED_YAML).integrations);

/// Panics on a malformed or inconsistent manifest; the shipped catalog is test-proven well-formed, so the production caller never hits those arms.
fn parse_catalog(yaml_src: &str) -> Catalog {
    let catalog: Catalog =
        serde_yaml::from_str(yaml_src).expect("bundled integration catalog must be valid YAML");
    catalog
        .validate()
        .expect("bundled integration catalog must be internally consistent");
    catalog
}

pub fn bundled_integrations() -> &'static [Integration] {
    BUNDLED.as_slice()
}

/// Falls back to `./.lns-integrations.yaml` when `HOME` is unset rather than panicking.
pub fn default_integrations_path() -> PathBuf {
    if let Some(p) = std::env::var_os("LNS_INTEGRATIONS_PATH") {
        return PathBuf::from(p);
    }
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".lns-integrations.yaml"))
        .unwrap_or_else(|| PathBuf::from(".lns-integrations.yaml"))
}

/// The effective catalog is the bundled set extended with user entries whose id isn't already shipped — a bundled id can never be shadowed.
pub fn effective_integrations(user: &Catalog) -> Vec<Integration> {
    let mut out: Vec<Integration> = bundled_integrations().to_vec();
    let bundled_ids: HashSet<&str> = out.iter().map(|i| i.id.as_str()).collect();
    let extra: Vec<Integration> = user
        .integrations
        .iter()
        .filter(|i| !bundled_ids.contains(i.id.as_str()))
        .cloned()
        .collect();
    out.extend(extra);
    out
}

pub trait CatalogStore: Send + Sync {
    fn save(&self, catalog: &Catalog) -> io::Result<()>;
}

pub struct FileCatalogStore {
    pub path: PathBuf,
}

impl FileCatalogStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl CatalogStore for FileCatalogStore {
    fn save(&self, catalog: &Catalog) -> io::Result<()> {
        catalog.save_atomic(&self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::InjectionKind;

    fn credential(env_var: &str, placeholder: &str, domain: &str) -> CredentialAuth {
        CredentialAuth {
            env_var: env_var.into(),
            placeholder: placeholder.into(),
            injections: vec![InjectionDef {
                kind: InjectionKind::BearerHeader,
                domain: domain.into(),
                header: None,
            }],
        }
    }

    fn route(host: &str) -> IntegrationRoute {
        IntegrationRoute {
            match_pattern: host.into(),
            transport: None,
            scheme: None,
            tls_terminate: false,
            rules: Vec::new(),
        }
    }

    fn sample_integration() -> Integration {
        Integration {
            id: "acme".into(),
            auth_kind: AuthKind::Credential,
            routes: vec![route("api.acme.corp")],
            credential: Some(credential(
                "ACME_API_KEY",
                "acme_LNSPLACEHOLDER0000",
                "api.acme.corp",
            )),
        }
    }

    fn oauth_integration() -> Integration {
        Integration {
            id: "examplehub".into(),
            auth_kind: AuthKind::Oauth,
            routes: vec![route("api.examplehub.com")],
            credential: None,
        }
    }

    #[test]
    fn a_simple_integration_route_round_trips_as_just_a_match() {
        let r = route("gitlab.com");
        let yaml = serde_yaml::to_string(&r).unwrap();
        assert!(yaml.contains("match: gitlab.com"), "got: {yaml}");
        assert!(
            !yaml.contains("verdict")
                && !yaml.contains("transport")
                && !yaml.contains("tlsTerminate"),
            "a bare route must stay minimal: {yaml}"
        );
        let parsed: IntegrationRoute = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed, r);
    }

    #[test]
    fn a_least_privilege_integration_route_round_trips_with_scheme_and_http_rules() {
        let r = IntegrationRoute {
            match_pattern: "gitlab.com".into(),
            transport: None,
            scheme: Some(Scheme::Https),
            tls_terminate: false,
            rules: vec![HttpRule {
                method: Some("GET".into()),
                path: Some("/api/v4/**".into()),
            }],
        };
        let yaml = serde_yaml::to_string(&r).unwrap();
        assert!(yaml.contains("scheme: https"), "got: {yaml}");
        assert!(yaml.contains("path: /api/v4/**"), "got: {yaml}");
        let parsed: IntegrationRoute = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed, r);
    }

    #[test]
    fn to_route_rule_grants_allow_and_defaults_transport_to_direct() {
        let rr = route("gitlab.com").to_route_rule();
        assert_eq!(rr.match_pattern, "gitlab.com");
        assert_eq!(rr.verdict, Verdict::Allow);
        assert_eq!(rr.transport, Transport::Direct);
        assert!(!rr.tls_terminate);
        assert!(rr.rules.is_empty());
    }

    #[test]
    fn to_route_rule_honours_an_explicit_transport() {
        let mut r = route("gitlab.com");
        r.transport = Some(Transport::Upstream);
        assert_eq!(r.to_route_rule().transport, Transport::Upstream);
    }

    #[test]
    fn to_route_rule_implies_tls_termination_when_http_rules_are_present() {
        let r = IntegrationRoute {
            match_pattern: "gitlab.com".into(),
            transport: None,
            scheme: Some(Scheme::Https),
            tls_terminate: false,
            rules: vec![HttpRule {
                method: Some("GET".into()),
                path: None,
            }],
        };
        let rr = r.to_route_rule();
        assert!(
            rr.tls_terminate,
            "HTTP-level rules can't be enforced without terminating TLS"
        );
        assert_eq!(rr.scheme, Some(Scheme::Https));
        assert_eq!(rr.rules.len(), 1);
    }

    #[test]
    fn auth_kind_serializes_in_snake_case() {
        assert_eq!(
            serde_yaml::to_string(&AuthKind::Credential).unwrap().trim(),
            "credential"
        );
        assert_eq!(
            serde_yaml::to_string(&AuthKind::Oauth).unwrap().trim(),
            "oauth"
        );
    }

    #[test]
    fn credential_integration_round_trips_with_a_named_credential_block() {
        let i = sample_integration();
        let yaml = serde_yaml::to_string(&i).unwrap();
        assert!(yaml.contains("authKind: credential"), "got: {yaml}");
        assert!(yaml.contains("credential:"), "got: {yaml}");
        assert!(yaml.contains("envVar: ACME_API_KEY"), "got: {yaml}");
        let parsed: Integration = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed, i);
    }

    #[test]
    fn an_oauth_integration_round_trips_without_a_credential_block() {
        let i = oauth_integration();
        let yaml = serde_yaml::to_string(&i).unwrap();
        assert!(yaml.contains("authKind: oauth"), "got: {yaml}");
        assert!(
            !yaml.contains("credential"),
            "an oauth entry must not carry credential fields: {yaml}"
        );
        let parsed: Integration = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed, i);
    }

    #[test]
    fn validate_rejects_a_credential_integration_missing_its_block() {
        let bad = Integration {
            id: "x".into(),
            auth_kind: AuthKind::Credential,
            routes: Vec::new(),
            credential: None,
        };
        let err = bad.validate().unwrap_err();
        assert!(err.contains("credential"), "got: {err}");
    }

    #[test]
    fn validate_accepts_a_well_formed_credential_integration() {
        assert!(sample_integration().validate().is_ok());
    }

    #[test]
    fn validate_accepts_an_oauth_integration_with_no_block() {
        assert!(oauth_integration().validate().is_ok());
    }

    #[test]
    fn catalog_round_trips_and_empty_integrations_is_the_default() {
        let empty: Catalog = serde_yaml::from_str("{}").unwrap();
        assert!(empty.integrations.is_empty());
        let c = Catalog {
            integrations: vec![sample_integration()],
        };
        let parsed: Catalog = serde_yaml::from_str(&serde_yaml::to_string(&c).unwrap()).unwrap();
        assert_eq!(parsed, c);
    }

    #[test]
    fn bundled_catalog_parses_and_carries_no_builtin_id() {
        let ids: HashSet<&str> = bundled_integrations()
            .iter()
            .map(|i| i.id.as_str())
            .collect();
        assert!(
            !ids.is_empty(),
            "the bundled catalog should ship at least one service"
        );
        for builtin in ["github", "openai", "anthropic", "linear", "telegram"] {
            assert!(
                !ids.contains(builtin),
                "bundled catalog must not shadow the compiled-in built-in {builtin}"
            );
        }
    }

    #[test]
    fn bundled_catalog_ships_gitlab_and_huggingface() {
        let ids: HashSet<&str> = bundled_integrations()
            .iter()
            .map(|i| i.id.as_str())
            .collect();
        assert!(ids.contains("gitlab"), "got: {ids:?}");
        assert!(ids.contains("huggingface"), "got: {ids:?}");
    }

    #[test]
    fn bundled_gitlab_injects_both_private_token_for_glab_and_bearer_for_oauth_clients() {
        let gitlab = bundled_integrations()
            .iter()
            .find(|i| i.id == "gitlab")
            .expect("gitlab is bundled");
        let injections = &gitlab.credential.as_ref().unwrap().injections;
        assert!(
            injections
                .iter()
                .any(|inj| inj.kind == InjectionKind::ApiKeyHeader
                    && inj.domain == "gitlab.com"
                    && inj.header.as_deref() == Some("PRIVATE-TOKEN")),
            "glab sends its PAT in PRIVATE-TOKEN; expected an api_key_header injection for it, got: {injections:?}"
        );
        assert!(
            injections
                .iter()
                .any(|inj| inj.kind == InjectionKind::BearerHeader && inj.domain == "gitlab.com"),
            "Authorization: Bearer clients still need covering, got: {injections:?}"
        );
    }

    #[test]
    fn every_bundled_integration_is_a_valid_credential_with_self_identifying_placeholder_and_routes()
     {
        for i in bundled_integrations() {
            assert!(i.validate().is_ok(), "{} is inconsistent", i.id);
            assert_eq!(
                i.auth_kind,
                AuthKind::Credential,
                "{} ships as credential kind until oauth lands",
                i.id
            );
            let cred = i.credential.as_ref().expect("credential block present");
            assert!(
                crate::providers::is_self_identifying(&cred.placeholder),
                "{} placeholder must self-identify: {}",
                i.id,
                cred.placeholder
            );
            assert!(
                !i.routes.is_empty(),
                "{} must declare the routes it needs",
                i.id
            );
        }
    }

    #[test]
    #[should_panic(expected = "bundled integration catalog must be valid YAML")]
    fn parse_catalog_panics_on_malformed_yaml() {
        parse_catalog("integrations: [ this is : not valid");
    }

    #[test]
    #[should_panic(expected = "bundled integration catalog must be internally consistent")]
    fn parse_catalog_panics_on_a_credential_entry_missing_its_block() {
        parse_catalog("integrations:\n  - id: x\n    authKind: credential\n");
    }

    #[test]
    fn a_bundled_entry_pasted_into_a_user_file_deserializes_identically() {
        let entry = bundled_integrations()[0].clone();
        let as_user_catalog = Catalog {
            integrations: vec![entry.clone()],
        };
        let yaml = serde_yaml::to_string(&as_user_catalog).unwrap();
        let parsed: Catalog = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed.integrations, vec![entry]);
    }

    #[test]
    fn load_or_default_returns_empty_when_file_missing() {
        let dir = tempfile::TempDir::new().unwrap();
        let c = Catalog::load_or_default(&dir.path().join("nope.yaml")).unwrap();
        assert_eq!(c, Catalog::default());
    }

    #[test]
    fn load_or_default_reads_an_existing_user_catalog() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(".lns-integrations.yaml");
        Catalog {
            integrations: vec![sample_integration()],
        }
        .save_atomic(&path)
        .unwrap();
        let c = Catalog::load_or_default(&path).unwrap();
        assert_eq!(c.integrations.len(), 1);
        assert_eq!(c.integrations[0].id, "acme");
    }

    #[test]
    fn load_or_default_surfaces_non_not_found_io_errors() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("is-a-dir");
        fs::create_dir(&path).unwrap();
        let err = Catalog::load_or_default(&path).unwrap_err();
        assert_ne!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn load_or_default_surfaces_invalid_yaml_as_io_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("broken.yaml");
        fs::write(&path, "integrations: not-a-list\n").unwrap();
        let err = Catalog::load_or_default(&path).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn load_or_default_rejects_an_inconsistent_credential_entry_as_invalid_data() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("inconsistent.yaml");
        fs::write(
            &path,
            "integrations:\n  - id: x\n    authKind: credential\n",
        )
        .unwrap();
        let err = Catalog::load_or_default(&path).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn save_atomic_round_trips_creates_parent_and_leaves_no_tmp() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("nested/dir/.lns-integrations.yaml");
        let c = Catalog {
            integrations: vec![sample_integration()],
        };
        c.save_atomic(&path).unwrap();
        assert!(path.exists());
        assert!(!path.with_extension("yaml.tmp").exists());
        assert_eq!(Catalog::load_or_default(&path).unwrap(), c);
    }

    #[test]
    fn file_catalog_store_save_writes_yaml_readable_by_load_or_default() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join(".lns-integrations.yaml");
        let store = FileCatalogStore::new(path.clone());
        let c = Catalog {
            integrations: vec![sample_integration()],
        };
        store.save(&c).unwrap();
        assert_eq!(Catalog::load_or_default(&path).unwrap(), c);
    }

    #[test]
    fn file_catalog_store_save_to_unwritable_parent_surfaces_error() {
        let dir = tempfile::TempDir::new().unwrap();
        let not_a_dir = dir.path().join("file");
        fs::write(&not_a_dir, b"").unwrap();
        let store = FileCatalogStore::new(not_a_dir.join("nested/.lns-integrations.yaml"));
        let err = store.save(&Catalog::default()).unwrap_err();
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn effective_integrations_is_bundled_only_for_an_empty_user_catalog() {
        let eff = effective_integrations(&Catalog::default());
        assert_eq!(eff.len(), bundled_integrations().len());
    }

    #[test]
    fn effective_integrations_appends_a_user_only_integration() {
        let user = Catalog {
            integrations: vec![sample_integration()],
        };
        let eff = effective_integrations(&user);
        assert_eq!(eff.len(), bundled_integrations().len() + 1);
        assert!(eff.iter().any(|i| i.id == "acme"));
    }

    #[test]
    fn effective_integrations_drops_a_user_entry_that_shadows_a_bundled_id() {
        let mut shadow = sample_integration();
        shadow.id = "gitlab".into();
        shadow.credential = Some(credential("EVIL", "lns-evil", "gitlab.com"));
        let user = Catalog {
            integrations: vec![shadow],
        };
        let eff = effective_integrations(&user);
        assert_eq!(
            eff.len(),
            bundled_integrations().len(),
            "a user shadow must not add a second gitlab"
        );
        let gitlab = eff.iter().find(|i| i.id == "gitlab").unwrap();
        assert_ne!(
            gitlab.credential.as_ref().unwrap().env_var,
            "EVIL",
            "the bundled gitlab definition must win over a user shadow"
        );
    }

    #[test]
    #[serial_test::serial(env)]
    fn default_integrations_path_uses_override_when_set() {
        use crate::test_env::EnvVarGuard;
        let _g1 = EnvVarGuard::set("LNS_INTEGRATIONS_PATH", "/tmp/custom-integrations.yaml");
        let _g2 = EnvVarGuard::set("HOME", "/tmp/home-should-be-ignored");
        assert_eq!(
            default_integrations_path(),
            PathBuf::from("/tmp/custom-integrations.yaml")
        );
    }

    #[test]
    #[serial_test::serial(env)]
    fn default_integrations_path_falls_back_to_home_dotfile() {
        use crate::test_env::EnvVarGuard;
        let _g1 = EnvVarGuard::unset("LNS_INTEGRATIONS_PATH");
        let _g2 = EnvVarGuard::set("HOME", "/home/dev");
        assert_eq!(
            default_integrations_path(),
            PathBuf::from("/home/dev/.lns-integrations.yaml")
        );
    }

    #[test]
    #[serial_test::serial(env)]
    fn default_integrations_path_falls_back_to_cwd_when_home_unset() {
        use crate::test_env::EnvVarGuard;
        let _g1 = EnvVarGuard::unset("LNS_INTEGRATIONS_PATH");
        let _g2 = EnvVarGuard::unset("HOME");
        assert_eq!(
            default_integrations_path(),
            PathBuf::from(".lns-integrations.yaml")
        );
    }
}
