pub mod browser;
pub mod token;

use lns_ipc::OAuthProgress;

#[derive(Clone, PartialEq, Eq)]
pub struct Pending {
    pub progress: OAuthProgress,
    pub state: Vec<u8>,
    pub next_at_millis: u64,
    pub expires_at_millis: u64,
}

impl std::fmt::Debug for Pending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pending")
            .field("progress", &self.progress)
            .field("state", &"<redacted>")
            .finish()
    }
}

pub enum Advance {
    Pending(Pending),
    Done(super::Outcome),
}

pub mod flow;
