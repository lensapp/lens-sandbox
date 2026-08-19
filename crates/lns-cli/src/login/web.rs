use std::io::Write;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use lns_ipc::{Method, PlatformInfo};

use super::{FLAG_FALLBACK, WebLoginFlow, WebLoginOutcome};
use crate::connector::LocalBoxFuture;

/// RFC 8628 §3.5: a `slow_down` response bumps the poll interval by 5 seconds.
const SLOW_DOWN_INCREMENT: Duration = Duration::from_secs(5);

/// What the registry handed back when a device login started: the codes to show and poll with, and how long they live.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct DeviceAuthorization {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceStart {
    Started(DeviceAuthorization),
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenPoll {
    Pending,
    SlowDown,
    Denied,
    Expired,
    Issued { token: String, username: String },
}

/// Speaks the registry's device-login endpoints: start a grant, then poll it until it resolves.
pub trait DeviceAuthClient {
    fn start<'a>(&'a self) -> LocalBoxFuture<'a, Result<DeviceStart>>;
    fn poll<'a>(&'a self, device_code: &'a str) -> LocalBoxFuture<'a, Result<TokenPoll>>;
}

/// Opens a URL in the user's browser, reporting whether the opener could be spawned at all.
pub trait BrowserOpener {
    fn open(&self, url: &str) -> bool;
}

/// Drives a browser-based device login: starts the grant, surfaces the one-time code, opens the browser, and polls until the registry resolves the grant or it expires.
pub struct WebLogin<C, B> {
    client: C,
    browser: B,
}

impl<C: DeviceAuthClient, B: BrowserOpener> WebLogin<C, B> {
    pub fn new(client: C, browser: B) -> Self {
        Self { client, browser }
    }

    fn present(&self, auth: &DeviceAuthorization, out: &mut dyn Write) -> Result<()> {
        writeln!(
            out,
            "Your one-time confirmation code is: {}",
            auth.user_code
        )?;
        if self.browser.open(&auth.verification_uri_complete) {
            writeln!(
                out,
                "Opening {} in your browser. Enter the code there.",
                auth.verification_uri
            )?;
        } else {
            writeln!(
                out,
                "Could not open a browser. Go to {} and enter the code.",
                auth.verification_uri
            )?;
        }
        writeln!(out, "Waiting for you to approve the login in the browser…")?;
        writeln!(out, "To log in with a token instead, {FLAG_FALLBACK}.")?;
        Ok(())
    }

    async fn wait_for_grant(&self, auth: &DeviceAuthorization) -> Result<WebLoginOutcome> {
        let expiry = tokio::time::sleep(Duration::from_secs(auth.expires_in));
        tokio::pin!(expiry);
        let mut interval = Duration::from_secs(auth.interval);
        loop {
            tokio::select! {
                biased;
                () = &mut expiry => return Ok(WebLoginOutcome::Expired),
                outcome = sleep_then_poll(&self.client, &auth.device_code, interval) => {
                    match outcome? {
                        TokenPoll::Pending => {}
                        TokenPoll::SlowDown => interval = interval.saturating_add(SLOW_DOWN_INCREMENT),
                        TokenPoll::Denied => return Ok(WebLoginOutcome::Denied),
                        TokenPoll::Expired => return Ok(WebLoginOutcome::Expired),
                        TokenPoll::Issued { token, username } => {
                            return Ok(WebLoginOutcome::Completed { username, secret: token });
                        }
                    }
                }
            }
        }
    }
}

async fn sleep_then_poll(
    client: &dyn DeviceAuthClient,
    device_code: &str,
    interval: Duration,
) -> Result<TokenPoll> {
    tokio::time::sleep(interval).await;
    client.poll(device_code).await
}

impl<C: DeviceAuthClient, B: BrowserOpener> WebLoginFlow for WebLogin<C, B> {
    fn login<'a>(
        &'a self,
        _registry: &'a str,
        out: &'a mut dyn Write,
    ) -> LocalBoxFuture<'a, Result<WebLoginOutcome>> {
        Box::pin(async move {
            let auth = match self.client.start().await? {
                DeviceStart::Unsupported => return Ok(WebLoginOutcome::Unsupported),
                DeviceStart::Started(auth) => auth,
            };
            self.present(&auth, out)?;
            self.wait_for_grant(&auth).await
        })
    }
}

#[derive(serde::Deserialize)]
struct ErrorBody {
    error: String,
    #[serde(default)]
    error_description: Option<String>,
}

#[derive(serde::Deserialize)]
struct TokenBody {
    token: String,
    username: String,
}

