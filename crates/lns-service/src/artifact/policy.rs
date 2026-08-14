use lns_policy::matching::split_destination;
use lns_policy::{NetworkPolicy, Policy, RouteRule, Verdict};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardrailFlag {
    WildcardAllow,
    BroadCidrAllow,
}

impl GuardrailFlag {
    pub fn message(self) -> &'static str {
        match self {
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
    // A broad `egress.tcp` allow is the widest grant a policy can express, so it is held to the same checks.
    let http = policy
        .network
        .egress
        .http
        .iter()
        .map(|rule| (rule.verdict, rule.match_pattern.as_str()));
    let tcp = policy
        .network
        .egress
        .tcp
        .iter()
        .map(|rule| (rule.verdict, rule.match_pattern.as_str()));
    for (_, pattern) in http.chain(tcp).filter(|(v, _)| *v == Verdict::Allow) {
        // The breadth checks read the address a rule really covers: `0.0.0.0/0:5432` is no narrower than `0.0.0.0/0`.
        let (address, _) = split_destination(pattern);
        if is_broad_wildcard(address) {
            flags.push(GuardrailFlag::WildcardAllow);
        } else if is_broad_cidr(address) {
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

/// Fold one egress table across `layers` into the merged table the guest gate installs: a later source's entries sit ahead of an earlier one's, and each source keeps its own order (`docs/sandbox-spec.md` §4.2).
// The gate is first-match-wins, so placing a source ahead is what makes it decide.
fn merge_rule_table<R: Clone + PartialEq>(layers: &[&Policy], table: TableOf<R>) -> Vec<R> {
    let mut merged: Vec<R> = Vec::new();
    for layer in layers {
        for rule in table(layer) {
            push_unique(&mut merged, rule);
        }
    }
    merged
}

/// Adds a connector's derived rules ahead of any catch-all, since appended behind one they would never fire and a closed policy raises no card to say why.
pub fn splice_connector_routes(
    table: &mut Vec<RouteRule>,
    routes: impl IntoIterator<Item = RouteRule>,
) {
    let at = table
        .iter()
        .position(RouteRule::is_catch_all)
        .unwrap_or(table.len());
    table.splice(at..at, routes);
}

/// Whether the first catch-all the gate reaches in `egress.http` is a deny, read from that table alone because a raw rule must name a port and so can never be a catch-all.
pub fn is_closed(policy: &Policy) -> bool {
    policy.network.is_closed()
}

/// Merge a sandbox's shipped `baseline` policy under a local `overlay` into one effective policy for the guest gate: the overlay is the later source, so its entries decide every destination both name, and only its connectors are applied — an artifact-declared connector the user has not connected in this directory is never force-armed, so it stays connectable and is offered as a live connect on first use.
pub fn merge_effective(baseline: Option<&Policy>, overlay: &Policy) -> Policy {
    let layers: Vec<&Policy> = std::iter::once(overlay).chain(baseline).collect();
    Policy {
        network: NetworkPolicy {
            egress: lns_policy::Egress {
                http: merge_rule_table(&layers, |policy| &policy.network.egress.http),
                tcp: merge_rule_table(&layers, |policy| &policy.network.egress.tcp),
            },
        },
        connectors: overlay.connectors.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lns_policy::TcpEgressRule;

    /// The merged table as the guest's first-match gate reads it, which is the only thing the merge means.
    fn table(policy: &Policy) -> Vec<(&str, Verdict)> {
        policy
            .network
            .egress
            .http
            .iter()
            .map(|rule| (rule.match_pattern.as_str(), rule.verdict))
            .collect()
    }

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
    fn a_port_suffix_does_not_hide_the_address_a_rule_really_covers() {
        assert_eq!(
            guardrail_flags(&tcp_allowing("[::/0]:5432")),
            vec![GuardrailFlag::BroadCidrAllow],
            "a bracketed range with a port is the same range; reading the pattern verbatim would miss it"
        );
    }

    fn tcp_allowing(destination: &str) -> Policy {
        let mut p = Policy::default();
        p.network
            .egress
            .tcp
            .push(TcpEgressRule::allow_destination(destination));
        p
    }

    #[test]
    fn guardrail_flags_a_broad_raw_splice_and_leaves_a_scoped_one_alone() {
        assert_eq!(
            guardrail_flags(&tcp_allowing("0.0.0.0/0:5432")),
            vec![GuardrailFlag::BroadCidrAllow],
            "an opaque splice to every address is the widest grant a policy can express"
        );
        assert!(
            guardrail_flags(&tcp_allowing("db.internal:5432")).is_empty(),
            "one named database is exactly the least-privilege shape raw egress is for"
        );
    }

    #[test]
    fn guardrail_flags_a_whole_tld_raw_splice() {
        assert_eq!(
            guardrail_flags(&tcp_allowing("*.com:443")),
            vec![GuardrailFlag::WildcardAllow]
        );
    }

    #[test]
    fn guardrail_ignores_a_raw_deny_however_broad() {
        let mut policy = Policy::default();
        policy
            .network
            .egress
            .tcp
            .push(TcpEgressRule::deny_destination("0.0.0.0/0:5432"));
        assert!(
            guardrail_flags(&policy).is_empty(),
            "a broad deny narrows the sandbox; only a broad allow is a guardrail concern"
        );
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
    fn a_rule_a_layer_wrote_behind_its_own_catch_all_deny_stays_dead() {
        // Read position-blind, this layer's allow would be hoisted out from under the
        // deny it sits behind — so the merged policy would grant a destination the
        // same file denies when it runs with no baseline under it.
        let mut baseline = Policy::default();
        baseline.add_rule(RouteRule::deny_host("*"));
        baseline.add_rule(RouteRule::allow_host("attacker.example"));

        let merged = merge_effective(Some(&baseline), &Policy::default());

        let decided = merged
            .network
            .egress
            .http
            .iter()
            .find(|rule| rule.match_pattern == "attacker.example" || rule.match_pattern == "*")
            .expect("the closer at least must survive");
        assert_eq!(
            (decided.match_pattern.as_str(), decided.verdict),
            ("*", Verdict::Deny),
            "the first rule reached must be the closer: {:?}",
            merged.network.egress.http
        );
    }

    #[test]
    fn an_allow_stranded_behind_a_catch_all_does_not_decide_an_earlier_sources_copy_of_it() {
        // An unreachable approval is what a lockdown leaves in the file, and the gate stops before it.
        let mut overlay = Policy::default();
        overlay.add_rule(RouteRule::deny_host("*"));
        overlay.add_rule(RouteRule::allow_host("attacker.example"));
        let baseline = allow("attacker.example");

        let merged = merge_effective(Some(&baseline), &overlay);

        let decided = merged
            .network
            .egress
            .http
            .iter()
            .find(|rule| rule.match_pattern == "attacker.example" || rule.match_pattern == "*")
            .expect("the closer at least must survive");
        assert_eq!(
            (decided.match_pattern.as_str(), decided.verdict),
            ("*", Verdict::Deny),
            "a rule the gate never reaches decides nothing, so an earlier source's identical allow must not be reached either: {:?}",
            merged.network.egress.http
        );
    }

    #[test]
    fn a_local_catch_all_allow_decides_everything_under_it() {
        // The shape the `defaultVerdict: allow` migration prescribes.
        let mut local = Policy::default();
        local.add_rule(RouteRule::allow_host("*"));

        let merged = merge_effective(Some(&allow("api.example.test")), &local);

        assert_eq!(
            table(&merged).first().copied(),
            Some(("*", Verdict::Allow)),
            "a developer who opened their own directory opened it, and §4.2 makes that their call to see in the disclosure rather than the merge's to refuse"
        );
    }

    #[test]
    fn the_catch_all_a_layer_reaches_first_is_the_one_that_survives() {
        // Written this way the layer is open: its own gate stops at the allow and
        // never reaches the deny. Hoisting the dead deny would close a file that,
        // run alone, decides the opposite.
        let mut overlay = Policy::default();
        overlay.add_rule(RouteRule::allow_host("*"));
        overlay.add_rule(RouteRule::deny_host("*"));

        let merged = merge_effective(None, &overlay);

        let first_catch_all = merged
            .network
            .egress
            .http
            .iter()
            .find(|rule| rule.match_pattern == "*")
            .expect("a catch-all must survive");
        assert_eq!(
            first_catch_all.verdict,
            Verdict::Allow,
            "got: {:?}",
            merged.network.egress.http
        );
    }

    #[test]
    fn connector_routes_land_ahead_of_a_catch_all_so_the_gate_still_reaches_them() {
        // A closed policy says so with a rule now, and the gate stops at the first
        // match — appended behind it a connector's route never fires, and a closed
        // policy raises no card either, so the failure would be silent.
        let mut table = vec![
            RouteRule::allow_host("api.example.test"),
            RouteRule::deny_host("*"),
        ];
        splice_connector_routes(
            &mut table,
            [RouteRule::allow_host("api.some-oauth.example")],
        );
        let patterns: Vec<&str> = table.iter().map(|r| r.match_pattern.as_str()).collect();
        assert_eq!(
            patterns,
            vec!["api.example.test", "api.some-oauth.example", "*"],
            "the route must sit ahead of the catch-all"
        );
    }

    #[test]
    fn connector_routes_append_when_nothing_closes_the_table() {
        let mut table = vec![RouteRule::allow_host("api.example.test")];
        splice_connector_routes(
            &mut table,
            [RouteRule::allow_host("api.some-oauth.example")],
        );
        assert_eq!(
            table.last().unwrap().match_pattern,
            "api.some-oauth.example"
        );
    }

    #[test]
    fn a_later_sources_entries_precede_an_earlier_sources() {
        let baseline = allow("api.example.test");
        let overlay = deny("api.example.test");
        let merged = merge_effective(Some(&baseline), &overlay);
        let routes = &merged.network.egress.http;
        let deny_idx = routes
            .iter()
            .position(|r| r.verdict == Verdict::Deny)
            .expect("the later source's deny must survive the merge");
        let allow_idx = routes
            .iter()
            .position(|r| r.verdict == Verdict::Allow)
            .expect("the earlier source's allow stays in the table, behind what overruled it");
        assert!(
            deny_idx < allow_idx,
            "a first-match gate must see the later source first: {routes:?}"
        );
        assert!(
            !is_closed(&merged),
            "nothing closed either layer: {:?}",
            merged.network.egress.http
        );
    }

    fn tcp_allow(destination: &str) -> Policy {
        let mut p = Policy::default();
        p.network
            .egress
            .tcp
            .push(TcpEgressRule::allow_destination(destination));
        p
    }

    fn tcp_deny(destination: &str) -> Policy {
        let mut p = Policy::default();
        p.network
            .egress
            .tcp
            .push(TcpEgressRule::deny_destination(destination));
        p
    }

    #[test]
    fn the_raw_table_carries_both_sources_in_precedence_order() {
        let baseline = tcp_allow("db.internal:5432");
        let overlay = tcp_deny("db.internal:5432");
        let merged = merge_effective(Some(&baseline), &overlay);
        assert_eq!(
            merged
                .network
                .egress
                .tcp
                .iter()
                .map(|r| r.verdict)
                .collect::<Vec<_>>(),
            vec![Verdict::Deny, Verdict::Allow],
            "the raw table is first-match-wins too, so a deny that lands second never fires"
        );
    }

    #[test]
    fn a_local_allow_decides_a_destination_the_artifact_denies() {
        let mut artifact = Policy::default();
        artifact.add_rule(RouteRule::deny_host("docs.vendor.example"));

        let merged = merge_effective(Some(&artifact), &allow("docs.vendor.example"));

        let decided = merged
            .network
            .egress
            .http
            .iter()
            .find(|rule| rule.match_pattern == "docs.vendor.example")
            .expect("the destination both sources named is in the merged table");
        assert_eq!(
            decided.verdict,
            Verdict::Allow,
            "the local mixin is the last source, so nothing the developer pulled can overrule what they decided (docs/sandbox-spec.md §8.1)"
        );
    }

    #[test]
    fn a_local_catch_all_deny_decides_every_destination_the_artifact_allows() {
        let mut local = Policy::default();
        local.add_rule(RouteRule::deny_host("*"));

        let merged = merge_effective(Some(&allow("api.vendor.example")), &local);

        assert_eq!(
            merged
                .network
                .egress
                .http
                .first()
                .map(|rule| (rule.match_pattern.as_str(), rule.verdict)),
            Some(("*", Verdict::Deny)),
            "a developer locking their own directory down still holds, by sitting ahead of every pulled entry rather than by clamping them"
        );
    }

    #[test]
    fn a_destination_the_artifact_never_named_is_still_the_developers_to_allow() {
        let mut artifact = allow("*.example.test");
        artifact.add_rule(RouteRule::deny_host("*"));

        let merged = merge_effective(Some(&artifact), &allow("api.other.test"));

        assert_eq!(
            table(&merged).first().copied(),
            Some(("api.other.test", Verdict::Allow)),
            "an artifact denying by default is stating what it needs, not a boundary over the developer who ran it (docs/sandbox-spec.md §4.2)"
        );
    }

    #[test]
    fn the_developers_own_treatment_of_a_host_decides_over_the_artifacts() {
        let mut artifact = Policy::default();
        artifact.add_rule(RouteRule {
            tls_terminate: true,
            ..RouteRule::allow_host("*.example.test")
        });
        artifact.add_rule(RouteRule::deny_host("*"));

        let merged = merge_effective(Some(&artifact), &allow("api.example.test"));

        assert_eq!(
            table(&merged).first().copied(),
            Some(("api.example.test", Verdict::Allow)),
            "the developer asked for this host uninspected, and they are the later source"
        );
    }

    #[test]
    fn a_developers_http_catch_all_does_not_decide_the_raw_table_either() {
        let mut artifact = Policy::default();
        artifact
            .network
            .egress
            .tcp
            .push(TcpEgressRule::allow_destination("db.vendor.example:5432"));

        let merged = merge_effective(Some(&artifact), &closed_allowing("api.trusted.example"));

        assert_eq!(
            merged.network.egress.tcp.len(),
            1,
            "the tables fold independently in both directions (§4.2), and a raw rule must name a port, so a `deny *` written in http is not a raw lockdown — a developer who means to close a raw destination names it in tcp"
        );
    }

    #[test]
    fn an_artifacts_http_lockdown_does_not_reach_into_the_developers_raw_table() {
        let mut artifact = Policy::default();
        artifact.add_rule(RouteRule::deny_host("*"));

        let merged = merge_effective(Some(&artifact), &tcp_allow("db.internal:5432"));

        assert_eq!(
            merged.network.egress.tcp.len(),
            1,
            "the two tables fold independently, and neither decides the other"
        );
    }

    #[test]
    fn an_inspected_route_is_not_consent_to_splice_the_same_host_raw() {
        let mut artifact = allow("db.internal");
        artifact.add_rule(RouteRule::deny_host("*"));

        let merged = merge_effective(Some(&artifact), &tcp_allow("db.internal:5432"));

        assert!(
            merged
                .network
                .egress
                .http
                .iter()
                .all(|rule| rule.match_pattern != "db.internal:5432"),
            "the tables fold independently, so an http entry never becomes a raw one"
        );
    }

    #[test]
    fn merge_honors_a_deny_default_and_applies_only_overlay_connectors() {
        let mut overlay = Policy::default();
        overlay.add_rule(RouteRule::deny_host("*"));
        overlay.connect("some-overlay-connector");
        let mut baseline = Policy::default();
        baseline.connect("some-artifact-connector");
        let merged = merge_effective(Some(&baseline), &overlay);
        assert!(
            is_closed(&merged),
            "a closed layer must leave the merged policy closed: {:?}",
            merged.network.egress.http
        );
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
    fn a_local_allow_the_artifact_never_named_decides_it() {
        let mut artifact = allow("api.allowed.example");
        artifact.add_rule(RouteRule::deny_host("*"));

        let merged = merge_effective(Some(&artifact), &allow("api.overlay-only.example"));

        assert_eq!(
            table(&merged),
            [
                ("api.overlay-only.example", Verdict::Allow),
                ("api.allowed.example", Verdict::Allow),
                ("*", Verdict::Deny)
            ],
            "the developer's entry sits ahead of everything the artifact said, and the artifact's own backstop still closes what neither named"
        );
    }

    /// A closed layer: it allows `host` and denies everything else, which is what
    /// `defaultVerdict: deny` said before the key left the file.
    fn closed_allowing(host: &str) -> Policy {
        let mut p = allow(host);
        p.add_rule(RouteRule::deny_host("*"));
        p
    }

    #[test]
    fn a_developers_lockdown_still_decides_a_destination_the_artifact_allows() {
        let merged = merge_effective(
            Some(&allow("attacker.example")),
            &closed_allowing("api.trusted.example"),
        );

        assert_eq!(
            table(&merged),
            [
                ("api.trusted.example", Verdict::Allow),
                ("*", Verdict::Deny),
                ("attacker.example", Verdict::Allow)
            ],
            "the gate reads first-match, so the developer's catch-all deny decides attacker.example even though the artifact's allow is still in the table"
        );
    }

    #[test]
    fn two_lockdowns_leave_only_what_the_developer_named() {
        let merged = merge_effective(
            Some(&closed_allowing("api.artifact-only.example")),
            &closed_allowing("api.user-only.example"),
        );

        assert_eq!(
            table(&merged),
            [
                ("api.user-only.example", Verdict::Allow),
                ("*", Verdict::Deny),
                ("api.artifact-only.example", Verdict::Allow)
            ],
            "the developer's catch-all deny reaches the artifact's own allow first, so what they did not name stays shut"
        );
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
        assert!(
            !is_closed(&merged),
            "nothing closed either layer: {:?}",
            merged.network.egress.http
        );
    }
}
