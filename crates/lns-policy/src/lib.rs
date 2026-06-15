use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub mod credentials;
mod env_subst;
pub mod integrations;
pub mod providers;
pub mod registry_auth;
mod secure_file;
#[cfg(test)]
mod test_env;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Policy {
    #[serde(default)]
    pub network: NetworkPolicy,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub integrations: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkPolicy {
    #[serde(default)]
    pub allowed_routes: Vec<RouteRule>,
    pub default_verdict: Verdict,
    pub default_transport: Transport,
}

impl Default for NetworkPolicy {
    fn default() -> Self {
        Self {
            allowed_routes: Vec::new(),
            default_verdict: Verdict::Ask,
            default_transport: Transport::Direct,
        }
    }
}

pub(crate) fn is_false(b: &bool) -> bool {
    !*b
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteRule {
    // The schema field is named `match`; rename keeps the Rust ident legal.
    #[serde(rename = "match")]
    pub match_pattern: String,
    pub verdict: Verdict,
    pub transport: Transport,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheme: Option<Scheme>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub tls_terminate: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<HttpRule>,
}

/// HTTP method/path restriction within a route rule; wire-compatible with lens-sandbox-core's `HttpRule`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HttpRule {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Allow,
    Deny,
    Ask,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Transport {
    Upstream,
    Direct,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scheme {
    Http,
    Https,
}

impl Policy {
    pub fn load_or_default(path: &Path) -> io::Result<Self> {
        match fs::read_to_string(path) {
            Ok(text) => serde_yaml::from_str(&text)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
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

    pub fn add_rule(&mut self, rule: RouteRule) {
        self.network.allowed_routes.push(rule);
    }

    pub fn connect(&mut self, id: impl Into<String>) {
        let id = id.into();
        if !self.integrations.contains(&id) {
            self.integrations.push(id);
        }
    }

    pub fn disconnect(&mut self, id: &str) -> bool {
        let before = self.integrations.len();
        self.integrations.retain(|i| i != id);
        self.integrations.len() != before
    }
}

pub trait PolicyStore: Send + Sync {
    fn save(&self, policy: &Policy) -> io::Result<()>;
}

pub struct FilePolicyStore {
    pub path: PathBuf,
}

impl FilePolicyStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl PolicyStore for FilePolicyStore {
    fn save(&self, policy: &Policy) -> io::Result<()> {
        policy.save_atomic(&self.path)
    }
}

impl RouteRule {
    pub fn allow_host(host: impl Into<String>) -> Self {
        Self {
            match_pattern: host.into(),
            verdict: Verdict::Allow,
            transport: Transport::Direct,
            scheme: None,
            description: None,
            tls_terminate: false,
            rules: Vec::new(),
        }
    }

    pub fn deny_host(host: impl Into<String>) -> Self {
        Self {
            match_pattern: host.into(),
            verdict: Verdict::Deny,
            transport: Transport::Direct,
            scheme: None,
            description: None,
            tls_terminate: false,
            rules: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn default_policy_uses_verdict_ask_with_direct_transport() {
        let p = Policy::default();
        assert_eq!(p.network.default_verdict, Verdict::Ask);
        assert_eq!(p.network.default_transport, Transport::Direct);
        assert!(p.network.allowed_routes.is_empty());
    }

    #[test]
    fn default_policy_yaml_roundtrip_is_lossless() {
        let p = Policy::default();
        let yaml = serde_yaml::to_string(&p).unwrap();
        let parsed: Policy = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(p, parsed);
    }

    #[test]
    fn policy_with_rules_yaml_roundtrip_is_lossless() {
        let mut p = Policy::default();
        p.add_rule(RouteRule::allow_host("api.linear.app"));
        p.add_rule(RouteRule::deny_host("evil.example"));

        let yaml = serde_yaml::to_string(&p).unwrap();
        let parsed: Policy = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(p, parsed);
    }

    #[test]
    fn route_rule_serializes_with_match_key_not_match_pattern() {
        let rule = RouteRule::allow_host("api.linear.app");
        let yaml = serde_yaml::to_string(&rule).unwrap();
        assert!(
            yaml.contains("match:"),
            "expected `match:` key in YAML, got:\n{yaml}"
        );
        assert!(
            !yaml.contains("matchPattern"),
            "rust field name leaked into wire format:\n{yaml}"
        );
    }

    #[test]
    fn schemes_serialize_in_lowercase() {
        assert_eq!(serde_yaml::to_string(&Scheme::Http).unwrap().trim(), "http");
        assert_eq!(
            serde_yaml::to_string(&Scheme::Https).unwrap().trim(),
            "https"
        );
    }

    #[test]
    fn a_plain_route_omits_the_richness_keys() {
        let yaml = serde_yaml::to_string(&RouteRule::allow_host("api.example.com")).unwrap();
        assert!(
            !yaml.contains("scheme") && !yaml.contains("tlsTerminate") && !yaml.contains("rules"),
            "a plain allow rule must stay minimal:\n{yaml}"
        );
    }

    #[test]
    fn a_rich_route_rule_round_trips_with_sandbox_core_wire_names() {
        let rule = RouteRule {
            match_pattern: "gitlab.com".into(),
            verdict: Verdict::Allow,
            transport: Transport::Direct,
            scheme: Some(Scheme::Https),
            description: None,
            tls_terminate: true,
            rules: vec![HttpRule {
                method: Some("GET".into()),
                path: Some("/api/v4/**".into()),
            }],
        };
        let yaml = serde_yaml::to_string(&rule).unwrap();
        assert!(yaml.contains("scheme: https"), "got:\n{yaml}");
        assert!(yaml.contains("tlsTerminate: true"), "got:\n{yaml}");
        assert!(yaml.contains("path: /api/v4/**"), "got:\n{yaml}");
        let parsed: RouteRule = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed, rule);
    }

    #[test]
    fn an_http_rule_omits_absent_method_and_path() {
        let any = HttpRule {
            method: None,
            path: None,
        };
        let yaml = serde_yaml::to_string(&any).unwrap();
        assert!(!yaml.contains("method"), "got:\n{yaml}");
        assert!(!yaml.contains("path"), "got:\n{yaml}");
        let parsed: HttpRule = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed, any);
    }

    #[test]
    fn verdicts_serialize_in_lowercase() {
        assert_eq!(
            serde_yaml::to_string(&Verdict::Allow).unwrap().trim(),
            "allow"
        );
        assert_eq!(
            serde_yaml::to_string(&Verdict::Deny).unwrap().trim(),
            "deny"
        );
        assert_eq!(serde_yaml::to_string(&Verdict::Ask).unwrap().trim(), "ask");
    }

    #[test]
    fn transports_serialize_in_lowercase() {
        assert_eq!(
            serde_yaml::to_string(&Transport::Upstream).unwrap().trim(),
            "upstream"
        );
        assert_eq!(
            serde_yaml::to_string(&Transport::Direct).unwrap().trim(),
            "direct"
        );
    }

    #[test]
    fn load_or_default_returns_default_when_file_missing() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("no-such-file.yaml");
        let p = Policy::load_or_default(&path).unwrap();
        assert_eq!(p, Policy::default());
    }

    #[test]
    fn load_or_default_reads_existing_yaml() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("lns-policy.yaml");
        let yaml = "\
network:
  allowedRoutes:
    - match: api.linear.app
      verdict: allow
      transport: upstream
  defaultVerdict: ask
  defaultTransport: upstream
";
        fs::write(&path, yaml).unwrap();
        let p = Policy::load_or_default(&path).unwrap();
        assert_eq!(p.network.allowed_routes.len(), 1);
        assert_eq!(p.network.allowed_routes[0].match_pattern, "api.linear.app");
        assert_eq!(p.network.allowed_routes[0].verdict, Verdict::Allow);
    }

    #[test]
    fn load_or_default_surfaces_non_not_found_io_errors() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("is-actually-a-dir");
        fs::create_dir(&path).unwrap();
        let err = Policy::load_or_default(&path).unwrap_err();
        assert_ne!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn load_or_default_surfaces_invalid_yaml_as_io_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("broken.yaml");
        fs::write(&path, "network: not-a-map\n").unwrap();
        let err = Policy::load_or_default(&path).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn save_atomic_writes_file_readable_by_load_or_default() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("lns-policy.yaml");
        let mut p = Policy::default();
        p.add_rule(RouteRule::allow_host("api.linear.app"));

        p.save_atomic(&path).unwrap();
        let reloaded = Policy::load_or_default(&path).unwrap();
        assert_eq!(p, reloaded);
    }

    #[test]
    fn save_atomic_creates_parent_directory() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nested/dir/lns-policy.yaml");
        Policy::default().save_atomic(&path).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn save_atomic_leaves_no_tmp_file_on_success() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("lns-policy.yaml");
        Policy::default().save_atomic(&path).unwrap();
        let tmp = path.with_extension("yaml.tmp");
        assert!(!tmp.exists(), "tmp file should be renamed away");
    }

    #[test]
    fn file_policy_store_save_writes_yaml_readable_by_load_or_default() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("lns-policy.yaml");
        let store = FilePolicyStore::new(path.clone());

        let mut p = Policy::default();
        p.add_rule(RouteRule::allow_host("api.linear.app"));
        store.save(&p).unwrap();

        let reloaded = Policy::load_or_default(&path).unwrap();
        assert_eq!(reloaded, p);
    }

