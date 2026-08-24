//! Host-bind KEEP/DROP decisions live in `~/.lns/host-bind-decisions.json` — a KEEP exposes a real secret to the workload, which is a per-machine risk acceptance, not a shareable rule.

use serde::{Deserialize, Serialize};

use crate::decision_store::{DecisionFile, DecisionStore, JsonDecisionStore};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SecretDisposition {
    Keep,
    Drop,
}

pub type HostBindDecisionFile = DecisionFile<SecretDisposition>;
pub type HostBindDecisionStore = dyn DecisionStore<SecretDisposition>;
pub type JsonFileHostBindDecisionStore = JsonDecisionStore<SecretDisposition>;

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
}
