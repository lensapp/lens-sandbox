use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use futures_util::future::BoxFuture;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::traits::{
    AuthCodeFlow, CallbackHandle, CallbackListener, CallbackParams, Clock, DeviceCode, DeviceFlow,
    OauthConfig, PkceConfig, PollOutcome, TokenSet, UserInfoFetcher,
};

const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
const DEFAULT_INTERVAL: u64 = 5;
const DEFAULT_EXPIRES_IN: u64 = 3600;
const DEVICE_CODE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

pub struct RealClock;

impl Clock for RealClock {
    fn now_unix(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }
}

pub struct RealUserInfoFetcher;

impl UserInfoFetcher for RealUserInfoFetcher {
    fn fetch<'a>(&'a self, url: &'a str, access_token: &'a str) -> BoxFuture<'a, Result<Vec<u8>>> {
        Box::pin(async move {
            let resp = reqwest::Client::new()
                .get(url)
                .header("accept", "application/json")
                .header("user-agent", "lns")
                .bearer_auth(access_token)
                .timeout(HTTP_TIMEOUT)
                .send()
                .await
                .with_context(|| format!("GET {url}"))?
                .error_for_status()
                .with_context(|| format!("userinfo HTTP error from {url}"))?;
            let bytes = resp
                .bytes()
                .await
                .with_context(|| format!("reading userinfo body from {url}"))?;
            Ok(bytes.to_vec())
        })
    }
}

pub struct RealDeviceFlow;

#[derive(Deserialize)]
struct DeviceCodeResp {
    device_code: String,
    user_code: String,
    #[serde(alias = "verification_url")]
    verification_uri: String,
    interval: Option<u64>,
    expires_in: Option<u64>,
}

#[derive(Deserialize)]
struct TokenResp {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    scope: Option<String>,
    error: Option<String>,
}

fn granted_scopes(scope: Option<&str>) -> Vec<String> {
    scope
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_string)
        .collect()
}

/// reqwest here is built without the `json` feature, so bodies are read as bytes and parsed with serde_json (the crate-wide pattern).
async fn post_form_bytes(
    url: &str,
    form: &[(&str, &str)],
) -> Result<(reqwest::StatusCode, Vec<u8>)> {
    let resp = reqwest::Client::new()
        .post(url)
        .header("accept", "application/json")
        .form(form)
        .timeout(HTTP_TIMEOUT)
        .send()
        .await
        .with_context(|| format!("POST {url}"))?;
    let status = resp.status();
    let bytes = resp
        .bytes()
        .await
        .with_context(|| format!("reading body from {url}"))?;
    Ok((status, bytes.to_vec()))
}

fn parse_json<T: DeserializeOwned>(
    url: &str,
    status: reqwest::StatusCode,
    bytes: &[u8],
) -> Result<T> {
    serde_json::from_slice(bytes)
        .with_context(|| format!("parsing response from {url} (HTTP {status})"))
}

/// Confidential device clients (e.g. Google) must send `client_secret` in the token exchange; public clients (e.g. GitHub) leave it empty and omit it.
fn with_client_secret<'a>(
    mut form: Vec<(&'a str, &'a str)>,
    secret: &'a str,
) -> Vec<(&'a str, &'a str)> {
    if !secret.is_empty() {
        form.push(("client_secret", secret));
    }
    form
}

