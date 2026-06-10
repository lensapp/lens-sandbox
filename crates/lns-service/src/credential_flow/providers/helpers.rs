use crate::approval_flow::protocol::CredentialInjection;

pub fn bearer_header(value: &str, domain: &str) -> Vec<CredentialInjection> {
    vec![CredentialInjection::Header {
        domain: domain.to_string(),
        header: "Authorization".to_string(),
        value: format!("Bearer {value}"),
    }]
}

pub fn bearer_header_unarmed(domain: &str) -> Vec<CredentialInjection> {
    vec![CredentialInjection::Header {
        domain: domain.to_string(),
        header: "Authorization".to_string(),
        value: String::new(),
    }]
}

pub fn token_header(value: &str, domain: &str) -> Vec<CredentialInjection> {
    vec![CredentialInjection::Header {
        domain: domain.to_string(),
        header: "Authorization".to_string(),
        value: format!("token {value}"),
    }]
}

pub fn token_header_unarmed(domain: &str) -> Vec<CredentialInjection> {
    vec![CredentialInjection::Header {
        domain: domain.to_string(),
        header: "Authorization".to_string(),
        value: String::new(),
    }]
}

pub fn basic_x_access_token(value: &str, domain: &str) -> Vec<CredentialInjection> {
    let encoded = crate::base64::encode(format!("x-access-token:{value}").as_bytes());
    vec![CredentialInjection::Header {
        domain: domain.to_string(),
        header: "Authorization".to_string(),
        value: format!("Basic {encoded}"),
    }]
}

pub fn basic_x_access_token_unarmed(domain: &str) -> Vec<CredentialInjection> {
    vec![CredentialInjection::Header {
        domain: domain.to_string(),
        header: "Authorization".to_string(),
        value: String::new(),
    }]
}

pub fn api_key_header(value: &str, domain: &str, header: &str) -> Vec<CredentialInjection> {
    vec![CredentialInjection::Header {
        domain: domain.to_string(),
        header: header.to_string(),
        value: value.to_string(),
    }]
}

pub fn api_key_header_unarmed(domain: &str, header: &str) -> Vec<CredentialInjection> {
    vec![CredentialInjection::Header {
        domain: domain.to_string(),
        header: header.to_string(),
        value: String::new(),
    }]
}

pub fn uri_placeholder(value: &str, domain: &str) -> Vec<CredentialInjection> {
    vec![CredentialInjection::UriPlaceholder {
        domain: domain.to_string(),
        value: value.to_string(),
    }]
}

pub fn uri_placeholder_unarmed(domain: &str) -> Vec<CredentialInjection> {
    vec![CredentialInjection::UriPlaceholder {
        domain: domain.to_string(),
        value: String::new(),
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_header_uses_authorization_header_with_bearer_scheme_prefix() {
        let inj = bearer_header("some-token", "api.example.test");
        assert_eq!(inj.len(), 1);
        assert!(matches!(
            &inj[0],
            CredentialInjection::Header { domain, header, value }
                if domain == "api.example.test"
                    && header == "Authorization"
                    && value == "Bearer some-token"
        ));
    }

    #[test]
    fn token_header_uses_the_git_token_scheme() {
        let inj = token_header("some-token", "api.example.test");
        assert_eq!(inj.len(), 1);
        assert!(matches!(
            &inj[0],
            CredentialInjection::Header { domain, header, value }
                if domain == "api.example.test"
                    && header == "Authorization"
                    && value == "token some-token"
        ));
    }

    #[test]
    fn token_header_unarmed_declares_domain_with_empty_value() {
        let inj = token_header_unarmed("api.example.test");
        assert!(matches!(
            &inj[0],
            CredentialInjection::Header { domain, value, .. }
                if domain == "api.example.test" && value.is_empty()
        ));
    }

    #[test]
    fn basic_x_access_token_base64_encodes_the_x_access_token_userinfo() {
        let inj = basic_x_access_token("some-token", "example.test");
        assert_eq!(inj.len(), 1);
        let expected = format!(
            "Basic {}",
            crate::base64::encode(b"x-access-token:some-token")
        );
        assert!(matches!(
            &inj[0],
            CredentialInjection::Header { domain, header, value }
                if domain == "example.test"
                    && header == "Authorization"
                    && *value == expected
        ));
    }

    #[test]
    fn basic_x_access_token_unarmed_declares_domain_with_empty_value() {
        let inj = basic_x_access_token_unarmed("example.test");
        assert!(matches!(
            &inj[0],
            CredentialInjection::Header { domain, value, .. }
                if domain == "example.test" && value.is_empty()
        ));
    }

    #[test]
    fn api_key_header_uses_the_named_header_with_the_raw_value_and_no_scheme_prefix() {
        let inj = api_key_header("some-secret", "api.example.test", "x-api-key");
        assert_eq!(inj.len(), 1);
        assert!(matches!(
            &inj[0],
            CredentialInjection::Header { domain, header, value }
                if domain == "api.example.test"
                    && header == "x-api-key"
                    && value == "some-secret"
        ));
    }

    #[test]
    fn api_key_header_unarmed_declares_named_header_with_empty_value() {
        let inj = api_key_header_unarmed("api.example.test", "x-api-key");
        assert_eq!(inj.len(), 1);
        assert!(matches!(
            &inj[0],
            CredentialInjection::Header { domain, header, value }
                if domain == "api.example.test"
                    && header == "x-api-key"
                    && value.is_empty()
        ));
    }

    #[test]
    fn uri_placeholder_carries_domain_and_real_value_for_url_substitution() {
        let inj = uri_placeholder("123:realtoken", "api.telegram.org");
        assert_eq!(inj.len(), 1);
        assert!(matches!(
            &inj[0],
            CredentialInjection::UriPlaceholder { domain, value }
                if domain == "api.telegram.org" && value == "123:realtoken"
        ));
    }

    #[test]
    fn bearer_header_unarmed_declares_domain_with_empty_value() {
        let inj = bearer_header_unarmed("api.example.test");
        assert_eq!(inj.len(), 1);
        assert!(matches!(
            &inj[0],
            CredentialInjection::Header { domain, header, value }
                if domain == "api.example.test"
                    && header == "Authorization"
                    && value.is_empty()
        ));
    }

    #[test]
    fn uri_placeholder_unarmed_declares_domain_with_empty_value() {
        let inj = uri_placeholder_unarmed("api.telegram.org");
        assert_eq!(inj.len(), 1);
        assert!(matches!(
            &inj[0],
            CredentialInjection::UriPlaceholder { domain, value }
                if domain == "api.telegram.org" && value.is_empty()
        ));
    }
}
