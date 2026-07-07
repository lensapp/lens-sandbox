use lns_policy::Policy;
use sha2::{Digest, Sha256};

use crate::artifact::policy::EffectivePolicy;
use crate::artifact::signature::Verdict;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentDigest {
    pub name: String,
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WithOverrideRecord {
    pub name: String,
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleRunAudit {
    pub bundle_digest: String,
    pub components: Vec<ComponentDigest>,
    pub with_overrides: Vec<WithOverrideRecord>,
    pub effective_policy_source: String,
    pub effective_policy_hash: String,
    pub integrations: Vec<String>,
    pub signature_verdict: String,
}

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

#[allow(clippy::too_many_arguments)]
pub fn record_bundle_run(
    bundle_digest: String,
    components: Vec<ComponentDigest>,
    with_overrides: Vec<WithOverrideRecord>,
    effective: &EffectivePolicy,
    policy: &Policy,
    integrations: Vec<String>,
    verdict: &Verdict,
) -> BundleRunAudit {
    BundleRunAudit {
        bundle_digest,
        components,
        with_overrides,
        effective_policy_source: effective.baseline.describe(),
        effective_policy_hash: policy_hash(policy),
        integrations,
        signature_verdict: verdict_label(verdict),
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
