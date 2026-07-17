use lns_ipc::CredentialBindDecision;
use lns_policy::integrations::{AuthKind, Integration};
use lns_policy::providers::ProviderDef;

use crate::credential_flow::providers::DefProvider;
use crate::credential_flow::session::{CredentialDecisionRequest, CredentialPendingPrompt};
use crate::credential_flow::store::CredentialEntry;

/// The connect-time value-decision card for a credential integration; an oauth id signs in instead, and a blockless entry has nothing to bind.
pub fn bind_prompt(integ: &Integration) -> Option<CredentialPendingPrompt> {
    if integ.auth_kind != AuthKind::Credential {
        return None;
    }
    let cred = integ.credential.as_ref()?;
    Some(CredentialPendingPrompt {
        id: format!("bind-{}", integ.id),
        credential_id: integ.id.clone(),
        action: format!("bind a value for \"{}\" on this machine", integ.id),
        oauth_display_name: None,
        token_fallback: integ.token_fallback.clone(),
        env_var: Some(cred.env_var.clone()),
        injection_domains: cred.injections.iter().map(|i| i.domain.clone()).collect(),
        is_project_defined: false,
    })
}

/// The provider the bind card detects a host value through — the same wiring a run would seed.
pub fn bind_provider(integ: &Integration) -> Option<DefProvider> {
    let cred = integ.credential.as_ref()?;
    Some(DefProvider::new(ProviderDef {
        id: integ.id.clone(),
        env_var: cred.env_var.clone(),
        placeholder: cred.placeholder.clone(),
        injections: cred.injections.clone(),
    }))
}

/// Every completed decision — a value, host-detect, or an explicit deny — persists to the per-machine store; only a timeout fails the bind.
#[derive(Debug, PartialEq, Eq)]
pub enum BindResolution {
    Persist(CredentialEntry, CredentialBindDecision),
    Failed(String),
}

pub fn resolve_bind_decision(request: CredentialDecisionRequest) -> BindResolution {
    match request {
        CredentialDecisionRequest::Allow(entry) => {
            let decision = match &entry {
                CredentialEntry::HostDetect => CredentialBindDecision::HostDetect,
                CredentialEntry::Deny => CredentialBindDecision::Denied,
                CredentialEntry::Stored { .. } | CredentialEntry::Oauth { .. } => {
                    CredentialBindDecision::Stored
                }
            };
            BindResolution::Persist(entry, decision)
        }
        CredentialDecisionRequest::Deny => {
            BindResolution::Persist(CredentialEntry::Deny, CredentialBindDecision::Denied)
        }
        CredentialDecisionRequest::Timeout => {
            BindResolution::Failed("the value decision timed out before it was made".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lns_policy::integrations::CredentialAuth;
    use lns_policy::providers::{InjectionDef, InjectionKind};

    fn credential_integration(id: &str, env: &str) -> Integration {
        Integration {
            id: id.into(),
            name: None,
            auth_kind: AuthKind::Credential,
            routes: Vec::new(),
            credential: Some(CredentialAuth {
                env_var: env.into(),
                placeholder: format!("{id}-LNSPLACEHOLDER0000"),
                injections: vec![InjectionDef {
                    kind: InjectionKind::BearerHeader,
                    domain: "api.example.test".into(),
                    header: None,
                }],
            }),
            oauth: None,
            token_fallback: None,
        }
    }

    fn oauth_integration(id: &str) -> Integration {
        Integration {
            id: id.into(),
            name: None,
            auth_kind: AuthKind::Oauth,
            routes: Vec::new(),
            credential: None,
            oauth: None,
            token_fallback: None,
        }
    }

    #[test]
    fn bind_prompt_discloses_the_injection_target_before_any_value_is_entered() {
        let prompt = bind_prompt(&credential_integration("some-provider", "SOME_TOKEN"))
            .expect("a credential integration is bindable");
        assert_eq!(prompt.credential_id, "some-provider");
        assert_eq!(prompt.env_var.as_deref(), Some("SOME_TOKEN"));
        assert_eq!(
            prompt.injection_domains,
            vec!["api.example.test".to_string()]
        );
        assert!(prompt.oauth_display_name.is_none());
    }

    #[test]
    fn bind_prompt_refuses_an_oauth_or_blockless_integration() {
        assert_eq!(bind_prompt(&oauth_integration("some-oauth")), None);
        let blockless = Integration {
            credential: None,
            ..credential_integration("some-blockless", "SOME_TOKEN")
        };
        assert_eq!(bind_prompt(&blockless), None);
    }

    #[test]
    fn bind_provider_wires_the_catalog_env_and_placeholder_for_host_detection() {
        use crate::credential_flow::providers::Provider;
        let provider = bind_provider(&credential_integration("some-provider", "SOME_TOKEN"))
            .expect("a credential integration detects through its provider");
        assert_eq!(provider.id(), "some-provider");
        assert_eq!(provider.env_var(), "SOME_TOKEN");
        assert!(bind_provider(&oauth_integration("some-oauth")).is_none());
    }

    #[test]
    fn an_allowed_value_persists_as_stored() {
        let resolution =
            resolve_bind_decision(CredentialDecisionRequest::Allow(CredentialEntry::Stored {
                value: "some-secret".into(),
            }));
        assert_eq!(
            resolution,
            BindResolution::Persist(
                CredentialEntry::Stored {
                    value: "some-secret".into()
                },
                CredentialBindDecision::Stored
            )
        );
    }

    #[test]
    fn a_host_detect_choice_persists_as_host_detect() {
        let resolution = resolve_bind_decision(CredentialDecisionRequest::Allow(
            CredentialEntry::HostDetect,
        ));
        assert_eq!(
            resolution,
            BindResolution::Persist(
                CredentialEntry::HostDetect,
                CredentialBindDecision::HostDetect
            )
        );
    }

    #[test]
    fn a_deny_persists_the_deny_so_the_launch_gate_reports_it_distinctly() {
        for request in [
            CredentialDecisionRequest::Deny,
            CredentialDecisionRequest::Allow(CredentialEntry::Deny),
        ] {
            assert_eq!(
                resolve_bind_decision(request),
                BindResolution::Persist(CredentialEntry::Deny, CredentialBindDecision::Denied)
            );
        }
    }

    #[test]
    fn an_oauth_entry_reports_as_a_stored_value() {
        let entry = CredentialEntry::Oauth {
            access_token: "some-access".into(),
            refresh_token: "some-refresh".into(),
            expires_at: 9999,
            scopes: vec![],
            account: None,
        };
        assert_eq!(
            resolve_bind_decision(CredentialDecisionRequest::Allow(entry.clone())),
            BindResolution::Persist(entry, CredentialBindDecision::Stored)
        );
    }

    #[test]
    fn a_timeout_fails_the_bind_instead_of_persisting_anything() {
        assert_eq!(
            resolve_bind_decision(CredentialDecisionRequest::Timeout),
            BindResolution::Failed("the value decision timed out before it was made".into())
        );
    }
}
