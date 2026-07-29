use std::net::IpAddr;
use std::path::Path;

use ipnet::IpNet;

use crate::{NetworkPolicy, RouteRule, Scheme, TcpEgressRule, Verdict};

const CATCH_ALL: &str = "*";

/// Ported from lens-sandbox-core's `domain_matches` so host coverage is decided the same way here as in the guest gate.
pub fn domain_matches(pattern: &str, hostname: &str) -> bool {
    let pattern = pattern.to_ascii_lowercase();
    let hostname = hostname.to_ascii_lowercase();
    if pattern == CATCH_ALL {
        !hostname.is_empty()
    } else if let Some(suffix) = pattern.strip_prefix("*.") {
        hostname == suffix || hostname.ends_with(&format!(".{suffix}"))
    } else if pattern.contains("*.") {
        let (prefix, suffix) = pattern.split_once('*').unwrap_or((&pattern, ""));
        hostname.starts_with(prefix)
            && hostname[prefix.len()..].ends_with(suffix)
            && hostname.len() > prefix.len() + suffix.len()
    } else {
        hostname == pattern
    }
}

impl NetworkPolicy {
    /// The first rule the guest gate's first-match scan would reach before `new`, for every request `new` could match — so a `new` appended after it could never fire.
    pub fn first_shadowing_rule(&self, new: &RouteRule) -> Option<(usize, &RouteRule)> {
        self.egress
            .http
            .iter()
            .enumerate()
            .find(|(_, existing)| shadows(existing, new))
    }

    /// The [`Self::first_shadowing_rule`] of the raw table, which the gate scans first-match-wins the same way.
    pub fn first_shadowing_tcp_rule(&self, new: &TcpEgressRule) -> Option<(usize, &TcpEgressRule)> {
        self.egress
            .tcp
            .iter()
            .enumerate()
            .find(|(_, existing)| tcp_shadows(existing, new))
    }

    /// The `egress.http` rules a raw rule pre-empts on its port. The raw table is the pre-filter, so those rules are simply not applied there and the traffic is spliced unread — lens-sandbox-core computes the same pairs and leaves reporting them to its callers, silence being the failure mode.
    pub fn http_rules_pre_empted_by(&self, raw: &TcpEgressRule) -> Vec<&RouteRule> {
        if raw.verdict == Verdict::Deny {
            return Vec::new();
        }
        let raw = classify(&raw.match_pattern);
        // Whether a range covers a name is DNS's answer, so an address-matched raw rule is left to the guest, which sees the resolution.
        let Hosts::Pattern(host) = raw.hosts else {
            return Vec::new();
        };
        self.egress
            .http
            .iter()
            .filter(|route| {
                let route = classify(&route.match_pattern);
                port_covers(&route.port, &raw.port)
                    && matches!(route.hosts, Hosts::Pattern(pattern) if patterns_overlap(pattern, host))
            })
            .collect()
    }
}

/// Whether two host patterns can name the same host, either one possibly a wildcard. Two mid-segment wildcards that nonetheless share hosts are not detected: this backs a warning, so a miss costs a line of output, not enforcement.
fn patterns_overlap(one: &str, other: &str) -> bool {
    domain_matches(one, other) || domain_matches(other, one)
}

fn shadows(existing: &RouteRule, new: &RouteRule) -> bool {
    scheme_covers(existing.scheme, new.scheme)
        && destination_covers(&existing.match_pattern, &new.match_pattern)
        && caller_covers(existing.binaries.as_deref(), new.binaries.as_deref())
}

fn tcp_shadows(existing: &TcpEgressRule, new: &TcpEgressRule) -> bool {
    destination_covers(&existing.match_pattern, &new.match_pattern)
        && caller_covers(existing.binaries.as_deref(), new.binaries.as_deref())
}

fn scheme_covers(existing: Option<Scheme>, new: Option<Scheme>) -> bool {
    existing.is_none() || existing == new
}

/// The destinations one `match` pattern names, read the way lens-sandbox-core's `parse_matcher` reads it: a set of hosts, open to every port or pinned to one.
struct Destination<'a> {
    hosts: Hosts<'a>,
    port: PortScope,
}

enum Hosts<'a> {
    Range(IpNet),
    Pattern(&'a str),
}

