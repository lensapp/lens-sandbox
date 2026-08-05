use lns_ipc::CredentialBindDecision;
use lns_policy::connectors::{AuthKind, Connector};
use lns_policy::providers::ProviderDef;

use crate::credential_flow::providers::DefProvider;
use crate::credential_flow::session::{
    CredentialDecisionRequest, CredentialPendingPrompt, DenyScope,
};
use crate::credential_flow::store::CredentialEntry;

/// The connect-time value-decision card for a credential connector; an oauth id signs in instead, and a blockless entry has nothing to bind.
pub fn bind_prompt(integ: &Connector) -> Option<CredentialPendingPrompt> {
    let method = integ.default_method()?;
    if method.kind != AuthKind::Credential {
        return None;
    }
    let cred = method.credential.as_ref()?;
    Some(CredentialPendingPrompt {
        id: format!("bind-{}", integ.id),
        credential_id: integ.id.clone(),
        action: format!("bind a value for \"{}\" on this machine", integ.id),
        oauth_display_name: None,
        token_fallback: method.token_fallback.clone(),
        env_var: Some(cred.env_var.clone()),
        injection_domains: cred.injections.iter().map(|i| i.domain.clone()).collect(),
        is_project_defined: false,
        // A bind card exists to make the machine binding, so it never offers to reuse one.
        bound_value_available: false,
        // The bind card holds no request and speaks for the machine, so its deny is the standing one.
        deny_scope: DenyScope::Machine,
    })
}

