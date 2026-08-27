#![cfg(test)]

pub(crate) struct EnvVarGuard {
    key: &'static str,
    previous: Option<std::ffi::OsString>,
}

impl EnvVarGuard {
    pub(crate) fn capture(key: &'static str) -> Self {
        Self {
            key,
            previous: std::env::var_os(key),
        }
    }

    pub(crate) fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let guard = Self::capture(key);
        // SAFETY: caller is `#[serial(env)]`; rollback happens in Drop.
        unsafe {
            std::env::set_var(key, value);
        }
        guard
    }

    pub(crate) fn unset(key: &'static str) -> Self {
        let guard = Self::capture(key);
        // SAFETY: caller is `#[serial(env)]`; rollback happens in Drop.
        unsafe {
            std::env::remove_var(key);
        }
        guard
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        // SAFETY: caller holds `#[serial(env)]` for the guard's whole lifetime.
        unsafe {
            match &self.previous {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

#[derive(Default)]
struct MessageCapture(std::sync::Mutex<Vec<String>>);

struct MessageLayer(std::sync::Arc<MessageCapture>);

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for MessageLayer {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        struct Message<'a>(&'a mut String);
        impl tracing::field::Visit for Message<'_> {
            fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                if field.name() == "message" {
                    *self.0 = format!("{value:?}");
                }
            }
        }
        let mut message = String::new();
        event.record(&mut Message(&mut message));
        self.0.0.lock().unwrap().push(message);
    }
}

/// Every message `f` logs, so a test can pin what an operator is told rather than only that a branch ran.
pub(crate) fn captured_messages(f: impl FnOnce()) -> Vec<String> {
    use tracing_subscriber::layer::SubscriberExt;
    let capture = std::sync::Arc::new(MessageCapture::default());
    let subscriber =
        tracing_subscriber::registry().with(MessageLayer(std::sync::Arc::clone(&capture)));
    tracing::subscriber::with_default(subscriber, f);
    capture.0.lock().unwrap().clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_captured_message_carries_what_was_logged() {
        let messages = captured_messages(|| crate::log::warn!("something to say"));
        assert!(
            messages.iter().any(|m| m.contains("something to say")),
            "{messages:?}"
        );
    }

    #[test]
    #[serial_test::serial(env)]
    fn drop_restores_captured_value() {
        let key = "LNS_TEST_ENV_GUARD_RESTORE";
        let _outer = EnvVarGuard::set(key, "before");
        {
            let _g = EnvVarGuard::set(key, "during");
            assert_eq!(
                std::env::var_os(key).as_deref(),
                Some(std::ffi::OsStr::new("during"))
            );
        }
        assert_eq!(
            std::env::var_os(key).as_deref(),
            Some(std::ffi::OsStr::new("before"))
        );
    }

    #[test]
    #[serial_test::serial(env)]
    fn drop_removes_when_no_previous_value() {
        let key = "LNS_TEST_ENV_GUARD_REMOVE";
        let _clean = EnvVarGuard::unset(key);
        {
            let _g = EnvVarGuard::set(key, "scratch");
            assert!(std::env::var_os(key).is_some());
        }
        assert!(std::env::var_os(key).is_none());
    }

    #[test]
    #[serial_test::serial(env)]
    fn unset_removes_inside_scope_and_restores_on_drop() {
        let key = "LNS_TEST_ENV_GUARD_UNSET";
        let _outer = EnvVarGuard::set(key, "outer");
        {
            let _g = EnvVarGuard::unset(key);
            assert!(std::env::var_os(key).is_none());
        }
        assert_eq!(
            std::env::var_os(key).as_deref(),
            Some(std::ffi::OsStr::new("outer"))
        );
    }

    #[test]
    #[serial_test::serial(env)]
    fn capture_only_rolls_back_arbitrary_in_scope_mutations() {
        let key = "LNS_TEST_ENV_GUARD_CAPTURE";
        let _outer = EnvVarGuard::set(key, "initial");
        {
            let _g = EnvVarGuard::capture(key);
            // SAFETY: ad-hoc mutations exercise capture's rollback; #[serial(env)] gates the test.
            unsafe {
                std::env::set_var(key, "mutated-once");
                std::env::set_var(key, "mutated-twice");
                std::env::remove_var(key);
            }
            assert!(std::env::var_os(key).is_none());
        }
        assert_eq!(
            std::env::var_os(key).as_deref(),
            Some(std::ffi::OsStr::new("initial"))
        );
    }
}
