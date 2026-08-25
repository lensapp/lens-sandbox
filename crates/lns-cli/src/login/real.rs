use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::FromArgMatches;
use lns_ipc::{Request, Response, decode_frame, encode_frame, read_frame_bytes_async};
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;

use super::{
    ListLoginsOutcome, LoginArgs, LoginOutcome, LogoutArgs, LogoutOutcome, RegistryAuthClient,
    WebLoginFlow, WebLoginOutcome,
};
use crate::command::{RunCtx, RunFuture};
use crate::connector::LocalBoxFuture;

pub fn run_login<'a>(matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = LoginArgs::from_arg_matches(matches)?;
        let default_registry = configured_default_registry()?;
        let client = RealRegistryAuthClient::new(crate::service::socket_path()?);
        let input = ctx.input;
        let mut out = ctx.out;
        super::run(
            &args,
            &default_registry,
            &client,
            &RealWebLoginFlow,
            input,
            &mut out,
        )
        .await
    })
}

pub fn run_logout<'a>(matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = LogoutArgs::from_arg_matches(matches)?;
        let default_registry = configured_default_registry()?;
        let client = RealRegistryAuthClient::new(crate::service::socket_path()?);
        let mut out = ctx.out;
        super::logout(&args, &default_registry, &client, &mut out).await
    })
}

fn configured_default_registry() -> Result<String> {
    let path = crate::config::default_config_path()?;
    let defaults = crate::config::load_run_defaults(&path)?;
    Ok(defaults.registry_or_default().to_string())
}

/// Opens `url` via the platform opener (`open` on macOS, `xdg-open` elsewhere), reporting whether it spawned.
struct RealBrowserOpener;

impl super::BrowserOpener for RealBrowserOpener {
    fn open(&self, url: &str) -> bool {
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        std::process::Command::new(opener).arg(url).spawn().is_ok()
    }
}

/// Builds the reqwest device-auth client for the registry and drives the browser device flow with it.
struct RealWebLoginFlow;

impl WebLoginFlow for RealWebLoginFlow {
    fn login<'a>(
        &'a self,
        registry: &'a str,
        out: &'a mut dyn std::io::Write,
    ) -> LocalBoxFuture<'a, Result<WebLoginOutcome>> {
        Box::pin(async move {
            let client = super::RealDeviceAuthClient::for_registry(
                registry,
                env!("CARGO_PKG_VERSION"),
                &crate::platform::detect(),
            )?;
            let flow = super::WebLogin::new(client, RealBrowserOpener);
            flow.login(registry, out).await
        })
    }
}

pub struct RealRegistryAuthClient {
    socket: PathBuf,
}

impl RealRegistryAuthClient {
    pub fn new(socket: PathBuf) -> Self {
        Self { socket }
    }

    async fn roundtrip(&self, request: &Request, what: &str) -> Result<Option<Response>> {
        let Ok(mut stream) = UnixStream::connect(&self.socket).await else {
            return Ok(None);
        };
        let frame = encode_frame(request).with_context(|| format!("encoding {what} request"))?;
        stream
            .write_all(&frame)
            .await
            .with_context(|| format!("writing {what} request"))?;
        let bytes = read_frame_bytes_async(&mut stream)
            .await
            .with_context(|| format!("reading {what} response"))?;
        let response = decode_frame::<Response, _>(&mut &bytes[..])
            .with_context(|| format!("decoding {what} response"))?;
        Ok(Some(response))
    }
}

impl RegistryAuthClient for RealRegistryAuthClient {
    fn available<'a>(&'a self) -> LocalBoxFuture<'a, Result<bool>> {
        Box::pin(async move {
            match self.roundtrip(&Request::Ping, "ping").await? {
                Some(response) => Ok(matches!(response, Response::Pong)),
                None => Ok(false),
            }
        })
    }

    fn login<'a>(
        &'a self,
        registry: &'a str,
        username: &'a str,
        secret: &'a str,
    ) -> LocalBoxFuture<'a, Result<LoginOutcome>> {
        Box::pin(async move {
            let request = Request::RegistryLogin {
                registry: registry.to_string(),
                username: username.to_string(),
                secret: secret.to_string(),
            };
            match self.roundtrip(&request, "login").await? {
                None => Ok(LoginOutcome::ServiceUnavailable),
                Some(Response::RegistryLoginStored) => Ok(LoginOutcome::Stored),
                Some(Response::Error { message }) => Ok(LoginOutcome::Rejected(message)),
                Some(other) => bail!("unexpected response during login: {other:?}"),
            }
        })
    }

    fn logout<'a>(&'a self, registry: &'a str) -> LocalBoxFuture<'a, Result<LogoutOutcome>> {
        Box::pin(async move {
            let request = Request::RegistryLogout {
                registry: registry.to_string(),
            };
            match self.roundtrip(&request, "logout").await? {
                None => Ok(LogoutOutcome::ServiceUnavailable),
                Some(Response::RegistryLoggedOut) => Ok(LogoutOutcome::LoggedOut),
                Some(Response::Error { message }) => Ok(LogoutOutcome::Failed(message)),
                Some(other) => bail!("unexpected response during logout: {other:?}"),
            }
        })
    }

    fn list<'a>(&'a self) -> LocalBoxFuture<'a, Result<ListLoginsOutcome>> {
        Box::pin(async move {
            match self
                .roundtrip(&Request::ListRegistryLogins, "login list")
                .await?
            {
                None => Ok(ListLoginsOutcome::ServiceUnavailable),
                Some(Response::RegistryLogins { logins }) => Ok(ListLoginsOutcome::Logins(logins)),
                Some(Response::Error { message }) => bail!("listing registry logins: {message}"),
                Some(other) => bail!("unexpected response during login list: {other:?}"),
            }
        })
    }
}
