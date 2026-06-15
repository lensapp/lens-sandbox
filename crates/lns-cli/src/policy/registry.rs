use std::future::Future;
use std::pin::Pin;

/// Non-`Send` boxed future: `lns` drives policy push/pull on the root (block_on) future, never a spawned task.
pub type LocalBoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

/// Pushes/pulls a policy artifact through the running service, which owns the OCI registry client.
pub trait PolicyRegistry {
    fn push<'a>(
        &'a self,
        reference: &'a str,
        config_blob: &'a [u8],
    ) -> LocalBoxFuture<'a, anyhow::Result<String>>;

    fn pull<'a>(
        &'a self,
        reference: &'a str,
    ) -> LocalBoxFuture<'a, anyhow::Result<(Vec<u8>, String)>>;
}
