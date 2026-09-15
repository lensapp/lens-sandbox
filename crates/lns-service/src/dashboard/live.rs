use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(not(target_os = "macos"))]
pub mod desktop;

static GENERATION: AtomicU64 = AtomicU64::new(0);
static CHANGES: std::sync::LazyLock<tokio::sync::watch::Sender<()>> =
    std::sync::LazyLock::new(|| tokio::sync::watch::channel(()).0);

pub fn subscribe() -> tokio::sync::watch::Receiver<()> {
    CHANGES.subscribe()
}

pub fn note_write() {
    GENERATION.fetch_add(1, Ordering::Release);
    CHANGES.send_replace(());
    #[cfg(not(target_os = "macos"))]
    desktop::repaint_if_watching();
}

pub fn generation() -> u64 {
    GENERATION.load(Ordering::Acquire)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_clients_are_woken_when_dashboard_data_changes() {
        let mut receiver = subscribe();
        receiver.borrow_and_update();
        note_write();
        assert!(
            receiver.has_changed().unwrap(),
            "a native dashboard must learn about writes without polling files"
        );
    }

    #[test]
    fn note_write_advances_the_generation() {
        let before = generation();
        note_write();
        assert!(generation() > before);
    }
}
