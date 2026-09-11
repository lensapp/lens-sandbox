use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum OAuth {
    #[serde(rename = "oauth_device", rename_all = "camelCase")]
    Device {
        client_id: String,
        token_endpoint: String,
        device_authorization_endpoint: String,
        verification_hosts: Vec<String>,
        #[serde(default)]
        scopes: Vec<String>,
    },
    #[serde(rename = "oauth_authorization_code", rename_all = "camelCase")]
    AuthorizationCode {
        client_id: String,
        token_endpoint: String,
        authorization_endpoint: String,
        redirect: Redirect,
        #[serde(default)]
        scopes: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum Redirect {
    #[serde(rename = "loopback")]
    Loopback {
        #[serde(default = "callback_path")]
        path: String,
        port: Option<u16>,
    },
}

fn callback_path() -> String {
    "/callback".into()
}

impl OAuth {
    pub fn client_id(&self) -> &str {
        match self {
            Self::Device { client_id, .. } | Self::AuthorizationCode { client_id, .. } => client_id,
        }
    }

    pub fn token_endpoint(&self) -> &str {
        match self {
            Self::Device { token_endpoint, .. }
            | Self::AuthorizationCode { token_endpoint, .. } => token_endpoint,
        }
    }

    pub fn scopes(&self) -> &[String] {
        match self {
            Self::Device { scopes, .. } | Self::AuthorizationCode { scopes, .. } => scopes,
        }
    }
}

pub(super) fn parse(value: serde_json::Value) -> Result<OAuth> {
    if value.get("clientSecret").is_some() {
        bail!(
            "native OAuth supports public clients only; confidential clients and clientSecret are not supported yet"
        );
    }
    let oauth: OAuth = serde_json::from_value(value).context("reading native OAuth auth")?;
    let client = oauth.client_id();
    if client.is_empty()
        || client.len() > 4096
        || client.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        bail!(
            "clientId must be a nonempty public identifier without whitespace, at most 4096 bytes"
        );
    }
    endpoint(oauth.token_endpoint())?;
    if oauth.scopes().len() > 128 || oauth.scopes().iter().any(|s| !scope_token(s)) {
        bail!("scopes must contain at most 128 individual OAuth scope tokens, each 1–256 bytes");
    }
    match &oauth {
        OAuth::Device {
            device_authorization_endpoint,
            verification_hosts,
            ..
        } => {
            endpoint(device_authorization_endpoint)?;
            if verification_hosts.is_empty() || verification_hosts.len() > 128 {
                bail!("verificationHosts must name 1–128 explicit hosts");
            }
            for host in verification_hosts {
                verification_host(host)?;
            }
        }
        OAuth::AuthorizationCode {
            authorization_endpoint,
            redirect,
            ..
        } => {
            endpoint(authorization_endpoint)?;
            let Redirect::Loopback { path, port } = redirect;
            if *port == Some(0) {
                bail!("redirect.port must be in 1–65535; omit it for an ephemeral port");
            }
            if !callback_path_valid(path) {
                bail!(
                    "redirect.path must be a normalized absolute path without query, fragment, escapes, or traversal"
                );
            }
        }
    }
    Ok(oauth)
}

pub fn scope_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|b| b == 0x21 || (0x23..=0x5b).contains(&b) || (0x5d..=0x7e).contains(&b))
}

pub fn endpoint(value: &str) -> Result<url::Url> {
    let url = https_url(value)?;
    if url.query().is_some() {
        bail!("OAuth endpoints must not contain a query");
    }
    Ok(url)
}

pub fn https_url(value: &str) -> Result<url::Url> {
    if value.len() > 8192
        || value
            .chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '\\')
    {
        bail!("OAuth URL exceeds its bound or contains whitespace or backslashes");
    }
    let url = url::Url::parse(value).context("OAuth URL must be absolute HTTPS")?;
    if !value.starts_with("https://")
        || url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || value
            .split('/')
            .nth(2)
            .is_some_and(|host| host.contains('@'))
    {
        bail!("OAuth URL must be absolute HTTPS without userinfo or fragment");
    }
    Ok(url)
}

pub fn verification_host(value: &str) -> Result<url::Url> {
    if value.is_empty()
        || value.contains(['*', '/', '?', '#', '@', '%', '\\'])
        || value.ends_with(':')
    {
        bail!(
            "verificationHosts requires explicit hosts with optional ports, without wildcards or URL syntax"
        );
    }
    let url = endpoint(&format!("https://{value}"))?;
    if url.port() == Some(0) {
        bail!("verificationHosts port must be in 1–65535");
    }
    Ok(url)
}

