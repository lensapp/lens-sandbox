use lns_policy::Policy;
use lns_service::artifact::audit::{BundleRunAudit, ComponentDigest, WithOverrideRecord};
use lns_service::artifact::policy::{EffectivePolicy, PolicySource};
use lns_service::artifact::signature::Verdict;

#[derive(Debug)]
pub struct AuditRig {
    pub bundle_digest: String,
    pub components: Vec<ComponentDigest>,
    pub with_overrides: Vec<WithOverrideRecord>,
    pub effective: EffectivePolicy,
    pub policy: Policy,
    pub integrations: Vec<String>,
    pub verdict: Verdict,
    pub record: Option<BundleRunAudit>,
}

impl Default for AuditRig {
    fn default() -> Self {
        Self {
            bundle_digest:
                "sha256:bundle0000000000000000000000000000000000000000000000000000000000".into(),
            components: Vec::new(),
            with_overrides: Vec::new(),
            effective: EffectivePolicy {
                baseline: PolicySource::BundleShipped,
                overlay: None,
                replaced_bundle_baseline: false,
            },
            policy: Policy::default(),
            integrations: Vec::new(),
            verdict: Verdict::Skipped,
            record: None,
        }
    }
}