/// The provider the bind card detects a host value through — the same wiring a run would seed.
pub fn bind_provider(integ: &Connector) -> Option<DefProvider> {
    let method = integ
        .default_method()
        .filter(|m| m.kind == AuthKind::Credential)?;
    Some(DefProvider::new(ProviderDef {
        id: integ.id.clone(),
        env_var: method.env_var().to_string(),
        placeholder: method.placeholder().to_string(),
        injections: method.injections().to_vec(),
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
        CredentialDecisionRequest::DenyAlways => {
            BindResolution::Persist(CredentialEntry::Deny, CredentialBindDecision::Denied)
        }
        // Only a DenyScope::Machine card can bind a standing refusal, so a run-scoped deny arriving here is a misrouted card rather than a decision to persist.
        CredentialDecisionRequest::Deny => {
            BindResolution::Failed("a deny scoped to one run cannot bind a standing refusal".into())
        }
        // A bind card never offers it (there is no binding to reuse yet), so reaching here means no value was decided.
        CredentialDecisionRequest::AllowBound => {
            BindResolution::Failed("no value was bound by the decision".into())
        }
        CredentialDecisionRequest::Dismiss => {
            BindResolution::Failed("the value card was dismissed before a decision was made".into())
        }
        CredentialDecisionRequest::Timeout => {
            BindResolution::Failed("the value decision timed out before it was made".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lns_policy::connectors::{CredentialAuth, SignInMethod};
    use lns_policy::providers::{InjectionDef, InjectionKind};

    fn credential_connector(id: &str, env: &str) -> Connector {
        Connector {
            id: id.into(),
            name: None,
            routes: Vec::new(),
            methods: vec![SignInMethod::credential(
                "token",
                CredentialAuth {
                    env_var: env.into(),
                    placeholder: format!("{id}-LNSPLACEHOLDER0000"),
                    injections: vec![InjectionDef {
                        kind: InjectionKind::BearerHeader,
                        domain: "api.example.test".into(),
                        header: None,
                    }],
                },
            )],
        }
    }

    fn oauth_connector(id: &str) -> Connector {
        Connector {
            id: id.into(),
            name: None,
            routes: Vec::new(),
            methods: vec![SignInMethod {
                id: "device".into(),
                name: None,
                kind: AuthKind::Oauth,
                credential: None,
                oauth: None,
                token_fallback: None,
            }],
        }
    }

    #[test]
    fn bind_prompt_discloses_the_injection_target_before_any_value_is_entered() {
        let prompt = bind_prompt(&credential_connector("some-provider", "SOME_TOKEN"))
            .expect("a credential connector is bindable");
        assert_eq!(prompt.credential_id, "some-provider");
        assert_eq!(prompt.env_var.as_deref(), Some("SOME_TOKEN"));
        assert_eq!(
            prompt.injection_domains,
            vec!["api.example.test".to_string()]
        );
        assert!(prompt.oauth_display_name.is_none());
    }

    #[test]
    fn bind_prompt_refuses_an_oauth_or_blockless_connector() {
        assert_eq!(bind_prompt(&oauth_connector("some-oauth")), None);
        let mut blockless = credential_connector("some-blockless", "SOME_TOKEN");
        blockless.methods[0].credential = None;
        assert_eq!(bind_prompt(&blockless), None);
    }

    #[test]
    fn bind_provider_wires_the_catalog_env_and_placeholder_for_host_detection() {
        use crate::credential_flow::providers::Provider;
        let provider = bind_provider(&credential_connector("some-provider", "SOME_TOKEN"))
            .expect("a credential connector detects through its provider");
        assert_eq!(provider.id(), "some-provider");
        assert_eq!(provider.env_var(), "SOME_TOKEN");
        assert!(bind_provider(&oauth_connector("some-oauth")).is_none());
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
            CredentialDecisionRequest::DenyAlways,
            CredentialDecisionRequest::Allow(CredentialEntry::Deny),
        ] {
            assert_eq!(
                resolve_bind_decision(request),
                BindResolution::Persist(CredentialEntry::Deny, CredentialBindDecision::Denied)
            );
        }
    }

    #[test]
    fn a_run_scoped_deny_cannot_bind_a_standing_refusal() {
        assert_eq!(
            resolve_bind_decision(CredentialDecisionRequest::Deny),
            BindResolution::Failed(
                "a deny scoped to one run cannot bind a standing refusal".into()
            ),
            "only a card that speaks for the machine may write a machine-wide deny; a run's deny reaching here is a misroute, not a decision to persist"
        );
    }

    #[test]
    fn the_bind_card_is_the_only_one_entitled_to_a_standing_deny() {
        let prompt = bind_prompt(&credential_connector("some-provider", "SOME_TOKEN"))
            .expect("a credential connector is bindable");
        assert_eq!(
            prompt.deny_scope,
            DenyScope::Machine,
            "the bind card holds no request, so its deny is the standing one the run cards must never be able to ask for"
        );
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
    fn a_dismissed_card_fails_the_bind_and_says_so() {
        // The developer must not be told they denied when they only closed the card.
        assert_eq!(
            resolve_bind_decision(CredentialDecisionRequest::Dismiss),
            BindResolution::Failed(
                "the value card was dismissed before a decision was made".into()
            )
        );
    }

    #[test]
    fn a_timeout_fails_the_bind_instead_of_persisting_anything() {
        assert_eq!(
            resolve_bind_decision(CredentialDecisionRequest::Timeout),
            BindResolution::Failed("the value decision timed out before it was made".into())
        );
    }

    #[test]
    fn reusing_a_binding_is_not_a_bind_so_it_persists_nothing() {
        assert_eq!(
            resolve_bind_decision(CredentialDecisionRequest::AllowBound),
            BindResolution::Failed("no value was bound by the decision".into()),
            "the bind card exists to create the machine binding, so a grant of an existing one leaves it with nothing to report"
        );
    }

    #[test]
    fn a_bind_card_never_offers_to_reuse_an_existing_binding() {
        let prompt = bind_prompt(&credential_connector("acme", "ACME_API_KEY")).expect("a prompt");
        assert!(
            !prompt.bound_value_available,
            "the bind card is where the machine binding is made; offering to reuse one would be circular"
        );
    }
}
