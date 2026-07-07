use lns_policy::Policy;
use sha2::{Digest, Sha256};

use crate::artifact::signature::Verdict;

/// A stable content hash of a policy, so the audit chain pins which effective policy governed a run.
pub fn policy_hash(policy: &Policy) -> String {
    let canonical = serde_yaml::to_string(policy).unwrap_or_default();
    format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()))
}

pub fn verdict_label(verdict: &Verdict) -> String {
    match verdict {
        Verdict::Verified => "verified".to_string(),
        Verdict::ProceedUnverified { .. } => "unverified".to_string(),
        Verdict::Skipped => "skipped".to_string(),
        Verdict::Refused(reason) => format!("refused: {}", reason.as_message()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact::signature::RefusalReason;

    #[test]
    fn every_verdict_maps_to_a_distinct_label() {
        assert_eq!(verdict_label(&Verdict::Verified), "verified");
        assert_eq!(
            verdict_label(&Verdict::ProceedUnverified {
                warning: "w".into()
            }),
            "unverified"
        );
        assert_eq!(verdict_label(&Verdict::Skipped), "skipped");
        assert!(verdict_label(&Verdict::Refused(RefusalReason::Unsigned)).starts_with("refused:"));
    }

    #[test]
    fn the_policy_hash_changes_when_the_policy_changes() {
        let mut policy = Policy::default();
        let before = policy_hash(&policy);
        policy.add_rule(lns_policy::RouteRule::allow_host("api.example.test"));
        assert_ne!(before, policy_hash(&policy));
        assert!(before.starts_with("sha256:"));
    }
}
