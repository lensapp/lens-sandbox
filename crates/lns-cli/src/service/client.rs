use anyhow::Result;
use lns_ipc::{Request, Response, StatusInfo};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait ServiceClient: Send + Sync {
    fn ping(&self) -> BoxFuture<'_, bool>;
    fn status(&self) -> BoxFuture<'_, Option<StatusInfo>>;
    fn shutdown(&self) -> BoxFuture<'_, Option<()>>;
    fn start_and_wait_for_ready(&self, total_timeout: Duration) -> BoxFuture<'_, Result<bool>>;
    fn wait_for_ready(&self, total_timeout: Duration) -> BoxFuture<'_, bool>;
    fn wait_for_stopped(&self, total_timeout: Duration) -> BoxFuture<'_, bool>;
    fn cancel_run(&self, run_id: String) -> BoxFuture<'_, ()>;
}

/// The one request surface both namespaces drive the service through: `lns artifact` for what the cache holds, `lns sandbox` for what is running.
pub trait SandboxService: Send + Sync {
    type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static;
    fn one_shot(&self, request: Request) -> BoxFuture<'_, Result<Response>>;
    fn open_stream(&self, request: Request) -> BoxFuture<'_, Result<Self::Stream>>;
    fn aux_socket(&self) -> Option<PathBuf>;
    fn load_policy(&self, path: &str) -> Option<serde_json::Value>;
    /// Where a rendered document lands. The service renders it and this machine writes it, so `lns sandbox save` never has the service reach into the user's directory.
    fn document_exists(&self, path: &std::path::Path) -> bool;
    fn write_document(&self, path: &std::path::Path, contents: &str) -> std::io::Result<()>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct TermInfo {
    pub stdin_is_tty: bool,
    pub stdout_is_terminal: bool,
}
