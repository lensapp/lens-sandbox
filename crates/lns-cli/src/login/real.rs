use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::FromArgMatches;
use lns_ipc::{Request, Response, decode_frame, encode_frame, read_frame_bytes_async};
use lns_policy::registry_auth::default_registry_auth_path;
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;

use super::{LoginArgs, LoginOutcome, LogoutArgs, RegistryVerifier, WebLoginFlow, WebLoginOutcome};
use crate::command::{RunCtx, RunFuture};
use crate::connector::LocalBoxFuture;

pub fn run_login<'a>(matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = LoginArgs::from_arg_matches(matches)?;
        let default_registry = configured_default_registry()?;
        let auth_path = default_registry_auth_path();
        let verifier = RealRegistryVerifier::new(crate::service::socket_path()?);
        let input = ctx.input;
        let mut out = ctx.out;
        super::run(
            &args,
            &default_registry,
            &auth_path,
            &verifier,
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
        let auth_path = default_registry_auth_path();
        let mut out = ctx.out;
        super::logout(&args, &default_registry, &auth_path, &mut out)
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

pub struct RealRegistryVerifier {
    socket: PathBuf,
}

impl RealRegistryVerifier {
    pub fn new(socket: PathBuf) -> Self {
        Self { socket }
    }
}

impl RegistryVerifier for RealRegistryVerifier {
    fn available<'a>(&'a self) -> LocalBoxFuture<'a, Result<bool>> {
        Box::pin(async move {
            let Ok(mut stream) = UnixStream::connect(&self.socket).await else {
                return Ok(false);
            };
            let frame = encode_frame(&Request::Ping).context("encoding ping request")?;
            stream
                .write_all(&frame)
                .await
                .context("writing ping request")?;
            let bytes = read_frame_bytes_async(&mut stream)
                .await
                .context("reading ping response")?;
            let response =
                decode_frame::<Response, _>(&mut &bytes[..]).context("decoding ping response")?;
            Ok(matches!(response, Response::Pong))
        })
    }

    fn verify<'a>(
        &'a self,
        registry: &'a str,
        username: &'a str,
        secret: &'a str,
    ) -> LocalBoxFuture<'a, Result<LoginOutcome>> {
        Box::pin(async move {
            let Ok(mut stream) = UnixStream::connect(&self.socket).await else {
                return Ok(LoginOutcome::ServiceUnavailable);
            };
            let frame = encode_frame(&Request::RegistryLogin {
                registry: registry.to_string(),
                username: username.to_string(),
                secret: secret.to_string(),
            })
            .context("encoding login request")?;
            stream
                .write_all(&frame)
                .await
                .context("writing login request")?;
            let bytes = read_frame_bytes_async(&mut stream)
                .await
                .context("reading login response")?;
            match decode_frame::<Response, _>(&mut &bytes[..]).context("decoding login response")? {
                Response::RegistryLoginVerified => Ok(LoginOutcome::Verified),
                Response::Error { message } => Ok(LoginOutcome::Rejected(message)),
                other => bail!("unexpected response during login: {other:?}"),
            }
        })
    }
}
