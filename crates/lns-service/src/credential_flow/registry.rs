use crate::approval_flow::protocol::Credential;
use crate::credential_flow::providers::{DefProvider, Provider};
use crate::credential_flow::store::{CredentialEntry, CredentialStateFile};

/// Resolves a host value from the run's providers so a `host-detect` rule arms from its declared env var.
pub fn detect_for_with(id: &str, custom: &[DefProvider]) -> Option<String> {
    custom
        .iter()
        .find(|p| p.id() == id)
        .and_then(|p| p.detector().detect())
}

/// Every provider becomes one `Credential` even when unarmed so the MITM can detect every known placeholder in outbound requests; host detection resolves against the run's providers.
pub fn expand_credentials_for_wire_with_custom(
    state: &CredentialStateFile,
    custom: &[DefProvider],
) -> Vec<Credential> {
    expand_credentials_with_custom(state, custom, &|id| detect_for_with(id, custom))
}

/// Takes the host-detection source injected so Layer-2 tests can drive resolution from an in-memory map without touching process env.
pub fn expand_credentials_with_custom(
    state: &CredentialStateFile,
    custom: &[DefProvider],
    detect_host: &dyn Fn(&str) -> Option<String>,
) -> Vec<Credential> {
    custom
        .iter()
        .map(|p| p as &dyn Provider)
        .map(|p| Credential {
            id: p.id().to_string(),
            env_var: Some(p.env_var().to_string()),
            placeholder: Some(p.placeholder().to_string()),
            injections: match resolve(p, state, detect_host) {
                Some(v) => p.injections(&v),
                None => p.unarmed_injections(),
            },
        })
        .collect()
}