#[derive(PartialEq)]
enum PortScope {
    Any,
    Only(u16),
}

fn classify(pattern: &str) -> Destination<'_> {
    if let Ok(range) = pattern.parse::<IpNet>() {
        return Destination {
            hosts: Hosts::Range(range),
            port: PortScope::Any,
        };
    }
    match split_port(pattern) {
        Some((host, port)) => Destination {
            hosts: hosts_of(host),
            port: PortScope::Only(port),
        },
        None => Destination {
            hosts: Hosts::Pattern(pattern),
            port: PortScope::Any,
        },
    }
}

fn hosts_of(host: &str) -> Hosts<'_> {
    match host.parse::<IpNet>() {
        Ok(range) => Hosts::Range(range),
        Err(_) => Hosts::Pattern(host),
    }
}

fn split_port(pattern: &str) -> Option<(&str, u16)> {
    let (host, port) = split_destination(pattern);
    Some((host, port?.parse().ok()?))
}

/// A destination's address — brackets stripped, see [`unbracketed`] — and whatever sits in its port position, split the way lens-sandbox-core's `parse_matcher` splits one.
pub fn split_destination(destination: &str) -> (&str, Option<&str>) {
    if let Some((bracketed, port)) = destination.rsplit_once("]:") {
        return (bracketed.trim_start_matches('['), Some(port));
    }
    // Several colons and no bracketed port means a bare IPv6 literal, not a host:port pair.
    if destination.starts_with('[') || destination.matches(':').count() > 1 {
        return (unbracketed(destination), None);
    }
    match destination.rsplit_once(':') {
        // A tail that is no kind of number may be a hostname the author never meant to port-scope, so it is no port at all rather than a broken one.
        Some((address, port)) if port_shaped(port) => (address, Some(port)),
        _ => (destination, None),
    }
}

/// An IPv6 literal without the brackets a `host:port` pattern needs to carry it; every other host unchanged.
pub fn unbracketed(host: &str) -> &str {
    host.strip_prefix('[')
        .and_then(|inner| inner.strip_suffix(']'))
        .unwrap_or(host)
}

pub(crate) fn port_shaped(tail: &str) -> bool {
    !tail.is_empty() && tail.bytes().all(|b| b.is_ascii_digit())
}

fn destination_covers(existing: &str, new: &str) -> bool {
    let (existing, new) = (classify(existing), classify(new));
    port_covers(&existing.port, &new.port) && hosts_cover(&existing.hosts, &new.hosts)
}

/// A portless rule matches its hosts on every port, so it covers a rule pinned to one; a pinned rule covers only the same port.
fn port_covers(existing: &PortScope, new: &PortScope) -> bool {
    *existing == PortScope::Any || existing == new
}

fn hosts_cover(existing: &Hosts, new: &Hosts) -> bool {
    match (existing, new) {
        (Hosts::Range(existing), Hosts::Range(new)) => existing.contains(new),
        (Hosts::Range(existing), Hosts::Pattern(new)) => new
            .parse::<IpAddr>()
            .is_ok_and(|ip| existing.contains(&ip.to_canonical())),
        // Which addresses a name covers is DNS's answer, not ours, so only the catch-all is read as covering a range.
        (Hosts::Pattern(existing), Hosts::Range(_)) => *existing == CATCH_ALL,
        (Hosts::Pattern(existing), Hosts::Pattern(new)) => host_covers(existing, new),
    }
}

/// Whether every host `new` matches is also matched by `existing`.
fn host_covers(existing: &str, new: &str) -> bool {
    if new.contains('*') {
        wildcard_covers(existing, new)
    } else {
        domain_matches(existing, new)
    }
}

/// Every host a wildcard names ends in the fixed part after its last `*` — plus, for a leading wildcard, the apex — so a suffix wildcard covering that tail covers the pattern. Whether one mid-segment wildcard subsumes another is guesswork, and guessing wrong would silently reorder the file, so those stay equality-only.
fn wildcard_covers(existing: &str, new: &str) -> bool {
    let (existing, new) = (existing.to_ascii_lowercase(), new.to_ascii_lowercase());
    if existing == CATCH_ALL {
        return true;
    }
    match existing.strip_prefix("*.") {
        Some(suffix) => new
            .rsplit_once('*')
            .is_some_and(|(_, tail)| tail.ends_with(&format!(".{suffix}"))),
        None => existing == new,
    }
}

