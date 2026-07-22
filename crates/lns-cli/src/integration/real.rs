use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::FromArgMatches;
use lns_ipc::{Request, Response, decode_frame, encode_frame, read_frame_bytes_async};
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;

use super::service::{
    BindOutcome, IntegrationService, LocalBoxFuture, RevokeOutcome, SignInOutcome,
};
use crate::command::{RunCtx, RunFuture};

pub fn run<'a>(matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = super::IntegrationArgs::from_arg_matches(matches)?;
        let cwd = ctx.cwd()?;
        let catalog_path = lns_policy::integrations::default_integrations_path();
        let service = RealIntegrationService::new(crate::service::socket_path()?);
        let mut out = ctx.out;
        crate::integration::run(&args.command, &cwd, &catalog_path, &service, &mut out).await
    })
}

pub struct RealIntegrationService {
    socket: PathBuf,
}

impl RealIntegrationService {
    pub fn new(socket: PathBuf) -> Self {
        Self { socket }
    }
}

impl IntegrationService for RealIntegrationService {
    fn sign_in<'a>(
        &'a self,
        id: &'a str,
        out: &'a mut dyn Write,
    ) -> LocalBoxFuture<'a, Result<SignInOutcome>> {
        Box::pin(async move {
            let Ok(mut stream) = UnixStream::connect(&self.socket).await else {
                return Ok(SignInOutcome::ServiceUnavailable);
            };
            let frame = encode_frame(&Request::BeginIntegrationSignIn { id: id.to_string() })
                .context("encoding sign-in request")?;
            stream
                .write_all(&frame)
                .await
                .context("writing sign-in request")?;
            loop {
                let bytes = read_frame_bytes_async(&mut stream)
                    .await
                    .context("reading sign-in response")?;
                match decode_frame::<Response, _>(&mut &bytes[..])
                    .context("decoding sign-in response")?
                {
                    Response::OauthVerification {
                        verification_uri,
                        user_code,
                        expires_in_secs,
                    } => {
                        writeln!(
                            out,
                            "Open {verification_uri} and enter code {user_code} (expires in {}m)",
                            expires_in_secs / 60
                        )?;
                    }
                    Response::OauthBrowserOpened { authorization_url } => {
                        writeln!(
                            out,
                            "Opening your browser to authorize… (if it didn't open, visit {authorization_url})"
                        )?;
                    }
                    Response::OauthSignInComplete => return Ok(SignInOutcome::Completed),
                    Response::OauthSignInFailed { reason } => {
                        return Ok(SignInOutcome::Failed(reason));
                    }
                    Response::Error { message } => return Ok(SignInOutcome::Failed(message)),
                    other => bail!("unexpected response during sign-in: {other:?}"),
                }
            }
        })
    }

    fn bind_credential<'a>(
        &'a self,
        id: &'a str,
        out: &'a mut dyn Write,
    ) -> LocalBoxFuture<'a, Result<BindOutcome>> {
        Box::pin(async move {
            let Ok(mut stream) = UnixStream::connect(&self.socket).await else {
                return Ok(BindOutcome::ServiceUnavailable);
            };
            let frame = encode_frame(&Request::BindIntegrationCredential { id: id.to_string() })
                .context("encoding bind request")?;
            stream
                .write_all(&frame)
                .await
                .context("writing bind request")?;
            writeln!(
                out,
                "Decide how \"{id}\" binds in the approval window (use the host value, store one, or deny)…"
            )?;
            let bytes = read_frame_bytes_async(&mut stream)
                .await
                .context("reading bind response")?;
            match decode_frame::<Response, _>(&mut &bytes[..]).context("decoding bind response")? {
                Response::CredentialBindComplete { decision } => {
                    Ok(BindOutcome::Completed(decision))
                }
                Response::CredentialBindFailed { reason } => Ok(BindOutcome::Failed(reason)),
                Response::Error { message } => Ok(BindOutcome::Failed(message)),
                other => bail!("unexpected response during bind: {other:?}"),
            }
        })
    }

    fn revoke<'a>(&'a self, id: &'a str) -> LocalBoxFuture<'a, Result<RevokeOutcome>> {
        Box::pin(async move {
            match crate::service::real::send_request(
                &self.socket,
                &Request::RevokeIntegration { id: id.to_string() },
            )
            .await
            {
                Some(Response::IntegrationRevoked { existed }) => {
                    Ok(RevokeOutcome::Cleared { existed })
                }
                Some(Response::Error { message }) => bail!("revoking {id:?} failed: {message}"),
                Some(other) => bail!("unexpected response during revoke: {other:?}"),
                None => Ok(RevokeOutcome::ServiceUnavailable),
            }
        })
    }

    fn revoke_all(&self) -> LocalBoxFuture<'_, Result<RevokeOutcome>> {
        Box::pin(async move {
            match crate::service::real::send_request(&self.socket, &Request::RevokeAllIntegrations)
                .await
            {
                Some(Response::AllIntegrationsRevoked) => Ok(RevokeOutcome::AllCleared),
                Some(Response::Error { message }) => {
                    bail!("resetting the credential store failed: {message}")
                }
                Some(other) => bail!("unexpected response during revoke: {other:?}"),
                None => Ok(RevokeOutcome::ServiceUnavailable),
            }
        })
    }
}
