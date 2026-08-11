use serde::{Deserialize, Serialize};

pub use lns_spec::{InjectionDef, InjectionKind, is_self_identifying};

/// A catalog entry's credential, carrying the connector id the machine keys its value by; the injection contract itself is [`lns_spec::Credential`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderDef {
    pub id: String,
    pub env_var: String,
    pub placeholder: String,
    pub injections: Vec<InjectionDef>,
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
