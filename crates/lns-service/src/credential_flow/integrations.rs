use std::collections::{HashMap, HashSet};

use lns_policy::integrations::{AuthKind, Integration};
use lns_policy::providers::ProviderDef;
use lns_policy::{Policy, RouteRule};

use crate::credential_flow::providers::DefProvider;

/// The wire providers and routes a run's applied integrations contribute.
#[derive(Default)]
pub struct AppliedIntegrations {
    pub providers: Vec<DefProvider>,
    pub routes: Vec<RouteRule>,
}

/// Catalog credential-integrations that aren't yet connected: seeded unarmed for detection, with their routes held ready to allow live on connect.
#[derive(Default)]
pub struct ConnectableIntegrations {
    pub providers: Vec<DefProvider>,
    pub routes: HashMap<String, Vec<RouteRule>>,
}

fn def_provider_from(
    integ: &Integration,
    cred: &lns_policy::integrations::CredentialAuth,
) -> DefProvider {
    DefProvider::new(ProviderDef {
        id: integ.id.clone(),
        env_var: cred.env_var.clone(),
        placeholder: cred.placeholder.clone(),
        injections: cred.injections.clone(),
    })
}

/// Resolves the policy's applied integration ids against the effective catalog, skipping any id already owned by a built-in or a declared custom provider so the greenfield path never double-handles a service.
pub fn resolve_applied_integrations(
    policy: &Policy,
    catalog: &[Integration],
) -> AppliedIntegrations {
    let already_owned: HashSet<&str> = lns_policy::providers::builtins()
        .iter()
        .map(|p| p.id.as_str())
        .chain(
            policy
                .credentials
                .custom_providers
                .iter()
                .map(|p| p.id.as_str()),
        )
        .collect();
    let applied: HashSet<&str> = policy.integrations.iter().map(String::as_str).collect();

    let mut out = AppliedIntegrations::default();
    for integ in catalog {
        let id = integ.id.as_str();
        if !applied.contains(id) || already_owned.contains(id) {
            continue;
        }
        out.routes
            .extend(integ.routes.iter().map(|r| r.to_route_rule()));
        if integ.auth_kind == AuthKind::Credential
            && let Some(cred) = &integ.credential
        {
            out.providers.push(def_provider_from(integ, cred));
        }
    }
    out
}

