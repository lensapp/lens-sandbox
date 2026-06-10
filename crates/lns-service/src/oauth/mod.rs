//! Host-side OAuth 2.0 Device Authorization Grant (RFC 8628): the orchestration is here and unit-tested via a fake; the reqwest/SystemTime leaves live in `real.rs`.

mod real;
pub mod traits;

use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;

use crate::credential_flow::store::{CredentialEntry, CredentialStateFile, CredentialStore};
pub use real::{RealClock, RealDeviceFlow};
pub use traits::{Clock, DeviceCode, DeviceFlow, OauthConfig, PollOutcome, TokenSet};

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

/// Turns a freshly-obtained grant into the stored entry, stamping an absolute expiry off the injected clock.
pub fn entry_from_token(clock: &dyn Clock, token: &TokenSet) -> CredentialEntry {
    CredentialEntry::Oauth {
        access_token: token.access_token.clone(),
        refresh_token: token.refresh_token.clone(),
        expires_at: clock.now_unix().saturating_add(token.expires_in.as_secs()),
    }
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
    Ok(Some(entry_from_token(clock, &token)))
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
            client_id: "Iv1.test".into(),
            scopes: vec!["repo".into()],
            device_authorization_endpoint: "https://example.com/device/code".into(),
            token_endpoint: "https://example.com/oauth/token".into(),
        }
    }

    fn token(expires_in: u64) -> TokenSet {
        TokenSet {
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
}
