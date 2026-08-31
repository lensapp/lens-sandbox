//! The credential definition — `docs/sandbox-spec.md` §4.1.

use serde::{Deserialize, Serialize};

use crate::env_var::is_legal_env_var_name;

/// Where a credential is declared, which decides whether it must name a variable and may draw from an `auth` (§4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// A `sandbox` or `mixin`, where the workload reads the value from a variable, so the variable is the declaration.
    Document,
    /// A connector method, where the credential is a supply that may land in a fileset instead of a variable.
    Method,
}

/// The one injection contract: this placeholder, in this variable, replaced by the real value on a request to each declared domain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Credential {
    /// The variable the workload sees. Required outside a method; inside one a credential may exist only to be injected on the wire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_var: Option<String>,
    pub placeholder: String,
    /// Which of the method's `auth` outputs supplies this value; refused outside a method, where there is no `auth` to draw from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub injections: Vec<InjectionDef>,
}

impl Credential {
    /// What names this credential in a diagnostic: the variable where there is one, else the marker that is always there.
    pub fn owner(&self) -> &str {
        self.env_var.as_deref().unwrap_or(&self.placeholder)
    }

    fn validate(&self, source: Source) -> Result<(), String> {
        match (&self.env_var, source) {
            (Some(env_var), _) if !is_legal_env_var_name(env_var) => {
                return Err(format!(
                    "invalid credential env var {env_var:?}: an env var name must be non-empty and free of '=', whitespace, and control characters"
                ));
            }
            (None, Source::Document) => {
                return Err(format!(
                    "credential {:?} must name an envVar: outside a connector method the workload reads the value from a variable",
                    self.placeholder
                ));
            }
            _ => {}
        }
        if self.field.is_some() && source == Source::Document {
            return Err(format!(
                "credential {} must not declare field: there is no auth to draw from outside a connector method",
                self.owner()
            ));
        }
        if self.field.as_ref().is_some_and(|f| f.trim().is_empty()) {
            return Err(format!(
                "credential {} field must name an auth output",
                self.owner()
            ));
        }
        validate_placeholder(&self.placeholder, self.owner())?;
        for injection in &self.injections {
            injection.validate(self.owner())?;
        }
        Ok(())
    }
}

/// The two rules a placeholder obeys wherever it is written, because the boundary substitutes it by substring and the document carrying it may be published.
fn validate_placeholder(placeholder: &str, owner: &str) -> Result<(), String> {
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

/// The §5 rule that one source's plain `env` and its credentials do not claim one variable.
pub fn refuse_a_variable_a_credential_also_fills<'a>(
    env_keys: impl IntoIterator<Item = &'a String>,
    credentials: &[Credential],
) -> Result<(), String> {
    for key in env_keys {
        if credentials
            .iter()
            .any(|credential| credential.env_var.as_ref() == Some(key))
        {
            return Err(format!(
                "a source that sets {key} also fills it from a credential: one variable holds one value, so drop the env entry or the credential's envVar"
            ));
        }
    }
    Ok(())
}

