use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use lns_ipc::{Request, Response, decode_frame, encode_frame};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

use super::registry::{LocalBoxFuture, PolicyRegistry};

pub struct RealPolicyRegistry {
    socket: PathBuf,
}

impl RealPolicyRegistry {
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

impl PolicyRegistry for RealPolicyRegistry {
    fn push<'a>(
        &'a self,
        reference: &'a str,
        config_blob: &'a [u8],
    ) -> LocalBoxFuture<'a, Result<String>> {
        Box::pin(async move {
            let request = Request::PolicyPush {
                reference: reference.to_string(),
                config_blob: config_blob.to_vec(),
            };
            match self.round_trip(request).await? {
                Response::PolicyPushed { digest } => Ok(digest),
                Response::Error { message } => bail!("{message}"),
                other => bail!("unexpected response to policy push: {other:?}"),
            }
        })
    }

    fn pull<'a>(&'a self, reference: &'a str) -> LocalBoxFuture<'a, Result<(Vec<u8>, String)>> {
        Box::pin(async move {
            let request = Request::PolicyPull {
                reference: reference.to_string(),
            };
            match self.round_trip(request).await? {
                Response::PolicyPulled {
                    config_blob,
                    digest,
                } => Ok((config_blob, digest)),
                Response::Error { message } => bail!("{message}"),
                other => bail!("unexpected response to policy pull: {other:?}"),
            }
        })
    }
}