/// The reqwest-backed [`DeviceAuthClient`] speaking to `https://{registry}`.
pub struct RealDeviceAuthClient {
    base: String,
    http: reqwest::Client,
}

impl RealDeviceAuthClient {
    pub fn for_registry(registry: &str, version: &str, platform: &PlatformInfo) -> Result<Self> {
        Self::with_base(format!("https://{registry}"), version, platform)
    }

    fn with_base(base: String, version: &str, platform: &PlatformInfo) -> Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(lns_ipc::user_agent(version, platform, Method::CliLogin))
            .timeout(Duration::from_secs(10))
            .build()
            .context("building the device-login HTTP client")?;
        Ok(Self { base, http })
    }

    async fn post(&self, url: &str, form_body: Option<String>) -> Result<(u16, Vec<u8>)> {
        let mut request = self.http.post(url);
        if let Some(body) = form_body {
            request = request
                .header("content-type", "application/x-www-form-urlencoded")
                .body(body);
        }
        let response = request
            .send()
            .await
            .with_context(|| format!("requesting {url}"))?;
        let status = response.status().as_u16();
        let bytes = response
            .bytes()
            .await
            .with_context(|| format!("reading the response from {url}"))?;
        Ok((status, bytes.to_vec()))
    }
}

fn parse<'a, T: serde::Deserialize<'a>>(bytes: &'a [u8], url: &str) -> Result<T> {
    serde_json::from_slice(bytes).with_context(|| format!("parsing the response from {url}"))
}