impl DeviceFlow for RealDeviceFlow {
    fn request_device_code<'a>(
        &'a self,
        cfg: &'a OauthConfig,
    ) -> BoxFuture<'a, Result<DeviceCode>> {
        Box::pin(async move {
            let scope = cfg.scopes.join(" ");
            let (status, bytes) = post_form_bytes(
                &cfg.device_authorization_endpoint,
                &[("client_id", cfg.client_id.as_str()), ("scope", &scope)],
            )
            .await?;
            if !status.is_success() {
                bail!(
                    "device authorization endpoint {} returned HTTP {status}: {}",
                    cfg.device_authorization_endpoint,
                    String::from_utf8_lossy(&bytes)
                );
            }
            let d: DeviceCodeResp = parse_json(&cfg.device_authorization_endpoint, status, &bytes)?;
            Ok(DeviceCode {
                device_code: d.device_code,
                user_code: d.user_code,
                verification_uri: d.verification_uri,
                interval: Duration::from_secs(d.interval.unwrap_or(DEFAULT_INTERVAL)),
                expires_in: Duration::from_secs(d.expires_in.unwrap_or(DEFAULT_EXPIRES_IN)),
            })
        })
    }

    fn poll_token<'a>(
        &'a self,
        cfg: &'a OauthConfig,
        device_code: &'a str,
    ) -> BoxFuture<'a, Result<PollOutcome>> {
        Box::pin(async move {
            let (status, bytes) = post_form_bytes(
                &cfg.token_endpoint,
                &with_client_secret(
                    vec![
                        ("client_id", cfg.client_id.as_str()),
                        ("device_code", device_code),
                        ("grant_type", DEVICE_CODE_GRANT),
                    ],
                    &cfg.client_secret,
                ),
            )
            .await?;
            let t: TokenResp = parse_json(&cfg.token_endpoint, status, &bytes)?;
            if let Some(access_token) = t.access_token {
                return Ok(PollOutcome::Token(TokenSet {
                    access_token,
                    refresh_token: t.refresh_token.unwrap_or_default(),
                    expires_in: Duration::from_secs(t.expires_in.unwrap_or(DEFAULT_EXPIRES_IN)),
                    scopes: granted_scopes(t.scope.as_deref()),
                    account: None,
                }));
            }
            match t.error.as_deref() {
                Some("authorization_pending") => Ok(PollOutcome::Pending),
                Some("slow_down") => Ok(PollOutcome::SlowDown),
                Some("access_denied") => Ok(PollOutcome::Denied),
                Some("expired_token") => Ok(PollOutcome::Expired),
                other => bail!(
                    "device token poll failed: {}",
                    other.unwrap_or("no token and no error in response")
                ),
            }
        })
    }

    fn refresh<'a>(
        &'a self,
        cfg: &'a OauthConfig,
        refresh_token: &'a str,
    ) -> BoxFuture<'a, Result<TokenSet>> {
        Box::pin(async move {
            let (status, bytes) = post_form_bytes(
                &cfg.token_endpoint,
                &with_client_secret(
                    vec![
                        ("client_id", cfg.client_id.as_str()),
                        ("grant_type", "refresh_token"),
                        ("refresh_token", refresh_token),
                    ],
                    &cfg.client_secret,
                ),
            )
            .await?;
            let t: TokenResp = parse_json(&cfg.token_endpoint, status, &bytes)?;
            match t.access_token {
                Some(access_token) => Ok(TokenSet {
                    access_token,
                    refresh_token: t.refresh_token.unwrap_or_else(|| refresh_token.to_string()),
                    expires_in: Duration::from_secs(t.expires_in.unwrap_or(DEFAULT_EXPIRES_IN)),
                    scopes: granted_scopes(t.scope.as_deref()),
                    account: None,
                }),
                None => bail!(
                    "refresh failed: {}",
                    t.error.as_deref().unwrap_or("no access token in response")
                ),
            }
        })
    }
}

pub struct RealAuthCodeFlow;

#[derive(Serialize)]
struct ExchangeReq<'a> {
    code: &'a str,
    code_verifier: &'a str,
    code_challenge_method: &'a str,
}

#[derive(Deserialize)]
struct ExchangeResp {
    key: Option<String>,
    error: Option<String>,
}

async fn post_json_bytes(
    url: &str,
    body: &impl Serialize,
) -> Result<(reqwest::StatusCode, Vec<u8>)> {
    let json = serde_json::to_vec(body).context("serializing pkce exchange request")?;
    let resp = reqwest::Client::new()
        .post(url)
        .header("accept", "application/json")
        .header("content-type", "application/json")
        .body(json)
        .timeout(HTTP_TIMEOUT)
        .send()
        .await
        .with_context(|| format!("POST {url}"))?;
    let status = resp.status();
    let bytes = resp
        .bytes()
        .await
        .with_context(|| format!("reading body from {url}"))?;
    Ok((status, bytes.to_vec()))
}

