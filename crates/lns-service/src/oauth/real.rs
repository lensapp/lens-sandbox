use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use futures_util::future::BoxFuture;
use serde::Deserialize;
use serde::de::DeserializeOwned;

use super::traits::{Clock, DeviceCode, DeviceFlow, OauthConfig, PollOutcome, TokenSet};

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
