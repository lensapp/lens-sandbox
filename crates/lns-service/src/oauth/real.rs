use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use futures_util::future::BoxFuture;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::traits::{
    AuthCodeFlow, CallbackHandle, CallbackListener, CallbackParams, Clock, DeviceCode, DeviceFlow,
    OauthConfig, PkceConfig, PollOutcome, TokenSet,
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

pub struct RealDeviceFlow;

#[derive(Deserialize)]
struct DeviceCodeResp {
    device_code: String,
    user_code: String,
    verification_uri: String,
    interval: Option<u64>,
    expires_in: Option<u64>,
}

#[derive(Deserialize)]
struct TokenResp {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    error: Option<String>,
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
                &[
                    ("client_id", cfg.client_id.as_str()),
                    ("device_code", device_code),
                    ("grant_type", DEVICE_CODE_GRANT),
                ],
            )
            .await?;
            let t: TokenResp = parse_json(&cfg.token_endpoint, status, &bytes)?;
            if let Some(access_token) = t.access_token {
                return Ok(PollOutcome::Token(TokenSet {
                    access_token,
                    refresh_token: t.refresh_token.unwrap_or_default(),
                    expires_in: Duration::from_secs(t.expires_in.unwrap_or(DEFAULT_EXPIRES_IN)),
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
                &[
                    ("client_id", cfg.client_id.as_str()),
                    ("grant_type", "refresh_token"),
                    ("refresh_token", refresh_token),
                ],
            )
            .await?;
            let t: TokenResp = parse_json(&cfg.token_endpoint, status, &bytes)?;
            match t.access_token {
                Some(access_token) => Ok(TokenSet {
                    access_token,
                    refresh_token: t.refresh_token.unwrap_or_else(|| refresh_token.to_string()),
                    expires_in: Duration::from_secs(t.expires_in.unwrap_or(DEFAULT_EXPIRES_IN)),
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
