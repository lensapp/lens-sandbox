use std::collections::{HashMap, HashSet};

use lns_policy::integrations::{AuthKind, Integration, OauthAuth, OauthFlow};
use lns_policy::providers::ProviderDef;
use lns_policy::{Policy, RouteRule};

use crate::credential_flow::providers::DefProvider;

/// The wire providers, routes, and (for oauth entries) device-flow / pkce configs a run's applied integrations contribute.
#[derive(Default)]
pub struct AppliedIntegrations {
    pub providers: Vec<DefProvider>,
    pub routes: Vec<RouteRule>,
    pub oauth_configs: HashMap<String, OauthAuth>,
    pub pkce_configs: HashMap<String, OauthAuth>,
}

/// Catalog integrations that aren't yet connected: seeded unarmed for detection, with their routes held ready to allow live on connect, and device-flow / pkce configs for a sign-in dance on connect.
#[derive(Default)]
pub struct ConnectableIntegrations {
    pub providers: Vec<DefProvider>,
    pub routes: HashMap<String, Vec<RouteRule>>,
    pub oauth_configs: HashMap<String, OauthAuth>,
    pub pkce_configs: HashMap<String, OauthAuth>,
}

/// The env/placeholder/injection wiring an integration seeds, taken from whichever block its authKind carries.
fn wire_provider_def(integ: &Integration) -> Option<ProviderDef> {
    let (env_var, placeholder, injections) = match integ.auth_kind {
        AuthKind::Credential => {
            let c = integ.credential.as_ref()?;
            (&c.env_var, &c.placeholder, &c.injections)
        }
        AuthKind::Oauth => {
            let o = integ.oauth.as_ref()?;
            (&o.env_var, &o.placeholder, &o.injections)
        }
    };
    Some(ProviderDef {
        id: integ.id.clone(),
        env_var: env_var.clone(),
        placeholder: placeholder.clone(),
        injections: injections.clone(),
    })
}

fn wire_provider(integ: &Integration) -> Option<DefProvider> {
    wire_provider_def(integ).map(DefProvider::new)
}

/// The oauth block usable for a device sign-in: the device flow with a client_id baked in (community builds ship none, so they fall back to the token paste).
fn signin_oauth(integ: &Integration) -> Option<&OauthAuth> {
    integ.oauth.as_ref().filter(|o| {
        o.flow == OauthFlow::Device && o.client_id.as_deref().is_some_and(|c| !c.is_empty())
    })
}

/// The oauth block usable for a pkce browser sign-in: the pkce flow with an authorization endpoint to redirect through.
fn signin_pkce(integ: &Integration) -> Option<&OauthAuth> {
    integ
        .oauth
        .as_ref()
        .filter(|o| o.flow == OauthFlow::Pkce && o.authorization_endpoint.is_some())
}

/// The allow-routes a set of connected integration ids contributes, re-derived from the catalog so boot and a watcher reload reconstruct the same live routes from an id-only policy.
pub fn applied_integration_routes(ids: &[String], catalog: &[Integration]) -> Vec<RouteRule> {
    let applied: HashSet<&str> = ids.iter().map(String::as_str).collect();
    catalog
        .iter()
        .filter(|integ| applied.contains(integ.id.as_str()))
        .flat_map(|integ| integ.routes.iter().map(|r| r.to_route_rule()))
        .collect()
}

/// Definition-declared ids the effective catalog cannot arm; each refuses the launch, unlike a stale `lns-policy.yaml` id which stays a tolerant skip.
pub fn unknown_integration_ids(declared: &[String], catalog: &[Integration]) -> Vec<String> {
    let known: HashSet<&str> = catalog.iter().map(|i| i.id.as_str()).collect();
    declared
        .iter()
        .filter(|id| !known.contains(id.as_str()))
        .cloned()
        .collect()
}

