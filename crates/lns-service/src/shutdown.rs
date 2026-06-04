use std::sync::Condvar;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::Notify;

#[derive(Default)]
pub struct Shutdown {
    flag: AtomicBool,
    notify: Notify,
    sync_mutex: Mutex<()>,
    sync_cv: Condvar,
}

impl Shutdown {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_set(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    pub fn signal(&self) {
        if self.flag.swap(true, Ordering::SeqCst) {
            return;
        }
        self.notify.notify_waiters();
        let _guard = self.sync_mutex.lock().expect("shutdown mutex poisoned");
        self.sync_cv.notify_all();
    }

    pub fn wait_sync(&self) {
        let mut guard = self.sync_mutex.lock().expect("shutdown mutex poisoned");
        while !self.is_set() {
            guard = self.sync_cv.wait(guard).expect("shutdown condvar poisoned");
        }
    }

    pub async fn wait_async(&self) {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.is_set() {
                return;
            }
            notified.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::time::Duration;

    #[test]
    fn signal_unblocks_wait_sync() {
        let shutdown = Arc::new(Shutdown::new());
        let s2 = shutdown.clone();
        let handle = std::thread::spawn(move || s2.wait_sync());
        std::thread::sleep(Duration::from_millis(50));
        assert!(!handle.is_finished(), "wait_sync must block until signal");
        shutdown.signal();
        handle.join().expect("waiter thread panicked");
    }

    #[tokio::test]
    async fn signal_unblocks_wait_async() {
        let shutdown = Arc::new(Shutdown::new());
        let s2 = shutdown.clone();
        let waiter = tokio::spawn(async move { s2.wait_async().await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            !waiter.is_finished(),
            "wait_async must suspend until signal"
        );
        shutdown.signal();
        waiter.await.expect("waiter task panicked");
    }

    #[tokio::test]
    async fn double_signal_is_idempotent() {
        let shutdown = Shutdown::new();
        shutdown.signal();
        shutdown.signal();
        assert!(shutdown.is_set());
    }

    #[tokio::test]
    async fn wait_async_returns_immediately_if_already_signaled() {
        let shutdown = Shutdown::new();
        shutdown.signal();
        tokio::time::timeout(Duration::from_millis(50), shutdown.wait_async())
            .await
            .expect("wait_async should return immediately when already signaled");
    }

    #[test]
    fn wait_sync_returns_immediately_if_already_signaled() {
        let shutdown = Shutdown::new();
        shutdown.signal();
        shutdown.wait_sync();
    }
}
