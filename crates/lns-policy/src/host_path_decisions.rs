//! Whether a pulled sandbox may read one of this machine's files lives in `~/.lns-host-path-decisions.json` — a `hostPath` makes what a document mounts depend on the machine running it, which is a risk one developer accepts on one computer, not a rule a directory keeps.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::decision_store::{DecisionFile, DecisionStore, JsonDecisionStore, default_path};

const OVERRIDE_ENV: &str = "LNS_HOST_PATH_DECISIONS_PATH";
const FILENAME: &str = ".lns-host-path-decisions.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HostPathDecision {
    Allow,
    Deny,
}

pub type HostPathDecisionFile = DecisionFile<HostPathDecision>;
pub type HostPathDecisionStore = dyn DecisionStore<HostPathDecision>;
pub type JsonFileHostPathDecisionStore = JsonDecisionStore<HostPathDecision>;

/// The repository, tag and digest stripped, so a version bump keeps the answer and a different sandbox never inherits it.
pub fn decision_key(reference: &str, host_path: &str) -> String {
    format!("{}|{host_path}", repository_of(reference))
}

/// The reference without its tag or digest — what a decision is keyed on, and what the prompt names so the developer sees which artifact is asking.
pub fn repository_of(reference: &str) -> &str {
    let reference = match reference.split_once('@') {
        Some((repository, _digest)) => repository,
        None => reference,
    };
    match reference.rsplit_once(':') {
        Some((repository, tag)) if !tag.contains('/') => repository,
        _ => reference,
    }
}

pub fn default_host_path_decisions_path() -> PathBuf {
    default_path(OVERRIDE_ENV, FILENAME)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn decisions_serialize_to_kebab_case() {
        assert_eq!(
            serde_json::to_value(HostPathDecision::Allow).unwrap(),
            json!("allow")
        );
        assert_eq!(
            serde_json::to_value(HostPathDecision::Deny).unwrap(),
            json!("deny")
        );
    }

    #[test]
    fn an_unknown_answer_is_an_error_rather_than_a_default() {
        let parsed: serde_json::Result<HostPathDecision> = serde_json::from_str(r#""ask""#);
        assert!(
            parsed.is_err(),
            "a decision file naming an answer this version does not know must fail loudly, never read as allow"
        );
    }

    #[test]
    fn a_tag_bump_keeps_the_same_key() {
        assert_eq!(
            decision_key("ghcr.io/team/hermes:1.4.0", "~/.gitconfig"),
            decision_key("ghcr.io/team/hermes:2.0.0", "~/.gitconfig")
        );
    }

    #[test]
    fn a_digest_pin_keeps_the_same_key_as_its_tag() {
        assert_eq!(
            decision_key(
                &format!("ghcr.io/team/hermes@sha256:{}", "a".repeat(64)),
                "~/.gitconfig"
            ),
            decision_key("ghcr.io/team/hermes:1.4.0", "~/.gitconfig")
        );
    }

    #[test]
    fn a_different_repository_is_a_different_key() {
        assert_ne!(
            decision_key("ghcr.io/other/agent:1.0.0", "~/.gitconfig"),
            decision_key("ghcr.io/team/hermes:1.0.0", "~/.gitconfig")
        );
    }

    #[test]
    fn a_different_host_path_is_a_different_key() {
        assert_ne!(
            decision_key("ghcr.io/team/hermes:1.0.0", "~/.gitconfig"),
            decision_key("ghcr.io/team/hermes:1.0.0", "~/.npmrc")
        );
    }

    #[test]
    fn a_registry_port_is_not_read_as_a_tag() {
        assert_eq!(
            decision_key("localhost:5000/team/hermes", "~/.gitconfig"),
            "localhost:5000/team/hermes|~/.gitconfig"
        );
    }

    #[test]
    fn a_reference_with_no_tag_keys_on_itself() {
        assert_eq!(
            decision_key("ghcr.io/team/hermes", "~/.gitconfig"),
            "ghcr.io/team/hermes|~/.gitconfig"
        );
    }

    #[test]
    #[serial_test::serial(env)]
    fn the_decisions_file_is_a_home_dotfile_of_its_own() {
        use crate::test_env::EnvVarGuard;
        let _g1 = EnvVarGuard::unset(OVERRIDE_ENV);
        let _g2 = EnvVarGuard::set("HOME", "/home/dev");
        assert_eq!(
            default_host_path_decisions_path(),
            PathBuf::from("/home/dev/.lns-host-path-decisions.json"),
            "host-path answers must not share a file with the host-bind keep/drop answers"
        );
    }
}
