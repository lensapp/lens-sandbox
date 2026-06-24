use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use lns_ipc::{Request, Response, decode_frame, encode_frame};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

use super::{Pulled, RegistryClient};
use crate::integration::LocalBoxFuture;

pub struct RealRegistryClient {
    socket: PathBuf,
}

impl RealRegistryClient {
    pub fn new(socket: PathBuf) -> Self {
        Self { socket }
    }

    async fn round_trip(&self, request: Request) -> Result<Response> {
        let mut stream = UnixStream::connect(&self.socket)
            .await
            .context("connecting to the lns-service socket")?;
        let frame = encode_frame(&request).context("encoding request")?;
        stream.write_all(&frame).await.context("writing request")?;
        stream.shutdown().await.ok();
        let mut buf = Vec::new();
        stream
            .read_to_end(&mut buf)
            .await
            .context("reading response")?;
        decode_frame::<Response, _>(&mut &buf[..]).context("decoding response")
    }
}

impl RegistryClient for RealRegistryClient {
    fn push_artifact<'a>(
        &'a self,
        reference: &'a str,
        artifact_type: &'a str,
        config_media_type: &'a str,
        config_blob: &'a [u8],
        layers: &'a [Vec<u8>],
    ) -> LocalBoxFuture<'a, Result<String>> {
        Box::pin(async move {
            let request = Request::PushArtifact {
                reference: reference.to_string(),
                artifact_type: artifact_type.to_string(),
                config_media_type: config_media_type.to_string(),
                config_blob: config_blob.to_vec(),
                layers: layers.to_vec(),
            };
            match self.round_trip(request).await? {
                Response::Pushed { digest } => Ok(digest),
                Response::Error { message } => bail!("{message}"),
                other => bail!("unexpected response to push: {other:?}"),
            }
        })
    }

    fn push_image<'a>(
        &'a self,
        source_reference: &'a str,
        target_reference: &'a str,
    ) -> LocalBoxFuture<'a, Result<String>> {
        Box::pin(async move {
            let request = Request::PushImage {
                source_reference: source_reference.to_string(),
                target_reference: target_reference.to_string(),
            };
            match self.round_trip(request).await? {
                Response::Pushed { digest } => Ok(digest),
                Response::Error { message } => bail!("{message}"),
                other => bail!("unexpected response to image push: {other:?}"),
            }
        })
    }

    fn pull<'a>(&'a self, reference: &'a str) -> LocalBoxFuture<'a, Result<Pulled>> {
        Box::pin(async move {
            let request = Request::Pull {
                reference: reference.to_string(),
            };
            match self.round_trip(request).await? {
                Response::PulledArtifact {
                    artifact_type,
                    config_blob,
                    digest,
                } => Ok(Pulled::Artifact {
                    artifact_type,
                    config_blob,
                    digest,
                }),
                Response::PulledImage { digest } => Ok(Pulled::Image { digest }),
                Response::Error { message } => bail!("{message}"),
                other => bail!("unexpected response to pull: {other:?}"),
            }
        })
    }
}
