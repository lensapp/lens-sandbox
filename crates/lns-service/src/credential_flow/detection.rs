pub trait HostDetector: std::fmt::Debug + Send + Sync {
    fn detect(&self) -> Option<String>;
}

/// Empty values are treated as absent so `GITHUB_TOKEN=` doesn't masquerade as a detected credential.
#[derive(Debug)]
pub struct EnvVarDetector {
    pub var_name: String,
}

impl EnvVarDetector {
    pub fn new(var_name: impl Into<String>) -> Self {
        Self {
            var_name: var_name.into(),
        }
    }
}

impl HostDetector for EnvVarDetector {
    fn detect(&self) -> Option<String> {
        std::env::var(&self.var_name).ok().filter(|s| !s.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_env::EnvVarGuard;

    #[test]
    #[serial_test::serial(env)]
    fn env_var_detector_returns_value_when_env_is_set() {
        let _g = EnvVarGuard::set("LNS_TEST_CRED_DETECTION_PRESENT", "ghp_real");
        let d = EnvVarDetector::new("LNS_TEST_CRED_DETECTION_PRESENT");
        assert_eq!(d.detect().as_deref(), Some("ghp_real"));
    }

    #[test]
    #[serial_test::serial(env)]
    fn env_var_detector_returns_none_when_env_is_unset() {
        let _g = EnvVarGuard::unset("LNS_TEST_CRED_DETECTION_ABSENT");
        let d = EnvVarDetector::new("LNS_TEST_CRED_DETECTION_ABSENT");
        assert_eq!(d.detect(), None);
    }

    #[test]
    #[serial_test::serial(env)]
    fn env_var_detector_treats_empty_value_as_absent() {
        let _g = EnvVarGuard::set("LNS_TEST_CRED_DETECTION_EMPTY", "");
        let d = EnvVarDetector::new("LNS_TEST_CRED_DETECTION_EMPTY");
        assert_eq!(d.detect(), None);
    }
}
