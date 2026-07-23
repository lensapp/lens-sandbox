use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::FromArgMatches;
use lns_ipc::{Request, Response, decode_frame, encode_frame, read_frame_bytes_async};
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;

use lns_artifact::build::BuiltArtifact;

use super::sign_in::{BindOutcome, ConnectorSignIn, LocalBoxFuture, SignInOutcome};
use super::{ConnectorPublisher, ConnectorPuller, PullReport};
use crate::command::{RunCtx, RunFuture};

pub fn run<'a>(matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = super::ConnectorArgs::from_arg_matches(matches)?;
        let cwd = ctx.cwd()?;
        let catalog_path = lns_policy::connectors::default_connectors_path();
        let pulled_path = lns_policy::pulled::default_pulled_connectors_path();
        let socket = crate::service::socket_path()?;
        let signin = RealConnectorSignIn::new(socket.clone());
        let publisher = RealConnectorPublisher;
        let puller = RealConnectorPuller::new(socket);
        let mut out = ctx.out;
        crate::connector::run(
            &args.command,
            &cwd,
            &catalog_path,
            &pulled_path,
            &signin,
            &publisher,
            &puller,
            &mut out,
        )
        .await
    })
}

pub struct RealConnectorPublisher;

impl ConnectorPublisher for RealConnectorPublisher {
    fn push<'a>(
        &'a self,
        built: &'a BuiltArtifact,
        reference: &'a str,
    ) -> LocalBoxFuture<'a, Result<()>> {
        Box::pin(async move { crate::build::push::push_artifact(built, reference).await })
    }
}

pub struct RealConnectorPuller {
    socket: PathBuf,
}

impl RealConnectorPuller {
    pub fn new(socket: PathBuf) -> Self {
        Self { socket }
    }

    async fn request(&self, request: Request) -> Result<Response> {
        let mut stream = UnixStream::connect(&self.socket).await.with_context(|| {
            "the background service must be running to pull connectors; start it with `lns service start`".to_string()
        })?;
        let frame = encode_frame(&request).context("encoding connector request")?;
        stream
            .write_all(&frame)
            .await
            .context("writing connector request")?;
        let bytes = read_frame_bytes_async(&mut stream)
            .await
            .context("reading connector response")?;
        decode_frame::<Response, _>(&mut &bytes[..]).context("decoding connector response")
    }
}

impl ConnectorPuller for RealConnectorPuller {
    fn pull<'a>(
        &'a self,
        reference: &'a str,
        confirm_replace: bool,
    ) -> LocalBoxFuture<'a, Result<PullReport>> {
        Box::pin(async move {
            match self
                .request(Request::PullConnector {
                    reference: reference.to_string(),
                    confirm_replace,
                })
                .await?
            {
                Response::ConnectorPulled {
                    id,
                    config_digest,
                    replaced,
                } => Ok(PullReport::Pulled {
                    id,
                    config_digest,
                    replaced,
                }),
                Response::ConnectorReplaceNeedsConfirm { id, changes } => {
                    Ok(PullReport::NeedsConfirm { id, changes })
                }
                Response::Error { message } => bail!("{message}"),
                other => bail!("unexpected response during connector pull: {other:?}"),
            }
        })
    }

    fn remove<'a>(&'a self, id: &'a str) -> LocalBoxFuture<'a, Result<()>> {
        Box::pin(async move {
            match self
                .request(Request::RemoveConnector { id: id.to_string() })
                .await?
            {
                Response::ConnectorRemoved { .. } => Ok(()),
                Response::Error { message } => bail!("{message}"),
                other => bail!("unexpected response during connector remove: {other:?}"),
            }
        })
    }
}

pub struct RealConnectorSignIn {
    socket: PathBuf,
}

impl RealConnectorSignIn {
    pub fn new(socket: PathBuf) -> Self {
        Self { socket }
    }
}

impl ConnectorSignIn for RealConnectorSignIn {
    fn sign_in<'a>(
        &'a self,
        id: &'a str,
        out: &'a mut dyn Write,
    ) -> LocalBoxFuture<'a, Result<SignInOutcome>> {
        Box::pin(async move {
            let Ok(mut stream) = UnixStream::connect(&self.socket).await else {
                return Ok(SignInOutcome::ServiceUnavailable);
            };
            let frame = encode_frame(&Request::BeginConnectorSignIn { id: id.to_string() })
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
            let frame = encode_frame(&Request::BindConnectorCredential { id: id.to_string() })
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
}
