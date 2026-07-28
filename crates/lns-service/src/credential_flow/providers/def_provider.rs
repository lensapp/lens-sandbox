use lns_policy::providers::{InjectionDef, InjectionKind, ProviderDef};

use super::Provider;
use super::helpers::{
    api_key_header, api_key_header_unarmed, basic_x_access_token, basic_x_access_token_unarmed,
    bearer_header, bearer_header_unarmed, token_header, token_header_unarmed, uri_placeholder,
    uri_placeholder_unarmed,
};
use crate::approval_flow::protocol::CredentialInjection;
use crate::credential_flow::detection::{EnvVarDetector, HostDetector};

/// Wraps a [`ProviderDef`] — built-in or policy-declared — with the host-detection and injection-building machinery that stays inside lns-service.
pub struct DefProvider {
    def: ProviderDef,
    detector: EnvVarDetector,
    seed_env: bool,
}

impl DefProvider {
    pub fn new(def: ProviderDef) -> Self {
        let detector = EnvVarDetector::new(def.env_var.clone());
        Self {
            def,
            detector,
            seed_env: true,
        }
    }

    /// Mark a connectable (undeclared) provider so its placeholder is known to the MITM for an on-domain connect offer but never seeded into the workload env.
    pub fn detect_only(mut self) -> Self {
        self.seed_env = false;
        self
    }

    /// The effective env var and sorted, de-duplicated injection domains this provider discloses — the one snapshot both the consent card records a grant against and the boot gate later verifies, so disclosure and verification can never drift.
    pub fn disclosure_snapshot(&self) -> (String, Vec<String>) {
        let domains: std::collections::BTreeSet<String> = self
            .unarmed_injections()
            .iter()
            .map(|inj| inj.domain().to_string())
            .collect();
        (self.env_var().to_string(), domains.into_iter().collect())
    }
}

fn armed(inj: &InjectionDef, value: &str) -> Vec<CredentialInjection> {
    match inj.kind {
        InjectionKind::BearerHeader => bearer_header(value, &inj.domain),
        InjectionKind::TokenHeader => token_header(value, &inj.domain),
        InjectionKind::BasicXAccessToken => basic_x_access_token(value, &inj.domain),
        InjectionKind::ApiKeyHeader => match &inj.header {
            Some(header) => api_key_header(value, &inj.domain, header),
            None => Vec::new(),
        },
        InjectionKind::UriPlaceholder => uri_placeholder(value, &inj.domain),
    }
}

fn unarmed(inj: &InjectionDef) -> Vec<CredentialInjection> {
    match inj.kind {
        InjectionKind::BearerHeader => bearer_header_unarmed(&inj.domain),
        InjectionKind::TokenHeader => token_header_unarmed(&inj.domain),
        InjectionKind::BasicXAccessToken => basic_x_access_token_unarmed(&inj.domain),
        InjectionKind::ApiKeyHeader => match &inj.header {
            Some(header) => api_key_header_unarmed(&inj.domain, header),
            None => Vec::new(),
        },
        InjectionKind::UriPlaceholder => uri_placeholder_unarmed(&inj.domain),
    }
}

