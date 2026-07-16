use std::future::Future;
use std::io::Write;
use std::pin::Pin;

/// Non-`Send` boxed future: the sign-in driver borrows the caller's writer across awaits, and `lns` drives it on the root (block_on) future, never a spawned task.
pub type LocalBoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

/// The result of driving an oauth integration's interactive sign-in through the service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SignInOutcome {
    Completed,
    Failed(String),
    ServiceUnavailable,
}

/// The result of driving a credential integration's value decision through the service's approval window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindOutcome {
    Completed(lns_ipc::CredentialBindDecision),
    Failed(String),
    ServiceUnavailable,
}

/// Drives an oauth integration's device sign-in — or a credential integration's value-decision bind — through the running service, rendering progress to `out`.
pub trait IntegrationSignIn {
    fn sign_in<'a>(
        &'a self,
        id: &'a str,
        out: &'a mut dyn Write,
    ) -> LocalBoxFuture<'a, anyhow::Result<SignInOutcome>>;

    fn bind_credential<'a>(
        &'a self,
        id: &'a str,
        out: &'a mut dyn Write,
    ) -> LocalBoxFuture<'a, anyhow::Result<BindOutcome>>;
}
