mod def_provider;
mod helpers;
mod provider;

pub use def_provider::DefProvider;
pub use provider::Provider;

use std::collections::HashSet;
use std::sync::LazyLock;

/// Tests rely on the order being deterministic, so don't reshuffle `lns_policy::providers::builtins` without updating them.
pub static ALL: LazyLock<Vec<&'static dyn Provider>> = LazyLock::new(|| {
    lns_policy::providers::builtins()
        .iter()
        .map(|def| {
            let leaked: &'static dyn Provider = Box::leak(Box::new(DefProvider::new(def.clone())));
            leaked
        })
        .collect()
});

/// Returns `None` for an unknown id so a rule pointing at a removed provider goes inert rather than crashing the resolver.
pub fn by_id(id: &str) -> Option<&'static dyn Provider> {
    ALL.iter().copied().find(|p| p.id() == id)
}

/// Builds the per-run custom providers from the loaded policy, dropping any id that collides with a built-in so a shipped provider can never be shadowed.
pub fn build_policy_providers(policy: &lns_policy::Policy) -> Vec<DefProvider> {
    let builtin_ids: HashSet<&str> = lns_policy::providers::builtins()
        .iter()
        .map(|p| p.id.as_str())
        .collect();
    policy
        .credentials
        .custom_providers
        .iter()
        .filter(|def| !builtin_ids.contains(def.id.as_str()))
        .map(|def| DefProvider::new(def.clone()))
        .collect()
}

pub fn is_managed_env(env_var: &str) -> bool {
    ALL.iter().any(|p| p.env_var() == env_var)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lns_policy::providers::{InjectionDef, InjectionKind, ProviderDef};

    #[test]
    fn by_id_returns_none_for_unknown_provider() {
        assert!(by_id("not-a-real-provider").is_none());
    }

    #[test]
    fn by_id_returns_a_provider_whose_id_matches_the_query() {
        let p = by_id("openai").expect("openai present");
        assert_eq!(p.id(), "openai");
        assert_eq!(p.env_var(), "OPENAI_API_KEY");
    }

    fn policy_with(custom: Vec<ProviderDef>) -> lns_policy::Policy {
        let mut p = lns_policy::Policy::default();
        p.credentials.custom_providers = custom;
        p
    }

    fn acme() -> ProviderDef {
        ProviderDef {
            id: "acme".into(),
            env_var: "ACME_API_KEY".into(),
            placeholder: "acme_LNSPLACEHOLDER".into(),
            injections: vec![InjectionDef {
                kind: InjectionKind::BearerHeader,
                domain: "api.acme.corp".into(),
                header: None,
            }],
        }
    }

    #[test]
    fn build_policy_providers_wraps_each_declared_custom_provider() {
        let providers = build_policy_providers(&policy_with(vec![acme()]));
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].id(), "acme");
        assert_eq!(providers[0].env_var(), "ACME_API_KEY");
    }

    #[test]
    fn build_policy_providers_drops_an_id_that_collides_with_a_builtin() {
        let mut shadow = acme();
        shadow.id = "openai".into();
        let providers = build_policy_providers(&policy_with(vec![shadow, acme()]));
        let ids: Vec<&str> = providers.iter().map(|p| p.id()).collect();
        assert_eq!(ids, ["acme"], "a built-in id must never be shadowed");
    }

    #[test]
    fn build_policy_providers_is_empty_for_a_network_only_policy() {
        assert!(build_policy_providers(&lns_policy::Policy::default()).is_empty());
    }

    #[test]
    fn is_managed_env_recognises_a_shipped_credential_var() {
        assert!(is_managed_env("OPENAI_API_KEY"));
    }

    #[test]
    fn is_managed_env_rejects_an_unmanaged_var() {
        assert!(!is_managed_env("CLAUDE_CODE_USE_BEDROCK"));
    }
}
