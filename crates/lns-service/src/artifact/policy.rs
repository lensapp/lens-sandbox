use std::path::Path;

use lns_policy::matching::split_destination;
use lns_policy::{NetworkPolicy, Policy, RouteRule, TcpEgressRule, Verdict};

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
    // A raw splice is granted with no inspection at all, so a broad `egress.tcp` allow is the widest grant a policy can express and is held to the same checks.
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

/// A deny-by-default layer permits only the destinations it names, so an allow survives the merge only if every such ceiling layer carries a rule permitting it.
fn allowed_by_every_ceiling<R>(
    rule: &R,
    ceilings: &[&Policy],
    table: TableOf<R>,
    closes: fn(&R) -> bool,
    permits: fn(&R, &R) -> bool,
) -> bool {
    // A ceiling grants only what its own gate reaches: a line stranded behind its
    // catch-all authorizes nothing, here least of all another layer's copy of it.
    ceilings
        .iter()
        .all(|ceiling| live_rules(table(ceiling), closes).any(|permitted| permits(permitted, rule)))
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

/// The `egress.tcp` counterpart of [`permits`], scoping included: a ceiling that splices a destination for every caller also covers a rule splicing it for some of them.
fn tcp_permits(permitted: &TcpEgressRule, rule: &TcpEgressRule) -> bool {
    let widened = TcpEgressRule {
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

/// A layer's rules up to its catch-all deny — the ones its own first-match gate can
/// still reach. Read position-blind, a rule the author wrote *behind* their catch-all
/// would be hoisted out from under it, so the merged policy would grant what the same
/// file grants nothing of when it runs alone.
fn live_rules<R>(table: &[R], closes: fn(&R) -> bool) -> impl Iterator<Item = &R> + Clone {
    table.iter().take_while(move |rule| !closes(rule))
}

/// Fold one egress table across `layers` into the merged table the guest gate installs, deny-first and clamped by the deny-by-default `ceilings`.
// Deny-first ordering is load-bearing: lens-sandbox-core's rule lookup is first-match-wins, so a destination denied by any layer must have its deny rule appear before any allow.
fn merge_rule_table<R: Clone + PartialEq>(
    layers: &[&Policy],
    ceilings: &[&Policy],
    table: TableOf<R>,
    is_deny: fn(&R) -> bool,
    closes: fn(&R) -> bool,
    permits: fn(&R, &R) -> bool,
) -> Vec<R> {
    let mut merged: Vec<R> = Vec::new();
    for layer in layers {
        for rule in live_rules(table(layer), closes).filter(|rule| is_deny(rule)) {
            push_unique(&mut merged, rule);
        }
    }
    for layer in layers {
        for rule in live_rules(table(layer), closes).filter(|rule| !is_deny(rule)) {
            if allowed_by_every_ceiling(rule, ceilings, table, closes, permits) {
                push_unique(&mut merged, rule);
            }
        }
    }
    // The catch-all goes last, not first: it decides every destination, so ahead of
    // the allows it would answer for the ones its own layer permits. Only the closer
    // each layer actually reaches counts, and a deny wins over an allow so neither
    // layer's lockdown can be opened by the other's backstop.
    let reached: Vec<&R> = layers
        .iter()
        .filter_map(|layer| table(layer).iter().find(|rule| closes(rule)))
        .collect();
    let closer = reached
        .iter()
        .find(|rule| is_deny(rule))
        // Vacuous while http is the only table with a catch-all: a ceiling's own first
        // catch-all is a deny, so the deny branch above already won.
        .or_else(|| {
            reached
                .iter()
                .find(|rule| allowed_by_every_ceiling(**rule, ceilings, table, closes, permits))
        })
        .copied();
    if let Some(closer) = closer {
        push_unique(&mut merged, closer);
    }
    merged
}

/// Add rules a connector derived to the table the guest will gate on, ahead of any
/// catch-all so they are still reached.
///
/// A closed policy used to say so with `defaultVerdict`, which the gate consulted only
/// where no rule matched, so appending was enough. Saying it with a rule instead makes
/// appending a way to write lines that never fire — and a closed policy raises no card,
/// so the connector would fail with nothing said.
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

/// Whether a layer is closed: it holds a catch-all deny, so it permits only what it names.
///
/// This is what `defaultVerdict: deny` used to say before the key left the file. It
/// is read from the `http` table alone and clamps both tables, because a raw rule
/// must name a port — a catch-all is not expressible in `egress.tcp`, so reading
/// them separately would leave a locked-down layer unable to clamp raw destinations
/// at all.
pub fn is_closed(policy: &Policy) -> bool {
    policy.network.is_closed()
}

/// Merge a sandbox's shipped `baseline` policy under a local `overlay` into one effective policy for the guest gate: denies from every layer come first so a first-match gate stays deny-dominant, a closed layer (see [`is_closed`]) is a ceiling an allow must clear in every such layer (so neither the artifact nor the user can widen the other's lockdown), and only the user's overlay connectors are applied — an artifact-declared connector the user hasn't connected in this directory is never force-armed, so it stays connectable and is offered as a live connect on first use.
pub fn merge_effective(baseline: Option<&Policy>, overlay: &Policy) -> Policy {
    let layers: Vec<&Policy> = std::iter::once(overlay).chain(baseline).collect();
    let ceilings: Vec<&Policy> = layers
        .iter()
        .copied()
        .filter(|policy| is_closed(policy))
        .collect();
    let http = merge_rule_table(
        &layers,
        &ceilings,
        |policy| &policy.network.egress.http,
        |rule: &RouteRule| rule.verdict == Verdict::Deny,
        RouteRule::is_catch_all,
        permits,
    );
    // Folded independently of `http`: an inspected route in a ceiling is not consent to splice the same host raw.
    let tcp = merge_rule_table(
        &layers,
        &ceilings,
        |policy| &policy.network.egress.tcp,
        |rule: &TcpEgressRule| rule.verdict == Verdict::Deny,
        // A raw rule must name a port, so no `egress.tcp` rule can be a catch-all.
        |_| false,
        tcp_permits,
    );
    Policy {
        network: NetworkPolicy {
            egress: lns_policy::Egress { http, tcp },
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
    fn a_dead_allow_in_a_ceiling_does_not_legitimize_another_layers_copy_of_it() {
        // The ceiling grants only what its own gate would reach. A line stranded
        // behind its catch-all — which is exactly what an unreachable approval
        // leaves in the file — must not authorize an untrusted baseline's identical
        // allow, or the lockdown is widened through a rule that never fires.
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
            "the baseline allow must not clear a ceiling through a dead rule: {:?}",
            merged.network.egress.http
        );
    }

    #[test]
    fn a_catch_all_allow_survives_the_merge() {
        // The shape the `defaultVerdict: allow` migration prescribes. Dropping it
        // would rewrite the file's meaning to prompt-for-everything the moment a
        // baseline appears under it, and only then.
        let mut overlay = Policy::default();
        overlay.add_rule(RouteRule::allow_host("*"));
        let baseline = allow("api.example.test");

        let merged = merge_effective(Some(&baseline), &overlay);

        let last = merged
            .network
            .egress
            .http
            .last()
            .expect("the merged table cannot be empty");
        assert_eq!(
            (last.match_pattern.as_str(), last.verdict),
            ("*", Verdict::Allow),
            "the catch-all must survive, behind the rules it backstops: {:?}",
            merged.network.egress.http
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
    fn merge_carries_both_layers_tcp_rules_denies_first() {
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
    fn a_deny_by_default_layer_clamps_the_other_layers_tcp_allow() {
        let mut ceiling = Policy::default();
        ceiling.add_rule(RouteRule::deny_host("*"));
        let merged = merge_effective(Some(&ceiling), &tcp_allow("db.internal:5432"));
        assert!(
            merged.network.egress.tcp.is_empty(),
            "a locked-down layer names every destination it permits; the user cannot widen it"
        );
    }

    #[test]
    fn an_http_allow_in_a_ceiling_does_not_legitimize_a_raw_splice() {
        let mut ceiling = allow("db.internal");
        ceiling.add_rule(RouteRule::deny_host("*"));
        let merged = merge_effective(Some(&ceiling), &tcp_allow("db.internal:5432"));
        assert!(
            merged.network.egress.tcp.is_empty(),
            "an inspected route is not consent to splice the same host opaquely; the tables clamp independently"
        );
    }

    #[test]
    fn merge_keeps_a_raw_allow_that_only_narrows_which_binary_a_ceiling_already_splices() {
        let mut overlay = tcp_allow("db.internal:5432");
        overlay.network.egress.tcp[0].binaries = Some(vec!["/usr/bin/psql".into()]);
        let mut ceiling = tcp_allow("db.internal:5432");
        ceiling.add_rule(RouteRule::deny_host("*"));

        let merged = merge_effective(Some(&ceiling), &overlay);

        let scoped = merged
            .network
            .egress
            .tcp
            .iter()
            .position(|rule| rule.binaries.as_deref() == Some(&["/usr/bin/psql".to_string()]));
        let open =
            merged.network.egress.tcp.iter().position(|rule| {
                rule.match_pattern == "db.internal:5432" && rule.binaries.is_none()
            });
        assert!(
            scoped.is_some_and(|scoped| open.is_none_or(|open| scoped < open)),
            "narrowing a splice the ceiling already grants every caller must survive, and a first-match gate must reach the narrower rule first or the narrowing is void: {merged:?}"
        );
    }

    #[test]
    fn merge_drops_a_raw_allow_for_a_binary_the_ceilings_own_scope_excludes() {
        let mut overlay = tcp_allow("db.internal:5432");
        overlay.network.egress.tcp[0].binaries = Some(vec!["/usr/bin/nc".into()]);
        let mut ceiling = tcp_allow("db.internal:5432");
        ceiling.network.egress.tcp[0].binaries = Some(vec!["/usr/bin/psql".into()]);
        ceiling.add_rule(RouteRule::deny_host("*"));

        let merged = merge_effective(Some(&ceiling), &overlay);

        assert!(
            !merged
                .network
                .egress
                .tcp
                .iter()
                .any(|rule| rule.binaries.as_deref() == Some(&["/usr/bin/nc".to_string()])),
            "a ceiling that splices a destination for one binary must not be widened to another by an overlay scoped to it: {merged:?}"
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
    fn merge_does_not_let_an_overlay_allow_widen_a_deny_by_default_baseline() {
        let mut baseline = allow("api.allowed.example");
        baseline.add_rule(RouteRule::deny_host("*"));
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
        assert!(
            is_closed(&merged),
            "a closed layer must leave the merged policy closed: {:?}",
            merged.network.egress.http
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
    fn merge_clamps_an_untrusted_baseline_allow_against_a_user_deny_by_default() {
        let overlay = closed_allowing("api.trusted.example");
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
        assert!(
            is_closed(&merged),
            "a closed layer must leave the merged policy closed: {:?}",
            merged.network.egress.http
        );
    }

    #[test]
    fn merge_does_not_let_an_unscoped_allow_clear_a_binary_scoped_ceiling() {
        let overlay = allow("git.example.test");
        let mut baseline = closed_allowing("git.example.test");
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
        let baseline = closed_allowing("git.example.test");

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
        let mut baseline = closed_allowing("git.example.test");
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
        let mut baseline = closed_allowing("git.example.test");
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
        let mut baseline = closed_allowing("git.example.test");
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
        let baseline = closed_allowing("other.example.test");

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
        let overlay = closed_allowing("api.shared.example");
        let baseline = closed_allowing("api.shared.example");

        let merged = merge_effective(Some(&baseline), &overlay);

        assert!(
            merged.network.egress.http.iter().any(|rule| {
                rule.match_pattern == "api.shared.example" && rule.verdict == Verdict::Allow
            }),
            "a host both the user and the artifact allow under deny-by-default is in the intersection and must survive: {merged:?}"
        );
        assert!(
            is_closed(&merged),
            "a closed layer must leave the merged policy closed: {:?}",
            merged.network.egress.http
        );
    }

    #[test]
    fn merge_drops_disjoint_allows_when_both_layers_deny_by_default() {
        let overlay = closed_allowing("api.user-only.example");
        let baseline = closed_allowing("api.artifact-only.example");

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
        assert!(
            is_closed(&merged),
            "a closed layer must leave the merged policy closed: {:?}",
            merged.network.egress.http
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
