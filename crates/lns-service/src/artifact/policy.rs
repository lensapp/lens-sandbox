use lns_policy::{NetworkPolicy, Policy, RouteRule, Verdict};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardrailFlag {
    PermissiveDefaultVerdict,
    WildcardAllow,
    BroadCidrAllow,
}

impl GuardrailFlag {
    pub fn message(self) -> &'static str {
        match self {
            GuardrailFlag::PermissiveDefaultVerdict => {
                "permissive defaultVerdict: allow — the sandbox is open by default"
            }
            GuardrailFlag::WildcardAllow => "wildcard allow (*) — every host is permitted",
            GuardrailFlag::BroadCidrAllow => {
                "broad CIDR allow — a large address range is permitted"
            }
        }
    }
}

fn is_broad_cidr(pattern: &str) -> bool {
    match pattern.split_once('/') {
        Some((_, prefix)) => prefix.parse::<u8>().map(|len| len <= 8).unwrap_or(false),
        None => false,
    }
}

pub fn guardrail_flags(policy: &Policy) -> Vec<GuardrailFlag> {
    let mut flags = Vec::new();
    if policy.network.default_verdict == Verdict::Allow {
        flags.push(GuardrailFlag::PermissiveDefaultVerdict);
    }
    for rule in &policy.network.allowed_routes {
        if rule.verdict != Verdict::Allow {
            continue;
        }
        if rule.match_pattern == "*" {
            flags.push(GuardrailFlag::WildcardAllow);
        } else if is_broad_cidr(&rule.match_pattern) {
            flags.push(GuardrailFlag::BroadCidrAllow);
        }
    }
    flags
}

pub fn run_summary(flags: &[GuardrailFlag]) -> String {
    if flags.is_empty() {
        return String::new();
    }
    let mut summary = String::from("⚠ over-broad shipped policy:");
    for flag in flags {
        summary.push_str("\n  - ");
        summary.push_str(flag.message());
    }
    summary
}

/// Merge a bundle's shipped `baseline` policy under a local `overlay` into one effective policy for the guest gate: every layer's denies are ordered first so a first-match gate stays deny-dominant, `defaultVerdict` never exceeds `ask` (a permissive bundle default is backstopped, a `deny` default is honored), and connected integrations union.
// Deny-first ordering is load-bearing: lens-sandbox-core's `find_matching_route` is first-match-wins, so a host denied by any layer must have its deny rule appear before any allow.
pub fn merge_effective(baseline: Option<&Policy>, overlay: &Policy) -> Policy {
    let layers: Vec<&Policy> = std::iter::once(overlay).chain(baseline).collect();
    let mut routes: Vec<RouteRule> = Vec::new();
    let mut push_unique = |rule: &RouteRule| {
        if !routes.contains(rule) {
            routes.push(rule.clone());
        }
    };
    for layer in &layers {
        for rule in &layer.network.allowed_routes {
            if rule.verdict == Verdict::Deny {
                push_unique(rule);
            }
        }
    }
    for layer in &layers {
        for rule in &layer.network.allowed_routes {
            if rule.verdict != Verdict::Deny {
                push_unique(rule);
            }
        }
    }
    let default_verdict = if layers
        .iter()
        .any(|l| l.network.default_verdict == Verdict::Deny)
    {
        Verdict::Deny
    } else {
        Verdict::Ask
    };
    let mut integrations = overlay.integrations.clone();
    for id in baseline.into_iter().flat_map(|b| &b.integrations) {
        if !integrations.contains(id) {
            integrations.push(id.clone());
        }
    }
    Policy {
        network: NetworkPolicy {
            allowed_routes: routes,
            default_verdict,
            default_transport: overlay.network.default_transport,
        },
        integrations,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allow(host: &str) -> Policy {
        let mut p = Policy::default();
        p.add_rule(RouteRule::allow_host(host));
        p
    }

    #[test]
    fn guardrail_ignores_a_narrow_allow_and_a_deny_rule() {
        let mut policy = allow("api.example.test");
        policy.add_rule(RouteRule::deny_host("blocked.example.test"));
        assert!(guardrail_flags(&policy).is_empty());
    }

    #[test]
    fn a_narrow_cidr_is_not_flagged_as_broad() {
        assert!(!is_broad_cidr("10.1.2.3/32"));
        assert!(!is_broad_cidr("api.example.test"));
        assert!(!is_broad_cidr("bad/prefix"));
        assert!(is_broad_cidr("0.0.0.0/0"));
    }

    #[test]
    fn an_empty_flag_set_produces_no_summary() {
        assert!(run_summary(&[]).is_empty());
    }

    fn deny(host: &str) -> Policy {
        let mut p = Policy::default();
        p.add_rule(RouteRule::deny_host(host));
        p
    }

    #[test]
    fn merge_orders_every_layers_denies_before_allows() {
        let baseline = allow("api.example.test");
        let overlay = deny("api.example.test");
        let merged = merge_effective(Some(&baseline), &overlay);
        let routes = &merged.network.allowed_routes;
        let deny_idx = routes.iter().position(|r| r.verdict == Verdict::Deny);
        let allow_idx = routes.iter().position(|r| r.verdict == Verdict::Allow);
        assert!(
            deny_idx < allow_idx,
            "a first-match gate must see the deny first: {routes:?}"
        );
        assert_eq!(merged.network.default_verdict, Verdict::Ask);
    }

    #[test]
    fn merge_backstops_a_permissive_bundle_default_to_ask() {
        let mut baseline = Policy::default();
        baseline.network.default_verdict = Verdict::Allow;
        let merged = merge_effective(Some(&baseline), &Policy::default());
        assert_eq!(
            merged.network.default_verdict,
            Verdict::Ask,
            "a bundle's allow-by-default must never survive the merge"
        );
    }

    #[test]
    fn merge_honors_a_deny_default_and_unions_integrations() {
        let mut overlay = Policy::default();
        overlay.network.default_verdict = Verdict::Deny;
        overlay.connect("some-overlay-integration");
        let mut baseline = Policy::default();
        baseline.connect("some-bundle-integration");
        let merged = merge_effective(Some(&baseline), &overlay);
        assert_eq!(merged.network.default_verdict, Verdict::Deny);
        assert!(
            merged
                .integrations
                .contains(&"some-overlay-integration".to_string())
        );
        assert!(
            merged
                .integrations
                .contains(&"some-bundle-integration".to_string())
        );
    }

    #[test]
    fn merge_of_a_plain_overlay_with_no_baseline_keeps_the_overlay_rules() {
        let overlay = allow("api.example.test");
        let merged = merge_effective(None, &overlay);
        assert!(
            merged
                .network
                .allowed_routes
                .iter()
                .any(|r| r.match_pattern == "api.example.test")
        );
        assert_eq!(merged.network.default_verdict, Verdict::Ask);
    }
}
