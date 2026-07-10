#![cfg(test)]

use std::ffi::{OsStr, OsString};

pub struct EnvScope {
    key: &'static str,
    prev: Option<OsString>,
}

impl EnvScope {
    pub fn set(key: &'static str, value: impl AsRef<OsStr>) -> Self {
        let prev = std::env::var_os(key);
        // SAFETY: callers gate with #[serial_test::serial(env)].
        unsafe { std::env::set_var(key, value) };
        Self { key, prev }
    }

    pub fn unset(key: &'static str) -> Self {
        let prev = std::env::var_os(key);
        // SAFETY: callers gate with #[serial_test::serial(env)].
        unsafe { std::env::remove_var(key) };
        Self { key, prev }
    }
}

impl Drop for EnvScope {
    fn drop(&mut self) {
        // SAFETY: callers gate with #[serial_test::serial(env)].
        unsafe {
            match &self.prev {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
    }
}

pub(crate) fn capture_events(emit: impl FnOnce()) -> Vec<String> {
    use std::sync::{Arc, Mutex, Once};
    use tracing::field::{Field, Visit};
    use tracing::{Event, Subscriber};
    use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

    static OPEN_GATE: Once = Once::new();
    OPEN_GATE.call_once(|| {
        let gate = tracing_subscriber::fmt()
            .with_writer(std::io::sink)
            .with_max_level(tracing::Level::TRACE)
            .finish();
        tracing::subscriber::set_global_default(gate).ok();
    });

    type Sink = Arc<Mutex<Vec<String>>>;
    struct CapturingLayer(Sink);
    impl<S: Subscriber> Layer<S> for CapturingLayer {
        fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
            struct V<'a>(&'a mut String);
            impl Visit for V<'_> {
                fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
                    if field.name() == "message" {
                        *self.0 = format!("{value:?}");
                    }
                }
            }
            let mut message = String::new();
            event.record(&mut V(&mut message));
            self.0.lock().unwrap().push(message);
        }
    }

    let sink: Sink = Arc::new(Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::registry().with(CapturingLayer(sink.clone()));
    tracing::subscriber::with_default(subscriber, emit);
    let captured = sink.lock().unwrap();
    captured.clone()
}

#[cfg(test)]
mod tests {
    use super::EnvScope;

    const PROBE: &str = "LNS_TEST_ENV_SCOPE_PROBE";

    #[test]
    #[serial_test::serial(env)]
    fn restores_previous_value_on_drop() {
        let _pre = EnvScope::set(PROBE, "pre");
        {
            let _g = EnvScope::set(PROBE, "during");
            assert_eq!(std::env::var(PROBE).unwrap(), "during");
        }
        assert_eq!(std::env::var(PROBE).unwrap(), "pre");
    }

    #[test]
    #[serial_test::serial(env)]
    fn restores_unset_state_on_drop_when_no_prior_value() {
        let _ensure_unset = EnvScope::unset(PROBE);
        {
            let _g = EnvScope::set(PROBE, "during");
            assert_eq!(std::env::var(PROBE).unwrap(), "during");
        }
        assert!(std::env::var_os(PROBE).is_none());
    }
}