/// Validate one source's whole `credentials` block: every entry on its own, plus the per-source rule that no two claim one variable and no two claim one marker.
pub fn validate_all(credentials: &[Credential], source: Source) -> Result<(), String> {
    let mut variables = std::collections::BTreeSet::new();
    let mut markers = std::collections::BTreeSet::new();
    for credential in credentials {
        credential.validate(source)?;
        if let Some(env_var) = &credential.env_var
            && !variables.insert(env_var)
        {
            return Err(format!("duplicate credential env var {env_var:?}"));
        }
        if !markers.insert(&credential.placeholder) {
            return Err(format!(
                "duplicate credential placeholder {:?}: the placeholder is what identifies an entry the merge has no envVar for",
                credential.placeholder
            ));
        }
    }
    Ok(())
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
const MIN_PLACEHOLDER_LEN: usize = 16;

/// The egress pattern that matches every host, which an injection may never name.
const CATCH_ALL_DOMAIN: &str = "*";

/// A placeholder must self-identify as fake so no real credential can leak into a document that gets published.
fn is_self_identifying(placeholder: &str) -> bool {
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
        assert_eq!(credential.env_var.as_deref(), Some("SOME_TOKEN"));
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
            env_var: Some("SOME_TOKEN".into()),
            placeholder: "some_LNSPLACEHOLDER0000".into(),
            field: None,
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
            env_var: Some(env_var.into()),
            placeholder: placeholder.into(),
            field: None,
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
        assert_eq!(credential.validate(Source::Document), Ok(()));
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
                .validate(Source::Document)
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
                .validate(Source::Document)
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
            credential("SOME_TOKEN", "lns-placeholder0", Vec::new()).validate(Source::Document),
            Ok(())
        );
    }

    #[test]
    fn a_placeholder_a_real_token_could_pass_for_is_refused() {
        let error = credential("SOME_TOKEN", "sk-live-0123456789", Vec::new())
            .validate(Source::Document)
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
        .validate(Source::Document)
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
        .validate(Source::Document)
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
            .validate(Source::Document),
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
            .validate(Source::Document)
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
        .validate(Source::Document)
        .expect_err("the proxy has no header to set, so the injection does nothing");
        assert!(
            error.contains("api_key_header injection must name the header"),
            "got: {error}"
        );
    }

    #[test]
    fn an_api_key_header_naming_its_header_validates() {
        assert_eq!(
            credential(
                "SOME_TOKEN",
                "some_LNSPLACEHOLDER0000",
                vec![injection(
                    InjectionKind::ApiKeyHeader,
                    "api.example.test",
                    Some("x-api-key")
                )],
            )
            .validate(Source::Document),
            Ok(()),
            "the one kind that carries a header name has to accept one, or no api_key_header injection could ever load"
        );
    }

    #[test]
    fn a_document_credential_that_names_no_variable_is_refused() {
        let error = Credential {
            env_var: None,
            placeholder: "some_LNSPLACEHOLDER0000".into(),
            field: None,
            injections: Vec::new(),
        }
        .validate(Source::Document)
        .expect_err("outside a method the workload reads the value from a variable");
        assert!(error.contains("must name an envVar"), "got: {error}");
    }

    #[test]
    fn a_method_credential_may_name_no_variable() {
        assert_eq!(
            Credential {
                env_var: None,
                placeholder: "some_LNSPLACEHOLDER0000".into(),
                field: None,
                injections: Vec::new(),
            }
            .validate(Source::Method),
            Ok(()),
            "a fileset-delivered credential serves a client that never reads the environment"
        );
    }

    #[test]
    fn a_source_may_not_set_a_variable_its_own_credential_fills() {
        // §5: one variable holds one value, and nothing downstream decides between the plain value and the placeholder the workload is meant to read.
        let credentials = [Credential {
            env_var: Some("SOME_TOKEN".into()),
            placeholder: "some_LNSPLACEHOLDER0000000000".into(),
            field: None,
            injections: Vec::new(),
        }];
        let clashing = ["SOME_TOKEN".to_string()];
        let error = refuse_a_variable_a_credential_also_fills(&clashing, &credentials)
            .expect_err("one variable claimed twice");
        assert!(error.contains("SOME_TOKEN"), "got: {error}");

        let separate = ["SOME_REGION".to_string()];
        assert!(refuse_a_variable_a_credential_also_fills(&separate, &credentials).is_ok());
    }

    #[test]
    fn a_field_outside_a_method_is_refused() {
        let error = Credential {
            env_var: Some("SOME_TOKEN".into()),
            placeholder: "some_LNSPLACEHOLDER0000".into(),
            field: Some("token".into()),
            injections: Vec::new(),
        }
        .validate(Source::Document)
        .expect_err("there is no auth to draw from outside a connector method");
        assert!(error.contains("must not declare field"), "got: {error}");
    }

    #[test]
    fn a_field_that_names_no_auth_output_is_refused() {
        let error = Credential {
            env_var: Some("SOME_TOKEN".into()),
            placeholder: "some_LNSPLACEHOLDER0000".into(),
            field: Some("  ".into()),
            injections: Vec::new(),
        }
        .validate(Source::Method)
        .expect_err("a blank field draws from nothing");
        assert!(error.contains("must name an auth output"), "got: {error}");
    }

    #[test]
    fn two_credentials_in_one_source_sharing_a_marker_are_refused() {
        let error = validate_all(
            &[
                credential("A", "some_LNSPLACEHOLDER0000", Vec::new()),
                credential("B", "some_LNSPLACEHOLDER0000", Vec::new()),
            ],
            Source::Method,
        )
        .expect_err("the placeholder is what identifies an entry the merge has no envVar for");
        assert!(
            error.contains("duplicate credential placeholder"),
            "got: {error}"
        );
    }

    #[test]
    fn two_credentials_claiming_one_env_var_are_refused() {
        let error = validate_all(
            &[
                credential("SOME_TOKEN", "some_LNSPLACEHOLDER0000", Vec::new()),
                credential("SOME_TOKEN", "other_LNSPLACEHOLDER0000", Vec::new()),
            ],
            Source::Document,
        )
        .expect_err("nothing inside one document disambiguates two entries claiming one variable");
        assert!(
            error.contains("duplicate credential env var \"SOME_TOKEN\""),
            "got: {error}"
        );
    }

    #[test]
    fn a_list_of_distinct_credentials_validates_each_entry() {
        assert_eq!(
            validate_all(
                &[
                    credential("SOME_TOKEN", "some_LNSPLACEHOLDER0000", Vec::new()),
                    credential("OTHER_TOKEN", "other_LNSPLACEHOLDER0000", Vec::new()),
                ],
                Source::Document
            ),
            Ok(())
        );
        let error = validate_all(
            &[
                credential("SOME_TOKEN", "some_LNSPLACEHOLDER0000", Vec::new()),
                credential("OTHER_TOKEN", "a-real-looking-token", Vec::new()),
            ],
            Source::Document,
        )
        .expect_err("a per-entry rule holds for every entry, not just the first");
        assert!(error.contains("must self-identify as fake"), "got: {error}");
    }
}
