//! The credential definition — `docs/sandbox-spec.md` §4.1.

use serde::{Deserialize, Serialize};

/// The one injection contract: this placeholder, in this variable, replaced by the real value on a request to each declared domain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Credential {
    pub env_var: String,
    pub placeholder: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
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

/// A placeholder must self-identify as fake so no real credential can leak into a document that gets published.
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
    fn a_credential_reads_the_field_names_the_specification_uses() {
        let credential: Credential = serde_yaml::from_str(
            "envVar: SOME_TOKEN\n\
             placeholder: some_LNSPLACEHOLDER0000000000\n\
             injections:\n\
             \x20 - kind: bearer_header\n\
             \x20   domain: api.some-provider.example\n",
        )
        .expect("the §4.1 example shape parses");
        assert_eq!(credential.env_var, "SOME_TOKEN");
        assert_eq!(credential.placeholder, "some_LNSPLACEHOLDER0000000000");
        assert_eq!(credential.injections[0].kind, InjectionKind::BearerHeader);
        assert_eq!(credential.injections[0].domain, "api.some-provider.example");
        assert_eq!(credential.injections[0].header, None);
    }

    #[test]
    fn injections_default_to_none_declared() {
        let credential: Credential =
            serde_yaml::from_str("envVar: SOME_TOKEN\nplaceholder: lns-fake\n").unwrap();
        assert!(credential.injections.is_empty());
    }

    #[test]
    fn a_credential_round_trips_and_omits_what_it_does_not_carry() {
        let credential = Credential {
            env_var: "SOME_TOKEN".into(),
            placeholder: "some_LNSPLACEHOLDER0000".into(),
            injections: Vec::new(),
        };
        let yaml = serde_yaml::to_string(&credential).unwrap();
        assert!(yaml.contains("envVar: SOME_TOKEN"), "got: {yaml}");
        assert!(
            !yaml.contains("injections"),
            "an empty injection list is absent rather than written as [], so a hand-edited file stays as its author left it: {yaml}"
        );
        assert_eq!(
            serde_yaml::from_str::<Credential>(&yaml).unwrap(),
            credential
        );
    }

    #[test]
    fn every_injection_kind_round_trips_through_its_wire_name() {
        for (kind, wire) in [
            (InjectionKind::BearerHeader, "bearer_header"),
            (InjectionKind::TokenHeader, "token_header"),
            (InjectionKind::BasicXAccessToken, "basic_x_access_token"),
            (InjectionKind::ApiKeyHeader, "api_key_header"),
            (InjectionKind::UriPlaceholder, "uri_placeholder"),
        ] {
            let def = InjectionDef {
                kind,
                domain: "api.example.test".into(),
                header: None,
            };
            let yaml = serde_yaml::to_string(&def).unwrap();
            assert!(yaml.contains(&format!("kind: {wire}")), "got: {yaml}");
            assert_eq!(serde_yaml::from_str::<InjectionDef>(&yaml).unwrap(), def);
        }
    }

    #[test]
    fn api_key_header_carries_its_header_name_and_others_omit_it() {
        let named = InjectionDef {
            kind: InjectionKind::ApiKeyHeader,
            domain: "api.example.test".into(),
            header: Some("x-api-key".into()),
        };
        let yaml = serde_yaml::to_string(&named).unwrap();
        assert!(yaml.contains("header: x-api-key"), "got: {yaml}");
        assert_eq!(serde_yaml::from_str::<InjectionDef>(&yaml).unwrap(), named);

        let bare = InjectionDef {
            header: None,
            ..named
        };
        assert!(
            !serde_yaml::to_string(&bare).unwrap().contains("header:"),
            "a headerless kind must not write an empty header key"
        );
    }

    #[test]
    fn an_unknown_injection_kind_is_refused_rather_than_defaulted() {
        let error =
            serde_yaml::from_str::<InjectionDef>("kind: telepathy\ndomain: api.example.test\n")
                .expect_err("an injection kind the proxy cannot perform must not load");
        assert!(
            error.to_string().contains("unknown variant"),
            "got: {error}"
        );
    }
}