impl AuthCodeFlow for RealAuthCodeFlow {
    fn exchange_code<'a>(
        &'a self,
        cfg: &'a PkceConfig,
        code: &'a str,
        verifier: &'a str,
    ) -> BoxFuture<'a, Result<String>> {
        Box::pin(async move {
            let (status, bytes) = post_json_bytes(
                &cfg.token_endpoint,
                &ExchangeReq {
                    code,
                    code_verifier: verifier,
                    code_challenge_method: "S256",
                },
            )
            .await?;
            let r: ExchangeResp = parse_json(&cfg.token_endpoint, status, &bytes)?;
            match r.key {
                Some(key) if !key.is_empty() => Ok(key),
                _ => bail!(
                    "pkce code exchange at {} failed: {}",
                    cfg.token_endpoint,
                    r.error.as_deref().unwrap_or("no key in response")
                ),
            }
        })
    }
}

pub struct RealCallbackListener;

struct RealCallbackHandle {
    listener: tokio::net::TcpListener,
    redirect_url: String,
}

impl CallbackListener for RealCallbackListener {
    fn bind(&self) -> BoxFuture<'_, Result<Box<dyn CallbackHandle>>> {
        Box::pin(async move {
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
                .await
                .context("binding loopback pkce callback listener")?;
            let port = listener
                .local_addr()
                .context("reading pkce callback listener address")?
                .port();
            let redirect_url = format!("http://localhost:{port}/callback");
            Ok(Box::new(RealCallbackHandle {
                listener,
                redirect_url,
            }) as Box<dyn CallbackHandle>)
        })
    }
}

impl CallbackHandle for RealCallbackHandle {
    fn redirect_url(&self) -> &str {
        &self.redirect_url
    }

    fn wait(self: Box<Self>) -> BoxFuture<'static, Result<CallbackParams>> {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        Box::pin(async move {
            let (stream, _) = self
                .listener
                .accept()
                .await
                .context("accepting the pkce callback connection")?;
            let (read_half, mut write_half) = stream.into_split();
            let mut request_line = String::new();
            BufReader::new(read_half)
                .read_line(&mut request_line)
                .await
                .context("reading the pkce callback request line")?;
            let params = super::parse_callback_target(&request_line);
            let body = "<html><body>Signed in. You can close this tab and return to your terminal.</body></html>";
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = write_half.write_all(response.as_bytes()).await;
            let _ = write_half.flush().await;
            params
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_confidential_client_appends_its_secret_to_the_token_form() {
        let form = with_client_secret(vec![("client_id", "some-id")], "some-secret");
        assert_eq!(
            form,
            vec![("client_id", "some-id"), ("client_secret", "some-secret")]
        );
    }

    #[test]
    fn a_public_client_omits_the_secret_from_the_token_form() {
        let form = with_client_secret(vec![("client_id", "some-id")], "");
        assert_eq!(form, vec![("client_id", "some-id")]);
    }

    #[test]
    fn a_device_code_response_accepts_a_providers_nonstandard_verification_url_key() {
        let body = r#"{"device_code":"some-device","user_code":"SOME-CODE","verification_url":"https://example.com/device"}"#;
        let d: DeviceCodeResp = serde_json::from_str(body).unwrap();
        assert_eq!(d.verification_uri, "https://example.com/device");
    }

    #[test]
    fn a_device_code_response_accepts_the_rfc8628_verification_uri_key() {
        let body = r#"{"device_code":"some-device","user_code":"SOME-CODE","verification_uri":"https://example.com/device"}"#;
        let d: DeviceCodeResp = serde_json::from_str(body).unwrap();
        assert_eq!(d.verification_uri, "https://example.com/device");
    }
}