fn caller_covers(existing: Option<&[String]>, new: Option<&[String]>) -> bool {
    match (existing, new) {
        (None, _) => true,
        // A scoped rule claims the destination: the gate stops there for the callers it lists, and suppresses a later unrestricted allow or ask rather than let it re-open the rest. A later unrestricted deny is still reached, but it only repeats the denial they already get, so it is dead either way.
        (Some(_), None) => true,
        (Some(existing), Some(new)) => new
            .iter()
            .all(|binary| existing.iter().any(|listed| same_path(listed, binary))),
    }
}

fn same_path(one: &str, other: &str) -> bool {
    Path::new(one) == Path::new(other)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scoped(pattern: &str, binaries: &[&str]) -> RouteRule {
        RouteRule {
            binaries: Some(binaries.iter().map(|b| (*b).to_string()).collect()),
            ..RouteRule::allow_host(pattern)
        }
    }

    fn shadower_of(existing: Vec<RouteRule>, new: &RouteRule) -> Option<usize> {
        let mut network = NetworkPolicy::default();
        network.egress.http = existing;
        network.first_shadowing_rule(new).map(|(index, _)| index)
    }

    #[test]
    fn domain_matches_agrees_with_the_gate_on_every_pattern_shape() {
        assert!(domain_matches("*", "api.example.test"));
        assert!(!domain_matches("*", ""));
        assert!(domain_matches("*.example.test", "api.example.test"));
        assert!(
            domain_matches("*.example.test", "example.test"),
            "a leading wildcard covers the apex"
        );
        assert!(!domain_matches("*.example.test", "notexample.test"));
        assert!(domain_matches("api.*.example.test", "api.eu.example.test"));
        assert!(
            !domain_matches("*.example.test", "us-east-1example.test"),
            "the dot stays in the suffix, so a label boundary is required"
        );
        assert!(domain_matches("API.Example.Test", "api.example.test"));
        assert!(!domain_matches("api.example.test", "other.example.test"));
    }

    #[test]
    fn domain_matches_refuses_the_near_misses_a_wildcard_must_not_admit() {
        assert!(
            !domain_matches("*.example.test", "example.test.evil.test"),
            "the pattern is a suffix, so a host merely starting with it is a different name"
        );
        assert!(
            !domain_matches("api.*.example.test", "other.eu.example.test"),
            "the fixed prefix has to match too"
        );
        assert!(
            !domain_matches("api.*.example.test", "api.eu.example.test.evil.test"),
            "the fixed suffix has to end the host"
        );
        assert!(
            !domain_matches("api.*.example.test", "api.example.test"),
            "the wildcard label cannot be empty"
        );
    }

    #[test]
    fn a_wildcard_allow_shadows_a_later_rule_for_a_host_it_covers() {
        let existing = vec![RouteRule::allow_host("*.example.test")];
        assert_eq!(
            shadower_of(existing, &scoped("api.example.test", &["/usr/bin/curl"])),
            Some(0),
            "the gate stops at the wildcard, so narrowing api.example.test behind it would never fire"
        );
    }

    #[test]
    fn a_rule_for_one_host_does_not_shadow_a_wildcard_covering_more() {
        let existing = vec![RouteRule::allow_host("api.example.test")];
        assert_eq!(
            shadower_of(existing, &RouteRule::allow_host("*.example.test")),
            None,
            "the wildcard still fires for every other host under the suffix"
        );
    }

    #[test]
    fn an_identical_wildcard_shadows_a_later_one() {
        let existing = vec![RouteRule::allow_host("*.EXAMPLE.test")];
        assert_eq!(
            shadower_of(existing, &scoped("*.example.test", &["/usr/bin/git"])),
            Some(0)
        );
    }

    #[test]
    fn a_portless_rule_shadows_a_host_port_rule_for_the_same_host() {
        let existing = vec![RouteRule::allow_host("db.example.test")];
        assert_eq!(
            shadower_of(
                existing,
                &scoped("db.example.test:5432", &["/usr/bin/psql"])
            ),
            Some(0),
            "a portless domain rule matches every port"
        );
    }

    #[test]
    fn a_host_port_rule_does_not_shadow_the_portless_rule_for_that_host() {
        let existing = vec![RouteRule::allow_host("db.example.test:5432")];
        assert_eq!(
            shadower_of(existing, &RouteRule::allow_host("db.example.test")),
            None,
            "the portless rule still fires on every other port"
        );
    }

    #[test]
    fn host_port_rules_for_different_ports_do_not_shadow_each_other() {
        let existing = vec![RouteRule::allow_host("db.example.test:5432")];
        assert_eq!(
            shadower_of(
                existing,
                &scoped("db.example.test:6379", &["/usr/bin/redis-cli"])
            ),
            None
        );
    }

    /// The gate refuses an unbracketed IPv6 pattern outright; all this pins is that we don't mis-split one into a host and a port and compare the wrong host.
    #[test]
    fn a_bare_ipv6_literal_is_not_read_as_a_host_and_port() {
        let existing = vec![RouteRule::allow_host("2001:db8::1")];
        assert_eq!(
            shadower_of(existing, &RouteRule::allow_host("2001:db8::1")),
            Some(0)
        );
        assert_eq!(
            shadower_of(
                vec![RouteRule::allow_host("2001:db8::9")],
                &RouteRule::allow_host("2001:db8::1")
            ),
            None,
            "dropping the last group would read both as the same host"
        );
    }

    #[test]
    fn a_bracketed_pattern_the_gate_reads_as_a_name_is_not_split_on_its_inner_colon() {
        let existing = vec![RouteRule::allow_host("api.example.test")];
        assert_eq!(
            shadower_of(
                existing,
                &scoped("[api.example.test:443", &["/usr/bin/curl"])
            ),
            None,
            "the bracket makes this one malformed name, not the host the existing rule covers"
        );
    }

    #[test]
    fn the_catch_all_shadows_a_wildcard_rule_written_behind_it() {
        let existing = vec![RouteRule::allow_host("*")];
        assert_eq!(
            shadower_of(existing, &scoped("*.example.test", &["/usr/bin/git"])),
            Some(0),
            "the catch-all matches every host, so scoping a suffix behind it would restrict nobody"
        );
    }

    #[test]
    fn a_leading_wildcard_shadows_every_pattern_confined_to_its_suffix() {
        let existing = vec![RouteRule::allow_host("*.example.test")];
        assert_eq!(
            shadower_of(
                existing.clone(),
                &scoped("*.eu.example.test", &["/usr/bin/git"])
            ),
            Some(0),
            "every host under the narrower suffix is a host under the wider one"
        );
        assert_eq!(
            shadower_of(existing, &scoped("api.*.example.test", &["/usr/bin/git"])),
            Some(0),
            "a mid-segment pattern ending in the same suffix names hosts the wildcard already covers"
        );
    }

    #[test]
    fn a_wildcard_does_not_shadow_one_reaching_outside_its_suffix() {
        assert_eq!(
            shadower_of(
                vec![RouteRule::allow_host("*.eu.example.test")],
                &RouteRule::allow_host("*.example.test")
            ),
            None,
            "the wider wildcard still fires for every host outside the narrower suffix"
        );
        assert_eq!(
            shadower_of(
                vec![RouteRule::allow_host("*.example.test")],
                &RouteRule::allow_host("*")
            ),
            None,
            "a suffix rule leaves every other host to the catch-all"
        );
        assert_eq!(
            shadower_of(
                vec![RouteRule::allow_host("api.*.example.test")],
                &RouteRule::allow_host("*.example.test")
            ),
            None,
            "whether a mid-segment wildcard subsumes another is guesswork, and guessing wrong would silently reorder the file"
        );
    }

    #[test]
    fn a_bracketed_ipv6_host_port_shadows_the_same_host_and_port() {
        let existing = vec![RouteRule::allow_host("[2001:db8::1]:5432")];
        assert_eq!(
            shadower_of(existing, &scoped("[2001:db8::1]:5432", &["/usr/bin/psql"])),
            Some(0)
        );
    }

    #[test]
    fn a_cidr_shadows_the_identical_range_and_every_narrower_one_inside_it() {
        let existing = vec![RouteRule::allow_host("10.0.0.0/8")];
        assert_eq!(
            shadower_of(existing.clone(), &scoped("10.0.0.0/8", &["/usr/bin/curl"])),
            Some(0)
        );
        assert_eq!(
            shadower_of(existing, &scoped("10.1.0.0/16", &["/usr/bin/curl"])),
            Some(0),
            "the gate matches an address against the whole range, so a rule for a subrange behind it never fires"
        );
    }

    #[test]
    fn a_narrower_range_does_not_shadow_the_range_containing_it() {
        let existing = vec![RouteRule::allow_host("10.1.0.0/16")];
        assert_eq!(
            shadower_of(existing, &RouteRule::allow_host("10.0.0.0/8")),
            None,
            "the wider rule still fires for every address outside the narrower one"
        );
    }

    #[test]
    fn a_cidr_shadows_a_later_rule_for_an_address_inside_it() {
        let existing = vec![RouteRule::allow_host("10.0.0.0/8")];
        assert_eq!(
            shadower_of(existing.clone(), &scoped("10.1.2.3", &["/usr/bin/curl"])),
            Some(0),
            "the gate parses the request host as an address, so a bare literal inside the range is already claimed"
        );
        assert_eq!(
            shadower_of(existing, &scoped("10.1.2.3:5432", &["/usr/bin/psql"])),
            Some(0),
            "a portless range covers that address on every port too"
        );
    }

    #[test]
    fn a_cidr_does_not_shadow_a_rule_for_an_address_outside_it() {
        let existing = vec![RouteRule::allow_host("10.0.0.0/8")];
        assert_eq!(
            shadower_of(existing.clone(), &scoped("192.168.1.1", &["/usr/bin/curl"])),
            None
        );
        assert_eq!(
            shadower_of(existing, &scoped("[2001:db8::1]:443", &["/usr/bin/curl"])),
            None,
            "an IPv4 range holds no IPv6 address"
        );
    }

    #[test]
    fn a_cidr_reads_a_mapped_address_as_the_v4_address_it_names() {
        let existing = vec![RouteRule::allow_host("10.0.0.0/8")];
        assert_eq!(
            shadower_of(
                existing,
                &scoped("[::ffff:10.1.2.3]:443", &["/usr/bin/curl"])
            ),
            Some(0),
            "the gate canonicalizes the request address before the range test, so the mapped form is the same destination"
        );
    }

    #[test]
    fn a_portless_range_shadows_the_same_range_pinned_to_a_port() {
        let existing = vec![RouteRule::allow_host("10.0.0.0/8")];
        assert_eq!(
            shadower_of(existing, &scoped("10.0.0.0/8:5432", &["/usr/bin/psql"])),
            Some(0)
        );
    }

    #[test]
    fn a_range_pinned_to_a_port_does_not_shadow_the_portless_range() {
        let existing = vec![RouteRule::allow_host("10.0.0.0/8:5432")];
        assert_eq!(
            shadower_of(existing.clone(), &RouteRule::allow_host("10.0.0.0/8")),
            None,
            "the portless rule still fires on every other port"
        );
        assert_eq!(
            shadower_of(existing, &scoped("10.1.2.3:5432", &["/usr/bin/psql"])),
            Some(0),
            "on its own port it claims every address in the range"
        );
    }

    #[test]
    fn a_bracketed_ipv6_range_shadows_an_address_inside_it_on_the_same_port() {
        let existing = vec![RouteRule::allow_host("[2001:db8::/32]:5432")];
        assert_eq!(
            shadower_of(existing, &scoped("[2001:db8::1]:5432", &["/usr/bin/psql"])),
            Some(0)
        );
    }

    #[test]
    fn the_catch_all_shadows_a_range_rule_but_a_name_pattern_does_not() {
        assert_eq!(
            shadower_of(
                vec![RouteRule::allow_host("*")],
                &scoped("10.0.0.0/8", &["/usr/bin/curl"])
            ),
            Some(0),
            "the catch-all matches an address literal too, so a range rule behind it never fires"
        );
        assert_eq!(
            shadower_of(
                vec![RouteRule::allow_host("*.example.test")],
                &scoped("10.0.0.0/8", &["/usr/bin/curl"])
            ),
            None,
            "which addresses a name covers is DNS's answer, so the range rule is left where the user put it"
        );
    }

    #[test]
    fn a_range_does_not_shadow_a_rule_for_a_name() {
        let existing = vec![RouteRule::allow_host("10.0.0.0/8")];
        assert_eq!(
            shadower_of(existing, &scoped("api.example.test", &["/usr/bin/curl"])),
            None,
            "the name may resolve outside the range, so this is not ours to decide"
        );
    }

    #[test]
    fn a_pattern_whose_port_does_not_parse_is_read_as_a_name() {
        let existing = vec![RouteRule::allow_host("api.example.test")];
        assert_eq!(
            shadower_of(
                existing,
                &scoped("api.example.test:oops", &["/usr/bin/curl"])
            ),
            None,
            "the gate reads the whole thing as one name, which is not the host the existing rule covers"
        );
    }

    #[test]
    fn a_scoped_rule_shadows_a_later_unrestricted_allow_for_the_same_host() {
        let existing = vec![scoped("git.example.test", &["/usr/bin/git"])];
        assert_eq!(
            shadower_of(existing, &RouteRule::allow_host("git.example.test")),
            Some(0),
            "the gate serves the listed callers from the scoped rule and refuses to re-open the rest, so the later allow is dead"
        );
    }

    #[test]
    fn a_scoped_rule_shadows_a_later_deny_too() {
        let existing = vec![scoped("git.example.test", &["/usr/bin/git"])];
        assert_eq!(
            shadower_of(existing, &RouteRule::deny_host("git.example.test")),
            Some(0),
            "the listed caller is served by the scoped allow before the deny is ever reached, and the rest were already failing closed, so the deny behind it blocks nobody"
        );
    }

    #[test]
    fn a_scoped_rule_shadows_a_later_rule_scoped_to_a_subset_of_its_binaries() {
        let existing = vec![scoped(
            "git.example.test",
            &["/usr/bin/git", "/usr/bin/curl"],
        )];
        assert_eq!(
            shadower_of(existing, &scoped("git.example.test", &["/usr/bin/git/"])),
            Some(0),
            "the guest compares path components, so a trailing separator is the same binary"
        );
    }

    #[test]
    fn a_scoped_rule_does_not_shadow_one_naming_a_binary_it_excludes() {
        let existing = vec![scoped("git.example.test", &["/usr/bin/git"])];
        assert_eq!(
            shadower_of(existing, &scoped("git.example.test", &["/usr/bin/curl"])),
            None,
            "the excluded caller scans on and the second scoped rule admits it"
        );
    }

    #[test]
    fn a_rule_bound_to_one_scheme_does_not_shadow_the_other() {
        let https = RouteRule {
            scheme: Some(Scheme::Https),
            ..RouteRule::allow_host("api.example.test")
        };
        let http = RouteRule {
            scheme: Some(Scheme::Http),
            ..scoped("api.example.test", &["/usr/bin/curl"])
        };
        assert_eq!(shadower_of(vec![https.clone()], &http), None);
        assert_eq!(
            shadower_of(vec![https], &scoped("api.example.test", &["/usr/bin/curl"])),
            None,
            "a scheme-bound rule leaves plain-http requests to later rules"
        );
    }

    fn tcp_shadower_of(existing: Vec<TcpEgressRule>, new: &TcpEgressRule) -> Option<usize> {
        let mut network = NetworkPolicy::default();
        network.egress.tcp = existing;
        network
            .first_shadowing_tcp_rule(new)
            .map(|(index, _)| index)
    }

    #[test]
    fn a_raw_range_shadows_a_later_rule_for_an_address_and_port_it_covers() {
        assert_eq!(
            tcp_shadower_of(
                vec![TcpEgressRule::deny_destination("10.0.0.0/8:5432")],
                &TcpEgressRule::allow_destination("10.0.0.5:5432")
            ),
            Some(0),
            "the gate stops at the range, so an allow for one address behind it would never fire"
        );
    }

    #[test]
    fn a_raw_rule_on_one_port_does_not_shadow_another_port_on_the_same_host() {
        assert_eq!(
            tcp_shadower_of(
                vec![TcpEgressRule::deny_destination("db.internal:5432")],
                &TcpEgressRule::allow_destination("db.internal:6379")
            ),
            None,
            "a raw grant is per port, so the other port is still undecided"
        );
    }

    #[test]
    fn a_destination_splits_into_the_address_and_port_core_reads_it_as() {
        for (destination, address, port) in [
            ("db.internal:5432", "db.internal", Some("5432")),
            ("0.0.0.0/0:5432", "0.0.0.0/0", Some("5432")),
            ("[::/0]:5432", "::/0", Some("5432")),
            ("[2001:db8::1]:5432", "2001:db8::1", Some("5432")),
            ("[2001:db8::1]:notaport", "2001:db8::1", Some("notaport")),
            ("*:443", "*", Some("443")),
            ("db.internal", "db.internal", None),
            ("10.0.0.0/8", "10.0.0.0/8", None),
            // A tail that is no number is no port, so the pattern keeps it: `db:notaport` names one host, not port `notaport` on `db`.
            ("db:notaport", "db:notaport", None),
            ("db.internal:65536", "db.internal", Some("65536")),
            // Unbracketed IPv6 is a bare literal; reading its tail as a port would silently narrow the rule to port 1.
            ("2001:db8::1", "2001:db8::1", None),
            ("[2001:db8::1]", "2001:db8::1", None),
        ] {
            assert_eq!(
                split_destination(destination),
                (address, port),
                "for {destination}"
            );
        }
    }

    fn tcp_allow_over(http: Vec<RouteRule>, raw: &str) -> Vec<String> {
        let mut network = NetworkPolicy::default();
        network.egress.http = http;
        network
            .http_rules_pre_empted_by(&TcpEgressRule::allow_destination(raw))
            .iter()
            .map(|rule| rule.match_pattern.clone())
            .collect()
    }

    #[test]
    fn a_raw_allow_pre_empts_the_http_rules_covering_its_host() {
        assert_eq!(
            tcp_allow_over(
                vec![
                    RouteRule::deny_host("db.internal"),
                    RouteRule::allow_host("*.internal"),
                    RouteRule::allow_host("other.internal"),
                ],
                "db.internal:5432"
            ),
            vec!["db.internal", "*.internal"],
            "the raw table is consulted first, so both rules naming that host stop applying on that port"
        );
    }

    #[test]
    fn a_raw_allow_leaves_an_http_rule_pinned_to_another_port_alone() {
        assert_eq!(
            tcp_allow_over(
                vec![RouteRule::deny_host("db.internal:6379")],
                "db.internal:5432"
            ),
            Vec::<String>::new(),
            "the pre-filter claims one port; the http rule for the other one still runs"
        );
    }

    #[test]
    fn a_raw_deny_pre_empts_nothing_because_it_grants_nothing() {
        let mut network = NetworkPolicy::default();
        network.egress.http = vec![RouteRule::allow_host("db.internal")];
        assert!(
            network
                .http_rules_pre_empted_by(&TcpEgressRule::deny_destination("db.internal:5432"))
                .is_empty(),
            "a raw deny blocks the port outright; nothing is spliced past the http rules"
        );
    }

    #[test]
    fn a_raw_allow_matched_by_address_reports_no_pre_empted_http_rule() {
        assert_eq!(
            tcp_allow_over(
                vec![RouteRule::allow_host("db.internal")],
                "10.0.0.0/24:5432"
            ),
            Vec::<String>::new(),
            "whether that range covers the name is only knowable once it resolves, which the guest sees and we do not"
        );
    }

    #[test]
    fn a_scoped_raw_rule_does_not_shadow_one_naming_a_binary_it_excludes() {
        let existing = vec![TcpEgressRule {
            binaries: Some(vec!["/usr/bin/psql".into()]),
            ..TcpEgressRule::allow_destination("db.internal:5432")
        }];
        assert_eq!(
            tcp_shadower_of(
                existing,
                &TcpEgressRule {
                    binaries: Some(vec!["/usr/bin/nc".into()]),
                    ..TcpEgressRule::allow_destination("db.internal:5432")
                }
            ),
            None,
            "the excluded caller scans on and the second scoped rule decides it"
        );
    }

    #[test]
    fn the_first_shadowing_rule_is_the_one_reported() {
        let existing = vec![
            RouteRule::allow_host("other.example.test"),
            RouteRule::allow_host("*.example.test"),
            RouteRule::allow_host("api.example.test"),
        ];
        assert_eq!(
            shadower_of(existing, &scoped("api.example.test", &["/usr/bin/curl"])),
            Some(1),
            "the gate stops at the first match, so that is the rule to sit in front of"
        );
    }
}
