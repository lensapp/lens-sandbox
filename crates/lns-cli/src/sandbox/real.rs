use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use lns_ipc::{Request, Response, encode_frame};
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;

use super::{SandboxService, TermInfo, run_with_writers};
use crate::service::client::BoxFuture;

pub struct RealSandboxService {
    socket: PathBuf,
}

impl SandboxService for RealSandboxService {
    type Stream = UnixStream;

    fn one_shot(&self, request: Request) -> BoxFuture<'_, Result<Response>> {
        Box::pin(async move {
            crate::service::real::send_request(&self.socket, &request)
                .await
                .ok_or_else(|| anyhow::anyhow!("no response from lns-service (is it running?)"))
        })
    }

    fn open_stream(&self, request: Request) -> BoxFuture<'_, Result<UnixStream>> {
        Box::pin(async move {
            let mut stream = UnixStream::connect(&self.socket)
                .await
                .with_context(|| format!("connecting to {}", self.socket.display()))?;
            let frame = encode_frame(&request).context("encoding stream request")?;
            stream
                .write_all(&frame)
                .await
                .context("writing stream request")?;
            Ok(stream)
        })
    }

    fn aux_socket(&self) -> Option<PathBuf> {
        Some(self.socket.clone())
    }

    fn load_policy(&self, path: &str) -> Option<serde_json::Value> {
        let path = Path::new(path);
        if !path.exists() {
            return None;
        }
        let policy = lns_policy::Policy::load_or_default(path).ok()?;
        serde_json::to_value(&policy).ok()
    }
}

pub async fn dispatch(args: &crate::cli::SandboxArgs) -> Result<i32> {
    let svc = RealSandboxService {
        socket: crate::service::socket_path()?,
    };
    let term = TermInfo {
        stdin_is_tty: crate::raw_mode::stdin_is_tty(),
        stdout_is_terminal: std::io::stdout().is_terminal(),
    };
    let mut out = std::io::stdout();
    let mut stdout = tokio::io::stdout();
    let mut stderr = tokio::io::stderr();
    run_with_writers(
        &args.command,
        &svc,
        term,
        &mut out,
        &mut stdout,
        &mut stderr,
    )
    .await
}