fn resolve(
    p: &dyn Provider,
    state: &CredentialStateFile,
    detect_host: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    match state.get(p.id())? {
        CredentialEntry::Stored { value } => Some(value.clone()),
        CredentialEntry::HostDetect => detect_host(p.id()),
        CredentialEntry::Oauth { access_token, .. } => Some(access_token.clone()),
        CredentialEntry::Deny => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::approval_flow::protocol::CredentialInjection;
    use crate::test_env::EnvVarGuard;
    use std::collections::HashSet;

    #[test]
    fn expand_credentials_for_wire_emits_every_provider_with_placeholder_and_env_var() {
        let custom = vec![
            DefProvider::new(acme_def()),
            DefProvider::new(token_and_basic_def()),
        ];
        let creds = expand_credentials_for_wire_with_custom(&CredentialStateFile::new(), &custom);
        let ids: HashSet<_> = creds.iter().map(|c| c.id.as_str()).collect();
        for id in ["acme", "some-provider"] {
            assert!(ids.contains(id), "missing {id} in expanded credentials");
        }
        for c in &creds {
            assert!(c.env_var.is_some(), "{} missing env_var", c.id);
            assert!(c.placeholder.is_some(), "{} missing placeholder", c.id);
            assert!(
                !c.injections.is_empty(),
                "{} should declare at least one unarmed injection without a rule",
                c.id
            );
        }
    }

    #[test]
    fn expand_for_wire_declares_each_domain_of_a_multi_injection_provider_unarmed() {
        let custom = vec![DefProvider::new(token_and_basic_def())];
        let creds = expand_credentials_for_wire_with_custom(&CredentialStateFile::new(), &custom);
        let p = creds.iter().find(|c| c.id == "some-provider").unwrap();
        assert_eq!(p.injections.len(), 2);
        assert!(p.injections.iter().all(|i| matches!(
            i,
            CredentialInjection::Header { value, .. } if value.is_empty()
        )));
        assert!(p.injections.iter().any(|i| matches!(
            i, CredentialInjection::Header { domain, .. } if domain == "api.some-provider.example"
        )));
        assert!(p.injections.iter().any(|i| matches!(
            i, CredentialInjection::Header { domain, .. } if domain == "some-provider.example"
        )));
    }

    #[test]
    fn expand_arms_a_stored_multi_injection_provider_as_token_and_basic() {
        let mut state = CredentialStateFile::new();
        state.insert(
            "some-provider".into(),
            CredentialEntry::Stored {
                value: "some-secret".into(),
            },
        );
        let custom = vec![DefProvider::new(token_and_basic_def())];
        let creds = expand_credentials_for_wire_with_custom(&state, &custom);
        let p = creds.iter().find(|c| c.id == "some-provider").unwrap();
        assert_eq!(p.injections.len(), 2);
        let basic = format!(
            "Basic {}",
            crate::base64::encode(b"x-access-token:some-secret")
        );
        assert!(p.injections.iter().any(|i| matches!(
            i,
            CredentialInjection::Header { domain, value, .. }
                if domain == "api.some-provider.example" && value == "token some-secret"
        )));
        assert!(p.injections.iter().any(|i| matches!(
            i,
            CredentialInjection::Header { domain, value, .. }
                if domain == "some-provider.example" && *value == basic
        )));
    }

    #[test]
    #[serial_test::serial(env)]
    fn expand_credentials_for_wire_leaves_host_detect_unarmed_when_source_empty() {
        let _g = EnvVarGuard::unset("SOME_TOKEN");
        let mut state = CredentialStateFile::new();
        state.insert("some-provider".into(), CredentialEntry::HostDetect);
        let custom = vec![DefProvider::new(token_and_basic_def())];
        let creds = expand_credentials_for_wire_with_custom(&state, &custom);
        let p = creds.iter().find(|c| c.id == "some-provider").unwrap();
        assert_eq!(
            p.injections.len(),
            2,
            "a multi-injection provider declares every domain it covers"
        );
        assert!(
            p.injections.iter().all(|i| matches!(
                i,
                CredentialInjection::Header { value, .. } if value.is_empty()
            )),
            "every injection stays unarmed when the host source is empty"
        );
        assert!(p.placeholder.is_some());
    }

    #[test]
    #[serial_test::serial(env)]
    fn expand_credentials_for_wire_leaves_deny_unarmed_even_when_host_has_value() {
        let _g = EnvVarGuard::set("ACME_API_KEY", "acme_real");
        let mut state = CredentialStateFile::new();
        state.insert("acme".into(), CredentialEntry::Deny);
        let custom = vec![DefProvider::new(acme_def())];
        let creds = expand_credentials_for_wire_with_custom(&state, &custom);
        let acme = creds.iter().find(|c| c.id == "acme").unwrap();
        assert_eq!(acme.injections.len(), 1);
        assert!(
            matches!(
                &acme.injections[0],
                CredentialInjection::Header { domain, value, .. }
                    if domain == "api.acme.corp" && value.is_empty()
            ),
            "Deny declares its domain unarmed (empty value) so the request is intercepted, never armed with the host value"
        );
    }

    #[test]
    fn expand_credentials_with_custom_resolves_host_detect_via_injected_source_not_env() {
        let custom = vec![DefProvider::new(acme_def())];
        let mut state = CredentialStateFile::new();
        state.insert("acme".into(), CredentialEntry::HostDetect);
        let creds = expand_credentials_with_custom(&state, &custom, &|id| {
            (id == "acme").then(|| "acme-injected".to_string())
        });
        let acme = creds.iter().find(|c| c.id == "acme").unwrap();
        assert!(acme.injections.iter().any(|i| matches!(
            i,
            CredentialInjection::Header { value, .. }
                if value == "Bearer acme-injected"
        )));
    }

    fn acme_def() -> lns_policy::providers::ProviderDef {
        use lns_policy::providers::{InjectionDef, InjectionKind, ProviderDef};
        ProviderDef {
            id: "acme".into(),
            env_var: "ACME_API_KEY".into(),
            placeholder: "acme_LNSPLACEHOLDER0000000000000000000000".into(),
            injections: vec![InjectionDef {
                kind: InjectionKind::BearerHeader,
                domain: "api.acme.corp".into(),
                header: None,
            }],
        }
    }

    /// A fixture exercising the `token_header` + `basic_x_access_token` kinds (no built-in carries them anymore).
    fn token_and_basic_def() -> lns_policy::providers::ProviderDef {
        use lns_policy::providers::{InjectionDef, InjectionKind, ProviderDef};
        ProviderDef {
            id: "some-provider".into(),
            env_var: "SOME_TOKEN".into(),
            placeholder: "some-placeholder-0000000000000000000000".into(),
            injections: vec![
                InjectionDef {
                    kind: InjectionKind::TokenHeader,
                    domain: "api.some-provider.example".into(),
                    header: None,
                },
                InjectionDef {
                    kind: InjectionKind::BasicXAccessToken,
                    domain: "some-provider.example".into(),
                    header: None,
                },
            ],
        }
    }

    #[test]
    fn expand_for_wire_with_custom_emits_an_unarmed_custom_provider() {
        let custom = vec![DefProvider::new(acme_def())];
        let creds = expand_credentials_for_wire_with_custom(&CredentialStateFile::new(), &custom);
        let acme = creds
            .iter()
            .find(|c| c.id == "acme")
            .expect("custom acme present");
        assert_eq!(acme.env_var.as_deref(), Some("ACME_API_KEY"));
        assert!(matches!(
            &acme.injections[0],
            CredentialInjection::Header { domain, value, .. }
                if domain == "api.acme.corp" && value.is_empty()
        ));
    }

    #[test]
    fn expand_arms_an_oauth_credential_with_its_access_token() {
        let custom = vec![DefProvider::new(token_and_basic_def())];
        let mut state = CredentialStateFile::new();
        state.insert(
            "some-provider".into(),
            CredentialEntry::Oauth {
                access_token: "some-access".into(),
                refresh_token: "some-refresh".into(),
                expires_at: 0,
            },
        );
        let creds = expand_credentials_with_custom(&state, &custom, &|_| None);
        let p = creds.iter().find(|c| c.id == "some-provider").unwrap();
        assert!(
            p.injections.iter().any(|i| matches!(
                i,
                CredentialInjection::Header { value, .. } if value.contains("some-access")
            )),
            "an oauth credential arms with its access token, not its refresh token: {:?}",
            p.injections
        );
    }

    #[test]
    fn expand_with_custom_arms_a_stored_custom_provider() {
        let custom = vec![DefProvider::new(acme_def())];
        let mut state = CredentialStateFile::new();
        state.insert(
            "acme".into(),
            CredentialEntry::Stored {
                value: "acme_real".into(),
            },
        );
        let creds = expand_credentials_with_custom(&state, &custom, &|_| None);
        let acme = creds.iter().find(|c| c.id == "acme").unwrap();
        assert!(matches!(
            &acme.injections[0],
            CredentialInjection::Header { value, .. } if value == "Bearer acme_real"
        ));
    }

    #[test]
    fn expand_with_custom_leaves_a_denied_custom_provider_unarmed() {
        let custom = vec![DefProvider::new(acme_def())];
        let mut state = CredentialStateFile::new();
        state.insert("acme".into(), CredentialEntry::Deny);
        let creds = expand_credentials_with_custom(&state, &custom, &|_| Some("leaked".into()));
        let acme = creds.iter().find(|c| c.id == "acme").unwrap();
        assert!(matches!(
            &acme.injections[0],
            CredentialInjection::Header { value, .. } if value.is_empty()
        ));
    }

    #[test]
    #[serial_test::serial(env)]
    fn detect_for_with_resolves_a_custom_provider_via_its_own_detector() {
        let _g = EnvVarGuard::set("ACME_API_KEY", "acme_real");
        let custom = vec![DefProvider::new(acme_def())];
        assert_eq!(
            detect_for_with("acme", &custom).as_deref(),
            Some("acme_real")
        );
    }

    #[test]
    #[serial_test::serial(env)]
    fn detect_for_with_returns_none_for_an_unknown_id() {
        let custom = vec![DefProvider::new(acme_def())];
        assert_eq!(detect_for_with("not-a-provider", &custom), None);
    }

    #[test]
    #[serial_test::serial(env)]
    fn expand_for_wire_with_custom_arms_host_detect_custom_from_env() {
        let _g = EnvVarGuard::set("ACME_API_KEY", "acme_from_env");
        let custom = vec![DefProvider::new(acme_def())];
        let mut state = CredentialStateFile::new();
        state.insert("acme".into(), CredentialEntry::HostDetect);
        let creds = expand_credentials_for_wire_with_custom(&state, &custom);
        let acme = creds.iter().find(|c| c.id == "acme").unwrap();
        assert!(matches!(
            &acme.injections[0],
            CredentialInjection::Header { value, .. } if value == "Bearer acme_from_env"
        ));
    }

    #[test]
    fn a_multi_domain_custom_provider_expands_to_one_injection_per_domain() {
        use lns_policy::providers::{InjectionDef, InjectionKind};
        let mut def = acme_def();
        def.injections = vec![
            InjectionDef {
                kind: InjectionKind::BearerHeader,
                domain: "api.acme.corp".into(),
                header: None,
            },
            InjectionDef {
                kind: InjectionKind::BearerHeader,
                domain: "api-eu.acme.corp".into(),
                header: None,
            },
        ];
        let custom = vec![DefProvider::new(def)];
        let creds = expand_credentials_for_wire_with_custom(&CredentialStateFile::new(), &custom);
        let acme = creds.iter().find(|c| c.id == "acme").unwrap();
        assert_eq!(acme.injections.len(), 2);
    }
}
