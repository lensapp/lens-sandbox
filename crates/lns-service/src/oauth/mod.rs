//! Host-side interactive OAuth sign-in: the RFC 8628 device grant and the browser-redirect authorization-code + PKCE flow; the orchestration is here and unit-tested via fakes, while the reqwest/socket/SystemTime leaves live in `real.rs`.

mod real;
pub mod traits;

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Result, bail};
use base64::Engine;
use sha2::{Digest, Sha256};

use crate::credential_flow::store::{CredentialEntry, CredentialStateFile, CredentialStore};
pub use real::{
    RealAuthCodeFlow, RealCallbackListener, RealClock, RealDeviceFlow, RealUserInfoFetcher,
};
pub use traits::{
    AuthCodeFlow, CallbackHandle, CallbackListener, CallbackParams, Clock, DeviceCode, DeviceFlow,
    OauthConfig, PkceConfig, PollOutcome, TokenSet, UserInfoFetcher,
};

/// RFC 8628 §3.5: a `slow_down` response bumps the poll interval by 5 seconds.
const SLOW_DOWN_INCREMENT: Duration = Duration::from_secs(5);

/// What the user did on the in-flight sign-in card: abandon it, or pivot to a pasted token instead of finishing the browser dance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignInPivot {
    Cancel,
    UseToken(String),
}

/// The terminal result of driving a device sign-in to completion; `Token` is the user pivoting to a pasted fallback token mid-flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignIn {
    Completed(TokenSet),
    Token(String),
    Denied,
    Expired,
    Cancelled,
}

async fn sleep_then_poll(
    flow: &dyn DeviceFlow,
    cfg: &OauthConfig,
    device_code: &str,
    interval: Duration,
) -> Result<PollOutcome> {
    tokio::time::sleep(interval).await;
    flow.poll_token(cfg, device_code).await
}

/// Requests a device code, surfaces it to the user via `present`, then polls the token endpoint until the grant resolves or `cancel` fires.
pub async fn run_device_flow(
    flow: &dyn DeviceFlow,
    cfg: &OauthConfig,
    present: impl FnOnce(&DeviceCode),
    cancel: impl std::future::Future<Output = SignInPivot>,
) -> Result<SignIn> {
    let code = flow.request_device_code(cfg).await?;
    present(&code);
    let mut interval = code.interval;
    tokio::pin!(cancel);
    loop {
        tokio::select! {
            biased;
            pivot = &mut cancel => return Ok(match pivot {
                SignInPivot::Cancel => SignIn::Cancelled,
                SignInPivot::UseToken(value) => SignIn::Token(value),
            }),
            outcome = sleep_then_poll(flow, cfg, &code.device_code, interval) => {
                match outcome? {
                    PollOutcome::Pending => {}
                    PollOutcome::SlowDown => interval = interval.saturating_add(SLOW_DOWN_INCREMENT),
                    PollOutcome::Token(token) => return Ok(SignIn::Completed(token)),
                    PollOutcome::Denied => return Ok(SignIn::Denied),
                    PollOutcome::Expired => return Ok(SignIn::Expired),
                }
            }
        }
    }
}

/// A PKCE challenge: the random verifier, its base64url(SHA-256) `S256` challenge, and a CSRF `state` nonce.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PkceChallenge {
    pub verifier: String,
    pub challenge: String,
    pub state: String,
}

impl PkceChallenge {
    /// Draws a fresh high-entropy verifier and state and derives the verifier's S256 challenge.
    pub fn generate() -> Self {
        let verifier = random_token();
        let challenge = s256_challenge(&verifier);
        let state = random_token();
        Self {
            verifier,
            challenge,
            state,
        }
    }
}

fn random_token() -> String {
    use rand::RngCore;
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn s256_challenge(verifier: &str) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// Builds the provider authorization URL carrying the PKCE challenge and a `callback_url` that embeds the loopback `redirect_url` plus the `state` nonce, so the provider round-trips `state` by redirecting to that exact URL rather than by echoing a sibling param.
pub fn authorization_url(
    cfg: &PkceConfig,
    redirect_url: &str,
    challenge: &PkceChallenge,
) -> String {
    let scope = cfg.scopes.join(" ");
    let callback_url = format!(
        "{redirect_url}?{}",
        form_urlencoded::Serializer::new(String::new())
            .append_pair("state", &challenge.state)
            .finish()
    );
    let mut ser = form_urlencoded::Serializer::new(String::new());
    ser.append_pair("callback_url", &callback_url)
        .append_pair("code_challenge", &challenge.challenge)
        .append_pair("code_challenge_method", "S256");
    if !scope.is_empty() {
        ser.append_pair("scope", &scope);
    }
    format!("{}?{}", cfg.authorization_endpoint, ser.finish())
}

/// Extracts `code` and `state` from a loopback callback's HTTP request line (`GET /callback?code=…&state=… HTTP/1.1`).
pub(crate) fn parse_callback_target(request_line: &str) -> Result<CallbackParams> {
    let target = request_line.split_whitespace().nth(1).unwrap_or("");
    let query = target.split_once('?').map_or("", |(_, q)| q);
    let mut code = None;
    let mut state = None;
    for (k, v) in form_urlencoded::parse(query.as_bytes()) {
        match k.as_ref() {
            "code" => code = Some(v.into_owned()),
            "state" => state = Some(v.into_owned()),
            _ => {}
        }
    }
    match (code, state) {
        (Some(code), Some(state)) => Ok(CallbackParams { code, state }),
        _ => bail!("pkce callback missing code or state in {request_line:?}"),
    }
}

/// The terminal result of driving a PKCE sign-in to completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PkceSignIn {
    Completed(String),
    Cancelled,
    TimedOut,
}

