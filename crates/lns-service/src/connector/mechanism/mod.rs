//! One interface, several adapters (`docs/sandbox-spec.md` §3.2.6).
//!
//! `token` is a mechanism lns implements; a `code` method is the same interface
//! with the author's implementation behind it. The types here are the Rust
//! spelling of `wit/connector.wit`, so a native adapter and a component adapter
//! answer the same shapes.

pub mod host;
pub mod token;
pub mod traits;
pub mod wasm;

use std::collections::{BTreeMap, BTreeSet};

use lns_artifact::connector::CodeAuth;

/// One value a component asks the user for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub label: String,
    pub secret: bool,
}

pub type Answers = BTreeMap<String, String>;

/// What a finished connect produced. `expires_at_millis` is what the mechanism believes; lns owns the schedule and a renewal may not shorten it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Outcome {
    pub values: Answers,
    pub authority: BTreeSet<String>,
    pub expires_at_millis: Option<u64>,
}

/// One turn of a connect. A mechanism that needs nothing from the user answers `Done` on its first call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Ask {
        /// The connector author's words, not lns's, so whatever shows them says whose they are (§3.2.6).
        message: String,
        fields: Vec<Field>,
        state: Vec<u8>,
    },
    Done(Outcome),
    Failed(String),
}

/// A call the host refused is told apart from one that failed, so a component can distinguish a bound it crossed from a network that was down.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallError {
    Refused(String),
    Failed(String),
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(why) => write!(f, "refused: {why}"),
            Self::Failed(why) => write!(f, "failed: {why}"),
        }
    }
}

/// What lns enforces around a mechanism it cannot read (§3.2.6). Never read from the mechanism, only from the method that declared it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bounds {
    pub hosts: Vec<String>,
    pub exec: bool,
    pub call_seconds: u32,
    pub session_seconds: u32,
}

impl Default for Bounds {
    /// A mechanism lns implements reaches nothing on its own behalf and runs nothing.
    fn default() -> Self {
        Self {
            hosts: Vec::new(),
            exec: false,
            call_seconds: 0,
            session_seconds: 0,
        }
    }
}

impl Bounds {
    pub fn of(code: &CodeAuth) -> Self {
        let limits = code.limits();
        Self {
            hosts: code.hosts.clone(),
            exec: code.exec,
            call_seconds: limits.call_seconds,
            session_seconds: limits.session_seconds,
        }
    }

    /// A bound that names a port holds only against that port; one that names none holds against whichever the URL reaches, the way a `match` entry does.
    pub fn allows(&self, host: &str, port: &str) -> bool {
        self.hosts.iter().any(|pattern| {
            let (name, only) = lns_policy::matching::split_destination(pattern);
            lns_policy::matching::domain_matches(name, host) && only.is_none_or(|p| p == port)
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecOutput {
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

#[cfg(test)]
mod tests;
