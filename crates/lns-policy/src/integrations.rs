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

/// Which interactive sign-in an `oauth` integration uses: the RFC 8628 device flow (default) or the browser-redirect authorization-code + PKCE flow.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OauthFlow {
    #[default]
    Device,
    Pkce,
}

impl OauthFlow {
    fn is_device(&self) -> bool {
        matches!(self, OauthFlow::Device)
    }
}

/// Interactive sign-in configuration for an `oauth` integration: `flow` selects device (RFC 8628) or pkce, alongside the same env/placeholder/injection wiring a credential carries; `clientId` is optional (community builds ship none and fall back to a pasted token), with `deviceAuthorizationEndpoint` required for device and `authorizationEndpoint` for pkce; `clientSecret` is set only for confidential device clients (e.g. Google) that require it in the token exchange.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OauthAuth {
    #[serde(default, skip_serializing_if = "OauthFlow::is_device")]
    pub flow: OauthFlow,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scopes: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_authorization_endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authorization_endpoint: Option<String>,
    pub token_endpoint: String,
    pub env_var: String,
    pub placeholder: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub injections: Vec<InjectionDef>,
}

/// An offer-time fallback letting the user paste a token into the integration's existing credential slot when the primary auth (e.g. an oauth device flow) is blocked; `help` is an optional URL or hint for creating one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenFallback {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Integration {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub auth_kind: AuthKind,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub routes: Vec<IntegrationRoute>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<CredentialAuth>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oauth: Option<OauthAuth>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_fallback: Option<TokenFallback>,
}