impl Provider for DefProvider {
    fn id(&self) -> &str {
        &self.def.id
    }
    fn env_var(&self) -> &str {
        &self.def.env_var
    }
    fn placeholder(&self) -> &str {
        &self.def.placeholder
    }
    fn detector(&self) -> &dyn HostDetector {
        &self.detector
    }
    fn injections(&self, value: &str) -> Vec<CredentialInjection> {
        self.def
            .injections
            .iter()
            .flat_map(|inj| armed(inj, value))
            .collect()
    }
    fn unarmed_injections(&self) -> Vec<CredentialInjection> {
        self.def.injections.iter().flat_map(unarmed).collect()
    }
    fn seeds_env(&self) -> bool {
        self.seed_env
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(injections: Vec<InjectionDef>) -> DefProvider {
        DefProvider::new(ProviderDef {
            id: "test".into(),
            env_var: "TEST_TOKEN".into(),
            placeholder: "test_LNSPLACEHOLDER".into(),
            injections,
        })
    }

    fn single(kind: InjectionKind, domain: &str) -> DefProvider {
        provider(vec![InjectionDef {
            kind,
            domain: domain.into(),
            header: None,
        }])
    }

    #[test]
    fn bearer_header_provider_builds_authorization_header() {
        let inj = single(InjectionKind::BearerHeader, "api.openai.com").injections("sk-real");
        assert_eq!(inj.len(), 1);
        assert!(matches!(
            &inj[0],
            CredentialInjection::Header { domain, header, value }
                if domain == "api.openai.com" && header == "Authorization" && value == "Bearer sk-real"
        ));
    }

    #[test]
    fn token_header_provider_builds_the_git_token_scheme() {
        let inj = single(InjectionKind::TokenHeader, "api.example.test").injections("some-token");
        assert_eq!(inj.len(), 1);
        assert!(matches!(
            &inj[0],
            CredentialInjection::Header { domain, header, value }
                if domain == "api.example.test" && header == "Authorization" && value == "token some-token"
        ));
    }

    #[test]
    fn basic_x_access_token_provider_base64_encodes_the_userinfo() {
        let inj = single(InjectionKind::BasicXAccessToken, "example.test").injections("some-token");
        assert_eq!(inj.len(), 1);
        let expected = format!(
            "Basic {}",
            crate::base64::encode(b"x-access-token:some-token")
        );
        assert!(matches!(
            &inj[0],
            CredentialInjection::Header { domain, header, value }
                if domain == "example.test" && header == "Authorization" && *value == expected
        ));
    }

    #[test]
    fn api_key_header_provider_builds_the_named_header_with_the_raw_value() {
        let p = provider(vec![InjectionDef {
            kind: InjectionKind::ApiKeyHeader,
            domain: "api.example.test".into(),
            header: Some("x-api-key".into()),
        }]);
        let inj = p.injections("some-secret");
        assert_eq!(inj.len(), 1);
        assert!(matches!(
            &inj[0],
            CredentialInjection::Header { domain, header, value }
                if domain == "api.example.test" && header == "x-api-key" && value == "some-secret"
        ));
    }

    #[test]
    fn api_key_header_without_a_header_name_injects_nothing() {
        let p = provider(vec![InjectionDef {
            kind: InjectionKind::ApiKeyHeader,
            domain: "api.example.test".into(),
            header: None,
        }]);
        assert!(
            p.injections("some-secret").is_empty(),
            "a malformed api_key_header must fail closed, injecting nothing"
        );
        assert!(p.unarmed_injections().is_empty());
    }

    #[test]
    fn uri_placeholder_provider_builds_url_substitution() {
        let inj = single(InjectionKind::UriPlaceholder, "api.telegram.org").injections("123:real");
        assert_eq!(inj.len(), 1);
        assert!(matches!(
            &inj[0],
            CredentialInjection::UriPlaceholder { domain, value }
                if domain == "api.telegram.org" && value == "123:real"
        ));
    }

    #[test]
    fn every_header_kind_declares_its_domain_unarmed_with_an_empty_value() {
        let p = provider(vec![
            InjectionDef {
                kind: InjectionKind::TokenHeader,
                domain: "api.example.test".into(),
                header: None,
            },
            InjectionDef {
                kind: InjectionKind::BasicXAccessToken,
                domain: "example.test".into(),
                header: None,
            },
            InjectionDef {
                kind: InjectionKind::ApiKeyHeader,
                domain: "api.example.test".into(),
                header: Some("x-api-key".into()),
            },
        ]);
        let inj = p.unarmed_injections();
        assert_eq!(inj.len(), 3);
        assert!(inj.iter().all(|i| matches!(
            i,
            CredentialInjection::Header { value, .. } if value.is_empty()
        )));
    }

    #[test]
    fn bearer_header_unarmed_declares_domain_with_empty_value() {
        let inj = single(InjectionKind::BearerHeader, "api.example.test").unarmed_injections();
        assert_eq!(inj.len(), 1);
        assert!(matches!(
            &inj[0],
            CredentialInjection::Header { value, .. } if value.is_empty()
        ));
    }

    #[test]
    fn uri_placeholder_unarmed_declares_domain_with_empty_value() {
        let inj = single(InjectionKind::UriPlaceholder, "api.telegram.org").unarmed_injections();
        assert_eq!(inj.len(), 1);
        assert!(matches!(
            &inj[0],
            CredentialInjection::UriPlaceholder { value, .. } if value.is_empty()
        ));
    }

    #[test]
    #[serial_test::serial(env)]
    fn detector_is_bound_to_the_def_env_var() {
        use crate::test_env::EnvVarGuard;
        let p = single(InjectionKind::BearerHeader, "api.test");
        let _g = EnvVarGuard::set("TEST_TOKEN", "secret-from-env");
        assert_eq!(p.detector().detect().as_deref(), Some("secret-from-env"));
    }

    #[test]
    fn id_env_var_and_placeholder_pass_through_the_def() {
        let p = single(InjectionKind::BearerHeader, "api.test");
        assert_eq!(p.id(), "test");
        assert_eq!(p.env_var(), "TEST_TOKEN");
        assert_eq!(p.placeholder(), "test_LNSPLACEHOLDER");
    }

    #[test]
    fn a_provider_seeds_its_env_by_default_and_detect_only_opts_out() {
        let p = single(InjectionKind::BearerHeader, "api.test");
        assert!(
            p.seeds_env(),
            "a declared provider seeds its placeholder env"
        );
        let connectable = single(InjectionKind::BearerHeader, "api.test").detect_only();
        assert!(
            !connectable.seeds_env(),
            "a connectable provider must not seed the workload env"
        );
        assert_eq!(
            connectable.unarmed_injections().len(),
            1,
            "detect-only keeps its injection so an on-domain connect offer still fires"
        );
    }

    #[test]
    fn a_multi_domain_def_emits_one_injection_per_domain_armed_and_unarmed() {
        let p = provider(vec![
            InjectionDef {
                kind: InjectionKind::BearerHeader,
                domain: "api.acme.corp".into(),
                header: None,
            },
            InjectionDef {
                kind: InjectionKind::UriPlaceholder,
                domain: "api-eu.acme.corp".into(),
                header: None,
            },
        ]);
        assert_eq!(p.injections("real").len(), 2);
        assert_eq!(p.unarmed_injections().len(), 2);
    }
}
