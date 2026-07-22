use std::time::Duration;

use anyhow::Result;
use futures_util::future::BoxFuture;

use lns_policy::connectors::OauthAuth;

/// The slice of a connector's oauth block the device flow needs to talk to the provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OauthConfig {
    pub client_id: String,
    pub client_secret: String,
    pub scopes: Vec<String>,
    pub device_authorization_endpoint: String,
    pub token_endpoint: String,
    pub userinfo_endpoint: Option<String>,
    pub account_field: Option<String>,
}

impl From<&OauthAuth> for OauthConfig {
    fn from(o: &OauthAuth) -> Self {
        Self {
            client_id: o.client_id.clone().unwrap_or_default(),
            client_secret: o.client_secret.clone().unwrap_or_default(),
            scopes: o.scopes.clone(),
            device_authorization_endpoint: o
                .device_authorization_endpoint
                .clone()
                .unwrap_or_default(),
            token_endpoint: o.token_endpoint.clone(),
            userinfo_endpoint: o.userinfo_endpoint.clone(),
            account_field: o.account_field.clone(),
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
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TokenSet {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: Duration,
    pub scopes: Vec<String>,
    pub account: Option<String>,
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

/// Fetches a provider's userinfo document for an armed access token, injected so account resolution is host-tested without real network.
pub trait UserInfoFetcher: Send + Sync {
    fn fetch<'a>(&'a self, url: &'a str, access_token: &'a str) -> BoxFuture<'a, Result<Vec<u8>>>;
}

/// The slice of a connector's oauth block the PKCE authorization-code flow needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PkceConfig {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub scopes: Vec<String>,
}

impl From<&OauthAuth> for PkceConfig {
    fn from(o: &OauthAuth) -> Self {
        Self {
            authorization_endpoint: o.authorization_endpoint.clone().unwrap_or_default(),
            token_endpoint: o.token_endpoint.clone(),
            scopes: o.scopes.clone(),
        }
    }
}

/// The query parameters a provider returns to the loopback callback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallbackParams {
    pub code: String,
    pub state: String,
}

/// The provider-facing side of the PKCE code exchange, injected so the orchestration is host-tested without real network.
pub trait AuthCodeFlow: Send + Sync {
    /// Exchanges an authorization `code` plus its PKCE `verifier` for the provider's durable key.
    fn exchange_code<'a>(
        &'a self,
        cfg: &'a PkceConfig,
        code: &'a str,
        verifier: &'a str,
    ) -> BoxFuture<'a, Result<String>>;
}

/// A transient loopback callback listener bound for one PKCE sign-in, injected so the orchestration is host-tested without a real socket.
pub trait CallbackListener: Send + Sync {
    /// Binds a loopback listener and reports the URL to register as `callback_url`, or fails closed if it can't bind.
    fn bind(&self) -> BoxFuture<'_, Result<Box<dyn CallbackHandle>>>;
}

/// A bound listener awaiting the single browser redirect.
pub trait CallbackHandle: Send {
    fn redirect_url(&self) -> &str;
    fn wait(self: Box<Self>) -> BoxFuture<'static, Result<CallbackParams>>;
}