impl Integration {
    /// The user-facing label for cards and prompts; falls back to the id when no `name` is set.
    pub fn display_name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.id)
    }

    /// Each authKind must carry its matching block, and an oauth block must carry the endpoint its `flow` needs.
    pub fn validate(&self) -> Result<(), String> {
        match self.auth_kind {
            AuthKind::Credential if self.credential.is_none() => Err(format!(
                "integration {:?} declares authKind credential but has no `credential:` block",
                self.id
            )),
            AuthKind::Oauth => self.validate_oauth(),
            _ => Ok(()),
        }
    }

    fn validate_oauth(&self) -> Result<(), String> {
        let Some(oauth) = self.oauth.as_ref() else {
            return Err(format!(
                "integration {:?} declares authKind oauth but has no `oauth:` block",
                self.id
            ));
        };
        match oauth.flow {
            OauthFlow::Device if oauth.device_authorization_endpoint.is_none() => Err(format!(
                "integration {:?} uses the oauth device flow but has no `deviceAuthorizationEndpoint`",
                self.id
            )),
            OauthFlow::Pkce if oauth.authorization_endpoint.is_none() => Err(format!(
                "integration {:?} uses the oauth pkce flow but has no `authorizationEndpoint`",
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

mod build_env {
    include!(concat!(env!("OUT_DIR"), "/env_substitutions.rs"));
}

static BUNDLED: LazyLock<Vec<Integration>> = LazyLock::new(|| {
    let resolved = crate::env_subst::apply_substitutions(
        include_str!("integrations.yaml"),
        build_env::ENV_SUBSTITUTIONS,
    );
    parse_catalog(&resolved).integrations
});

/// Panics on a malformed or inconsistent manifest; the shipped catalog is test-proven well-formed, so the production caller never hits those arms.
fn parse_catalog(yaml_src: &str) -> Catalog {
    let catalog: Catalog =
        serde_yaml::from_str(yaml_src).expect("bundled integration catalog must be valid YAML");
    let consistent = "bundled integration catalog must be internally consistent";
    catalog.validate().expect(consistent);
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

    fn oauth_auth(env_var: &str, placeholder: &str, domain: &str) -> OauthAuth {
        OauthAuth {
            flow: OauthFlow::Device,
            client_id: Some("Iv1.example0000".into()),
            client_secret: None,
            scopes: vec!["repo".into()],
            device_authorization_endpoint: Some("https://example.com/login/device/code".into()),
            authorization_endpoint: None,
            token_endpoint: "https://example.com/login/oauth/access_token".into(),
            env_var: env_var.into(),
            placeholder: placeholder.into(),
            injections: vec![InjectionDef {
                kind: InjectionKind::BearerHeader,
                domain: domain.into(),
                header: None,
            }],
        }
    }

    fn pkce_oauth_auth(env_var: &str, placeholder: &str, domain: &str) -> OauthAuth {
        OauthAuth {
            flow: OauthFlow::Pkce,
            client_id: None,
            client_secret: None,
            scopes: Vec::new(),
            device_authorization_endpoint: None,
            authorization_endpoint: Some("https://example.com/auth".into()),
            token_endpoint: "https://example.com/api/v1/auth/keys".into(),
            env_var: env_var.into(),
            placeholder: placeholder.into(),
            injections: vec![InjectionDef {
                kind: InjectionKind::BearerHeader,
                domain: domain.into(),
                header: None,
            }],
        }
    }

    fn pkce_integration() -> Integration {
        Integration {
            id: "examplepkce".into(),
            name: None,
            auth_kind: AuthKind::Oauth,
            routes: vec![route("api.examplepkce.com")],
            credential: None,
            oauth: Some(pkce_oauth_auth(
                "EXAMPLEPKCE_TOKEN",
                "examplepkce_LNSPLACEHOLDER0000",
                "api.examplepkce.com",
            )),
            token_fallback: None,
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
            name: None,
            auth_kind: AuthKind::Credential,
            routes: vec![route("api.acme.corp")],
            credential: Some(credential(
                "ACME_API_KEY",
                "acme_LNSPLACEHOLDER0000",
                "api.acme.corp",
            )),
            oauth: None,
            token_fallback: None,
        }
    }

    fn oauth_integration() -> Integration {
        Integration {
            id: "examplehub".into(),
            name: None,
            auth_kind: AuthKind::Oauth,
            routes: vec![route("api.examplehub.com")],
            credential: None,
            oauth: Some(oauth_auth(
                "EXAMPLEHUB_TOKEN",
                "examplehub_LNSPLACEHOLDER0000",
                "api.examplehub.com",
            )),
            token_fallback: None,
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
    fn oauth_flow_serializes_in_snake_case() {
        assert_eq!(
            serde_yaml::to_string(&OauthFlow::Device).unwrap().trim(),
            "device"
        );
        assert_eq!(
            serde_yaml::to_string(&OauthFlow::Pkce).unwrap().trim(),
            "pkce"
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
    fn an_oauth_integration_round_trips_with_an_oauth_block_and_no_credential_block() {
        let i = oauth_integration();
        let yaml = serde_yaml::to_string(&i).unwrap();
        assert!(yaml.contains("authKind: oauth"), "got: {yaml}");
        assert!(yaml.contains("oauth:"), "got: {yaml}");
        assert!(yaml.contains("clientId:"), "got: {yaml}");
        assert!(yaml.contains("deviceAuthorizationEndpoint:"), "got: {yaml}");
        assert!(
            !yaml.contains("credential:"),
            "an oauth entry must not carry a credential block: {yaml}"
        );
        let parsed: Integration = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed, i);
    }

    #[test]
    fn an_oauth_integration_round_trips_an_optional_client_secret() {
        let mut i = oauth_integration();
        i.oauth.as_mut().unwrap().client_secret = Some("some-client-secret".into());
        let yaml = serde_yaml::to_string(&i).unwrap();
        assert!(
            yaml.contains("clientSecret: some-client-secret"),
            "got: {yaml}"
        );
        let parsed: Integration = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed, i);
    }

    #[test]
    fn an_oauth_integration_without_a_client_secret_omits_it_from_yaml() {
        let yaml = serde_yaml::to_string(&oauth_integration()).unwrap();
        assert!(
            !yaml.contains("clientSecret"),
            "a public-client oauth entry must not serialize an empty client secret: {yaml}"
        );
    }

    #[test]
    fn display_name_prefers_an_explicit_name_and_falls_back_to_id() {
        let mut i = sample_integration();
        assert_eq!(
            i.display_name(),
            "acme",
            "an absent name falls back to the id"
        );
        i.name = Some("Acme Corp".into());
        assert_eq!(i.display_name(), "Acme Corp");
    }

    #[test]
    fn an_integration_round_trips_its_optional_display_name() {
        let mut i = oauth_integration();
        i.name = Some("ExampleHub".into());
        let yaml = serde_yaml::to_string(&i).unwrap();
        assert!(yaml.contains("name: ExampleHub"), "got: {yaml}");
        let parsed: Integration = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed, i);
    }

    #[test]
    fn an_integration_without_a_name_omits_it_from_yaml() {
        let yaml = serde_yaml::to_string(&sample_integration()).unwrap();
        assert!(
            !yaml.contains("name:"),
            "an absent name must not serialize: {yaml}"
        );
    }

    #[test]
    fn an_integration_round_trips_its_optional_token_fallback_with_a_help_url() {
        let mut i = oauth_integration();
        i.token_fallback = Some(TokenFallback {
            help: Some("https://example.com/tokens/new".into()),
        });
        let yaml = serde_yaml::to_string(&i).unwrap();
        assert!(yaml.contains("tokenFallback:"), "got: {yaml}");
        assert!(
            yaml.contains("help: https://example.com/tokens/new"),
            "got: {yaml}"
        );
        let parsed: Integration = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed, i);
    }

    #[test]
    fn a_token_fallback_round_trips_with_no_help() {
        let mut i = oauth_integration();
        i.token_fallback = Some(TokenFallback { help: None });
        let yaml = serde_yaml::to_string(&i).unwrap();
        assert!(yaml.contains("tokenFallback:"), "got: {yaml}");
        assert!(
            !yaml.contains("help:"),
            "an absent help must not serialize: {yaml}"
        );
        let parsed: Integration = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed, i);
    }

    #[test]
    fn an_integration_without_a_token_fallback_omits_it_from_yaml() {
        let yaml = serde_yaml::to_string(&oauth_integration()).unwrap();
        assert!(
            !yaml.contains("tokenFallback"),
            "an absent token fallback must not serialize: {yaml}"
        );
    }

    #[test]
    fn validate_rejects_a_credential_integration_missing_its_block() {
        let bad = Integration {
            id: "x".into(),
            name: None,
            auth_kind: AuthKind::Credential,
            routes: Vec::new(),
            credential: None,
            oauth: None,
            token_fallback: None,
        };
        let err = bad.validate().unwrap_err();
        assert!(err.contains("credential"), "got: {err}");
    }

    #[test]
    fn validate_accepts_a_well_formed_credential_integration() {
        assert!(sample_integration().validate().is_ok());
    }

    #[test]
    fn validate_rejects_an_oauth_integration_missing_its_block() {
        let bad = Integration {
            id: "x".into(),
            name: None,
            auth_kind: AuthKind::Oauth,
            routes: Vec::new(),
            credential: None,
            oauth: None,
            token_fallback: None,
        };
        let err = bad.validate().unwrap_err();
        assert!(err.contains("oauth"), "got: {err}");
    }

    #[test]
    fn validate_accepts_a_well_formed_oauth_integration() {
        assert!(oauth_integration().validate().is_ok());
    }

    #[test]
    fn an_oauth_block_defaults_to_the_device_flow_and_omits_it_from_yaml() {
        let yaml = serde_yaml::to_string(&oauth_integration()).unwrap();
        assert!(
            !yaml.contains("flow:"),
            "the default device flow must not serialize: {yaml}"
        );
        let parsed: Integration = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed.oauth.unwrap().flow, OauthFlow::Device);
    }

    #[test]
    fn a_pkce_oauth_integration_round_trips_with_flow_and_an_authorization_endpoint_and_no_client_id()
     {
        let i = pkce_integration();
        let yaml = serde_yaml::to_string(&i).unwrap();
        assert!(yaml.contains("flow: pkce"), "got: {yaml}");
        assert!(
            yaml.contains("authorizationEndpoint: https://example.com/auth"),
            "got: {yaml}"
        );
        assert!(
            !yaml.contains("clientId:"),
            "a pkce entry needs no client id: {yaml}"
        );
        assert!(
            !yaml.contains("deviceAuthorizationEndpoint:"),
            "a pkce entry has no device endpoint: {yaml}"
        );
        let parsed: Integration = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed, i);
    }

    #[test]
    fn validate_accepts_a_pkce_oauth_integration_without_a_client_id() {
        assert!(pkce_integration().validate().is_ok());
    }

    #[test]
    fn validate_rejects_a_pkce_oauth_integration_missing_its_authorization_endpoint() {
        let mut i = pkce_integration();
        i.oauth.as_mut().unwrap().authorization_endpoint = None;
        let err = i.validate().unwrap_err();
        assert!(err.contains("authorizationEndpoint"), "got: {err}");
    }

    #[test]
    fn validate_rejects_a_device_oauth_integration_missing_its_device_authorization_endpoint() {
        let mut i = oauth_integration();
        i.oauth.as_mut().unwrap().device_authorization_endpoint = None;
        let err = i.validate().unwrap_err();
        assert!(err.contains("deviceAuthorizationEndpoint"), "got: {err}");
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
    fn bundled_catalog_parses_and_ships_services() {
        let ids: HashSet<&str> = bundled_integrations()
            .iter()
            .map(|i| i.id.as_str())
            .collect();
        assert!(
            !ids.is_empty(),
            "the bundled catalog should ship at least one service"
        );
    }

    #[test]
    fn bundled_openai_injects_a_bearer_header() {
        let openai = bundled_integrations()
            .iter()
            .find(|i| i.id == "openai")
            .expect("openai is bundled");
        let cred = openai.credential.as_ref().unwrap();
        assert_eq!(cred.env_var, "OPENAI_API_KEY");
        assert!(
            cred.injections
                .iter()
                .any(|i| i.kind == InjectionKind::BearerHeader && i.domain == "api.openai.com"),
            "got: {:?}",
            cred.injections
        );
    }

    #[test]
    fn bundled_anthropic_covers_both_x_api_key_and_bearer_for_openai_compatible_clients() {
        let anthropic = bundled_integrations()
            .iter()
            .find(|i| i.id == "anthropic")
            .expect("anthropic is bundled");
        let cred = anthropic.credential.as_ref().unwrap();
        assert_eq!(cred.env_var, "ANTHROPIC_API_KEY");
        assert!(
            cred.injections
                .iter()
                .any(|i| i.kind == InjectionKind::ApiKeyHeader
                    && i.domain == "api.anthropic.com"
                    && i.header.as_deref() == Some("x-api-key")),
            "native SDKs send x-api-key, got: {:?}",
            cred.injections
        );
        assert!(
            cred.injections
                .iter()
                .any(|i| i.kind == InjectionKind::BearerHeader && i.domain == "api.anthropic.com"),
            "OpenAI-compatible clients send Authorization: Bearer, got: {:?}",
            cred.injections
        );
    }

    #[test]
    fn bundled_bedrock_injects_a_bearer_header_on_the_regional_runtime_and_control_planes() {
        let bedrock = bundled_integrations()
            .iter()
            .find(|i| i.id == "bedrock")
            .expect("bedrock is bundled");
        assert_eq!(bedrock.auth_kind, AuthKind::Credential);
        let cred = bedrock.credential.as_ref().unwrap();
        assert_eq!(cred.env_var, "AWS_BEARER_TOKEN_BEDROCK");
        for domain in ["bedrock-runtime.*.amazonaws.com", "bedrock.*.amazonaws.com"] {
            assert!(
                cred.injections
                    .iter()
                    .any(|i| i.kind == InjectionKind::BearerHeader && i.domain == domain),
                "the Bedrock API key rides as Authorization: Bearer to {domain}, got: {:?}",
                cred.injections
            );
        }
    }

    #[test]
    fn bundled_linear_injects_a_bearer_header() {
        let linear = bundled_integrations()
            .iter()
            .find(|i| i.id == "linear")
            .expect("linear is bundled");
        let cred = linear.credential.as_ref().unwrap();
        assert_eq!(cred.env_var, "LINEAR_API_KEY");
        assert!(
            cred.injections
                .iter()
                .any(|i| i.kind == InjectionKind::BearerHeader && i.domain == "api.linear.app"),
            "got: {:?}",
            cred.injections
        );
    }

    #[test]
    fn bundled_telegram_injects_via_uri_placeholder() {
        let telegram = bundled_integrations()
            .iter()
            .find(|i| i.id == "telegram")
            .expect("telegram is bundled");
        let cred = telegram.credential.as_ref().unwrap();
        assert_eq!(cred.env_var, "TELEGRAM_BOT_TOKEN");
        assert!(
            cred.injections
                .iter()
                .any(|i| i.kind == InjectionKind::UriPlaceholder && i.domain == "api.telegram.org"),
            "telegram embeds the token in the URL path, got: {:?}",
            cred.injections
        );
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
    fn every_bundled_integration_is_valid_with_a_self_identifying_placeholder_and_routes() {
        for i in bundled_integrations() {
            assert!(i.validate().is_ok(), "{} is inconsistent", i.id);
            let placeholder = match i.auth_kind {
                AuthKind::Credential => {
                    &i.credential
                        .as_ref()
                        .expect("credential block present")
                        .placeholder
                }
                AuthKind::Oauth => &i.oauth.as_ref().expect("oauth block present").placeholder,
            };
            assert!(
                crate::providers::is_self_identifying(placeholder),
                "{} placeholder must self-identify: {placeholder}",
                i.id,
            );
            assert!(
                !i.routes.is_empty(),
                "{} must declare the routes it needs",
                i.id
            );
        }
    }

    #[test]
    fn bundled_catalog_ships_github_as_an_oauth_integration() {
        let gh = bundled_integrations()
            .iter()
            .find(|i| i.id == "github")
            .expect("github is bundled");
        assert_eq!(gh.auth_kind, AuthKind::Oauth);
        let oauth = gh.oauth.as_ref().expect("oauth block present");
        assert_eq!(oauth.env_var, "GH_TOKEN");
        assert!(
            oauth
                .injections
                .iter()
                .any(|i| i.kind == InjectionKind::TokenHeader && i.domain == "api.github.com"),
            "gh/API use `Authorization: token`, got: {:?}",
            oauth.injections
        );
        assert!(
            oauth
                .injections
                .iter()
                .any(|i| i.kind == InjectionKind::BasicXAccessToken && i.domain == "github.com"),
            "git-over-HTTPS uses basic x-access-token, got: {:?}",
            oauth.injections
        );
    }

    #[test]
    fn bundled_github_declares_a_token_fallback_so_a_blocked_oauth_can_pivot_to_a_pat() {
        let gh = bundled_integrations()
            .iter()
            .find(|i| i.id == "github")
            .expect("github is bundled");
        let fallback = gh
            .token_fallback
            .as_ref()
            .expect("github must offer a token fallback for SSO/approval-gated orgs");
        assert!(
            fallback
                .help
                .as_deref()
                .is_some_and(|h| h.starts_with("https://")),
            "the fallback should point at a token-creation URL, got: {:?}",
            fallback.help
        );
    }

    #[test]
    fn bundled_catalog_ships_openrouter_as_a_pkce_integration() {
        let or = bundled_integrations()
            .iter()
            .find(|i| i.id == "openrouter")
            .expect("openrouter is bundled");
        assert_eq!(or.auth_kind, AuthKind::Oauth);
        let oauth = or.oauth.as_ref().expect("oauth block present");
        assert_eq!(
            oauth.flow,
            OauthFlow::Pkce,
            "OpenRouter signs in by the browser-redirect PKCE flow"
        );
        assert_eq!(oauth.env_var, "OPENROUTER_API_KEY");
        assert!(
            oauth.client_id.is_none(),
            "OpenRouter's PKCE flow takes no client id"
        );
        assert_eq!(
            oauth.authorization_endpoint.as_deref(),
            Some("https://openrouter.ai/auth")
        );
        assert_eq!(
            oauth.token_endpoint,
            "https://openrouter.ai/api/v1/auth/keys"
        );
        assert!(
            oauth
                .injections
                .iter()
                .any(|i| i.kind == InjectionKind::BearerHeader && i.domain == "openrouter.ai"),
            "OpenRouter keys are sent as Authorization: Bearer, got: {:?}",
            oauth.injections
        );
    }

    #[test]
    fn bundled_google_signs_in_via_oauth_device_flow_with_a_client_secret_and_injects_a_bearer_header()
     {
        let google = bundled_integrations()
            .iter()
            .find(|i| i.id == "google")
            .expect("google is bundled");
        assert_eq!(google.auth_kind, AuthKind::Oauth);
        let oauth = google.oauth.as_ref().expect("oauth block present");
        assert_eq!(oauth.flow, OauthFlow::Device);
        assert_eq!(oauth.env_var, "GOOGLE_OAUTH_ACCESS_TOKEN");
        assert_eq!(
            oauth.device_authorization_endpoint.as_deref(),
            Some("https://oauth2.googleapis.com/device/code")
        );
        assert_eq!(oauth.token_endpoint, "https://oauth2.googleapis.com/token");
        assert!(
            oauth.client_secret.is_some(),
            "Google's device flow requires a client secret in the token exchange"
        );
        assert!(
            oauth
                .injections
                .iter()
                .any(|i| i.kind == InjectionKind::BearerHeader && i.domain == "www.googleapis.com"),
            "Google APIs take Authorization: Bearer, got: {:?}",
            oauth.injections
        );
        assert!(
            google
                .token_fallback
                .as_ref()
                .and_then(|f| f.help.as_deref())
                .is_some_and(|h| h.starts_with("https://")),
            "an unconfigured build must let the user pivot to a pasted token, got: {:?}",
            google.token_fallback
        );
    }

    #[test]
    fn no_bundled_integration_commits_a_literal_oauth_client_id() {
        let raw = include_str!("integrations.yaml");
        for line in raw.lines() {
            if let Some(rest) = line.trim_start().strip_prefix("clientId:") {
                let value = rest.trim();
                assert!(
                    value.starts_with("\"${") && value.ends_with("}\""),
                    "clientId must be a build-time env reference, never a committed literal: {line:?}"
                );
            }
        }
    }

    #[test]
    fn no_bundled_integration_commits_a_literal_oauth_client_secret() {
        let raw = include_str!("integrations.yaml");
        for line in raw.lines() {
            if let Some(rest) = line.trim_start().strip_prefix("clientSecret:") {
                let value = rest.trim();
                assert!(
                    value.starts_with("\"${") && value.ends_with("}\""),
                    "clientSecret must be a build-time env reference, never a committed literal: {line:?}"
                );
            }
        }
    }

    #[test]
    fn the_github_oauth_client_id_is_sourced_from_the_build_env() {
        let raw = include_str!("integrations.yaml");
        assert!(
            raw.contains("clientId: \"${LNS_OAUTH_CLIENT_ID_GITHUB}\""),
            "the github oauth client_id must come from the LNS_OAUTH_CLIENT_ID_GITHUB build var, not a committed literal"
        );
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