fn callback_path_valid(path: &str) -> bool {
    path.starts_with('/')
        && path.len() <= 1024
        && !path.contains(['?', '#', '%', '\\'])
        && path
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/-._~!$&'()*+,;=:@".contains(&b))
        && (path == "/"
            || path[1..]
                .split('/')
                .all(|part| !matches!(part, "" | "." | "..")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_templates_are_valid_public_client_connectors() {
        for document in [
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../docs/examples/oauth/github-device/lns.yaml"
            )),
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../docs/examples/oauth/linear-native/lns.yaml"
            )),
        ] {
            let value: serde_json::Value = serde_yaml::from_str(document).unwrap();
            let connector = crate::connector::parse(&serde_json::to_vec(&value).unwrap()).unwrap();
            let config = connector.spec.methods[0]
                .auth
                .as_ref()
                .unwrap()
                .oauth()
                .unwrap()
                .unwrap();
            assert_eq!(
                config.client_id(),
                "REPLACE_WITH_REGISTERED_PUBLIC_CLIENT_ID"
            );
        }
    }

    #[test]
    fn endpoints_never_hide_a_destination_or_parameters() {
        for value in [
            "",
            "/token",
            "http://a.example",
            "https:///",
            "https://@a.example",
            "https://a.example?",
            "https://a.example#",
            "https://a.example\\evil",
            "https://a.example/ path",
        ] {
            assert!(endpoint(value).is_err(), "{value}");
        }
        assert!(endpoint(&format!("https://a.example/{}", "a".repeat(8192))).is_err());
        assert_eq!(
            endpoint("https://a.example:8443/token").unwrap().port(),
            Some(8443)
        );
    }

    #[test]
    fn verification_hosts_are_exact_https_authorities() {
        for host in [
            "",
            "*",
            "*.example",
            "a.example/",
            "a.example?x",
            "a.example#x",
            "user@a.example",
            "a.example:",
            "a.example:0",
            "a.example:65536",
            "a.example:abc",
            "a example",
        ] {
            assert!(verification_host(host).is_err(), "{host}");
        }
        assert_eq!(
            verification_host("a.example")
                .unwrap()
                .port_or_known_default(),
            Some(443)
        );
        assert_eq!(verification_host("[::1]:8443").unwrap().port(), Some(8443));
    }

    #[test]
    fn callback_paths_are_unambiguous_and_scopes_are_individual_tokens() {
        for path in [
            "",
            "callback",
            "//callback",
            "/a/../callback",
            "/a/./callback",
            "/a//b",
            "/callback?x",
            "/callback#x",
            "/%2e",
            "/a\\b",
            "/a b",
            "/é",
            "/a/",
            "/a|b",
            "/a<b",
            "/a\"b",
            "/a[b",
            "/a`b",
            "/a{b",
        ] {
            assert!(!callback_path_valid(path), "{path}");
        }
        assert!(!callback_path_valid(&format!("/{}", "a".repeat(1024))));
        for path in ["/", "/callback", "/oauth/callback"] {
            assert!(callback_path_valid(path));
        }
        for scope in ["", "read write", "read\twrite", "é", "\"", "\\"] {
            assert!(!scope_token(scope));
        }
        assert!(!scope_token(&"a".repeat(257)));
        for scope in ["read", "read:user", "api://a/read", "!"] {
            assert!(scope_token(scope));
        }
    }

    #[test]
    fn kind_specific_fields_and_bounds_survive_round_trips() {
        let device = serde_json::json!({"kind":"oauth_device","clientId":"id","tokenEndpoint":"https://a.example/token","deviceAuthorizationEndpoint":"https://a.example/device","verificationHosts":["a.example"]});
        let code = serde_json::json!({"kind":"oauth_authorization_code","clientId":"id","tokenEndpoint":"https://a.example/token","authorizationEndpoint":"https://a.example/authorize","redirect":{"kind":"loopback"}});
        for original in [&device, &code] {
            let read = parse(original.clone()).unwrap();
            assert_eq!(parse(serde_json::to_value(&read).unwrap()).unwrap(), read);
            let mut invalid = original.clone();
            invalid["scopes"] = serde_json::json!(vec!["read"; 129]);
            assert!(parse(invalid).is_err());
            let mut invalid = original.clone();
            invalid["clientId"] = "a".repeat(4097).into();
            assert!(parse(invalid).is_err());
        }
        for hosts in [
            serde_json::json!([]),
            serde_json::json!(["*"]),
            serde_json::json!(vec!["a.example"; 129]),
        ] {
            let mut invalid = device.clone();
            invalid["verificationHosts"] = hosts;
            assert!(parse(invalid).is_err());
        }
        for redirect in [
            serde_json::json!({"kind":"loopback","port":0}),
            serde_json::json!({"kind":"loopback","port":65536}),
            serde_json::json!({"kind":"loopback","path":"/../callback"}),
            serde_json::json!({"kind":"custom"}),
            serde_json::json!({"kind":"loopback","extra":true}),
        ] {
            let mut invalid = code.clone();
            invalid["redirect"] = redirect;
            assert!(parse(invalid).is_err());
        }
        let mut invalid = device;
        invalid["authorizationEndpoint"] = "https://a.example/auth".into();
        assert!(parse(invalid).is_err());
        let mut invalid = code;
        invalid["verificationHosts"] = serde_json::json!(["a.example"]);
        assert!(parse(invalid).is_err());
    }
}
