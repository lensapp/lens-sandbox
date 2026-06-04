use crate::approval_flow::protocol::Credential;
use crate::credential_flow::providers::{self, DefProvider, Provider};
use crate::credential_flow::store::{CredentialEntry, CredentialStateFile};

/// Returns `None` for an unknown id so a stale rule pointing at a removed provider becomes inert.
pub fn detect_for(id: &str) -> Option<String> {
    providers::by_id(id)?.detector().detect()
}

/// Resolves a host value against built-ins first, then the run's custom providers, so a custom `host-detect` rule arms from its declared env var.
pub fn detect_for_with(id: &str, custom: &[DefProvider]) -> Option<String> {
    if let Some(p) = providers::by_id(id) {
        return p.detector().detect();
    }
    custom
        .iter()
        .find(|p| p.id() == id)
        .and_then(|p| p.detector().detect())
}

/// Every provider becomes one `Credential` even when unarmed so the MITM can detect every known placeholder in outbound requests.
pub fn expand_credentials_for_wire(state: &CredentialStateFile) -> Vec<Credential> {
    expand_credentials_for_wire_with_custom(state, &[])
}

/// The built-in set unioned with the run's custom providers; host detection resolves against both.
pub fn expand_credentials_for_wire_with_custom(
    state: &CredentialStateFile,
    custom: &[DefProvider],
) -> Vec<Credential> {
    expand_credentials_with_custom(state, custom, &|id| detect_for_with(id, custom))
}

pub fn expand_credentials_with(
    state: &CredentialStateFile,
    detect_host: &dyn Fn(&str) -> Option<String>,
) -> Vec<Credential> {
    expand_credentials_with_custom(state, &[], detect_host)
}