/// Binds a loopback callback, hands the authorization URL to `on_authorization_url` (which presents it and opens the browser), and exchanges the returned code for the provider's key; cancel and timeout resolve without a key, while a state mismatch or a failed bind/exchange fail closed as `Err`.
pub async fn run_pkce_flow(
    flow: &dyn AuthCodeFlow,
    listener: &dyn CallbackListener,
    cfg: &PkceConfig,
    challenge: &PkceChallenge,
    on_authorization_url: impl FnOnce(&str),
    cancel: impl std::future::Future<Output = ()>,
    timeout: Duration,
) -> Result<PkceSignIn> {
    let handle = listener.bind().await?;
    let auth_url = authorization_url(cfg, handle.redirect_url(), challenge);
    on_authorization_url(&auth_url);
    tokio::pin!(cancel);
    let params = tokio::select! {
        biased;
        () = &mut cancel => return Ok(PkceSignIn::Cancelled),
        () = tokio::time::sleep(timeout) => return Ok(PkceSignIn::TimedOut),
        result = handle.wait() => result?,
    };
    if params.state != challenge.state {
        bail!(
            "pkce callback state {:?} did not match the issued nonce",
            params.state
        );
    }
    let key = flow
        .exchange_code(cfg, &params.code, &challenge.verifier)
        .await?;
    Ok(PkceSignIn::Completed(key))
}

/// Turns a freshly-obtained grant into the stored entry, stamping an absolute expiry off the injected clock.
pub fn entry_from_token(clock: &dyn Clock, token: &TokenSet) -> CredentialEntry {
    CredentialEntry::Oauth {
        access_token: token.access_token.clone(),
        refresh_token: token.refresh_token.clone(),
        expires_at: clock.now_unix().saturating_add(token.expires_in.as_secs()),
        scopes: token.scopes.clone(),
        account: token.account.clone(),
    }
}

/// Resolves the signed-in account from the integration's userinfo endpoint; best-effort, leaving `account` unset when the catalog declares no endpoint or the lookup fails.
pub async fn resolve_account(
    fetcher: &dyn UserInfoFetcher,
    cfg: &OauthConfig,
    mut token: TokenSet,
) -> TokenSet {
    let (Some(url), Some(field)) = (&cfg.userinfo_endpoint, &cfg.account_field) else {
        return token;
    };
    match fetcher.fetch(url, &token.access_token).await {
        Ok(bytes) => token.account = account_from_userinfo(&bytes, field),
        Err(e) => crate::log::debug!("userinfo lookup for account failed: {e:#}"),
    }
    token
}

fn account_from_userinfo(bytes: &[u8], field: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
    value.get(field)?.as_str().map(str::to_string)
}

/// Refreshes an oauth entry whose access token is within `skew_secs` of expiry; `Ok(None)` means no refresh was needed (or the entry isn't oauth), `Err` means the grant can no longer be refreshed.
pub async fn refresh_if_due(
    flow: &dyn DeviceFlow,
    clock: &dyn Clock,
    cfg: &OauthConfig,
    entry: &CredentialEntry,
    skew_secs: u64,
) -> Result<Option<CredentialEntry>> {
    let CredentialEntry::Oauth {
        refresh_token,
        expires_at,
        scopes: prev_scopes,
        account: prev_account,
        ..
    } = entry
    else {
        return Ok(None);
    };
    // A refresh-less grant can't be renewed (GitHub device-flow tokens are long-lived and ship no refresh token); leave it in place rather than attempting a refresh that fails and drops a still-valid token.
    if refresh_token.is_empty() {
        return Ok(None);
    }
    if *expires_at > clock.now_unix().saturating_add(skew_secs) {
        return Ok(None);
    }
    let token = flow.refresh(cfg, refresh_token).await?;
    let mut refreshed = entry_from_token(clock, &token);
    // A refresh response carries neither userinfo nor (usually) the scope grant, so keep the ones resolved at sign-in.
    if let CredentialEntry::Oauth {
        scopes, account, ..
    } = &mut refreshed
    {
        if scopes.is_empty() {
            scopes.clone_from(prev_scopes);
        }
        if account.is_none() {
            account.clone_from(prev_account);
        }
    }
    Ok(Some(refreshed))
}

