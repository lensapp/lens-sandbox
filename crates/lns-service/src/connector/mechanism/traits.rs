//! The ports of `wit/connector.wit`: what a mechanism exports, and what lns
//! imports into it.

use anyhow::Result;

use super::host::Host;
use super::{Answers, CallError, ExecOutput, HttpRequest, HttpResponse, Outcome, Step};

/// What a mechanism exports. `token` implements it natively; a `code` method implements it in a component (§3.2.6).
pub trait Mechanism: Send + Sync {
    fn connect(&self, host: &Host, now_millis: u64) -> Result<Step>;
    fn resume(&self, host: &Host, state: &[u8], answers: &Answers, now_millis: u64)
    -> Result<Step>;
    fn refresh(&self, host: &Host, values: &Answers, now_millis: u64) -> Result<Outcome>;
    fn revoke(&self, host: &Host, values: &Answers, now_millis: u64) -> Result<()>;
}

pub trait Http: Send + Sync {
    fn fetch(&self, request: &HttpRequest) -> Result<HttpResponse, CallError>;
}

pub trait Exec: Send + Sync {
    fn run(&self, argv: &[String]) -> Result<ExecOutput, CallError>;
}

pub trait Entropy: Send + Sync {
    fn bytes(&self, count: u32) -> Vec<u8>;
}

/// Where a call a mechanism made is written down. It happens outside any run, so a run's audit chain could not account for it (§8.4).
pub trait Recorder: Send + Sync {
    fn reached(&self, connector: &str, host: &str, refused: bool);
    fn ran(&self, connector: &str, program: &str, refused: bool);
}
