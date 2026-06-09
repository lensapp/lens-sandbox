use std::time::Duration;

use anyhow::Result;
use futures_util::future::BoxFuture;

use lns_policy::integrations::OauthAuth;

/// The slice of an integration's oauth block the device flow needs to talk to the provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OauthConfig {
    pub client_id: String,
    pub scopes: Vec<String>,
    pub device_authorization_endpoint: String,
    pub token_endpoint: String,
}

impl From<&OauthAuth> for OauthConfig {
    fn from(o: &OauthAuth) -> Self {
        Self {
            client_id: o.client_id.clone(),
            scopes: o.scopes.clone(),
            device_authorization_endpoint: o.device_authorization_endpoint.clone(),
            token_endpoint: o.token_endpoint.clone(),
        }
    }
}

/// A provider's response to a device-authorization request (RFC 8628 §3.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub interval: Duration,
    pub expires_in: Duration,
}

/// An obtained or renewed grant from the token endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenSet {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: Duration,
}

/// The result of one poll of the token endpoint while a device sign-in is pending (RFC 8628 §3.5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PollOutcome {
    Pending,
    SlowDown,
    Token(TokenSet),
    Denied,
    Expired,
}

/// The provider-facing side of the device flow, injected so the orchestration is host-tested without real network.
pub trait DeviceFlow: Send + Sync {
    fn request_device_code<'a>(&'a self, cfg: &'a OauthConfig)
    -> BoxFuture<'a, Result<DeviceCode>>;
    fn poll_token<'a>(
        &'a self,
        cfg: &'a OauthConfig,
        device_code: &'a str,
    ) -> BoxFuture<'a, Result<PollOutcome>>;
    fn refresh<'a>(
        &'a self,
        cfg: &'a OauthConfig,
        refresh_token: &'a str,
    ) -> BoxFuture<'a, Result<TokenSet>>;
}

/// Wall-clock source for token-expiry arithmetic, injected so refresh decisions are deterministic in tests.
pub trait Clock: Send + Sync {
    fn now_unix(&self) -> u64;
}
