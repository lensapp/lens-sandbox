use anyhow::Result;
use lns_ipc::StatusInfo;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait ServiceClient: Send + Sync {
    fn ping(&self) -> BoxFuture<'_, bool>;
    fn status(&self) -> BoxFuture<'_, Option<StatusInfo>>;
    fn shutdown(&self) -> BoxFuture<'_, Option<()>>;
    fn start_and_wait_for_ready(&self, total_timeout: Duration) -> BoxFuture<'_, Result<bool>>;
    fn wait_for_ready(&self, total_timeout: Duration) -> BoxFuture<'_, bool>;
    fn wait_for_stopped(&self, total_timeout: Duration) -> BoxFuture<'_, bool>;
    fn cancel_run(&self, run_id: u32) -> BoxFuture<'_, ()>;
}