/// The catalog credential-integrations a run can offer to connect: every credential-kind entry not already owned by a built-in, a custom provider, or an applied integration.
pub fn resolve_connectable_integrations(
    policy: &Policy,
    catalog: &[Integration],
) -> ConnectableIntegrations {
    let owned: HashSet<&str> = lns_policy::providers::builtins()
        .iter()
        .map(|p| p.id.as_str())
        .chain(
            policy
                .credentials
                .custom_providers
                .iter()
                .map(|p| p.id.as_str()),
        )
        .chain(policy.integrations.iter().map(String::as_str))
        .collect();

    let mut out = ConnectableIntegrations::default();
    for integ in catalog {
        if owned.contains(integ.id.as_str()) || integ.auth_kind != AuthKind::Credential {
            continue;
        }
        if let Some(cred) = &integ.credential {
            out.providers.push(def_provider_from(integ, cred));
            out.routes.insert(
                integ.id.clone(),
                integ.routes.iter().map(|r| r.to_route_rule()).collect(),
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credential_flow::providers::Provider;
    use lns_policy::integrations::{CredentialAuth, IntegrationRoute};
    use lns_policy::providers::{InjectionDef, InjectionKind};

    fn cred_integration(id: &str, env_var: &str, domain: &str) -> Integration {
        Integration {
            id: id.into(),
            auth_kind: AuthKind::Credential,
            routes: vec![IntegrationRoute {
                match_pattern: domain.into(),
                transport: None,
                scheme: None,
                tls_terminate: false,
                rules: Vec::new(),
            }],
            credential: Some(CredentialAuth {
                env_var: env_var.into(),
                placeholder: format!("lns-{id}-placeholder"),
                injections: vec![InjectionDef {
                    kind: InjectionKind::BearerHeader,
                    domain: domain.into(),
                    header: None,
                }],
            }),
        }
    }

    fn policy_applying(ids: &[&str]) -> Policy {
        Policy {
            integrations: ids.iter().map(|s| s.to_string()).collect(),
            ..Policy::default()
        }
    }

    #[test]
    fn resolves_an_applied_credential_integration_into_a_provider_and_its_routes() {
        let catalog = vec![cred_integration("gitlab", "GITLAB_TOKEN", "gitlab.com")];
        let out = resolve_applied_integrations(&policy_applying(&["gitlab"]), &catalog);
        assert_eq!(out.providers.len(), 1);
        assert_eq!(out.providers[0].id(), "gitlab");
        assert_eq!(out.providers[0].env_var(), "GITLAB_TOKEN");
        assert_eq!(out.routes.len(), 1);
        assert_eq!(out.routes[0].match_pattern, "gitlab.com");
        assert_eq!(out.routes[0].verdict, lns_policy::Verdict::Allow);
    }

    #[test]
    fn skips_a_catalog_integration_that_is_not_applied() {
        let catalog = vec![cred_integration("gitlab", "GITLAB_TOKEN", "gitlab.com")];
        let out = resolve_applied_integrations(&policy_applying(&[]), &catalog);
        assert!(out.providers.is_empty());
        assert!(out.routes.is_empty());
    }

    #[test]
    fn skips_an_applied_id_that_collides_with_a_builtin() {
        // A catalog entry mis-id'd as a built-in must never be resolved — the built-in owns it.
        let catalog = vec![cred_integration("github", "GITHUB_TOKEN", "api.github.com")];
        let out = resolve_applied_integrations(&policy_applying(&["github"]), &catalog);
        assert!(out.providers.is_empty(), "built-in id must be skipped");
        assert!(out.routes.is_empty());
    }

    #[test]
    fn skips_an_applied_id_that_collides_with_a_declared_custom_provider() {
        let catalog = vec![cred_integration("acme", "ACME_API_KEY", "api.acme.corp")];
        let mut policy = policy_applying(&["acme"]);
        policy.credentials.custom_providers.push(ProviderDef {
            id: "acme".into(),
            env_var: "ACME_API_KEY".into(),
            placeholder: "acme_LNSPLACEHOLDER".into(),
            injections: Vec::new(),
        });
        let out = resolve_applied_integrations(&policy, &catalog);
        assert!(
            out.providers.is_empty(),
            "a declared custom provider owns the id; the integration path must defer"
        );
    }

    #[test]
    fn an_applied_oauth_integration_contributes_routes_but_no_provider() {
        let catalog = vec![Integration {
            id: "somesaas".into(),
            auth_kind: AuthKind::Oauth,
            routes: vec![IntegrationRoute {
                match_pattern: "api.somesaas.com".into(),
                transport: None,
                scheme: None,
                tls_terminate: false,
                rules: Vec::new(),
            }],
            credential: None,
        }];
        let out = resolve_applied_integrations(&policy_applying(&["somesaas"]), &catalog);
        assert!(
            out.providers.is_empty(),
            "oauth has no static credential to inject yet"
        );
        assert_eq!(
            out.routes.len(),
            1,
            "an integration's routes apply regardless of auth kind"
        );
    }

    #[test]
    fn resolves_only_the_applied_subset_of_a_multi_entry_catalog() {
        let catalog = vec![
            cred_integration("gitlab", "GITLAB_TOKEN", "gitlab.com"),
            cred_integration("huggingface", "HF_TOKEN", "huggingface.co"),
        ];
        let out = resolve_applied_integrations(&policy_applying(&["huggingface"]), &catalog);
        let ids: Vec<&str> = out.providers.iter().map(|p| p.id()).collect();
        assert_eq!(ids, ["huggingface"]);
    }

    #[test]
    fn connectable_includes_an_unconnected_catalog_credential_integration() {
        let catalog = vec![cred_integration("gitlab", "GITLAB_TOKEN", "gitlab.com")];
        let c = resolve_connectable_integrations(&policy_applying(&[]), &catalog);
        assert_eq!(c.providers.len(), 1);
        assert_eq!(c.providers[0].id(), "gitlab");
        assert_eq!(c.routes.get("gitlab").map(|r| r.len()), Some(1));
    }

    #[test]
    fn connectable_excludes_an_already_applied_integration() {
        let catalog = vec![cred_integration("gitlab", "GITLAB_TOKEN", "gitlab.com")];
        let c = resolve_connectable_integrations(&policy_applying(&["gitlab"]), &catalog);
        assert!(
            c.providers.is_empty(),
            "an applied integration is not connectable"
        );
        assert!(c.routes.is_empty());
    }

    #[test]
    fn connectable_excludes_builtin_and_custom_provider_ids() {
        let catalog = vec![
            cred_integration("github", "GITHUB_TOKEN", "api.github.com"),
            cred_integration("acme", "ACME_API_KEY", "api.acme.corp"),
        ];
        let mut policy = policy_applying(&[]);
        policy.credentials.custom_providers.push(ProviderDef {
            id: "acme".into(),
            env_var: "ACME_API_KEY".into(),
            placeholder: "acme_LNSPLACEHOLDER".into(),
            injections: Vec::new(),
        });
        let c = resolve_connectable_integrations(&policy, &catalog);
        assert!(
            c.providers.is_empty(),
            "built-in and custom-provider ids are already owned"
        );
    }

    #[test]
    fn connectable_excludes_an_oauth_integration() {
        let catalog = vec![Integration {
            id: "somesaas".into(),
            auth_kind: AuthKind::Oauth,
            routes: vec![IntegrationRoute {
                match_pattern: "api.somesaas.com".into(),
                transport: None,
                scheme: None,
                tls_terminate: false,
                rules: Vec::new(),
            }],
            credential: None,
        }];
        let c = resolve_connectable_integrations(&policy_applying(&[]), &catalog);
        assert!(
            c.providers.is_empty(),
            "oauth can't be connected via the credential flow yet"
        );
    }
}