/// Takes the host-detection source injected so Layer-2 tests can drive resolution from an in-memory map without touching process env.
pub fn expand_credentials_with_custom(
    state: &CredentialStateFile,
    custom: &[DefProvider],
    detect_host: &dyn Fn(&str) -> Option<String>,
) -> Vec<Credential> {
    providers::ALL
        .iter()
        .map(|p| *p as &dyn Provider)
        .chain(custom.iter().map(|p| p as &dyn Provider))
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
    fn registry_contains_v1_provider_set() {
        let ids: HashSet<_> = providers::ALL.iter().map(|p| p.id()).collect();
        for expected in ["github", "openai", "anthropic", "linear"] {
            assert!(ids.contains(expected), "missing v1 provider {expected}");
        }
    }

    #[test]
    fn provider_ids_are_unique() {
        let mut seen = HashSet::new();
        for p in providers::ALL.iter() {
            let id = p.id();
            assert!(seen.insert(id), "duplicate provider id: {id}");
        }
    }

    #[test]
    fn env_var_names_are_unique() {
        let mut seen = HashSet::new();
        for p in providers::ALL.iter() {
            let env = p.env_var();
            assert!(
                seen.insert(env),
                "duplicate env_var across providers: {env}"
            );
        }
    }

    #[test]
    fn no_placeholder_carries_the_string_real_or_secret() {
        for p in providers::ALL.iter() {
            let id = p.id();
            let placeholder = p.placeholder();
            let lower = placeholder.to_lowercase();
            assert!(
                lower.contains("placeholder") || lower.contains("lns"),
                "{id} placeholder doesn't self-identify: {placeholder}",
            );
        }
    }

    #[test]
    #[serial_test::serial(env)]
    fn detect_for_returns_value_when_provider_env_var_is_set() {
        let _g = EnvVarGuard::set("GITHUB_TOKEN", "ghp_real");
        assert_eq!(detect_for("github").as_deref(), Some("ghp_real"));
    }

    #[test]
    #[serial_test::serial(env)]
    fn detect_for_returns_none_when_provider_env_var_is_unset() {
        let _g = EnvVarGuard::unset("OPENAI_API_KEY");
        assert_eq!(detect_for("openai"), None);
    }

    #[test]
    #[serial_test::serial(env)]
    fn detect_for_unknown_provider_returns_none_without_panicking() {
        assert_eq!(detect_for("not-a-real-provider"), None);
    }

    #[test]
    #[serial_test::serial(env)]
    fn expand_credentials_for_wire_emits_every_provider_with_placeholder_and_env_var() {
        let _g1 = EnvVarGuard::unset("GITHUB_TOKEN");
        let _g2 = EnvVarGuard::unset("OPENAI_API_KEY");
        let _g3 = EnvVarGuard::unset("ANTHROPIC_API_KEY");
        let _g4 = EnvVarGuard::unset("LINEAR_API_KEY");
        let state = CredentialStateFile::new();
        let creds = expand_credentials_for_wire(&state);
        let ids: HashSet<_> = creds.iter().map(|c| c.id.as_str()).collect();
        for id in ["github", "openai", "anthropic", "linear"] {
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
    #[serial_test::serial(env)]
    fn expand_credentials_for_wire_unarmed_provider_declares_domain_with_empty_value() {
        let _g = EnvVarGuard::unset("GITHUB_TOKEN");
        let state = CredentialStateFile::new();
        let creds = expand_credentials_for_wire(&state);
        let github = creds.iter().find(|c| c.id == "github").unwrap();
        assert_eq!(github.injections.len(), 2);
        assert!(github.injections.iter().all(|i| matches!(
            i,
            CredentialInjection::Header { value, .. } if value.is_empty()
        )));
        assert!(github.injections.iter().any(|i| matches!(
            i, CredentialInjection::Header { domain, .. } if domain == "api.github.com"
        )));
        assert!(github.injections.iter().any(|i| matches!(
            i, CredentialInjection::Header { domain, .. } if domain == "github.com"
        )));
    }

    #[test]
    #[serial_test::serial(env)]
    fn expand_credentials_for_wire_arms_stored_with_provider_specific_injection() {
        let _g = EnvVarGuard::unset("GITHUB_TOKEN");
        let mut state = CredentialStateFile::new();
        state.insert(
            "github".into(),
            CredentialEntry::Stored {
                value: "ghp_real".into(),
            },
        );
        let creds = expand_credentials_for_wire(&state);
        let github = creds.iter().find(|c| c.id == "github").unwrap();
        assert_eq!(github.injections.len(), 2);
        let basic = format!(
            "Basic {}",
            crate::base64::encode(b"x-access-token:ghp_real")
        );
        assert!(github.injections.iter().any(|i| matches!(
            i,
            CredentialInjection::Header { domain, value, .. }
                if domain == "api.github.com" && value == "token ghp_real"
        )));
        assert!(github.injections.iter().any(|i| matches!(
            i,
            CredentialInjection::Header { domain, value, .. }
                if domain == "github.com" && *value == basic
        )));
    }

    #[test]
    #[serial_test::serial(env)]
    fn expand_credentials_for_wire_resolves_host_detect_via_provider_detector() {
        let _g = EnvVarGuard::set("OPENAI_API_KEY", "sk-from-env");
        let mut state = CredentialStateFile::new();
        state.insert("openai".into(), CredentialEntry::HostDetect);
        let creds = expand_credentials_for_wire(&state);
        let openai = creds.iter().find(|c| c.id == "openai").unwrap();
        assert_eq!(openai.injections.len(), 1);
        assert!(matches!(
            &openai.injections[0],
            CredentialInjection::Header { value, .. }
                if value == "Bearer sk-from-env"
        ));
    }

    #[test]
    #[serial_test::serial(env)]
    fn expand_credentials_for_wire_leaves_host_detect_unarmed_when_source_empty() {
        let _g = EnvVarGuard::unset("ANTHROPIC_API_KEY");
        let mut state = CredentialStateFile::new();
        state.insert("anthropic".into(), CredentialEntry::HostDetect);
        let creds = expand_credentials_for_wire(&state);
        let anthropic = creds.iter().find(|c| c.id == "anthropic").unwrap();
        assert_eq!(
            anthropic.injections.len(),
            2,
            "anthropic covers both x-api-key and Authorization: Bearer"
        );
        assert!(
            anthropic.injections.iter().all(|i| matches!(
                i,
                CredentialInjection::Header { domain, value, .. }
                    if domain == "api.anthropic.com" && value.is_empty()
            )),
            "every anthropic injection stays unarmed when the host source is empty"
        );
        assert!(anthropic.placeholder.is_some());
    }

    #[test]
    #[serial_test::serial(env)]
    fn expand_credentials_for_wire_leaves_deny_unarmed_even_when_host_has_value() {
        let _g = EnvVarGuard::set("LINEAR_API_KEY", "lin_real");
        let mut state = CredentialStateFile::new();
        state.insert("linear".into(), CredentialEntry::Deny);
        let creds = expand_credentials_for_wire(&state);
        let linear = creds.iter().find(|c| c.id == "linear").unwrap();
        assert_eq!(linear.injections.len(), 1);
        assert!(
            matches!(
                &linear.injections[0],
                CredentialInjection::Header { domain, value, .. }
                    if domain == "api.linear.app" && value.is_empty()
            ),
            "Deny declares its domain unarmed (empty value) so the request is intercepted, never armed with the host value"
        );
    }

    #[test]
    #[serial_test::serial(env)]
    fn detect_for_uses_each_providers_own_env_var_binding() {
        let _g_linear = EnvVarGuard::set("LINEAR_API_KEY", "lin_real");
        let _g_github = EnvVarGuard::unset("GITHUB_TOKEN");
        assert_eq!(detect_for("linear").as_deref(), Some("lin_real"));
        assert_eq!(detect_for("github"), None);
    }

    #[test]
    fn expand_credentials_with_resolves_host_detect_via_injected_source_not_env() {
        let mut state = CredentialStateFile::new();
        state.insert("github".into(), CredentialEntry::HostDetect);
        let creds = expand_credentials_with(&state, &|id| {
            (id == "github").then(|| "ghp_injected".to_string())
        });
        let github = creds.iter().find(|c| c.id == "github").unwrap();
        assert_eq!(github.injections.len(), 2);
        assert!(github.injections.iter().any(|i| matches!(
            i,
            CredentialInjection::Header { domain, value, .. }
                if domain == "api.github.com" && value == "token ghp_injected"
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

    #[test]
    fn expand_for_wire_with_custom_emits_an_unarmed_custom_provider_alongside_builtins() {
        let custom = vec![DefProvider::new(acme_def())];
        let creds = expand_credentials_for_wire_with_custom(&CredentialStateFile::new(), &custom);
        assert!(
            creds.iter().any(|c| c.id == "github"),
            "built-ins must still be present"
        );
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
    fn detect_for_with_resolves_a_builtin_through_the_registry() {
        let _g = EnvVarGuard::set("GITHUB_TOKEN", "ghp_real");
        assert_eq!(detect_for_with("github", &[]).as_deref(), Some("ghp_real"));
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
