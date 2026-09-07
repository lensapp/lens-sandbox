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

/// The mechanism a method connects with, and the bounds lns holds around it. One implementation reads `token` natively; the other compiles the component the method carries (§3.2.6).
pub trait Mechanisms: Send + Sync {
    fn for_method(
        &self,
        connector: &str,
        method: &lns_artifact::connector::Method,
        component: Option<Vec<u8>>,
    ) -> Result<Prepared>;
}

/// One mechanism, ready to call, and the host it calls through.
pub struct Prepared {
    pub mechanism: Box<dyn Mechanism>,
    pub host: Host,
}

pub trait Http: Send + Sync {
    /// `within` is the deadline the method declared. An epoch tick cannot interrupt a host call already in flight, so the call has to carry it.
    fn fetch(
        &self,
        request: &HttpRequest,
        within: std::time::Duration,
    ) -> Result<HttpResponse, CallError>;
}

pub trait Exec: Send + Sync {
    fn run(&self, argv: &[String], within: std::time::Duration) -> Result<ExecOutput, CallError>;
}

pub trait Entropy: Send + Sync {
    fn bytes(&self, count: u32) -> Vec<u8>;
}

/// Where a call a mechanism made is written down. It happens outside any run, so a run's audit chain could not account for it (§8.4).
pub trait Recorder: Send + Sync {
    fn reached(&self, connector: &str, host: &str, refused: bool);
    fn ran(&self, connector: &str, program: &str, refused: bool);
    /// A renewal lns scheduled. It leaves no state behind, so the entry is the only record there will ever be that it happened (§3.2.6).
    fn renewed(&self, connector: &str, target: &str, refused: bool);
}
