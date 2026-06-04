use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderDef {
    pub id: String,
    pub env_var: String,
    pub placeholder: String,
    pub injections: Vec<InjectionDef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InjectionDef {
    pub kind: InjectionKind,
    pub domain: String,
    /// Only `ApiKeyHeader` carries a header name (e.g. `x-api-key`); other kinds leave it `None`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InjectionKind {
    BearerHeader,
    TokenHeader,
    BasicXAccessToken,
    ApiKeyHeader,
    UriPlaceholder,
}

const MANIFEST_TOML: &str = include_str!("providers.toml");

#[derive(Deserialize)]
struct Manifest {
    provider: Vec<ProviderDef>,
}

/// Panics on malformed TOML; the shipped manifest is test-proven well-formed, so the production caller never hits that arm.
fn parse(toml_src: &str) -> Vec<ProviderDef> {
    let manifest: Manifest =
        toml::from_str(toml_src).expect("credential provider manifest must be valid TOML");
    manifest.provider
}

static BUILTINS: LazyLock<Vec<ProviderDef>> = LazyLock::new(|| parse(MANIFEST_TOML));

/// The compiled-in provider set, unioned with policy-declared custom providers at run start.
pub fn builtins() -> &'static [ProviderDef] {
    BUILTINS.as_slice()
}

/// A placeholder must self-identify as fake so no real credential can leak into the shipped or declared provider set.
pub fn is_self_identifying(placeholder: &str) -> bool {
    let lower = placeholder.to_lowercase();
    lower.contains("placeholder") || lower.contains("lns")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn builtins_expose_every_shipped_provider() {
        let ids: HashSet<_> = builtins().iter().map(|p| p.id.as_str()).collect();
        for expected in ["github", "openai", "anthropic", "linear", "telegram"] {
            assert!(ids.contains(expected), "missing built-in {expected}");
        }
        let github = builtins().iter().find(|p| p.id == "github").unwrap();
        assert_eq!(github.env_var, "GITHUB_TOKEN");
        assert!(github.placeholder.starts_with("ghp_"));
        assert_eq!(github.injections.len(), 2);
        assert_eq!(github.injections[0].kind, InjectionKind::TokenHeader);
        assert_eq!(github.injections[0].domain, "api.github.com");
        assert_eq!(github.injections[1].kind, InjectionKind::BasicXAccessToken);
        assert_eq!(github.injections[1].domain, "github.com");
    }

    #[test]
    fn telegram_built_in_uses_uri_placeholder_injection() {
        let telegram = builtins().iter().find(|p| p.id == "telegram").unwrap();
        assert_eq!(telegram.injections[0].kind, InjectionKind::UriPlaceholder);
    }

    #[test]
    fn anthropic_built_in_uses_an_api_key_header_named_x_api_key() {
        let anthropic = builtins().iter().find(|p| p.id == "anthropic").unwrap();
        assert_eq!(anthropic.injections[0].kind, InjectionKind::ApiKeyHeader);
        assert_eq!(anthropic.injections[0].domain, "api.anthropic.com");
        assert_eq!(anthropic.injections[0].header.as_deref(), Some("x-api-key"));
    }

    #[test]
    fn anthropic_built_in_also_covers_authorization_bearer_for_openai_compatible_clients() {
        // OpenAI-compatible clients (e.g. hermes) hit api.anthropic.com with
        // `Authorization: Bearer`; without a bearer injection the placeholder
        // survives in that header and trips the proxy's leak gate.
        let anthropic = builtins().iter().find(|p| p.id == "anthropic").unwrap();
        let bearer = anthropic
            .injections
            .iter()
            .find(|i| i.kind == InjectionKind::BearerHeader)
            .expect("anthropic must inject a bearer header for OpenAI-compatible clients");
        assert_eq!(bearer.domain, "api.anthropic.com");
        assert!(
            bearer.header.is_none(),
            "bearer_header carries no header name"
        );
    }

    #[test]
    fn builtin_ids_and_env_vars_are_unique() {
        let mut ids = HashSet::new();
        let mut envs = HashSet::new();
        for p in builtins() {
            assert!(ids.insert(&p.id), "duplicate id {}", p.id);
            assert!(envs.insert(&p.env_var), "duplicate env_var {}", p.env_var);
        }
    }

    #[test]
    fn every_builtin_placeholder_self_identifies() {
        for p in builtins() {
            assert!(
                is_self_identifying(&p.placeholder),
                "{} placeholder doesn't self-identify: {}",
                p.id,
                p.placeholder
            );
        }
    }

    #[test]
    fn is_self_identifying_accepts_placeholder_or_lns_markers() {
        assert!(is_self_identifying("acme_LNSPLACEHOLDER0000"));
        assert!(is_self_identifying("lns-fake-value"));
        assert!(is_self_identifying("XPLACEHOLDERX"));
    }

    #[test]
    fn is_self_identifying_rejects_a_real_looking_token() {
        assert!(!is_self_identifying("acme_real_looking_token"));
    }

    #[test]
    #[should_panic(expected = "credential provider manifest must be valid TOML")]
    fn parse_panics_on_malformed_manifest() {
        parse("this is definitely not valid toml ][");
    }

    #[test]
    fn provider_def_round_trips_through_yaml_with_both_injection_kinds() {
        let def = ProviderDef {
            id: "acme".into(),
            env_var: "ACME_API_KEY".into(),
            placeholder: "acme_LNSPLACEHOLDER0000".into(),
            injections: vec![
                InjectionDef {
                    kind: InjectionKind::BearerHeader,
                    domain: "api.acme.corp".into(),
                    header: None,
                },
                InjectionDef {
                    kind: InjectionKind::UriPlaceholder,
                    domain: "api.acme.dev".into(),
                    header: None,
                },
            ],
        };
        let yaml = serde_yaml::to_string(&def).unwrap();
        assert!(yaml.contains("envVar: ACME_API_KEY"), "got: {yaml}");
        assert!(yaml.contains("kind: bearer_header"), "got: {yaml}");
        assert!(yaml.contains("kind: uri_placeholder"), "got: {yaml}");
        assert!(
            !yaml.contains("header:"),
            "headerless kinds omit header: {yaml}"
        );
        let parsed: ProviderDef = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed, def);
    }

    #[test]
    fn api_key_header_injection_round_trips_with_its_header_name() {
        let def = InjectionDef {
            kind: InjectionKind::ApiKeyHeader,
            domain: "api.anthropic.com".into(),
            header: Some("x-api-key".into()),
        };
        let yaml = serde_yaml::to_string(&def).unwrap();
        assert!(yaml.contains("kind: api_key_header"), "got: {yaml}");
        assert!(yaml.contains("header: x-api-key"), "got: {yaml}");
        let parsed: InjectionDef = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed, def);
    }
}
