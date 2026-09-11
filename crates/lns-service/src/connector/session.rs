//! A connect the user is still answering: what the mechanism is waiting to be
//! told, and the state it holds meanwhile (`docs/sandbox-spec.md` §3.2.6).
//!
//! The state is secret material, so it is held in memory only, never written
//! down, and only ever handed back to the component that produced it.

use std::collections::BTreeMap;
use std::sync::Mutex;

/// One connect in progress.
pub struct Session {
    pub connector: String,
    /// The bytes the connect began against. A reinstall mid-exchange is a different implementation, and state is only ever handed back to the one that produced it (§3.2.6).
    pub digest: String,
    pub method: String,
    pub label: String,
    pub state: Vec<u8>,
    pub expires_at_millis: u64,
}

impl std::fmt::Debug for Session {
    /// Hand-written so a `log::debug!` of a session cannot print the state or the answers it holds.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("connector", &self.connector)
            .field("digest", &self.digest)
            .field("method", &self.method)
            .field("label", &self.label)
            .field(
                "state",
                &format_args!("<{} bytes redacted>", self.state.len()),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests;

/// Where a connect in progress lives. A handle this store did not mint is not one it will resume.
pub trait Sessions: Send + Sync {
    /// Opening also drops what nobody came back for: an abandoned connect holds secret material, and no one will ever call `take` on it.
    fn open(&self, session: Session, now_millis: u64) -> String;
    /// The session a handle names, taken rather than read, so one handle answers once.
    fn take(&self, handle: &str, now_millis: u64) -> Option<Session>;
    /// Drops what nobody came back for. Whoever owns the clock calls it, so an abandoned connect's state has a bounded lifetime rather than one that waits for the next connect (§3.2.6).
    fn sweep(&self, now_millis: u64);
}

/// The sessions this process holds. Nothing survives a restart, which is the point.
#[derive(Default)]
pub struct InMemorySessions {
    pub(super) open: Mutex<BTreeMap<String, Session>>,
    minted: Mutex<u64>,
}

impl Sessions for InMemorySessions {
    fn open(&self, session: Session, now_millis: u64) -> String {
        let mut minted = self.minted.lock().unwrap_or_else(|e| e.into_inner());
        *minted += 1;
        let handle = format!("{}/{}/{minted}", session.connector, session.method);
        self.sweep(now_millis);
        self.open
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(handle.clone(), session);
        handle
    }

    fn sweep(&self, now_millis: u64) {
        self.open
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|_, held| held.expires_at_millis > now_millis);
    }

    fn take(&self, handle: &str, now_millis: u64) -> Option<Session> {
        let taken = self
            .open
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(handle)?;
        // An expired session is dropped rather than resumed: sessionSeconds bounds the whole exchange (§3.2.6).
        (taken.expires_at_millis > now_millis).then_some(taken)
    }
}
