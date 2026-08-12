//! The credential definition — `docs/sandbox-spec.md` §4.1.

use serde::{Deserialize, Serialize};

/// The one injection contract: this placeholder, in this variable, replaced by the real value on a request to each declared domain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Credential {
    pub env_var: String,
    pub placeholder: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub injections: Vec<InjectionDef>,
}

impl Credential {
    pub fn validate(&self) -> Result<(), String> {
        if !is_legal_env_var_name(&self.env_var) {
            return Err(format!(
                "invalid credential env var {:?}: an env var name must be non-empty and free of '=', whitespace, and control characters",
                self.env_var
            ));
        }
        validate_placeholder(&self.placeholder, &self.env_var)?;
        for injection in &self.injections {
            injection.validate(&self.env_var)?;
        }
        Ok(())
    }
}

/// The two rules a placeholder obeys wherever it is written, because the boundary substitutes it by substring and the document carrying it may be published.
pub fn validate_placeholder(placeholder: &str, owner: &str) -> Result<(), String> {
    if placeholder.len() < MIN_PLACEHOLDER_LEN {
        return Err(format!(
            "placeholder {placeholder:?} for {owner} must be at least {MIN_PLACEHOLDER_LEN} characters: the proxy finds this marker by substring in outbound bytes, so a short one would be substituted where it was never meant to be"
        ));
    }
    if !is_self_identifying(placeholder) {
        return Err(format!(
            "placeholder {placeholder:?} for {owner} must self-identify as fake: it has to contain \"placeholder\" or \"lns\", so a document that publishes to a registry cannot carry a real token as one"
        ));
    }
    Ok(())
}

/// Validate a document's whole `credentials` block: every entry on its own, plus the per-document rule that no two claim one variable.
pub fn validate_all(credentials: &[Credential]) -> Result<(), String> {
    let mut claimed = std::collections::BTreeSet::new();
    for credential in credentials {
        credential.validate()?;
        if !claimed.insert(&credential.env_var) {
            return Err(format!(
                "duplicate credential env var {:?}",
                credential.env_var
            ));
        }
    }
    Ok(())
}

/// A connector id is one DNS label, which is also what keeps it out of the `env:<var>` keyspace an unsupplied declaration answers under.
pub fn is_legal_connector_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    let is_alnum = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    !bytes.is_empty()
        && bytes.len() <= 63
        && is_alnum(bytes[0])
        && is_alnum(bytes[bytes.len() - 1])
        && bytes.iter().all(|&b| is_alnum(b) || b == b'-')
}