/// The launch-refusal message for definition-declared ids missing from the machine catalog, pointing at `lns integration add`.
pub fn unknown_integrations_refusal(unknown: &[String]) -> String {
    let ids = unknown
        .iter()
        .map(|id| format!("\"{id}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "the sandbox definition declares integration {ids} which this machine's catalog does not know; \
         declare it with `lns integration add`, or remove it from spec.integrations"
    )
}

/// Resolves the policy's applied integration ids against the effective catalog.
pub fn resolve_applied_integrations(
    policy: &Policy,
    catalog: &[Integration],
) -> AppliedIntegrations {
    let applied: HashSet<&str> = policy.integrations.iter().map(String::as_str).collect();

    let mut out = AppliedIntegrations {
        routes: applied_integration_routes(&policy.integrations, catalog),
        ..AppliedIntegrations::default()
    };
    for integ in catalog {
        if !applied.contains(integ.id.as_str()) {
            continue;
        }
        if let Some(p) = wire_provider(integ) {
            out.providers.push(p);
        }
        if let Some(o) = signin_oauth(integ) {
            out.oauth_configs.insert(integ.id.clone(), o.clone());
        }
        if let Some(o) = signin_pkce(integ) {
            out.pkce_configs.insert(integ.id.clone(), o.clone());
        }
    }
    out
}

/// A definition's credential slots resolve like declared integrations, with each slot's env name overriding the catalog default and winning over a same-id declared entry so the remap holds.
pub fn resolve_applied_with_slots(
    policy: &Policy,
    slots: &[lns_artifact::spec::CredentialSlot],
    catalog: &[Integration],
) -> AppliedIntegrations {
    let slot_ids: HashSet<&str> = slots.iter().map(|s| s.name.as_str()).collect();
    let mut base = policy.clone();
    base.integrations
        .retain(|id| !slot_ids.contains(id.as_str()));
    let mut out = resolve_applied_integrations(&base, catalog);
    for slot in slots {
        let Some(integ) = catalog.iter().find(|i| i.id == slot.name) else {
            continue;
        };
        if let Some(mut def) = wire_provider_def(integ) {
            def.env_var = slot.env.clone();
            out.providers.push(DefProvider::new(def));
        }
        out.routes
            .extend(integ.routes.iter().map(|r| r.to_route_rule()));
        if let Some(o) = signin_oauth(integ) {
            out.oauth_configs.insert(integ.id.clone(), o.clone());
        }
        if let Some(o) = signin_pkce(integ) {
            out.pkce_configs.insert(integ.id.clone(), o.clone());
        }
    }
    out
}

/// A slot's integration is already reachable through the definition, so it is never offered as a fresh connect.
pub fn resolve_connectable_with_slots(
    policy: &Policy,
    slots: &[lns_artifact::spec::CredentialSlot],
    catalog: &[Integration],
) -> ConnectableIntegrations {
    let mut owned = policy.clone();
    owned
        .integrations
        .extend(slots.iter().map(|s| s.name.clone()));
    resolve_connectable_integrations(&owned, catalog)
}

/// Two route patterns collide if either matches the other as a host under the gate's own wildcard- and case-insensitive rule, so an applied domain suppresses a connectable that shares it even when the patterns aren't byte-identical.
fn domains_overlap(a: &str, b: &str) -> bool {
    use crate::approval_flow::session::host_matches_pattern;
    host_matches_pattern(a, b) || host_matches_pattern(b, a)
}

/// The catalog integrations a run can offer to connect: every entry (credential or oauth) not already applied and not colliding with an applied integration's domain.
pub fn resolve_connectable_integrations(
    policy: &Policy,
    catalog: &[Integration],
) -> ConnectableIntegrations {
    let owned: HashSet<&str> = policy.integrations.iter().map(String::as_str).collect();
    // An applied integration owns its route domains; a connectable that overlaps one must not ride the same run — otherwise its machine-global stored value would inject over the applied integration's credential (e.g. a leftover `anthropic` value clobbering a declared `claude-code-subscription` on api.anthropic.com).
    let owned_domains: Vec<&str> = catalog
        .iter()
        .filter(|integ| owned.contains(integ.id.as_str()))
        .flat_map(|integ| integ.routes.iter().map(|r| r.match_pattern.as_str()))
        .collect();

    let mut out = ConnectableIntegrations::default();
    for integ in catalog {
        if owned.contains(integ.id.as_str()) {
            continue;
        }
        if integ.routes.iter().any(|r| {
            owned_domains
                .iter()
                .copied()
                .any(|owned_domain| domains_overlap(owned_domain, r.match_pattern.as_str()))
        }) {
            continue;
        }
        if let Some(p) = wire_provider(integ) {
            out.providers.push(p);
            out.routes.insert(
                integ.id.clone(),
                integ.routes.iter().map(|r| r.to_route_rule()).collect(),
            );
            if let Some(o) = signin_oauth(integ) {
                out.oauth_configs.insert(integ.id.clone(), o.clone());
            }
            if let Some(o) = signin_pkce(integ) {
                out.pkce_configs.insert(integ.id.clone(), o.clone());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credential_flow::providers::Provider;
    use lns_policy::integrations::{CredentialAuth, IntegrationRoute, OauthAuth, OauthFlow};
    use lns_policy::providers::{InjectionDef, InjectionKind};

    fn cred_integration(id: &str, env_var: &str, domain: &str) -> Integration {
        Integration {
            id: id.into(),
            name: None,
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
            oauth: None,
            token_fallback: None,
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
    fn unknown_integration_ids_reports_only_ids_the_catalog_lacks_in_order() {
        let catalog = vec![cred_integration(
            "some-provider",
            "SOME_TOKEN",
            "api.example.test",
        )];
        let declared = vec![
            "some-unknown".to_string(),
            "some-provider".to_string(),
            "other-unknown".to_string(),
        ];
        assert_eq!(
            unknown_integration_ids(&declared, &catalog),
            vec!["some-unknown".to_string(), "other-unknown".to_string()]
        );
        assert!(unknown_integration_ids(&["some-provider".to_string()], &catalog).is_empty());
    }

    #[test]
    fn unknown_integrations_refusal_names_each_id_and_lns_integration_add() {
        let msg = unknown_integrations_refusal(&["some-unknown".to_string(), "other".to_string()]);
        assert!(msg.contains("\"some-unknown\""), "got: {msg}");
        assert!(msg.contains("\"other\""), "got: {msg}");
        assert!(msg.contains("`lns integration add`"), "got: {msg}");
    }

    #[test]
    fn skips_a_catalog_integration_that_is_not_applied() {
        let catalog = vec![cred_integration("gitlab", "GITLAB_TOKEN", "gitlab.com")];
        let out = resolve_applied_integrations(&policy_applying(&[]), &catalog);
        assert!(out.providers.is_empty());
        assert!(out.routes.is_empty());
    }

    #[test]
    fn applied_integration_routes_maps_connected_ids_to_their_catalog_routes() {
        let catalog = vec![cred_integration("gitlab", "GITLAB_TOKEN", "gitlab.com")];
        let routes = applied_integration_routes(&["gitlab".to_string()], &catalog);
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].match_pattern, "gitlab.com");
        assert_eq!(routes[0].verdict, lns_policy::Verdict::Allow);
    }

    #[test]
    fn applied_integration_routes_ignores_ids_absent_from_the_catalog() {
        let catalog = vec![cred_integration("gitlab", "GITLAB_TOKEN", "gitlab.com")];
        assert!(applied_integration_routes(&["nope".to_string()], &catalog).is_empty());
    }

    fn oauth_integration(id: &str, env_var: &str, domain: &str) -> Integration {
        Integration {
            id: id.into(),
            name: None,
            auth_kind: AuthKind::Oauth,
            routes: vec![IntegrationRoute {
                match_pattern: domain.into(),
                transport: None,
                scheme: None,
                tls_terminate: false,
                rules: Vec::new(),
            }],
            credential: None,
            oauth: Some(OauthAuth {
                userinfo_endpoint: None,
                account_field: None,
                flow: OauthFlow::Device,
                client_id: Some(format!("Iv1.{id}")),
                client_secret: None,
                scopes: vec!["repo".into()],
                device_authorization_endpoint: Some(format!("https://{domain}/login/device/code")),
                authorization_endpoint: None,
                token_endpoint: format!("https://{domain}/login/oauth/access_token"),
                env_var: env_var.into(),
                placeholder: format!("lns-{id}-placeholder"),
                injections: vec![InjectionDef {
                    kind: InjectionKind::BearerHeader,
                    domain: domain.into(),
                    header: None,
                }],
            }),
            token_fallback: None,
        }
    }

    fn pkce_integration(id: &str, env_var: &str, domain: &str) -> Integration {
        Integration {
            id: id.into(),
            name: None,
            auth_kind: AuthKind::Oauth,
            routes: vec![IntegrationRoute {
                match_pattern: domain.into(),
                transport: None,
                scheme: None,
                tls_terminate: false,
                rules: Vec::new(),
            }],
            credential: None,
            oauth: Some(OauthAuth {
                userinfo_endpoint: None,
                account_field: None,
                flow: OauthFlow::Pkce,
                client_id: None,
                client_secret: None,
                scopes: Vec::new(),
                device_authorization_endpoint: None,
                authorization_endpoint: Some(format!("https://{domain}/auth")),
                token_endpoint: format!("https://{domain}/api/v1/auth/keys"),
                env_var: env_var.into(),
                placeholder: format!("lns-{id}-placeholder"),
                injections: vec![InjectionDef {
                    kind: InjectionKind::BearerHeader,
                    domain: domain.into(),
                    header: None,
                }],
            }),
            token_fallback: None,
        }
    }

    #[test]
    fn an_applied_oauth_integration_contributes_a_provider_routes_and_its_oauth_config() {
        let catalog = vec![oauth_integration(
            "somesaas",
            "SOMESAAS_TOKEN",
            "api.somesaas.com",
        )];
        let out = resolve_applied_integrations(&policy_applying(&["somesaas"]), &catalog);
        let ids: Vec<&str> = out.providers.iter().map(|p| p.id()).collect();
        assert_eq!(
            ids,
            ["somesaas"],
            "an oauth integration seeds its placeholder like any provider"
        );
        assert_eq!(
            out.routes.len(),
            1,
            "an integration's routes apply regardless of auth kind"
        );
        assert!(
            out.oauth_configs.contains_key("somesaas"),
            "the device-flow config must be surfaced for the sign-in dance"
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
    fn connectable_excludes_a_catalog_entry_that_collides_with_an_applied_domain() {
        let catalog = vec![
            cred_integration("some-primary", "PRIMARY_TOKEN", "api.example.test"),
            cred_integration("some-other", "OTHER_TOKEN", "api.example.test"),
        ];
        let c = resolve_connectable_integrations(&policy_applying(&["some-primary"]), &catalog);
        assert!(
            c.providers.is_empty(),
            "a connectable that shares the applied integration's domain must be suppressed so its machine-global stored value can't inject over the declared credential"
        );
        assert!(c.routes.is_empty());
    }

    #[test]
    fn connectable_excludes_an_entry_whose_wildcard_covers_an_applied_domain() {
        let catalog = vec![
            cred_integration("some-primary", "PRIMARY_TOKEN", "api.example.test"),
            cred_integration("some-wild", "WILD_TOKEN", "*.example.test"),
        ];
        let c = resolve_connectable_integrations(&policy_applying(&["some-primary"]), &catalog);
        assert!(
            c.providers.is_empty(),
            "a connectable whose wildcard covers the applied integration's host must be suppressed, not just a byte-identical pattern"
        );
    }

    #[test]
    fn connectable_excludes_a_case_variant_of_an_applied_domain() {
        let catalog = vec![
            cred_integration("some-primary", "PRIMARY_TOKEN", "api.example.test"),
            cred_integration("some-upper", "UPPER_TOKEN", "API.Example.Test"),
        ];
        let c = resolve_connectable_integrations(&policy_applying(&["some-primary"]), &catalog);
        assert!(
            c.providers.is_empty(),
            "host matching is case-insensitive, so a case-variant of the applied domain must be suppressed"
        );
    }

    #[test]
    fn connectable_on_a_distinct_wildcard_domain_survives_an_applied_integration() {
        let catalog = vec![
            cred_integration("some-primary", "PRIMARY_TOKEN", "api.example.test"),
            cred_integration("some-wild", "WILD_TOKEN", "*.other.test"),
        ];
        let c = resolve_connectable_integrations(&policy_applying(&["some-primary"]), &catalog);
        assert_eq!(
            c.providers.len(),
            1,
            "a wildcard on an unrelated domain must not be over-suppressed"
        );
        assert_eq!(c.providers[0].id(), "some-wild");
    }

    #[test]
    fn connectable_on_a_distinct_domain_survives_an_applied_integration() {
        let catalog = vec![
            cred_integration("some-primary", "PRIMARY_TOKEN", "api.example.test"),
            cred_integration("some-other", "OTHER_TOKEN", "api.other.test"),
        ];
        let c = resolve_connectable_integrations(&policy_applying(&["some-primary"]), &catalog);
        assert_eq!(
            c.providers.len(),
            1,
            "a connectable on its own domain is unaffected by an applied integration elsewhere"
        );
        assert_eq!(c.providers[0].id(), "some-other");
    }

    #[test]
    fn connectable_includes_an_unconnected_oauth_integration_with_its_config() {
        let catalog = vec![oauth_integration(
            "somesaas",
            "SOMESAAS_TOKEN",
            "api.somesaas.com",
        )];
        let c = resolve_connectable_integrations(&policy_applying(&[]), &catalog);
        assert_eq!(
            c.providers.len(),
            1,
            "an unconnected oauth integration is offerable"
        );
        assert_eq!(c.providers[0].id(), "somesaas");
        assert_eq!(c.routes.get("somesaas").map(|r| r.len()), Some(1));
        assert!(
            c.oauth_configs.contains_key("somesaas"),
            "its device-flow config must be held ready for connect"
        );
    }

    #[test]
    fn an_applied_pkce_integration_contributes_a_provider_routes_and_its_pkce_config() {
        let catalog = vec![pkce_integration(
            "somepkce",
            "SOMEPKCE_TOKEN",
            "api.somepkce.com",
        )];
        let out = resolve_applied_integrations(&policy_applying(&["somepkce"]), &catalog);
        let ids: Vec<&str> = out.providers.iter().map(|p| p.id()).collect();
        assert_eq!(
            ids,
            ["somepkce"],
            "a pkce integration seeds its placeholder"
        );
        assert_eq!(out.routes.len(), 1, "its routes apply");
        assert!(
            out.pkce_configs.contains_key("somepkce"),
            "the pkce config must be surfaced for the browser sign-in"
        );
        assert!(
            out.oauth_configs.is_empty(),
            "a pkce entry must not be wired as a device flow"
        );
    }

    #[test]
    fn an_applied_device_oauth_integration_is_not_wired_as_pkce() {
        let catalog = vec![oauth_integration(
            "somesaas",
            "SOMESAAS_TOKEN",
            "api.somesaas.com",
        )];
        let out = resolve_applied_integrations(&policy_applying(&["somesaas"]), &catalog);
        assert!(out.oauth_configs.contains_key("somesaas"));
        assert!(
            out.pkce_configs.is_empty(),
            "a device entry must not be wired as pkce"
        );
    }

    #[test]
    fn connectable_includes_an_unconnected_pkce_integration_with_its_config() {
        let catalog = vec![pkce_integration(
            "somepkce",
            "SOMEPKCE_TOKEN",
            "api.somepkce.com",
        )];
        let c = resolve_connectable_integrations(&policy_applying(&[]), &catalog);
        assert_eq!(c.providers.len(), 1);
        assert_eq!(c.providers[0].id(), "somepkce");
        assert!(
            c.pkce_configs.contains_key("somepkce"),
            "its pkce config must be held ready for connect"
        );
        assert!(c.oauth_configs.is_empty());
    }

    fn oauth_integration_without_client_id(id: &str, env_var: &str, domain: &str) -> Integration {
        let mut i = oauth_integration(id, env_var, domain);
        i.oauth.as_mut().unwrap().client_id = None;
        i
    }

    #[test]
    fn an_applied_oauth_integration_with_no_client_id_seeds_a_provider_but_withholds_the_device_flow()
     {
        let catalog = vec![oauth_integration_without_client_id(
            "somesaas",
            "SOMESAAS_TOKEN",
            "api.somesaas.com",
        )];
        let out = resolve_applied_integrations(&policy_applying(&["somesaas"]), &catalog);
        let ids: Vec<&str> = out.providers.iter().map(|p| p.id()).collect();
        assert_eq!(
            ids,
            ["somesaas"],
            "the placeholder is still seeded so a pasted token can arm it"
        );
        assert_eq!(out.routes.len(), 1, "its routes still apply");
        assert!(
            out.oauth_configs.is_empty(),
            "an empty client_id can't drive a device flow, so no oauth config is surfaced"
        );
    }

    fn slot(name: &str, env: &str, required: bool) -> lns_artifact::spec::CredentialSlot {
        lns_artifact::spec::CredentialSlot {
            name: name.into(),
            env: env.into(),
            required,
        }
    }

    #[test]
    fn a_slot_seeds_its_provider_under_the_slot_env_name_with_the_catalog_placeholder() {
        let catalog = vec![cred_integration(
            "some-provider",
            "SOME_TOKEN",
            "api.example.test",
        )];
        let out = resolve_applied_with_slots(
            &policy_applying(&[]),
            &[slot("some-provider", "PROVIDER_KEY", false)],
            &catalog,
        );
        assert_eq!(out.providers.len(), 1);
        assert_eq!(out.providers[0].id(), "some-provider");
        assert_eq!(
            out.providers[0].env_var(),
            "PROVIDER_KEY",
            "the slot's env remap must win over the catalog default"
        );
        assert_eq!(
            out.providers[0].placeholder(),
            "lns-some-provider-placeholder",
            "the placeholder stays the catalog's so the boundary still detects it"
        );
        assert_eq!(
            out.routes.len(),
            1,
            "a slot allows its routes like a declared id"
        );
        assert_eq!(out.routes[0].match_pattern, "api.example.test");
    }

    #[test]
    fn a_slot_wins_over_a_same_id_declared_integration_so_the_remap_holds() {
        let catalog = vec![cred_integration(
            "some-provider",
            "SOME_TOKEN",
            "api.example.test",
        )];
        let out = resolve_applied_with_slots(
            &policy_applying(&["some-provider"]),
            &[slot("some-provider", "PROVIDER_KEY", true)],
            &catalog,
        );
        assert_eq!(
            out.providers.len(),
            1,
            "the slot and the declared id must not double-seed"
        );
        assert_eq!(out.providers[0].env_var(), "PROVIDER_KEY");
    }

    #[test]
    fn a_slot_alongside_a_different_declared_integration_unions_without_loss() {
        let catalog = vec![
            cred_integration("some-provider", "SOME_TOKEN", "api.example.test"),
            cred_integration("other-provider", "OTHER_TOKEN", "api.other.example"),
        ];
        let out = resolve_applied_with_slots(
            &policy_applying(&["other-provider"]),
            &[slot("some-provider", "SOME_TOKEN", false)],
            &catalog,
        );
        let mut ids: Vec<&str> = out.providers.iter().map(|p| p.id()).collect();
        ids.sort_unstable();
        assert_eq!(ids, ["other-provider", "some-provider"]);
    }

    #[test]
    fn a_slot_naming_an_unknown_id_contributes_nothing_here() {
        let out = resolve_applied_with_slots(
            &policy_applying(&[]),
            &[slot("some-unknown", "SOME_TOKEN", true)],
            &[],
        );
        assert!(
            out.providers.is_empty(),
            "unknown ids are the refusal's job"
        );
        assert!(out.routes.is_empty());
    }

    #[test]
    fn an_oauth_slot_surfaces_its_sign_in_config_under_the_slot_env() {
        let catalog = vec![oauth_integration(
            "some-oauth",
            "SOME_OAUTH_TOKEN",
            "api.some-oauth.example",
        )];
        let out = resolve_applied_with_slots(
            &policy_applying(&[]),
            &[slot("some-oauth", "OAUTH_KEY", true)],
            &catalog,
        );
        assert_eq!(out.providers[0].env_var(), "OAUTH_KEY");
        assert!(
            out.oauth_configs.contains_key("some-oauth"),
            "the sign-in config must surface for the launch gate"
        );
    }

    #[test]
    fn a_pkce_oauth_slot_surfaces_its_pkce_config() {
        let catalog = vec![pkce_integration(
            "somepkce",
            "SOMEPKCE_TOKEN",
            "api.somepkce.com",
        )];
        let out = resolve_applied_with_slots(
            &policy_applying(&[]),
            &[slot("somepkce", "SOMEPKCE_TOKEN", false)],
            &catalog,
        );
        assert!(out.pkce_configs.contains_key("somepkce"));
        assert!(out.oauth_configs.is_empty());
    }

    #[test]
    fn a_slot_named_integration_is_not_offered_as_a_fresh_connect() {
        let catalog = vec![cred_integration(
            "some-provider",
            "SOME_TOKEN",
            "api.example.test",
        )];
        let c = resolve_connectable_with_slots(
            &policy_applying(&[]),
            &[slot("some-provider", "SOME_TOKEN", false)],
            &catalog,
        );
        assert!(
            c.providers.is_empty(),
            "a slot's integration is already reachable, never a fresh offer"
        );
    }

    #[test]
    fn a_connectable_oauth_integration_with_no_client_id_is_offerable_without_a_device_flow() {
        let catalog = vec![oauth_integration_without_client_id(
            "somesaas",
            "SOMESAAS_TOKEN",
            "api.somesaas.com",
        )];
        let c = resolve_connectable_integrations(&policy_applying(&[]), &catalog);
        assert_eq!(
            c.providers.len(),
            1,
            "still offerable via its token fallback when no client_id is baked in"
        );
        assert_eq!(c.providers[0].id(), "somesaas");
        assert_eq!(c.routes.get("somesaas").map(|r| r.len()), Some(1));
        assert!(
            c.oauth_configs.is_empty(),
            "no client_id means there is no device flow to hold ready"
        );
    }
}
