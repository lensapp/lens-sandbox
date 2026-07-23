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
#[serde(rename_all = "camelCase", deny_unknown_fields)]
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

/// A placeholder must self-identify as fake so no real credential can leak into the shipped or declared provider set.
pub fn is_self_identifying(placeholder: &str) -> bool {
    let lower = placeholder.to_lowercase();
    lower.contains("placeholder") || lower.contains("lns")
}

#[cfg(test)]
mod tests {
    use super::*;

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
            domain: "api.example.test".into(),
            header: Some("x-api-key".into()),
        };
        let yaml = serde_yaml::to_string(&def).unwrap();
        assert!(yaml.contains("kind: api_key_header"), "got: {yaml}");
        assert!(yaml.contains("header: x-api-key"), "got: {yaml}");
        let parsed: InjectionDef = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(parsed, def);
    }
}
