//! Host-bind KEEP/DROP decisions live in `~/.lns-host-bind-decisions.json` — a KEEP exposes a real secret to the workload, which is a per-machine risk acceptance, not a shareable rule.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::decision_store::{DecisionFile, DecisionStore, JsonDecisionStore, default_path};

const OVERRIDE_ENV: &str = "LNS_HOST_BIND_DECISIONS_PATH";
const FILENAME: &str = ".lns-host-bind-decisions.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SecretDisposition {
    Keep,
    Drop,
}

pub type HostBindDecisionFile = DecisionFile<SecretDisposition>;
pub type HostBindDecisionStore = dyn DecisionStore<SecretDisposition>;
pub type JsonFileHostBindDecisionStore = JsonDecisionStore<SecretDisposition>;

pub fn default_host_bind_decisions_path() -> PathBuf {
    default_path(OVERRIDE_ENV, FILENAME)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn dispositions_serialize_to_kebab_case() {
        assert_eq!(
            serde_json::to_value(SecretDisposition::Keep).unwrap(),
            json!("keep")
        );
        assert_eq!(
            serde_json::to_value(SecretDisposition::Drop).unwrap(),
            json!("drop")
        );
    }

    #[test]
    fn an_unknown_disposition_is_an_error_rather_than_a_default() {
        let parsed: serde_json::Result<SecretDisposition> = serde_json::from_str(r#""mask""#);
        assert!(
            parsed.is_err(),
            "a decision file naming a disposition this version does not know must fail loudly, never read as keep"
        );
    }

    #[test]
    #[serial_test::serial(env)]
    fn the_decisions_file_is_a_home_dotfile_of_its_own() {
        use crate::test_env::EnvVarGuard;
        let _g1 = EnvVarGuard::unset(OVERRIDE_ENV);
        let _g2 = EnvVarGuard::set("HOME", "/home/dev");
        assert_eq!(
            default_host_bind_decisions_path(),
            PathBuf::from("/home/dev/.lns-host-bind-decisions.json"),
            "keep/drop answers must not share a file with the host-path answers"
        );
    }
}
