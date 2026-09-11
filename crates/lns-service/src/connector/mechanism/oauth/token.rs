use super::super::{Answers, HttpRequest, HttpResponse};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OAuthState {
    pub client_id: String,
    pub token_endpoint: String,
    pub kind: String,
    pub refresh_token: Option<String>,
    pub reconnect_required: bool,
}

impl std::fmt::Debug for OAuthState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("OAuthState(<redacted>)")
    }
}

#[derive(PartialEq, Eq)]
pub struct Tokens {
    pub values: Answers,
    pub authority: BTreeSet<String>,
    pub expires_at_millis: Option<u64>,
    pub private: OAuthState,
}

impl std::fmt::Debug for Tokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Tokens(<redacted>)")
    }
}

pub const MAX_BODY: usize = 64 * 1024;
pub const MAX_TOKEN: usize = 16 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum TokenError {
    #[error("OAuth authorization is no longer valid; reconnect")]
    Reconnect,
    #[error("OAuth provider is temporarily unavailable")]
    Transient,
    #[error("OAuth provider refused the token request")]
    Refused,
}

pub fn parse(
    response: &HttpResponse,
    mut binding: OAuthState,
    scopes: &BTreeSet<String>,
    now: u64,
) -> Result<Tokens> {
    let body = json_body(response)?;
    response_error(response.status, &body)?;
    let access = string(&body, "access_token")?;
    if !bearer(access) || !string(&body, "token_type")?.eq_ignore_ascii_case("Bearer") {
        bail!("OAuth requires a valid nonempty Bearer access token");
    }
    let expires_at_millis = body
        .get("expires_in")
        .map(|expiry| {
            expiry
                .as_u64()
                .and_then(|n| n.checked_mul(1000))
                .and_then(|n| now.checked_add(n))
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "OAuth expires_in must be a nonnegative integer within the clock range"
                    )
                })
        })
        .transpose()?;
    let authority = match body.get("scope") {
        None => scopes.clone(),
        Some(value) => parse_scopes(
            value
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("OAuth scope must be a string"))?,
        )?,
    };
    if body.get("refresh_token").is_some() {
        let token = string(&body, "refresh_token")?;
        if token.is_empty()
            || token.len() > MAX_TOKEN
            || !token.bytes().all(|b| (0x20..=0x7e).contains(&b))
        {
            bail!("OAuth refresh token is empty, malformed, or exceeds its bound");
        }
        binding.refresh_token = Some(token.into());
    }
    binding.reconnect_required = false;
    Ok(Tokens {
        values: Answers::from([("access_token".into(), access.into())]),
        authority,
        expires_at_millis,
        private: binding,
    })
}

pub fn json_body(response: &HttpResponse) -> Result<serde_json::Value> {
    if response.body.len() > MAX_BODY {
        bail!("OAuth response exceeds 65536 bytes");
    }
    serde_json::from_slice(&response.body)
        .map_err(|_| anyhow::anyhow!("OAuth provider returned malformed JSON"))
}

pub fn response_error(status: u16, body: &serde_json::Value) -> Result<()> {
    if status >= 500 || status == 429 {
        return Err(TokenError::Transient.into());
    }
    if let Some(error) = body.get("error") {
        return Err(match error.as_str() {
            Some("invalid_grant" | "invalid_client" | "unauthorized_client") => {
                TokenError::Reconnect
            }
            Some("temporarily_unavailable" | "server_error") => TokenError::Transient,
            _ => TokenError::Refused,
        }
        .into());
    }
    if !(200..300).contains(&status) {
        return Err(TokenError::Refused.into());
    }
    Ok(())
}

pub fn string<'a>(body: &'a serde_json::Value, key: &str) -> Result<&'a str> {
    body.get(key)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("OAuth response is missing a valid {key}"))
}

fn parse_scopes(value: &str) -> Result<BTreeSet<String>> {
    if value.is_empty() {
        return Ok(BTreeSet::new());
    }
    let scopes: Vec<_> = value.split(' ').collect();
    if scopes.len() > 128
        || scopes
            .iter()
            .any(|scope| !lns_artifact::connector::oauth::scope_token(scope))
    {
        bail!("OAuth response contains malformed scopes");
    }
    Ok(scopes.into_iter().map(str::to_string).collect())
}

fn bearer(value: &str) -> bool {
    let unpadded = value.trim_end_matches('=');
    !unpadded.is_empty()
        && value.len() <= MAX_TOKEN
        && unpadded
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._~+/".contains(&b))
}