/// Refreshes every due oauth grant in `state` (persisting through `store` if any changed) before a run's session begins serving; a grant that can no longer be refreshed is left in place for the next held request to re-prompt (scenario C).
pub async fn refresh_due_entries(
    state: &mut CredentialStateFile,
    oauth_configs: &HashMap<String, OauthConfig>,
    flow: &dyn DeviceFlow,
    clock: &dyn Clock,
    store: &dyn CredentialStore,
    skew_secs: u64,
) {
    let mut changed = false;
    for (id, cfg) in oauth_configs {
        let Some(entry) = state.get(id).cloned() else {
            continue;
        };
        match refresh_if_due(flow, clock, cfg, &entry, skew_secs).await {
            Ok(Some(refreshed)) => {
                state.insert(id.clone(), refreshed);
                changed = true;
            }
            Ok(None) => {}
            Err(e) => {
                // A grant that can't be refreshed is dead; drop it so the placeholder unarms and the next use re-prompts a fresh sign-in (scenario C).
                state.remove(id);
                changed = true;
                crate::log::warn!(
                    "oauth refresh for {id} failed ({e:#}); dropped the dead grant so the next use re-prompts"
                );
            }
        }
    }
    if changed && let Err(e) = store.save(state) {
        crate::log::warn!("persisting refreshed oauth tokens failed: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use anyhow::anyhow;
    use futures_util::future::BoxFuture;

    struct FakeDeviceFlow {
        code: DeviceCode,
        polls: Mutex<VecDeque<PollOutcome>>,
        refresh_result: Mutex<Option<Result<TokenSet>>>,
        seen_refresh: Mutex<Vec<String>>,
    }

    impl FakeDeviceFlow {
        fn polling(polls: Vec<PollOutcome>) -> Self {
            Self {
                code: sample_code(),
                polls: Mutex::new(polls.into()),
                refresh_result: Mutex::new(None),
                seen_refresh: Mutex::new(Vec::new()),
            }
        }
        fn refreshing(result: Result<TokenSet>) -> Self {
            Self {
                code: sample_code(),
                polls: Mutex::new(VecDeque::new()),
                refresh_result: Mutex::new(Some(result)),
                seen_refresh: Mutex::new(Vec::new()),
            }
        }
    }

    impl DeviceFlow for FakeDeviceFlow {
        fn request_device_code<'a>(
            &'a self,
            _cfg: &'a OauthConfig,
        ) -> BoxFuture<'a, Result<DeviceCode>> {
            Box::pin(async move { Ok(self.code.clone()) })
        }
        fn poll_token<'a>(
            &'a self,
            _cfg: &'a OauthConfig,
            _device_code: &'a str,
        ) -> BoxFuture<'a, Result<PollOutcome>> {
            Box::pin(async move {
                Ok(self
                    .polls
                    .lock()
                    .unwrap()
                    .pop_front()
                    .expect("poll_token called more times than scripted"))
            })
        }
        fn refresh<'a>(
            &'a self,
            _cfg: &'a OauthConfig,
            refresh_token: &'a str,
        ) -> BoxFuture<'a, Result<TokenSet>> {
            let seen = refresh_token.to_string();
            Box::pin(async move {
                self.seen_refresh.lock().unwrap().push(seen);
                self.refresh_result
                    .lock()
                    .unwrap()
                    .take()
                    .expect("refresh called but not scripted")
            })
        }
    }

    struct FakeClock(u64);
    impl Clock for FakeClock {
        fn now_unix(&self) -> u64 {
            self.0
        }
    }

    fn sample_code() -> DeviceCode {
        DeviceCode {
            device_code: "dc-xyz".into(),
            user_code: "WDJB-MJHT".into(),
            verification_uri: "https://some-oauth.example/login/device".into(),
            interval: Duration::from_secs(5),
            expires_in: Duration::from_secs(900),
        }
    }

    fn sample_cfg() -> OauthConfig {
        OauthConfig {
            userinfo_endpoint: None,
            account_field: None,
            client_id: "Iv1.test".into(),
            client_secret: String::new(),
            scopes: vec!["repo".into()],
            device_authorization_endpoint: "https://example.com/device/code".into(),
            token_endpoint: "https://example.com/oauth/token".into(),
        }
    }

    fn token(expires_in: u64) -> TokenSet {
        TokenSet {
            scopes: Vec::new(),
            account: None,
            access_token: "some-access".into(),
            refresh_token: "some-refresh".into(),
            expires_in: Duration::from_secs(expires_in),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn polling_through_pending_to_a_token_completes_and_surfaces_the_user_code() {
        let flow =
            FakeDeviceFlow::polling(vec![PollOutcome::Pending, PollOutcome::Token(token(3600))]);
        let shown = Mutex::new(Vec::new());
        let out = run_device_flow(
            &flow,
            &sample_cfg(),
            |c| {
                shown
                    .lock()
                    .unwrap()
                    .push((c.user_code.clone(), c.verification_uri.clone()));
            },
            std::future::pending::<SignInPivot>(),
        )
        .await
        .unwrap();
        assert_eq!(out, SignIn::Completed(token(3600)));
        assert_eq!(
            shown.lock().unwrap().as_slice(),
            &[(
                "WDJB-MJHT".to_string(),
                "https://some-oauth.example/login/device".to_string()
            )],
            "the user code and verification URL must be surfaced exactly once"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_slow_down_response_keeps_polling_to_completion() {
        let flow =
            FakeDeviceFlow::polling(vec![PollOutcome::SlowDown, PollOutcome::Token(token(60))]);
        let out = run_device_flow(
            &flow,
            &sample_cfg(),
            |_| {},
            std::future::pending::<SignInPivot>(),
        )
        .await
        .unwrap();
        assert_eq!(out, SignIn::Completed(token(60)));
        assert!(
            flow.polls.lock().unwrap().is_empty(),
            "both the slow_down and the token poll must be consumed"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_denied_grant_resolves_to_denied() {
        let flow = FakeDeviceFlow::polling(vec![PollOutcome::Denied]);
        let out = run_device_flow(
            &flow,
            &sample_cfg(),
            |_| {},
            std::future::pending::<SignInPivot>(),
        )
        .await
        .unwrap();
        assert_eq!(out, SignIn::Denied);
    }

    #[tokio::test(start_paused = true)]
    async fn an_expired_device_code_resolves_to_expired() {
        let flow = FakeDeviceFlow::polling(vec![PollOutcome::Pending, PollOutcome::Expired]);
        let out = run_device_flow(
            &flow,
            &sample_cfg(),
            |_| {},
            std::future::pending::<SignInPivot>(),
        )
        .await
        .unwrap();
        assert_eq!(out, SignIn::Expired);
    }

    #[tokio::test(start_paused = true)]
    async fn a_cancel_signal_aborts_before_any_token_poll() {
        let flow = FakeDeviceFlow::polling(vec![]);
        let out = run_device_flow(
            &flow,
            &sample_cfg(),
            |_| {},
            std::future::ready(SignInPivot::Cancel),
        )
        .await
        .unwrap();
        assert_eq!(out, SignIn::Cancelled);
        assert!(
            flow.polls.lock().unwrap().is_empty(),
            "cancel must abort before consuming a poll"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_token_pivot_resolves_the_flow_to_token_without_polling() {
        let flow = FakeDeviceFlow::polling(vec![]);
        let out = run_device_flow(
            &flow,
            &sample_cfg(),
            |_| {},
            std::future::ready(SignInPivot::UseToken("some-pasted-token".into())),
        )
        .await
        .unwrap();
        assert_eq!(
            out,
            SignIn::Token("some-pasted-token".into()),
            "pasting a token mid-flow resolves the device flow with the token, not by polling"
        );
        assert!(
            flow.polls.lock().unwrap().is_empty(),
            "the pivot must short-circuit before consuming a poll"
        );
    }

    #[test]
    fn pkce_challenge_s256_binds_the_verifier_to_the_challenge() {
        let c = PkceChallenge::generate();
        assert_eq!(
            c.challenge,
            s256_challenge(&c.verifier),
            "the challenge must be base64url(sha256(verifier))"
        );
        assert!(
            c.challenge
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'),
            "an S256 challenge is unpadded base64url: {}",
            c.challenge
        );
        let verifier_len = c.verifier.len();
        assert!(
            (43..=128).contains(&verifier_len),
            "the verifier must satisfy RFC 7636's 43..=128 length, got {verifier_len}"
        );
    }

    #[test]
    fn pkce_challenge_draws_distinct_unguessable_verifier_and_state_each_time() {
        let a = PkceChallenge::generate();
        let b = PkceChallenge::generate();
        assert_ne!(a.verifier, b.verifier, "verifiers must be unguessable");
        assert_ne!(a.state, b.state, "state nonces must be unguessable");
        assert_ne!(
            a.verifier, a.state,
            "the verifier and state are independent nonces"
        );
    }

    fn pkce_cfg(scopes: &[&str]) -> PkceConfig {
        PkceConfig {
            authorization_endpoint: "https://provider.example/auth".into(),
            token_endpoint: "https://provider.example/api/v1/auth/keys".into(),
            scopes: scopes.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn query_pairs(url: &str) -> std::collections::HashMap<String, String> {
        reqwest::Url::parse(url)
            .expect("a valid url")
            .query_pairs()
            .into_owned()
            .collect()
    }

    #[test]
    fn authorization_url_carries_the_callback_challenge_method_and_state() {
        let challenge = PkceChallenge {
            verifier: "v".into(),
            challenge: "the-challenge".into(),
            state: "the-state".into(),
        };
        let url = authorization_url(
            &pkce_cfg(&[]),
            "http://localhost:54321/callback",
            &challenge,
        );
        assert!(
            url.starts_with("https://provider.example/auth?"),
            "got: {url}"
        );
        let pairs = query_pairs(&url);
        assert_eq!(
            pairs.get("callback_url").map(String::as_str),
            Some("http://localhost:54321/callback?state=the-state"),
            "the state nonce rides inside the callback_url so the provider round-trips it by redirecting there"
        );
        assert_eq!(
            pairs.get("code_challenge").map(String::as_str),
            Some("the-challenge")
        );
        assert_eq!(
            pairs.get("code_challenge_method").map(String::as_str),
            Some("S256")
        );
        assert!(
            !pairs.contains_key("state"),
            "state is embedded in callback_url, not sent as a sibling param the provider must echo"
        );
        assert!(
            !pairs.contains_key("scope"),
            "no scope param when none configured"
        );
        assert!(!pairs.contains_key("client_id"), "PKCE sends no client id");
    }

    #[test]
    fn authorization_url_appends_space_joined_scopes_when_present() {
        let challenge = PkceChallenge {
            verifier: "v".into(),
            challenge: "c".into(),
            state: "s".into(),
        };
        let url = authorization_url(
            &pkce_cfg(&["read", "write"]),
            "http://localhost:1/callback",
            &challenge,
        );
        assert_eq!(
            query_pairs(&url).get("scope").map(String::as_str),
            Some("read write")
        );
    }

    #[test]
    fn parse_callback_target_extracts_code_and_state_ignoring_extra_params() {
        let p =
            parse_callback_target("GET /callback?iss=provider&code=abc123&state=xyz789 HTTP/1.1")
                .unwrap();
        assert_eq!(p.code, "abc123");
        assert_eq!(p.state, "xyz789");
    }

    #[test]
    fn parse_callback_target_errors_when_a_parameter_is_missing() {
        let err = parse_callback_target("GET /callback?code=only HTTP/1.1").unwrap_err();
        assert!(
            err.to_string().contains("missing code or state"),
            "got: {err}"
        );
    }

    #[test]
    fn parse_callback_target_errors_on_a_malformed_request_line() {
        let err = parse_callback_target("GET").unwrap_err();
        assert!(
            err.to_string().contains("missing code or state"),
            "got: {err}"
        );
    }

    struct FakeAuthCodeFlow {
        result: Mutex<Option<Result<String>>>,
        seen: Mutex<Vec<(String, String)>>,
    }
    impl FakeAuthCodeFlow {
        fn returning(result: Result<String>) -> Self {
            Self {
                result: Mutex::new(Some(result)),
                seen: Mutex::new(Vec::new()),
            }
        }
    }
    impl AuthCodeFlow for FakeAuthCodeFlow {
        fn exchange_code<'a>(
            &'a self,
            _cfg: &'a PkceConfig,
            code: &'a str,
            verifier: &'a str,
        ) -> BoxFuture<'a, Result<String>> {
            let pair = (code.to_string(), verifier.to_string());
            Box::pin(async move {
                self.seen.lock().unwrap().push(pair);
                self.result
                    .lock()
                    .unwrap()
                    .take()
                    .expect("exchange_code called but not scripted")
            })
        }
    }

    enum WaitPlan {
        Returns(CallbackParams),
        Pends,
        Fails,
    }
    struct FakeCallbackListener {
        bind_fails: bool,
        wait: Mutex<Option<WaitPlan>>,
    }
    impl FakeCallbackListener {
        fn returning(params: CallbackParams) -> Self {
            Self {
                bind_fails: false,
                wait: Mutex::new(Some(WaitPlan::Returns(params))),
            }
        }
        fn pending() -> Self {
            Self {
                bind_fails: false,
                wait: Mutex::new(Some(WaitPlan::Pends)),
            }
        }
        fn wait_failing() -> Self {
            Self {
                bind_fails: false,
                wait: Mutex::new(Some(WaitPlan::Fails)),
            }
        }
        fn bind_failing() -> Self {
            Self {
                bind_fails: true,
                wait: Mutex::new(None),
            }
        }
    }
    impl CallbackListener for FakeCallbackListener {
        fn bind(&self) -> BoxFuture<'_, Result<Box<dyn CallbackHandle>>> {
            Box::pin(async move {
                if self.bind_fails {
                    bail!("cannot bind loopback callback listener");
                }
                let plan = self.wait.lock().unwrap().take().expect("bind called twice");
                Ok(Box::new(FakeCallbackHandle { plan }) as Box<dyn CallbackHandle>)
            })
        }
    }
    struct FakeCallbackHandle {
        plan: WaitPlan,
    }
    impl CallbackHandle for FakeCallbackHandle {
        fn redirect_url(&self) -> &str {
            "http://localhost:0/callback"
        }
        fn wait(self: Box<Self>) -> BoxFuture<'static, Result<CallbackParams>> {
            Box::pin(async move {
                match self.plan {
                    WaitPlan::Returns(p) => Ok(p),
                    WaitPlan::Pends => std::future::pending().await,
                    WaitPlan::Fails => bail!("callback connection failed"),
                }
            })
        }
    }

    fn pkce_test_cfg() -> PkceConfig {
        PkceConfig {
            authorization_endpoint: "https://provider.example/auth".into(),
            token_endpoint: "https://provider.example/api/v1/auth/keys".into(),
            scopes: Vec::new(),
        }
    }
    fn fixed_challenge() -> PkceChallenge {
        PkceChallenge {
            verifier: "fixed-verifier".into(),
            challenge: "fixed-challenge".into(),
            state: "fixed-state".into(),
        }
    }

    // A shared no-op so the never-called case (a failed bind) reuses a body covered by the tests that do present.
    fn noop_url(_: &str) {}

    #[tokio::test]
    async fn run_pkce_flow_completes_and_exchanges_the_verifier_behind_the_challenge() {
        let flow = FakeAuthCodeFlow::returning(Ok("sk-or-the-key".into()));
        let listener = FakeCallbackListener::returning(CallbackParams {
            code: "the-code".into(),
            state: "fixed-state".into(),
        });
        let urls = Mutex::new(Vec::new());
        let out = run_pkce_flow(
            &flow,
            &listener,
            &pkce_test_cfg(),
            &fixed_challenge(),
            |url| urls.lock().unwrap().push(url.to_string()),
            std::future::pending::<()>(),
            Duration::from_secs(300),
        )
        .await
        .unwrap();
        assert_eq!(out, PkceSignIn::Completed("sk-or-the-key".into()));
        assert_eq!(
            flow.seen.lock().unwrap().as_slice(),
            &[("the-code".to_string(), "fixed-verifier".to_string())],
            "the exchange must send the returned code and the verifier behind the issued challenge"
        );
        let captured = urls.lock().unwrap();
        assert_eq!(captured.len(), 1, "the authorization URL is surfaced once");
        assert!(
            captured[0].contains("code_challenge=fixed-challenge"),
            "got: {}",
            captured[0]
        );
    }

    #[tokio::test]
    async fn run_pkce_flow_cancel_aborts_without_exchanging() {
        let flow = FakeAuthCodeFlow::returning(Err(anyhow!("must not exchange")));
        let listener = FakeCallbackListener::pending();
        let out = run_pkce_flow(
            &flow,
            &listener,
            &pkce_test_cfg(),
            &fixed_challenge(),
            noop_url,
            std::future::ready(()),
            Duration::from_secs(300),
        )
        .await
        .unwrap();
        assert_eq!(out, PkceSignIn::Cancelled);
        assert!(
            flow.seen.lock().unwrap().is_empty(),
            "a cancelled sign-in must not reach the exchange"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn run_pkce_flow_times_out_when_no_callback_arrives() {
        let flow = FakeAuthCodeFlow::returning(Err(anyhow!("must not exchange")));
        let listener = FakeCallbackListener::pending();
        let out = run_pkce_flow(
            &flow,
            &listener,
            &pkce_test_cfg(),
            &fixed_challenge(),
            noop_url,
            std::future::pending::<()>(),
            Duration::from_secs(120),
        )
        .await
        .unwrap();
        assert_eq!(out, PkceSignIn::TimedOut);
        assert!(flow.seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn run_pkce_flow_rejects_a_callback_whose_state_mismatches() {
        let flow = FakeAuthCodeFlow::returning(Err(anyhow!("must not exchange")));
        let listener = FakeCallbackListener::returning(CallbackParams {
            code: "c".into(),
            state: "forged".into(),
        });
        let err = run_pkce_flow(
            &flow,
            &listener,
            &pkce_test_cfg(),
            &fixed_challenge(),
            noop_url,
            std::future::pending::<()>(),
            Duration::from_secs(300),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("did not match"), "got: {err}");
        assert!(
            flow.seen.lock().unwrap().is_empty(),
            "a forged state must never reach the exchange"
        );
    }

    #[tokio::test]
    async fn run_pkce_flow_surfaces_a_failed_exchange() {
        let flow = FakeAuthCodeFlow::returning(Err(anyhow!("invalid_code")));
        let listener = FakeCallbackListener::returning(CallbackParams {
            code: "c".into(),
            state: "fixed-state".into(),
        });
        let err = run_pkce_flow(
            &flow,
            &listener,
            &pkce_test_cfg(),
            &fixed_challenge(),
            noop_url,
            std::future::pending::<()>(),
            Duration::from_secs(300),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("invalid_code"), "got: {err}");
    }

    #[tokio::test]
    async fn run_pkce_flow_fails_closed_when_the_listener_cannot_bind() {
        let flow = FakeAuthCodeFlow::returning(Err(anyhow!("must not exchange")));
        let listener = FakeCallbackListener::bind_failing();
        let err = run_pkce_flow(
            &flow,
            &listener,
            &pkce_test_cfg(),
            &fixed_challenge(),
            noop_url,
            std::future::pending::<()>(),
            Duration::from_secs(300),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("bind"), "got: {err}");
        assert!(flow.seen.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn run_pkce_flow_surfaces_a_callback_connection_error() {
        let flow = FakeAuthCodeFlow::returning(Err(anyhow!("must not exchange")));
        let listener = FakeCallbackListener::wait_failing();
        let err = run_pkce_flow(
            &flow,
            &listener,
            &pkce_test_cfg(),
            &fixed_challenge(),
            noop_url,
            std::future::pending::<()>(),
            Duration::from_secs(300),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("connection failed"), "got: {err}");
    }

    #[tokio::test]
    async fn refresh_if_due_skips_a_non_oauth_entry() {
        let flow = FakeDeviceFlow::polling(vec![]);
        let out = refresh_if_due(
            &flow,
            &FakeClock(1000),
            &sample_cfg(),
            &CredentialEntry::Deny,
            60,
        )
        .await
        .unwrap();
        assert_eq!(out, None);
    }

    #[tokio::test]
    async fn refresh_if_due_leaves_a_still_fresh_token_untouched() {
        let flow = FakeDeviceFlow::polling(vec![]);
        let entry = CredentialEntry::Oauth {
            scopes: Vec::new(),
            account: None,
            access_token: "still-good".into(),
            refresh_token: "r".into(),
            expires_at: 10_000,
        };
        let out = refresh_if_due(&flow, &FakeClock(1000), &sample_cfg(), &entry, 60)
            .await
            .unwrap();
        assert_eq!(
            out, None,
            "a token well before expiry must not be refreshed"
        );
        assert!(flow.seen_refresh.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn refresh_if_due_leaves_a_refreshless_grant_in_place_even_past_expiry() {
        let flow = FakeDeviceFlow::refreshing(Err(anyhow!("must not be called")));
        let entry = CredentialEntry::Oauth {
            scopes: Vec::new(),
            account: None,
            access_token: "long-lived".into(),
            refresh_token: String::new(),
            expires_at: 0,
        };
        let out = refresh_if_due(&flow, &FakeClock(1_000_000), &sample_cfg(), &entry, 60)
            .await
            .unwrap();
        assert_eq!(
            out, None,
            "a grant with no refresh token (e.g. a GitHub device-flow token) is kept, not refreshed-and-dropped"
        );
        assert!(
            flow.seen_refresh.lock().unwrap().is_empty(),
            "no refresh is attempted when there is no refresh token"
        );
    }

    #[tokio::test]
    async fn refresh_if_due_renews_a_token_within_the_skew_window() {
        let flow = FakeDeviceFlow::refreshing(Ok(token(3600)));
        let entry = CredentialEntry::Oauth {
            scopes: Vec::new(),
            account: None,
            access_token: "expired".into(),
            refresh_token: "old-refresh".into(),
            expires_at: 1030,
        };
        let out = refresh_if_due(&flow, &FakeClock(1000), &sample_cfg(), &entry, 60)
            .await
            .unwrap();
        assert_eq!(
            out,
            Some(CredentialEntry::Oauth {
                scopes: Vec::new(),
                account: None,
                access_token: "some-access".into(),
                refresh_token: "some-refresh".into(),
                expires_at: 1000 + 3600,
            }),
            "a refreshed entry carries the new tokens and an absolute expiry off the clock"
        );
        assert_eq!(
            flow.seen_refresh.lock().unwrap().as_slice(),
            &["old-refresh".to_string()],
            "the stored refresh token must be the one sent to the provider"
        );
    }

    #[tokio::test]
    async fn refresh_if_due_surfaces_a_failed_refresh_as_an_error() {
        let flow = FakeDeviceFlow::refreshing(Err(anyhow!("invalid_grant")));
        let entry = CredentialEntry::Oauth {
            scopes: Vec::new(),
            account: None,
            access_token: "expired".into(),
            refresh_token: "revoked".into(),
            expires_at: 0,
        };
        let err = refresh_if_due(&flow, &FakeClock(1000), &sample_cfg(), &entry, 60)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("invalid_grant"), "got: {err}");
    }

    struct CapturingStore {
        saved: Mutex<Vec<CredentialStateFile>>,
        fail: bool,
    }
    impl CapturingStore {
        fn new() -> Self {
            Self {
                saved: Mutex::new(Vec::new()),
                fail: false,
            }
        }
        fn failing() -> Self {
            Self {
                saved: Mutex::new(Vec::new()),
                fail: true,
            }
        }
    }
    impl CredentialStore for CapturingStore {
        fn load(&self) -> std::io::Result<CredentialStateFile> {
            Ok(CredentialStateFile::new())
        }
        fn save(&self, state: &CredentialStateFile) -> std::io::Result<()> {
            if self.fail {
                return Err(std::io::Error::other("simulated write failure"));
            }
            self.saved.lock().unwrap().push(state.clone());
            Ok(())
        }
    }

    #[test]
    fn capturing_store_load_is_empty() {
        // refresh_due_entries operates on the passed-in state and never reloads, so the fixture store's `load` is pinned directly.
        assert!(CapturingStore::new().load().unwrap().is_empty());
    }

    fn oauth_entry(access: &str, refresh: &str, expires_at: u64) -> CredentialEntry {
        CredentialEntry::Oauth {
            scopes: Vec::new(),
            account: None,
            access_token: access.into(),
            refresh_token: refresh.into(),
            expires_at,
        }
    }

    fn configs_for(id: &str) -> HashMap<String, OauthConfig> {
        HashMap::from([(id.to_string(), sample_cfg())])
    }

    #[tokio::test]
    async fn refresh_due_entries_renews_a_due_grant_and_persists_once() {
        let flow = FakeDeviceFlow::refreshing(Ok(token(3600)));
        let store = CapturingStore::new();
        let mut state = CredentialStateFile::new();
        state.insert("some-oauth".into(), oauth_entry("expired", "old", 0));
        refresh_due_entries(
            &mut state,
            &configs_for("some-oauth"),
            &flow,
            &FakeClock(1000),
            &store,
            60,
        )
        .await;
        assert_eq!(
            state.get("some-oauth"),
            Some(&oauth_entry("some-access", "some-refresh", 1000 + 3600))
        );
        assert_eq!(
            store.saved.lock().unwrap().len(),
            1,
            "a changed state is persisted exactly once"
        );
    }

    #[tokio::test]
    async fn refresh_due_entries_leaves_a_fresh_grant_untouched_without_persisting() {
        let flow = FakeDeviceFlow::polling(vec![]);
        let store = CapturingStore::new();
        let mut state = CredentialStateFile::new();
        state.insert("some-oauth".into(), oauth_entry("good", "r", 99_999));
        refresh_due_entries(
            &mut state,
            &configs_for("some-oauth"),
            &flow,
            &FakeClock(1000),
            &store,
            60,
        )
        .await;
        assert!(
            store.saved.lock().unwrap().is_empty(),
            "nothing due means nothing written"
        );
    }

    #[tokio::test]
    async fn refresh_due_entries_drops_an_unrefreshable_grant_so_it_re_prompts() {
        let flow = FakeDeviceFlow::refreshing(Err(anyhow!("invalid_grant")));
        let store = CapturingStore::new();
        let mut state = CredentialStateFile::new();
        state.insert("some-oauth".into(), oauth_entry("expired", "revoked", 0));
        refresh_due_entries(
            &mut state,
            &configs_for("some-oauth"),
            &flow,
            &FakeClock(1000),
            &store,
            60,
        )
        .await;
        assert!(
            !state.contains_key("some-oauth"),
            "a dead grant is dropped so the placeholder unarms and the next use re-prompts"
        );
        assert_eq!(
            store.saved.lock().unwrap().len(),
            1,
            "the removal is persisted"
        );
    }

    #[tokio::test]
    async fn refresh_due_entries_ignores_a_config_with_no_stored_entry() {
        let flow = FakeDeviceFlow::polling(vec![]);
        let store = CapturingStore::new();
        let mut state = CredentialStateFile::new();
        refresh_due_entries(
            &mut state,
            &configs_for("some-oauth"),
            &flow,
            &FakeClock(1000),
            &store,
            60,
        )
        .await;
        assert!(state.is_empty());
        assert!(store.saved.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn refresh_due_entries_updates_memory_even_when_the_persist_write_fails() {
        let flow = FakeDeviceFlow::refreshing(Ok(token(3600)));
        let store = CapturingStore::failing();
        let mut state = CredentialStateFile::new();
        state.insert("some-oauth".into(), oauth_entry("expired", "old", 0));
        refresh_due_entries(
            &mut state,
            &configs_for("some-oauth"),
            &flow,
            &FakeClock(1000),
            &store,
            60,
        )
        .await;
        assert_eq!(
            state.get("some-oauth"),
            Some(&oauth_entry("some-access", "some-refresh", 1000 + 3600)),
            "the refreshed token is live in memory even if the disk write failed"
        );
    }

    struct FakeUserInfoFetcher(Result<Vec<u8>, ()>);
    impl UserInfoFetcher for FakeUserInfoFetcher {
        fn fetch<'a>(
            &'a self,
            _url: &'a str,
            _access_token: &'a str,
        ) -> futures_util::future::BoxFuture<'a, Result<Vec<u8>>> {
            Box::pin(async move {
                match &self.0 {
                    Ok(bytes) => Ok(bytes.clone()),
                    Err(()) => bail!("userinfo unreachable"),
                }
            })
        }
    }

    fn userinfo_cfg() -> OauthConfig {
        OauthConfig {
            userinfo_endpoint: Some("https://api.example.test/user".into()),
            account_field: Some("login".into()),
            ..sample_cfg()
        }
    }

    #[test]
    fn account_from_userinfo_reads_the_named_field() {
        assert_eq!(
            account_from_userinfo(br#"{"login":"@hchen","id":7}"#, "login"),
            Some("@hchen".to_string())
        );
    }

    #[test]
    fn account_from_userinfo_is_none_for_missing_field_non_string_or_bad_json() {
        assert_eq!(account_from_userinfo(br#"{"id":7}"#, "login"), None);
        assert_eq!(account_from_userinfo(br#"{"login":7}"#, "login"), None);
        assert_eq!(account_from_userinfo(b"not json", "login"), None);
    }

    #[tokio::test]
    async fn resolve_account_sets_the_account_on_a_successful_lookup() {
        let fetcher = FakeUserInfoFetcher(Ok(br#"{"login":"@hchen"}"#.to_vec()));
        let out = resolve_account(&fetcher, &userinfo_cfg(), token(3600)).await;
        assert_eq!(out.account, Some("@hchen".to_string()));
    }

    #[tokio::test]
    async fn resolve_account_leaves_it_unset_when_the_lookup_fails() {
        let fetcher = FakeUserInfoFetcher(Err(()));
        let out = resolve_account(&fetcher, &userinfo_cfg(), token(3600)).await;
        assert_eq!(out.account, None);
    }

    #[tokio::test]
    async fn resolve_account_skips_the_lookup_when_the_catalog_declares_no_endpoint() {
        let fetcher = FakeUserInfoFetcher(Err(()));
        let out = resolve_account(&fetcher, &sample_cfg(), token(3600)).await;
        assert_eq!(
            out.account, None,
            "no endpoint means no lookup, not a failure"
        );
    }

    #[test]
    fn entry_from_token_carries_the_granted_scopes_and_account() {
        let token = TokenSet {
            scopes: vec!["repo".into(), "read:org".into()],
            account: Some("@hchen".into()),
            ..token(3600)
        };
        assert_eq!(
            entry_from_token(&FakeClock(0), &token),
            CredentialEntry::Oauth {
                access_token: "some-access".into(),
                refresh_token: "some-refresh".into(),
                expires_at: 3600,
                scopes: vec!["repo".into(), "read:org".into()],
                account: Some("@hchen".into()),
            }
        );
    }

    #[tokio::test]
    async fn refresh_preserves_the_scopes_and_account_resolved_at_sign_in() {
        let flow = FakeDeviceFlow::refreshing(Ok(token(3600)));
        let entry = CredentialEntry::Oauth {
            scopes: vec!["repo".into()],
            account: Some("@hchen".into()),
            access_token: "expired".into(),
            refresh_token: "old-refresh".into(),
            expires_at: 1030,
        };
        let refreshed = refresh_if_due(&flow, &FakeClock(1000), &sample_cfg(), &entry, 60)
            .await
            .unwrap();
        assert_eq!(
            refreshed,
            Some(CredentialEntry::Oauth {
                access_token: "some-access".into(),
                refresh_token: "some-refresh".into(),
                expires_at: 1000 + 3600,
                scopes: vec!["repo".into()],
                account: Some("@hchen".into()),
            }),
            "a refresh keeps the scopes and account resolved at sign-in"
        );
    }
}
