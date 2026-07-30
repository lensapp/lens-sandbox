use std::path::Path;

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
            GuardrailFlag::WildcardAllow => {
                "wildcard allow — a catch-all or whole-suffix host pattern is permitted"
            }
            GuardrailFlag::BroadCidrAllow => {
                "broad CIDR allow — a large address range is permitted"
            }
        }
    }
}

/// The warning's broad-CIDR line, kept as generous as the gate's own matcher: any prefix of `/16` or shorter covers 65k+ addresses.
const BROAD_CIDR_PREFIX: u8 = 16;

fn is_broad_cidr(pattern: &str) -> bool {
    match pattern.split_once('/') {
        Some((_, prefix)) => prefix
            .parse::<u8>()
            .map(|len| len <= BROAD_CIDR_PREFIX)
            .unwrap_or(false),
        None => false,
    }
}

/// A catch-all (`*`) or a wildcard sitting directly on a TLD (`*.com`), both of which the gate honors as a broad allow; a scoped wildcard like `*.example.com` is not.
fn is_broad_wildcard(pattern: &str) -> bool {
    pattern == "*"
        || pattern
            .strip_prefix("*.")
            .is_some_and(|rest| !rest.is_empty() && !rest.contains('.'))
}

pub fn guardrail_flags(policy: &Policy) -> Vec<GuardrailFlag> {
    let mut flags = Vec::new();
    if policy.network.default_verdict == Verdict::Allow {
        flags.push(GuardrailFlag::PermissiveDefaultVerdict);
    }
    for rule in &policy.network.egress.http {
        if rule.verdict != Verdict::Allow {
            continue;
        }
        if is_broad_wildcard(&rule.match_pattern) {
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

type TableOf<R> = fn(&Policy) -> &Vec<R>;

fn push_unique<R: Clone + PartialEq>(merged: &mut Vec<R>, rule: &R) {
    if !merged.contains(rule) {
        merged.push(rule.clone());
    }
}

/// A deny-by-default layer permits only the destinations it names, so an allow survives the merge only if every such ceiling layer carries a rule permitting it.
fn allowed_by_every_ceiling<R>(
    rule: &R,
    ceilings: &[&Policy],
    table: TableOf<R>,
    permits: fn(&R, &R) -> bool,
) -> bool {
    ceilings.iter().all(|ceiling| {
        table(ceiling)
            .iter()
            .any(|permitted| permits(permitted, rule))
    })
}

/// Whether `permitted` covers `rule`: identical apart from binary scoping and the human-readable note neither layer grants anything by, and scoped no tighter than `rule` — a rule that only narrows a ceiling's grant to fewer callers grants nothing the ceiling didn't.
fn permits(permitted: &RouteRule, rule: &RouteRule) -> bool {
    let widened = RouteRule {
        binaries: permitted.binaries.clone(),
        description: permitted.description.clone(),
        ..rule.clone()
    };
    widened == *permitted && scope_within(rule.binaries.as_deref(), permitted.binaries.as_deref())
}

fn scope_within(rule: Option<&[String]>, permitted: Option<&[String]>) -> bool {
    match (rule, permitted) {
        (_, None) => true,
        (None, Some(_)) => false,
        // Compared as paths, the way the guest compares them against /proc/<pid>/exe, so a redundant separator doesn't read as a different binary.
        (Some(rule), Some(permitted)) => rule.iter().all(|binary| {
            permitted
                .iter()
                .any(|listed| Path::new(listed) == Path::new(binary))
        }),
    }
}

/// Fold one egress table across `layers` into the merged table the guest gate installs, deny-first and clamped by the deny-by-default `ceilings`.
// Deny-first ordering is load-bearing: lens-sandbox-core's rule lookup is first-match-wins, so a destination denied by any layer must have its deny rule appear before any allow.
fn merge_rule_table<R: Clone + PartialEq>(
    layers: &[&Policy],
    ceilings: &[&Policy],
    table: TableOf<R>,
    is_deny: fn(&R) -> bool,
    permits: fn(&R, &R) -> bool,
) -> Vec<R> {
    let mut merged: Vec<R> = Vec::new();
    for layer in layers {
        for rule in table(layer).iter().filter(|rule| is_deny(rule)) {
            push_unique(&mut merged, rule);
        }
    }
    for layer in layers {
        for rule in table(layer).iter().filter(|rule| !is_deny(rule)) {
            if allowed_by_every_ceiling(rule, ceilings, table, permits) {
                push_unique(&mut merged, rule);
            }
        }
    }
    merged
}

/// Merge a sandbox's shipped `baseline` policy under a local `overlay` into one effective policy for the guest gate: denies from every layer come first so a first-match gate stays deny-dominant, a `deny`-by-default layer is a ceiling an allow must clear in every such layer (so neither the artifact nor the user can widen the other's lockdown), `defaultVerdict` is backstopped to `ask` unless a layer denies, and only the user's overlay connectors are applied — an artifact-declared connector the user hasn't connected in this directory is never force-armed, so it stays connectable and is offered as a live connect on first use.
pub fn merge_effective(baseline: Option<&Policy>, overlay: &Policy) -> Policy {
    let layers: Vec<&Policy> = std::iter::once(overlay).chain(baseline).collect();
    let ceilings: Vec<&Policy> = layers
        .iter()
        .copied()
        .filter(|policy| policy.network.default_verdict == Verdict::Deny)
        .collect();
    let http = merge_rule_table(
        &layers,
        &ceilings,
        |policy| &policy.network.egress.http,
        |rule: &RouteRule| rule.verdict == Verdict::Deny,
        permits,
    );
    let default_verdict = if layers
        .iter()
        .any(|l| l.network.default_verdict == Verdict::Deny)
    {
        Verdict::Deny
    } else {
        Verdict::Ask
    };
    Policy {
        network: NetworkPolicy {
            egress: lns_policy::Egress { http },
            default_verdict,
            default_transport: overlay.network.default_transport,
        },
        connectors: overlay.connectors.clone(),
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
        assert!(!is_broad_cidr("10.0.0.0/24"));
        assert!(!is_broad_cidr("api.example.test"));
        assert!(!is_broad_cidr("bad/prefix"));
        assert!(is_broad_cidr("0.0.0.0/0"));
        assert!(
            is_broad_cidr("10.0.0.0/12"),
            "a /12 the gate honors must warn"
        );
        assert!(is_broad_cidr("10.0.0.0/16"));
    }

    #[test]
    fn a_whole_tld_wildcard_is_broad_but_a_scoped_one_is_not() {
        assert!(is_broad_wildcard("*"));
        assert!(is_broad_wildcard("*.com"));
        assert!(!is_broad_wildcard("*.example.com"));
        assert!(!is_broad_wildcard("api.example.test"));
    }

    #[test]
    fn guardrail_flags_a_whole_tld_wildcard_the_gate_would_honor() {
        let flags = guardrail_flags(&allow("*.com"));
        assert_eq!(flags, vec![GuardrailFlag::WildcardAllow]);
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
        let routes = &merged.network.egress.http;
        let deny_idx = routes.iter().position(|r| r.verdict == Verdict::Deny);
        let allow_idx = routes.iter().position(|r| r.verdict == Verdict::Allow);
        assert!(
            deny_idx < allow_idx,
            "a first-match gate must see the deny first: {routes:?}"
        );
        assert_eq!(merged.network.default_verdict, Verdict::Ask);
    }

    #[test]
    fn merge_backstops_a_permissive_baseline_default_to_ask() {
        let mut baseline = Policy::default();
        baseline.network.default_verdict = Verdict::Allow;
        let merged = merge_effective(Some(&baseline), &Policy::default());
        assert_eq!(
            merged.network.default_verdict,
            Verdict::Ask,
            "a baseline's allow-by-default must never survive the merge"
        );
    }

    #[test]
    fn merge_honors_a_deny_default_and_applies_only_overlay_connectors() {
        let mut overlay = Policy::default();
        overlay.network.default_verdict = Verdict::Deny;
        overlay.connect("some-overlay-connector");
        let mut baseline = Policy::default();
        baseline.connect("some-artifact-connector");
        let merged = merge_effective(Some(&baseline), &overlay);
        assert_eq!(merged.network.default_verdict, Verdict::Deny);
        assert!(
            merged
                .connectors
                .contains(&"some-overlay-connector".to_string()),
            "a connector the user connected in this directory is applied"
        );
        assert!(
            !merged
                .connectors
                .contains(&"some-artifact-connector".to_string()),
            "an artifact-declared connector is never force-armed by the merge; it stays connectable and is offered on first use"
        );
    }

    #[test]
    fn merge_does_not_let_an_overlay_allow_widen_a_deny_by_default_baseline() {
        let mut baseline = allow("api.allowed.example");
        baseline.network.default_verdict = Verdict::Deny;
        let overlay = allow("api.overlay-only.example");

        let merged = merge_effective(Some(&baseline), &overlay);

        assert!(
            !merged.network.egress.http.iter().any(|rule| {
                rule.match_pattern == "api.overlay-only.example" && rule.verdict == Verdict::Allow
            }),
            "a local overlay must not widen a sandbox's deny-by-default baseline: {merged:?}"
        );
        assert!(merged.network.egress.http.iter().any(|rule| {
            rule.match_pattern == "api.allowed.example" && rule.verdict == Verdict::Allow
        }));
        assert_eq!(merged.network.default_verdict, Verdict::Deny);
    }

    fn deny_default_allowing(host: &str) -> Policy {
        let mut p = allow(host);
        p.network.default_verdict = Verdict::Deny;
        p
    }

    #[test]
    fn merge_clamps_an_untrusted_baseline_allow_against_a_user_deny_by_default() {
        let overlay = deny_default_allowing("api.trusted.example");
        let baseline = allow("attacker.example");

        let merged = merge_effective(Some(&baseline), &overlay);

        assert!(
            !merged.network.egress.http.iter().any(|rule| {
                rule.match_pattern == "attacker.example" && rule.verdict == Verdict::Allow
            }),
            "a pulled artifact's allow must not punch through the user's deny-by-default lockdown: {merged:?}"
        );
        assert!(merged.network.egress.http.iter().any(|rule| {
            rule.match_pattern == "api.trusted.example" && rule.verdict == Verdict::Allow
        }));
        assert_eq!(merged.network.default_verdict, Verdict::Deny);
    }

    #[test]
    fn merge_does_not_let_an_unscoped_allow_clear_a_binary_scoped_ceiling() {
        let overlay = allow("git.example.test");
        let mut baseline = deny_default_allowing("git.example.test");
        baseline.network.egress.http[0].binaries = Some(vec!["/usr/bin/git".into()]);

        let merged = merge_effective(Some(&baseline), &overlay);

        assert!(
            !merged.network.egress.http.iter().any(|rule| {
                rule.match_pattern == "git.example.test" && rule.binaries.is_none()
            }),
            "an artifact that only allows a host for one binary must not be widened to every caller by an unscoped overlay allow: {merged:?}"
        );
        assert!(
            merged
                .network
                .egress
                .http
                .iter()
                .any(|rule| { rule.binaries.as_deref() == Some(&["/usr/bin/git".to_string()]) }),
            "the scoped allow itself clears every ceiling and must survive, or the assertion above passes vacuously: {merged:?}"
        );
    }

    #[test]
    fn merge_keeps_a_scoped_allow_that_only_narrows_what_a_ceiling_already_permits() {
        let mut overlay = allow("git.example.test");
        overlay.network.egress.http[0].binaries = Some(vec!["/usr/bin/git".into()]);
        let baseline = deny_default_allowing("git.example.test");

        let merged = merge_effective(Some(&baseline), &overlay);

        let scoped = merged
            .network
            .egress
            .http
            .iter()
            .position(|rule| rule.binaries.as_deref() == Some(&["/usr/bin/git".to_string()]));
        let open =
            merged.network.egress.http.iter().position(|rule| {
                rule.match_pattern == "git.example.test" && rule.binaries.is_none()
            });
        assert!(
            scoped.is_some_and(|scoped| open.is_none_or(|open| scoped < open)),
            "narrowing a host the ceiling already allows to every caller must survive the merge, and a first-match gate must see the narrower rule before the ceiling's open allow or the narrowing is void: {merged:?}"
        );
    }

    #[test]
    fn merge_keeps_a_scoped_allow_whose_ceiling_rule_differs_only_in_its_description() {
        let mut overlay = allow("git.example.test");
        overlay.network.egress.http[0].binaries = Some(vec!["/usr/bin/git".into()]);
        let mut baseline = deny_default_allowing("git.example.test");
        baseline.network.egress.http[0].description = Some("git over https".into());

        let merged = merge_effective(Some(&baseline), &overlay);

        let scoped = merged
            .network
            .egress
            .http
            .iter()
            .position(|rule| rule.binaries.as_deref() == Some(&["/usr/bin/git".to_string()]));
        let open =
            merged.network.egress.http.iter().position(|rule| {
                rule.match_pattern == "git.example.test" && rule.binaries.is_none()
            });
        assert!(
            scoped.is_some_and(|scoped| open.is_none_or(|open| scoped < open)),
            "a note on the ceiling's rule is not part of its grant, so it must not decide whether the user's narrowing survives — dropping it leaves the ceiling's own open allow in force: {merged:?}"
        );
    }

    #[test]
    fn merge_reads_a_redundant_separator_as_the_same_binary_the_ceiling_named() {
        let mut overlay = allow("git.example.test");
        overlay.network.egress.http[0].binaries = Some(vec!["/usr/bin/git/".into()]);
        let mut baseline = deny_default_allowing("git.example.test");
        baseline.network.egress.http[0].binaries = Some(vec!["/usr/bin/git".into()]);

        let merged = merge_effective(Some(&baseline), &overlay);

        assert!(
            merged
                .network
                .egress
                .http
                .iter()
                .any(|rule| rule.binaries.as_deref() == Some(&["/usr/bin/git/".to_string()])),
            "the guest compares these as paths and admits the same caller, so the merge must not read them as different binaries and drop the overlay: {merged:?}"
        );
    }

    #[test]
    fn merge_drops_a_scoped_allow_for_a_binary_the_ceilings_own_scope_excludes() {
        let mut overlay = allow("git.example.test");
        overlay.network.egress.http[0].binaries = Some(vec!["/usr/bin/curl".into()]);
        let mut baseline = deny_default_allowing("git.example.test");
        baseline.network.egress.http[0].binaries = Some(vec!["/usr/bin/git".into()]);

        let merged = merge_effective(Some(&baseline), &overlay);

        assert!(
            !merged
                .network
                .egress
                .http
                .iter()
                .any(|rule| rule.binaries.as_deref() == Some(&["/usr/bin/curl".to_string()])),
            "a ceiling that names one binary must not be widened to another by an overlay scoped to it: {merged:?}"
        );
    }

    #[test]
    fn merge_does_not_let_a_scoped_overlay_allow_punch_through_a_ceiling() {
        let mut overlay = allow("git.example.test");
        overlay.network.egress.http[0].binaries = Some(vec!["/usr/bin/git".into()]);
        let baseline = deny_default_allowing("other.example.test");

        let merged = merge_effective(Some(&baseline), &overlay);

        assert!(
            !merged
                .network
                .egress
                .http
                .iter()
                .any(|rule| rule.match_pattern == "git.example.test"),
            "scoping an allow to one binary must not exempt it from an artifact's deny-by-default ceiling: {merged:?}"
        );
        assert!(
            merged
                .network
                .egress
                .http
                .iter()
                .any(|rule| rule.match_pattern == "other.example.test"),
            "the ceiling's own allow must survive, or the assertion above passes on an empty merge: {merged:?}"
        );
    }

    #[test]
    fn merge_keeps_an_allow_both_layers_carry_when_both_deny_by_default() {
        let overlay = deny_default_allowing("api.shared.example");
        let baseline = deny_default_allowing("api.shared.example");

        let merged = merge_effective(Some(&baseline), &overlay);

        assert!(
            merged.network.egress.http.iter().any(|rule| {
                rule.match_pattern == "api.shared.example" && rule.verdict == Verdict::Allow
            }),
            "a host both the user and the artifact allow under deny-by-default is in the intersection and must survive: {merged:?}"
        );
        assert_eq!(merged.network.default_verdict, Verdict::Deny);
    }

    #[test]
    fn merge_drops_disjoint_allows_when_both_layers_deny_by_default() {
        let overlay = deny_default_allowing("api.user-only.example");
        let baseline = deny_default_allowing("api.artifact-only.example");

        let merged = merge_effective(Some(&baseline), &overlay);

        assert!(
            !merged
                .network
                .egress
                .http
                .iter()
                .any(|rule| rule.verdict == Verdict::Allow),
            "two deny-by-default layers with disjoint allowlists intersect to nothing: {merged:?}"
        );
        assert_eq!(merged.network.default_verdict, Verdict::Deny);
    }

    #[test]
    fn merge_of_a_plain_overlay_with_no_baseline_keeps_the_overlay_rules() {
        let overlay = allow("api.example.test");
        let merged = merge_effective(None, &overlay);
        assert!(
            merged
                .network
                .egress
                .http
                .iter()
                .any(|r| r.match_pattern == "api.example.test")
        );
        assert_eq!(merged.network.default_verdict, Verdict::Ask);
    }
}