pub fn form(endpoint: &str, fields: &[(&str, &str)]) -> HttpRequest {
    HttpRequest {
        method: "POST".into(),
        url: endpoint.into(),
        headers: vec![
            ("Accept".into(), "application/json".into()),
            (
                "Content-Type".into(),
                "application/x-www-form-urlencoded".into(),
            ),
        ],
        body: form_urlencoded::Serializer::new(String::new())
            .extend_pairs(fields.iter().copied())
            .finish()
            .into_bytes(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn binding() -> OAuthState {
        OAuthState {
            client_id: "public-id".into(),
            token_endpoint: "https://auth.example/token".into(),
            kind: "oauth_device".into(),
            refresh_token: Some("old-refresh".into()),
            reconnect_required: false,
        }
    }
    fn response(body: serde_json::Value) -> HttpResponse {
        HttpResponse {
            status: 200,
            headers: vec![],
            body: body.to_string().into_bytes(),
        }
    }
    #[test]
    fn oversized_token_responses_are_refused_before_parsing_or_projection() {
        let response = HttpResponse {
            status: 200,
            headers: vec![],
            body: vec![b'x'; MAX_BODY + 1],
        };
        assert!(
            parse(&response, binding(), &BTreeSet::new(), 0)
                .unwrap_err()
                .to_string()
                .contains("65536")
        );
    }

    #[test]
    fn oauth_http_debug_never_prints_token_bodies_or_authorization_parameters() {
        let request = form(
            "https://auth.example/token?code=sensitive",
            &[("refresh_token", "sensitive")],
        );
        assert!(!format!("{request:?}").contains("sensitive"));
        assert!(
            !format!("{:?}", response(json!({"access_token":"sensitive"}))).contains("sensitive")
        );
    }

    #[test]
    fn oauth_tokens_keep_renewal_private_and_preserve_omitted_scopes() {
        let prior = BTreeSet::from(["read".into()]);
        let parsed = parse(
            &response(json!({"access_token":"access","token_type":"bearer","expires_in":60})),
            binding(),
            &prior,
            1000,
        )
        .unwrap();
        assert_eq!(
            parsed.values,
            Answers::from([("access_token".into(), "access".into())])
        );
        assert_eq!(parsed.authority, prior);
        assert_eq!(parsed.expires_at_millis, Some(61000));
        assert_eq!(parsed.private.refresh_token.as_deref(), Some("old-refresh"));
        let rotated = parse(&response(json!({"access_token":"new","token_type":"Bearer","scope":"write read","refresh_token":"rotated"})), binding(), &prior, 1000).unwrap();
        assert_eq!(rotated.private.refresh_token.as_deref(), Some("rotated"));
        assert_eq!(
            rotated.authority,
            BTreeSet::from(["read".into(), "write".into()])
        );
        assert_eq!(rotated.expires_at_millis, None);
    }
    #[test]
    fn oauth_form_encodes_values_and_explicitly_requests_json() {
        let request = form(
            "https://auth.example/token",
            &[("client_id", "id+&="), ("scope", "read write")],
        );
        assert_eq!(request.body, b"client_id=id%2B%26%3D&scope=read+write");
        assert!(
            request
                .headers
                .contains(&("Accept".into(), "application/json".into()))
        );
        assert!(request.headers.contains(&(
            "Content-Type".into(),
            "application/x-www-form-urlencoded".into()
        )));
    }
    #[test]
    fn oauth_refuses_malformed_tokens_expiry_and_scope_without_echoing_secrets() {
        let valid = json!({"access_token":"sensitive","token_type":"Bearer"});
        for (key, value) in [
            ("access_token", json!("")),
            ("access_token", json!("a b")),
            ("access_token", json!("a\\r\\n")),
            ("access_token", json!("=a")),
            ("access_token", json!("é")),
            ("token_type", json!("MAC")),
            ("token_type", json!(null)),
            ("expires_in", json!(-1)),
            ("expires_in", json!(1.5)),
            ("expires_in", json!("60")),
            ("expires_in", json!(null)),
            ("expires_in", json!(u64::MAX)),
            ("scope", json!(["read"])),
            ("scope", json!("read  write")),
            ("scope", json!("read\\twrite")),
            ("refresh_token", json!("")),
        ] {
            let mut body = valid.clone();
            body[key] = value;
            let err = parse(&response(body), binding(), &BTreeSet::new(), 1000).unwrap_err();
            assert!(!err.to_string().contains("sensitive"));
        }
        assert!(
            parse(
                &response(json!({"access_token":"a","token_type":"Bearer","expires_in":1})),
                binding(),
                &BTreeSet::new(),
                u64::MAX
            )
            .is_err()
        );
        for status in [302, 400, 401, 500] {
            let mut reply = response(valid.clone());
            reply.status = status;
            assert!(parse(&reply, binding(), &BTreeSet::new(), 0).is_err());
        }
    }
    #[test]
    fn oauth_nonrenewable_and_explicit_empty_scope_are_not_invented() {
        let mut initial = binding();
        initial.refresh_token = None;
        let parsed=parse(&response(json!({"access_token":"a.b_c-~/+=","token_type":"Bearer","scope":"","expires_in":0})),initial,&BTreeSet::from(["read".into()]),0).unwrap();
        assert!(parsed.private.refresh_token.is_none());
        assert!(parsed.authority.is_empty());
        assert_eq!(parsed.expires_at_millis, Some(0));
        assert!(!format!("{:?}", binding()).contains("old-refresh"));
        assert!(!format!("{parsed:?}").contains("a.b_c"));
    }
}
