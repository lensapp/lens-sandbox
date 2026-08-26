use std::time::{SystemTime, UNIX_EPOCH};

/// Wall-clock seconds, injected so anything that stamps a record is host-tested without a real clock.
pub trait Clock: Send + Sync {
    fn now_unix(&self) -> u64;
}

pub struct RealClock;

impl Clock for RealClock {
    fn now_unix(&self) -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }
}
