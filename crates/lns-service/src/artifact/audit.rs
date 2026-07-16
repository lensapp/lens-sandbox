use lns_policy::Policy;
use sha2::{Digest, Sha256};

/// A stable content hash of a policy, so the audit chain pins which effective policy governed a run.
pub fn policy_hash(policy: &Policy) -> String {
    let canonical = serde_yaml::to_string(policy).unwrap_or_default();
    format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_policy_hash_changes_when_the_policy_changes() {
        let mut policy = Policy::default();
        let before = policy_hash(&policy);
        policy.add_rule(lns_policy::RouteRule::allow_host("api.example.test"));
        assert_ne!(before, policy_hash(&policy));
        assert!(before.starts_with("sha256:"));
    }
}