impl DeviceAuthClient for RealDeviceAuthClient {
    fn start<'a>(&'a self) -> LocalBoxFuture<'a, Result<DeviceStart>> {
        Box::pin(async move {
            let url = format!("{}/cli/device/code", self.base);
            let (status, bytes) = self.post(&url, None).await?;
            match status {
                404 => Ok(DeviceStart::Unsupported),
                503 => {
                    let body: ErrorBody = parse(&bytes, &url)?;
                    let detail = body.error_description.unwrap_or(body.error);
                    bail!(
                        "browser login is not available right now: {detail}; retry shortly, or {FLAG_FALLBACK}"
                    )
                }
                200 => Ok(DeviceStart::Started(parse(&bytes, &url)?)),
                other => bail!("{url} answered HTTP {other}"),
            }
        })
    }

    fn poll<'a>(&'a self, device_code: &'a str) -> LocalBoxFuture<'a, Result<TokenPoll>> {
        Box::pin(async move {
            let url = format!("{}/cli/device/token", self.base);
            let body = format!("device_code={device_code}");
            let (status, bytes) = self.post(&url, Some(body)).await?;
            match status {
                200 => {
                    let issued: TokenBody = parse(&bytes, &url)?;
                    Ok(TokenPoll::Issued {
                        token: issued.token,
                        username: issued.username,
                    })
                }
                400 => {
                    let body: ErrorBody = parse(&bytes, &url)?;
                    match body.error.as_str() {
                        "authorization_pending" => Ok(TokenPoll::Pending),
                        "slow_down" => Ok(TokenPoll::SlowDown),
                        "access_denied" => Ok(TokenPoll::Denied),
                        "expired_token" => Ok(TokenPoll::Expired),
                        other => bail!("{url} answered with unexpected error code {other:?}"),
                    }
                }
                other => bail!("{url} answered HTTP {other}"),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    struct ScriptedClient {
        start: Mutex<Option<Result<DeviceStart>>>,
        polls: Mutex<VecDeque<Result<TokenPoll>>>,
        poll_instants: Mutex<Vec<tokio::time::Instant>>,
    }

    impl ScriptedClient {
        fn new(start: Result<DeviceStart>, polls: Vec<Result<TokenPoll>>) -> Self {
            Self {
                start: Mutex::new(Some(start)),
                polls: Mutex::new(polls.into_iter().collect()),
                poll_instants: Mutex::new(Vec::new()),
            }
        }
    }

    impl DeviceAuthClient for &ScriptedClient {
        fn start<'a>(&'a self) -> LocalBoxFuture<'a, Result<DeviceStart>> {
            let outcome = self.start.lock().unwrap().take().expect("start once");
            Box::pin(async move { outcome })
        }

        fn poll<'a>(&'a self, device_code: &'a str) -> LocalBoxFuture<'a, Result<TokenPoll>> {
            assert_eq!(device_code, "some-device-code");
            self.poll_instants
                .lock()
                .unwrap()
                .push(tokio::time::Instant::now());
            let outcome = self
                .polls
                .lock()
                .unwrap()
                .pop_front()
                .expect("no poll scripted");
            Box::pin(async move { outcome })
        }
    }

    struct ScriptedBrowser {
        opens: bool,
        seen: Mutex<Vec<String>>,
    }

    impl BrowserOpener for &ScriptedBrowser {
        fn open(&self, url: &str) -> bool {
            self.seen.lock().unwrap().push(url.to_string());
            self.opens
        }
    }

    fn authorization() -> DeviceAuthorization {
        DeviceAuthorization {
            device_code: "some-device-code".into(),
            user_code: "WXYZ-1234".into(),
            verification_uri: "https://registry.example.test/cli/authorize".into(),
            verification_uri_complete: "https://registry.example.test/cli/authorize?code=WXYZ-1234"
                .into(),
            expires_in: 900,
            interval: 5,
        }
    }

    async fn drive(
        client: &ScriptedClient,
        browser: &ScriptedBrowser,
    ) -> (Result<WebLoginOutcome>, String) {
        let flow = WebLogin::new(client, browser);
        let mut out = Vec::new();
        let result = flow.login("registry.example.test", &mut out).await;
        (result, String::from_utf8(out).unwrap())
    }

    fn issued() -> Result<TokenPoll> {
        Ok(TokenPoll::Issued {
            token: "some-web-token".into(),
            username: "webuser".into(),
        })
    }

    #[tokio::test(start_paused = true)]
    async fn pending_grant_polls_until_the_token_is_issued() {
        let client = ScriptedClient::new(
            Ok(DeviceStart::Started(authorization())),
            vec![Ok(TokenPoll::Pending), issued()],
        );
        let browser = ScriptedBrowser {
            opens: true,
            seen: Mutex::new(Vec::new()),
        };
        let (result, out) = drive(&client, &browser).await;
        assert_eq!(
            result.unwrap(),
            WebLoginOutcome::Completed {
                username: "webuser".into(),
                secret: "some-web-token".into()
            }
        );
        assert!(
            out.ends_with(
                "To log in with a token instead, pass --username/-u and --password-stdin (recommended) or --password.\n"
            ),
            "the token fallback hint is offered last: {out}"
        );
        assert!(
            out.contains("Your one-time confirmation code is: WXYZ-1234"),
            "got: {out}"
        );
        assert!(
            out.contains(
                "Opening https://registry.example.test/cli/authorize in your browser. Enter the code there."
            ),
            "got: {out}"
        );
        assert!(
            out.contains("Waiting for you to approve the login in the browser…"),
            "got: {out}"
        );
        assert_eq!(
            browser.seen.lock().unwrap().as_slice(),
            ["https://registry.example.test/cli/authorize?code=WXYZ-1234"],
            "the browser opens the code-carrying URL"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn slow_down_stretches_the_poll_interval_by_five_seconds() {
        let client = ScriptedClient::new(
            Ok(DeviceStart::Started(authorization())),
            vec![Ok(TokenPoll::SlowDown), issued()],
        );
        let browser = ScriptedBrowser {
            opens: true,
            seen: Mutex::new(Vec::new()),
        };
        let began = tokio::time::Instant::now();
        let (result, _) = drive(&client, &browser).await;
        result.unwrap();
        let instants = client.poll_instants.lock().unwrap();
        assert_eq!(instants[0] - began, Duration::from_secs(5));
        assert_eq!(
            instants[1] - instants[0],
            Duration::from_secs(10),
            "5s interval + 5s slow-down increment"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_grant_denied_in_the_browser_resolves_denied() {
        let client = ScriptedClient::new(
            Ok(DeviceStart::Started(authorization())),
            vec![Ok(TokenPoll::Denied)],
        );
        let browser = ScriptedBrowser {
            opens: true,
            seen: Mutex::new(Vec::new()),
        };
        let (result, _) = drive(&client, &browser).await;
        assert_eq!(result.unwrap(), WebLoginOutcome::Denied);
    }

    #[tokio::test(start_paused = true)]
    async fn a_grant_the_registry_expired_resolves_expired() {
        let client = ScriptedClient::new(
            Ok(DeviceStart::Started(authorization())),
            vec![Ok(TokenPoll::Expired)],
        );
        let browser = ScriptedBrowser {
            opens: true,
            seen: Mutex::new(Vec::new()),
        };
        let (result, _) = drive(&client, &browser).await;
        assert_eq!(result.unwrap(), WebLoginOutcome::Expired);
    }

    #[tokio::test(start_paused = true)]
    async fn the_local_deadline_expires_a_grant_the_registry_never_resolves() {
        let mut auth = authorization();
        auth.expires_in = 7;
        let client =
            ScriptedClient::new(Ok(DeviceStart::Started(auth)), vec![Ok(TokenPoll::Pending)]);
        let browser = ScriptedBrowser {
            opens: true,
            seen: Mutex::new(Vec::new()),
        };
        let (result, _) = drive(&client, &browser).await;
        assert_eq!(
            result.unwrap(),
            WebLoginOutcome::Expired,
            "one pending poll at 5s, then the 7s deadline beats the next poll"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn an_unsupported_registry_prints_nothing() {
        let client = ScriptedClient::new(Ok(DeviceStart::Unsupported), vec![]);
        let browser = ScriptedBrowser {
            opens: true,
            seen: Mutex::new(Vec::new()),
        };
        let (result, out) = drive(&client, &browser).await;
        assert_eq!(result.unwrap(), WebLoginOutcome::Unsupported);
        assert!(out.is_empty(), "got: {out}");
        assert!(browser.seen.lock().unwrap().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn a_browser_that_cannot_open_leaves_the_user_the_url() {
        let client = ScriptedClient::new(Ok(DeviceStart::Started(authorization())), vec![issued()]);
        let browser = ScriptedBrowser {
            opens: false,
            seen: Mutex::new(Vec::new()),
        };
        let (result, out) = drive(&client, &browser).await;
        result.unwrap();
        assert!(
            out.contains(
                "Could not open a browser. Go to https://registry.example.test/cli/authorize and enter the code."
            ),
            "got: {out}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_transport_error_mid_poll_aborts_the_flow() {
        let client = ScriptedClient::new(
            Ok(DeviceStart::Started(authorization())),
            vec![Err(anyhow::anyhow!("connection reset"))],
        );
        let browser = ScriptedBrowser {
            opens: true,
            seen: Mutex::new(Vec::new()),
        };
        let (result, _) = drive(&client, &browser).await;
        assert!(
            format!("{:#}", result.unwrap_err()).contains("connection reset"),
            "the poll error propagates"
        );
    }

    mod real_client {
        use super::*;
        use httpmock::Method::POST;
        use httpmock::MockServer;

        fn darwin_platform() -> PlatformInfo {
            PlatformInfo {
                os: "Darwin".into(),
                arch: "arm64".into(),
                kernel_release: "24.6.0".into(),
                shell: "zsh".into(),
            }
        }

        async fn client_for(server: &MockServer) -> RealDeviceAuthClient {
            RealDeviceAuthClient::with_base(server.base_url(), "0.16.0", &darwin_platform())
                .unwrap()
        }

        fn authorization_json() -> String {
            serde_json::json!({
                "device_code": "some-device-code",
                "user_code": "WXYZ-1234",
                "verification_uri": "https://registry.example.test/cli/authorize",
                "verification_uri_complete": "https://registry.example.test/cli/authorize?code=WXYZ-1234",
                "expires_in": 900,
                "interval": 5,
            })
            .to_string()
        }

        #[tokio::test]
        async fn start_parses_a_granted_device_authorization() {
            let server = MockServer::start_async().await;
            server
                .mock_async(|when, then| {
                    when.method(POST).path("/cli/device/code");
                    then.status(200).body(authorization_json());
                })
                .await;
            let start = client_for(&server).await.start().await.unwrap();
            assert_eq!(
                start,
                DeviceStart::Started(super::super::tests::authorization())
            );
        }

        #[tokio::test]
        async fn start_reports_a_registry_without_web_login_as_unsupported() {
            let server = MockServer::start_async().await;
            server
                .mock_async(|when, then| {
                    when.method(POST).path("/cli/device/code");
                    then.status(404);
                })
                .await;
            let start = client_for(&server).await.start().await.unwrap();
            assert_eq!(start, DeviceStart::Unsupported);
        }

        #[tokio::test]
        async fn start_surfaces_the_unavailable_description_and_the_flag_fallback() {
            let server = MockServer::start_async().await;
            server
                .mock_async(|when, then| {
                    when.method(POST).path("/cli/device/code");
                    then.status(503).body(
                        r#"{"error":"unavailable","error_description":"too many logins in flight"}"#,
                    );
                })
                .await;
            let err = format!("{:#}", client_for(&server).await.start().await.unwrap_err());
            assert!(
                err.contains("too many logins in flight") && err.contains("--password-stdin"),
                "got: {err}"
            );
        }

        #[tokio::test]
        async fn start_names_the_endpoint_when_the_body_is_not_json() {
            let server = MockServer::start_async().await;
            server
                .mock_async(|when, then| {
                    when.method(POST).path("/cli/device/code");
                    then.status(200).body("not json");
                })
                .await;
            let err = format!("{:#}", client_for(&server).await.start().await.unwrap_err());
            assert!(err.contains("/cli/device/code"), "got: {err}");
        }

        #[tokio::test]
        async fn start_reports_an_unexpected_status() {
            let server = MockServer::start_async().await;
            server
                .mock_async(|when, then| {
                    when.method(POST).path("/cli/device/code");
                    then.status(500);
                })
                .await;
            let err = format!("{:#}", client_for(&server).await.start().await.unwrap_err());
            assert!(err.contains("HTTP 500"), "got: {err}");
        }

        #[tokio::test]
        async fn an_unreachable_registry_is_a_transport_error() {
            let client = RealDeviceAuthClient::with_base(
                "http://127.0.0.1:1".into(),
                "0.16.0",
                &darwin_platform(),
            )
            .unwrap();
            let err = format!("{:#}", client.start().await.unwrap_err());
            assert!(err.contains("requesting http://127.0.0.1:1"), "got: {err}");
        }

        #[tokio::test]
        async fn poll_returns_the_issued_token_and_account() {
            let server = MockServer::start_async().await;
            let mock = server
                .mock_async(|when, then| {
                    when.method(POST)
                        .path("/cli/device/token")
                        .header("content-type", "application/x-www-form-urlencoded")
                        .body("device_code=some-device-code");
                    then.status(200)
                        .body(r#"{"token":"some-web-token","username":"webuser"}"#);
                })
                .await;
            let poll = client_for(&server)
                .await
                .poll("some-device-code")
                .await
                .unwrap();
            assert_eq!(
                poll,
                TokenPoll::Issued {
                    token: "some-web-token".into(),
                    username: "webuser".into()
                }
            );
            mock.assert_async().await;
        }

        #[tokio::test]
        async fn poll_maps_each_grant_error_code() {
            for (code, expected) in [
                ("authorization_pending", TokenPoll::Pending),
                ("slow_down", TokenPoll::SlowDown),
                ("access_denied", TokenPoll::Denied),
                ("expired_token", TokenPoll::Expired),
            ] {
                let server = MockServer::start_async().await;
                server
                    .mock_async(|when, then| {
                        when.method(POST).path("/cli/device/token");
                        then.status(400).body(format!(r#"{{"error":"{code}"}}"#));
                    })
                    .await;
                let poll = client_for(&server)
                    .await
                    .poll("some-device-code")
                    .await
                    .unwrap();
                assert_eq!(poll, expected, "for error code {code}");
            }
        }

        #[tokio::test]
        async fn poll_rejects_an_unknown_error_code() {
            let server = MockServer::start_async().await;
            server
                .mock_async(|when, then| {
                    when.method(POST).path("/cli/device/token");
                    then.status(400).body(r#"{"error":"mystery"}"#);
                })
                .await;
            let err = format!(
                "{:#}",
                client_for(&server)
                    .await
                    .poll("some-device-code")
                    .await
                    .unwrap_err()
            );
            assert!(err.contains("mystery"), "got: {err}");
        }

        #[tokio::test]
        async fn poll_reports_an_unexpected_status() {
            let server = MockServer::start_async().await;
            server
                .mock_async(|when, then| {
                    when.method(POST).path("/cli/device/token");
                    then.status(500);
                })
                .await;
            let err = format!(
                "{:#}",
                client_for(&server)
                    .await
                    .poll("some-device-code")
                    .await
                    .unwrap_err()
            );
            assert!(err.contains("HTTP 500"), "got: {err}");
        }

        #[test]
        fn for_registry_pins_the_https_base() {
            let client =
                RealDeviceAuthClient::for_registry("hub.lns.run", "0.16.0", &darwin_platform())
                    .unwrap();
            assert_eq!(client.base, "https://hub.lns.run");
        }

        #[tokio::test]
        async fn every_request_identifies_the_cli_with_the_standard_user_agent() {
            let server = MockServer::start_async().await;
            server
                .mock_async(|when, then| {
                    when.method(POST).path("/cli/device/code").header_matches(
                        "user-agent",
                        r"^lns/0\.16\.0 \(os=Darwin; arch=arm64; kernel=Darwin/24\.6\.0; shell=zsh; method=cli-login\)$",
                    );
                    then.status(200).body(authorization_json());
                })
                .await;
            let start = client_for(&server).await.start().await.unwrap();
            assert_eq!(
                start,
                DeviceStart::Started(super::super::tests::authorization())
            );
        }
    }
}
