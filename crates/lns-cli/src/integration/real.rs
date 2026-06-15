use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::FromArgMatches;
use lns_ipc::{Request, Response, decode_frame, encode_frame, read_frame_bytes_async};
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;

use super::sign_in::{IntegrationSignIn, LocalBoxFuture, SignInOutcome};
use crate::command::{RunCtx, RunFuture};

pub fn run<'a>(matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = crate::cli::IntegrationArgs::from_arg_matches(matches)?;
        let catalog_path = lns_policy::integrations::default_integrations_path();
        let signin = RealIntegrationSignIn::new(crate::service::socket_path()?);
        let mut out = ctx.out;
        crate::integration::run(&args.command, &ctx.cwd, &catalog_path, &signin, &mut out).await
    })
}

pub struct RealIntegrationSignIn {
    socket: PathBuf,
}

impl RealIntegrationSignIn {
    pub fn new(socket: PathBuf) -> Self {
        Self { socket }
    }
}

impl IntegrationSignIn for RealIntegrationSignIn {
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
}