    #[test]
    fn file_policy_store_save_to_unwritable_parent_surfaces_error() {
        let dir = TempDir::new().unwrap();
        let unwritable = dir.path().join("file-not-a-dir");
        fs::write(&unwritable, b"").unwrap();
        let path = unwritable.join("nested/lns-policy.yaml");
        let store = FilePolicyStore::new(path);

        let err = store.save(&Policy::default()).unwrap_err();
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn add_rule_appends_to_allowed_routes_in_order() {
        let mut p = Policy::default();
        p.add_rule(RouteRule::allow_host("a"));
        p.add_rule(RouteRule::deny_host("b"));
        assert_eq!(p.network.allowed_routes.len(), 2);
        assert_eq!(p.network.allowed_routes[0].match_pattern, "a");
        assert_eq!(p.network.allowed_routes[0].verdict, Verdict::Allow);
        assert_eq!(p.network.allowed_routes[1].match_pattern, "b");
        assert_eq!(p.network.allowed_routes[1].verdict, Verdict::Deny);
    }

    #[test]
    fn legacy_network_only_yaml_parses_with_empty_integrations() {
        let yaml = "\
network:
  allowedRoutes: []
  defaultVerdict: ask
  defaultTransport: direct
";
        let p: Policy = serde_yaml::from_str(yaml).unwrap();
        assert!(p.integrations.is_empty());
    }

    #[test]
    fn default_policy_omits_the_integrations_key() {
        let yaml = serde_yaml::to_string(&Policy::default()).unwrap();
        assert!(
            !yaml.contains("integrations"),
            "an empty integrations list must not clutter the shareable file:\n{yaml}"
        );
    }

    #[test]
    fn policy_with_integrations_yaml_roundtrip_is_lossless() {
        let mut p = Policy::default();
        p.connect("gitlab");
        p.connect("acme");
        let yaml = serde_yaml::to_string(&p).unwrap();
        assert!(yaml.contains("integrations"), "got:\n{yaml}");
        let parsed: Policy = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(p, parsed);
    }

    #[test]
    fn a_legacy_policy_carrying_the_removed_credentials_section_still_loads() {
        let yaml = "\
network:
  allowedRoutes: []
  defaultVerdict: ask
  defaultTransport: direct
credentials:
  customProviders:
    - id: acme
      envVar: ACME_API_KEY
      placeholder: acme_LNSPLACEHOLDER0000000000000000000000
      injections:
        - kind: bearer_header
          domain: api.acme.corp
";
        let p: Policy = serde_yaml::from_str(yaml).unwrap();
        assert!(
            p.integrations.is_empty(),
            "the now-unknown credentials section is ignored, not an error"
        );
    }

    #[test]
    fn connect_adds_an_integration_id() {
        let mut p = Policy::default();
        p.connect("github");
        assert_eq!(p.integrations, ["github"]);
    }

    #[test]
    fn connect_is_idempotent_and_does_not_duplicate() {
        let mut p = Policy::default();
        p.connect("github");
        p.connect("github");
        assert_eq!(p.integrations, ["github"]);
    }

    #[test]
    fn disconnect_removes_an_applied_integration_and_reports_true() {
        let mut p = Policy::default();
        p.connect("github");
        p.connect("gitlab");
        assert!(p.disconnect("github"));
        assert_eq!(p.integrations, ["gitlab"]);
    }

    #[test]
    fn disconnect_reports_false_when_the_id_is_not_applied() {
        let mut p = Policy::default();
        p.connect("gitlab");
        assert!(!p.disconnect("github"));
        assert_eq!(p.integrations, ["gitlab"]);
    }
}
