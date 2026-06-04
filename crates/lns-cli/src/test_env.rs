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
