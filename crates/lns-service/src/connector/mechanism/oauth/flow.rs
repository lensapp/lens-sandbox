use anyhow::{Result, bail};
use base64::Engine;
use lns_artifact::connector::oauth::{OAuth, Redirect};
use lns_ipc::OAuthProgress;
use serde::{Deserialize, Serialize};
use sha2::Digest;

use super::browser::{Browser, Callback};
use super::token::{self, OAuthState};
use super::{Advance, Pending};
use crate::connector::mechanism::host::Host;
use crate::connector::mechanism::{CallError, HttpResponse, Outcome};

pub struct Native {
    pub config: OAuth,
    pub browser: std::sync::Arc<dyn Browser>,
}

#[derive(Serialize, Deserialize)]
struct State {
    owner: String,
    deadline: u64,
    next: u64,
    phase: Phase,
    progress: OAuthProgress,
}

#[derive(Serialize, Deserialize)]
enum Phase {
    Start,
    Device {
        code: String,
        interval: u64,
        url: String,
    },
    Code {
        handle: String,
        redirect: String,
        verifier: String,
        url: String,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum FlowError {
    #[error("OAuth authorization expired; connect again")]
    Expired,
    #[error("OAuth authorization was denied")]
    Denied,
}

impl Native {
    pub fn start(&self, host: &Host, now: u64) -> Result<Pending> {
        let endpoint = match &self.config {
            OAuth::Device {
                device_authorization_endpoint,
                ..
            } => device_authorization_endpoint,
            OAuth::AuthorizationCode {
                authorization_endpoint,
                ..
            } => authorization_endpoint,
        };
        pending(&State {
            owner: random(host)?,
            deadline: now.checked_add(900_000).ok_or(FlowError::Expired)?,
            next: now,
            phase: Phase::Start,
            progress: OAuthProgress::Starting {
                destinations: vec![endpoint.clone(), self.config.token_endpoint().into()],
                scopes: self.config.scopes().to_vec(),
            },
        })
    }

    pub fn advance(&self, host: &Host, bytes: &[u8], now: u64) -> Result<Advance> {
        let mut state = read_state(bytes)?;
        if now >= state.deadline {
            self.cancel(bytes);
            return Err(FlowError::Expired.into());
        }
        if now < state.next {
            return pending(&state).map(Advance::Pending);
        }
        match &mut state.phase {
            Phase::Start => {
                self.begin(host, &mut state, now)?;
                pending(&state).map(Advance::Pending)
            }
            Phase::Device { code, interval, .. } => {
                match self.poll_device(host, code, interval, now)? {
                    Some(outcome) => Ok(Advance::Done(outcome)),
                    None => {
                        state.next = now.saturating_add(*interval);
                        pending(&state).map(Advance::Pending)
                    }
                }
            }
            Phase::Code {
                handle,
                redirect,
                verifier,
                ..
            } => match self.browser.poll(&state.owner, handle, now)? {
                Callback::Ignore => {
                    state.next = now.saturating_add(1000);
                    pending(&state).map(Advance::Pending)
                }
                Callback::Denied => {
                    self.browser.cancel(handle);
                    Err(FlowError::Denied.into())
                }
                Callback::Failed => {
                    self.browser.cancel(handle);
                    Err(token::TokenError::Refused.into())
                }
                Callback::Code(code) => {
                    self.browser.cancel(handle);
                    let response = fetch(
                        host,
                        self.config.token_endpoint(),
                        &[
                            ("grant_type", "authorization_code"),
                            ("client_id", self.config.client_id()),
                            ("redirect_uri", redirect),
                            ("code_verifier", verifier),
                            ("code", &code),
                        ],
                    )?;
                    self.complete(&response, now).map(Advance::Done)
                }
            },
        }
    }

    fn begin(&self, host: &Host, state: &mut State, now: u64) -> Result<()> {
        match &self.config {
            OAuth::Device {
                device_authorization_endpoint,
                verification_hosts,
                ..
            } => {
                let response = fetch(
                    host,
                    device_authorization_endpoint,
                    &[
                        ("client_id", self.config.client_id()),
                        ("scope", &self.config.scopes().join(" ")),
                    ],
                )?;
                let body = token::json_body(&response)?;
                token::response_error(response.status, &body)?;
                let code = bounded_string(&body, "device_code", token::MAX_TOKEN)?;
                let user_code = bounded_string(&body, "user_code", 256)?;
                let verification_uri = token::string(&body, "verification_uri")?;
                verify_url(verification_uri, verification_hosts)?;
                let url = match body.get("verification_uri_complete") {
                    Some(_) => token::string(&body, "verification_uri_complete")?,
                    None => verification_uri,
                };
                verify_url(url, verification_hosts)?;
                let interval = body
                    .get("interval")
                    .map(seconds)
                    .transpose()?
                    .unwrap_or(5000);
                let expiry =
                    seconds(body.get("expires_in").ok_or_else(|| {
                        anyhow::anyhow!("OAuth device response has no expires_in")
                    })?)?;
                state.deadline = state
                    .deadline
                    .min(now.checked_add(expiry).ok_or(FlowError::Expired)?);
                state.next = now.saturating_add(interval);
                state.progress = OAuthProgress::DeviceAuthorization {
                    verification_uri: verification_uri.into(),
                    user_code: user_code.into(),
                };
                state.phase = Phase::Device {
                    code: code.into(),
                    interval,
                    url: url.into(),
                };
                self.browser.open(url)?;
            }
            OAuth::AuthorizationCode {
                authorization_endpoint,
                redirect,
                ..
            } => {
                let verifier = random(host)?;
                let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .encode(sha2::Sha256::digest(verifier.as_bytes()));
                let Redirect::Loopback { path, port } = redirect;
                let (handle, actual) = self.browser.prepare(
                    &state.owner,
                    path,
                    *port,
                    &state.owner,
                    state.deadline,
                )?;
                let mut url = lns_artifact::connector::oauth::endpoint(authorization_endpoint)?;
                url.query_pairs_mut().extend_pairs([
                    ("response_type", "code"),
                    ("client_id", self.config.client_id()),
                    ("redirect_uri", actual.as_str()),
                    ("state", state.owner.as_str()),
                    ("code_challenge_method", "S256"),
                    ("code_challenge", challenge.as_str()),
                    ("scope", &self.config.scopes().join(" ")),
                ]);
                state.progress = OAuthProgress::WaitingForBrowser {
                    authorization_endpoint: authorization_endpoint.clone(),
                    redirect_uri: actual.clone(),
                };
                state.phase = Phase::Code {
                    handle: handle.clone(),
                    redirect: actual,
                    verifier,
                    url: url.to_string(),
                };
                state.next = now.saturating_add(1000);
                if let Err(error) = pending(state).and_then(|_| self.browser.open(url.as_str())) {
                    self.browser.cancel(&handle);
                    return Err(error);
                }
            }
        }
        Ok(())
    }

    fn poll_device(
        &self,
        host: &Host,
        code: &str,
        interval: &mut u64,
        now: u64,
    ) -> Result<Option<Outcome>> {
        let request = token::form(
            self.config.token_endpoint(),
            &[
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("client_id", self.config.client_id()),
                ("device_code", code),
            ],
        );
        let response = match host.fetch(&request) {
            Ok(response) => response,
            Err(CallError::Failed(_)) => {
                *interval = interval.saturating_mul(2);
                return Ok(None);
            }
            Err(CallError::Refused(_)) => bail!("OAuth device request was refused by the host"),
        };
        let body = token::json_body(&response)?;
        match body.get("error").and_then(serde_json::Value::as_str) {
            Some("authorization_pending") => {}
            Some("slow_down") => *interval = interval.saturating_add(5000),
            Some("access_denied") => return Err(FlowError::Denied.into()),
            Some("expired_token") => return Err(FlowError::Expired.into()),
            _ => return self.complete(&response, now).map(Some),
        }
        if !matches!(response.status, 200 | 400) {
            bail!("OAuth provider returned an invalid polling status");
        }
        Ok(None)
    }

    fn binding(&self) -> OAuthState {
        OAuthState {
            client_id: self.config.client_id().into(),
            token_endpoint: self.config.token_endpoint().into(),
            kind: match self.config {
                OAuth::Device { .. } => "oauth_device",
                OAuth::AuthorizationCode { .. } => "oauth_authorization_code",
            }
            .into(),
            refresh_token: None,
            reconnect_required: false,
        }
    }

    fn complete(&self, response: &HttpResponse, now: u64) -> Result<Outcome> {
        let tokens = token::parse(
            response,
            self.binding(),
            &self.config.scopes().iter().cloned().collect(),
            now,
        )?;
        Ok(outcome(tokens))
    }

    pub fn refresh(
        &self,
        host: &Host,
        connection: &crate::connector::store::Connection,
        now: u64,
    ) -> Result<Outcome> {
        let held = connection
            .oauth
            .as_ref()
            .ok_or(token::TokenError::Reconnect)?;
        let expected = self.binding();
        if held.reconnect_required
            || held.client_id != expected.client_id
            || held.token_endpoint != expected.token_endpoint
            || held.kind != expected.kind
        {
            return Err(token::TokenError::Reconnect.into());
        }
        let refresh = held
            .refresh_token
            .as_deref()
            .ok_or(token::TokenError::Reconnect)?;
        let response = fetch(
            host,
            &held.token_endpoint,
            &[
                ("grant_type", "refresh_token"),
                ("client_id", &held.client_id),
                ("refresh_token", refresh),
            ],
        )?;
        token::parse(&response, held.clone(), &connection.authority.0, now).map(outcome)
    }

    pub fn cancel(&self, bytes: &[u8]) {
        cancel(self.browser.as_ref(), bytes);
    }

    pub fn open_browser(&self, bytes: &[u8]) -> Result<()> {
        match read_state(bytes)?.phase {
            Phase::Device { url, .. } | Phase::Code { url, .. } => self.browser.open(&url),
            Phase::Start => bail!("OAuth is still preparing authorization"),
        }
    }
}

pub fn cancel(browser: &dyn Browser, bytes: &[u8]) {
    if let Ok(State {
        phase: Phase::Code { handle, .. },
        ..
    }) = read_state(bytes)
    {
        browser.cancel(&handle);
    }
}

fn outcome(tokens: token::Tokens) -> Outcome {
    Outcome {
        values: tokens.values,
        authority: tokens.authority,
        expires_at_millis: tokens.expires_at_millis,
        oauth: Some(tokens.private),
    }
}

fn fetch(host: &Host, endpoint: &str, fields: &[(&str, &str)]) -> Result<HttpResponse> {
    let request = token::form(endpoint, fields);
    if request.body.len() > token::MAX_BODY {
        bail!("OAuth request exceeds 65536 bytes");
    }
    host.fetch(&request)
        .map_err(|_| token::TokenError::Transient.into())
}

fn random(host: &Host) -> Result<String> {
    let bytes = host.bytes(32);
    if bytes.len() != 32 {
        bail!("OAuth could not obtain enough entropy");
    }
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

fn read_state(bytes: &[u8]) -> Result<State> {
    if bytes.len() > token::MAX_BODY {
        bail!("OAuth session exceeds 65536 bytes");
    }
    serde_json::from_slice(bytes).map_err(|_| anyhow::anyhow!("OAuth session is unavailable"))
}

fn pending(state: &State) -> Result<Pending> {
    let bytes = serde_json::to_vec(&state)?;
    if bytes.len() > token::MAX_BODY {
        bail!("OAuth session exceeds 65536 bytes");
    }
    Ok(Pending {
        progress: state.progress.clone(),
        state: bytes,
        next_at_millis: state.next.min(state.deadline),
        expires_at_millis: state.deadline,
    })
}

fn seconds(value: &serde_json::Value) -> Result<u64> {
    value
        .as_u64()
        .filter(|v| *v > 0)
        .and_then(|v| v.checked_mul(1000))
        .ok_or_else(|| {
            anyhow::anyhow!("OAuth device lifetime and interval must be positive whole seconds")
        })
}

fn bounded_string<'a>(body: &'a serde_json::Value, key: &str, limit: usize) -> Result<&'a str> {
    let value = token::string(body, key)?;
    if value.is_empty() || value.len() > limit || !value.bytes().all(|b| (0x21..=0x7e).contains(&b))
    {
        bail!("OAuth {key} is empty, malformed, or exceeds its bound");
    }
    Ok(value)
}

fn verify_url(value: &str, hosts: &[String]) -> Result<()> {
    let url = lns_artifact::connector::oauth::https_url(value)?;
    if !hosts
        .iter()
        .filter_map(|h| lns_artifact::connector::oauth::verification_host(h).ok())
        .any(|h| {
            h.host_str() == url.host_str()
                && h.port_or_known_default() == url.port_or_known_default()
        })
    {
        bail!("OAuth verification URL is outside verificationHosts");
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests;

impl crate::connector::mechanism::traits::Mechanism for Native {
    fn native(&self) -> Option<&Self> {
        Some(self)
    }
    fn connect(&self, _: &Host, _: u64) -> Result<crate::connector::mechanism::Step> {
        bail!("native OAuth requires an interactive operation")
    }
    fn resume(
        &self,
        _: &Host,
        _: &[u8],
        _: &crate::connector::mechanism::Answers,
        _: u64,
    ) -> Result<crate::connector::mechanism::Step> {
        bail!("native OAuth status does not accept answers")
    }
    fn refresh(
        &self,
        _: &Host,
        _: &crate::connector::mechanism::Answers,
        _: u64,
    ) -> Result<Outcome> {
        Err(token::TokenError::Reconnect.into())
    }
    fn refresh_private(
        &self,
        host: &Host,
        connection: &crate::connector::store::Connection,
        now: u64,
    ) -> Result<Outcome> {
        self.refresh(host, connection, now)
    }
    fn revoke(&self, _: &Host, _: &crate::connector::mechanism::Answers, _: u64) -> Result<()> {
        Ok(())
    }
}