/// The grammar every kind shares for a variable the guest environment has to be able to hold.
pub fn is_legal_env_var_name(name: &str) -> bool {
    !name.is_empty()
        && !name
            .chars()
            .any(|c| c == '=' || c.is_control() || c.is_whitespace())
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

impl InjectionDef {
    fn validate(&self, env_var: &str) -> Result<(), String> {
        if self.domain.trim().is_empty() {
            return Err(format!(
                "credential {env_var} injection must name the domain it applies to"
            ));
        }
        if self.domain.trim() == CATCH_ALL_DOMAIN {
            return Err(format!(
                "credential {env_var} injection must name one destination, not {CATCH_ALL_DOMAIN:?}: a catch-all would put the real value on every host the workload reaches"
            ));
        }
        match (self.kind, &self.header) {
            (InjectionKind::ApiKeyHeader, None) => Err(format!(
                "credential {env_var}: an api_key_header injection must name the header it sets"
            )),
            (InjectionKind::ApiKeyHeader, Some(_)) => Ok(()),
            (_, Some(header)) => Err(format!(
                "credential {env_var}: only an api_key_header injection carries a header name, not {header:?}"
            )),
            (_, None) => Ok(()),
        }
    }
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

/// Long enough that a stream cannot carry the marker by accident; every bundled connector's placeholder is more than twice this.
pub const MIN_PLACEHOLDER_LEN: usize = 16;

/// The egress pattern that matches every host, which an injection may never name.
pub const CATCH_ALL_DOMAIN: &str = "*";

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

    #[test]
    fn a_misspelled_credential_key_fails_on_its_line() {
        let error = serde_yaml::from_str::<Credential>(
            "envVar: SOME_TOKEN\nplaceholdr: some_LNSPLACEHOLDER0000\n",
        )
        .expect_err("a misspelled key must fail rather than load with a default");
        assert!(error.to_string().contains("unknown field"), "got: {error}");
    }

    #[test]
    fn a_misspelled_injection_key_fails_on_its_line() {
        let error = serde_yaml::from_str::<InjectionDef>(
            "kind: api_key_header\ndomain: api.example.test\nheaader: x-api-key\n",
        )
        .expect_err("a misspelled key must fail rather than load with a default");
        assert!(error.to_string().contains("unknown field"), "got: {error}");
    }

    fn credential(env_var: &str, placeholder: &str, injections: Vec<InjectionDef>) -> Credential {
        Credential {
            env_var: env_var.into(),
            placeholder: placeholder.into(),
            injections,
        }
    }

    fn injection(kind: InjectionKind, domain: &str, header: Option<&str>) -> InjectionDef {
        InjectionDef {
            kind,
            domain: domain.into(),
            header: header.map(str::to_string),
        }
    }

    #[test]
    fn the_specification_example_validates() {
        let credential = credential(
            "SOME_TOKEN",
            "some_LNSPLACEHOLDER0000000000",
            vec![injection(
                InjectionKind::BearerHeader,
                "api.some-provider.example",
                None,
            )],
        );
        assert_eq!(credential.validate(), Ok(()));
    }

    #[test]
    fn an_env_var_no_process_could_carry_is_refused() {
        for name in [
            "",
            " ",
            "SOME_TOKEN=x",
            "SOME TOKEN",
            "SOME_TOKEN\nLD_PRELOAD",
        ] {
            let error = credential(name, "some_LNSPLACEHOLDER0000", Vec::new())
                .validate()
                .expect_err("an env var the guest environment cannot hold must not load");
            assert!(
                error.contains("invalid credential env var"),
                "{name:?}: got {error}"
            );
        }
    }

    #[test]
    fn a_placeholder_short_enough_to_occur_naturally_is_refused() {
        for placeholder in ["lns", "lns-x", "PLACEHOLDER", "lns-placehold"] {
            let error = credential("SOME_TOKEN", placeholder, Vec::new())
                .validate()
                .expect_err("the proxy substring-matches this marker in outbound bytes");
            assert!(
                error.contains("must be at least"),
                "{placeholder:?}: a marker a stream could carry by accident would be substituted where it was never meant to be; got {error}"
            );
        }
    }

    #[test]
    fn a_placeholder_at_the_floor_validates() {
        assert_eq!(
            credential("SOME_TOKEN", "lns-placeholder0", Vec::new()).validate(),
            Ok(())
        );
    }

    #[test]
    fn a_placeholder_a_real_token_could_pass_for_is_refused() {
        let error = credential("SOME_TOKEN", "sk-live-0123456789", Vec::new())
            .validate()
            .expect_err("a placeholder that reads like a token must not load");
        assert!(error.contains("must self-identify as fake"), "got: {error}");
    }

    #[test]
    fn an_injection_with_no_domain_is_refused() {
        let error = credential(
            "SOME_TOKEN",
            "some_LNSPLACEHOLDER0000",
            vec![injection(InjectionKind::BearerHeader, "  ", None)],
        )
        .validate()
        .expect_err("injection is domain-keyed, so a blank domain injects nowhere");
        assert!(error.contains("must name the domain"), "got: {error}");
    }

    #[test]
    fn an_injection_onto_every_destination_is_refused() {
        let error = credential(
            "SOME_TOKEN",
            "some_LNSPLACEHOLDER0000",
            vec![injection(InjectionKind::BearerHeader, "*", None)],
        )
        .validate()
        .expect_err("a catch-all destination is not a destination");
        assert!(
            error.contains("must name one destination"),
            "an injection describes where a secret may travel, so a catch-all would put the real value on every host the workload reaches; got: {error}"
        );
    }

    #[test]
    fn an_injection_onto_a_family_of_hosts_still_validates() {
        assert_eq!(
            credential(
                "SOME_TOKEN",
                "some_LNSPLACEHOLDER0000",
                vec![injection(
                    InjectionKind::BearerHeader,
                    "api.*.example.test",
                    None
                )],
            )
            .validate(),
            Ok(()),
            "a service that spreads over a host family is one destination; only the catch-all is none"
        );
    }

    #[test]
    fn a_header_name_on_a_kind_that_cannot_use_one_is_refused() {
        for kind in [
            InjectionKind::BearerHeader,
            InjectionKind::TokenHeader,
            InjectionKind::BasicXAccessToken,
            InjectionKind::UriPlaceholder,
        ] {
            let error = credential(
                "SOME_TOKEN",
                "some_LNSPLACEHOLDER0000",
                vec![injection(kind, "api.example.test", Some("x-api-key"))],
            )
            .validate()
            .expect_err("a header name the kind cannot use would silently do nothing");
            assert!(
                error.contains("only an api_key_header injection carries a header name"),
                "{kind:?}: got {error}"
            );
        }
    }

    #[test]
    fn an_api_key_header_with_no_header_name_is_refused() {
        let error = credential(
            "SOME_TOKEN",
            "some_LNSPLACEHOLDER0000",
            vec![injection(
                InjectionKind::ApiKeyHeader,
                "api.example.test",
                None,
            )],
        )
        .validate()
        .expect_err("the proxy has no header to set, so the injection does nothing");
        assert!(
            error.contains("api_key_header injection must name the header"),
            "got: {error}"
        );
    }

    #[test]
    fn two_credentials_claiming_one_env_var_are_refused() {
        let error = validate_all(&[
            credential("SOME_TOKEN", "some_LNSPLACEHOLDER0000", Vec::new()),
            credential("SOME_TOKEN", "other_LNSPLACEHOLDER0000", Vec::new()),
        ])
        .expect_err("nothing inside one document disambiguates two entries claiming one variable");
        assert!(
            error.contains("duplicate credential env var \"SOME_TOKEN\""),
            "got: {error}"
        );
    }

    #[test]
    fn a_list_of_distinct_credentials_validates_each_entry() {
        assert_eq!(
            validate_all(&[
                credential("SOME_TOKEN", "some_LNSPLACEHOLDER0000", Vec::new()),
                credential("OTHER_TOKEN", "other_LNSPLACEHOLDER0000", Vec::new()),
            ]),
            Ok(())
        );
        let error = validate_all(&[
            credential("SOME_TOKEN", "some_LNSPLACEHOLDER0000", Vec::new()),
            credential("OTHER_TOKEN", "a-real-looking-token", Vec::new()),
        ])
        .expect_err("a per-entry rule holds for every entry, not just the first");
        assert!(error.contains("must self-identify as fake"), "got: {error}");
    }

    #[test]
    fn a_connector_id_is_a_dns_label_so_it_can_never_reach_the_env_var_keyspace() {
        assert!(is_legal_connector_id("some-provider"));
        assert!(is_legal_connector_id("a1"));
        assert!(!is_legal_connector_id(""));
        assert!(!is_legal_connector_id("-a"));
        assert!(!is_legal_connector_id("a-"));
        assert!(!is_legal_connector_id("a_b"));
        assert!(!is_legal_connector_id(&"a".repeat(64)));
        assert!(
            !is_legal_connector_id("env:SOME_TOKEN"),
            "a declaration nothing supplies answers under env:<var>, so an id that could spell one would let a catalog entry take over its value"
        );
        assert!(!is_legal_connector_id("Some-Provider"));
    }

    #[test]
    fn a_legal_env_var_name_is_the_grammar_every_kind_shares() {
        assert!(is_legal_env_var_name("SOME_TOKEN"));
        assert!(!is_legal_env_var_name(""));
        assert!(!is_legal_env_var_name("SOME=TOKEN"));
        assert!(!is_legal_env_var_name("SOME TOKEN"));
        assert!(!is_legal_env_var_name("SOME\u{7f}TOKEN"));
    }
}
