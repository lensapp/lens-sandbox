use lns_policy::{Policy, Verdict};
use std::collections::HashSet;

pub fn eligible(policy: &Policy, host: &str) -> bool {
    let host = host.strip_suffix('.').unwrap_or(host).to_ascii_lowercase();
    if !valid_name(&host) {
        return false;
    }
    let tcp = policy
        .network
        .egress
        .tcp
        .iter()
        .map(|rule| (rule.match_pattern.as_str(), rule.verdict));
    let http = policy
        .network
        .egress
        .http
        .iter()
        .map(|rule| (rule.match_pattern.as_str(), rule.verdict));
    let mut decided_ports = HashSet::new();
    for (pattern, verdict) in tcp.chain(http) {
        let (pattern, port) = lns_policy::matching::split_destination(pattern);
        if !lns_policy::matching::domain_matches(pattern, &host) {
            continue;
        }
        match port {
            Some(port) => {
                let Ok(port) = port.parse::<u16>() else {
                    return false;
                };
                if port != 0 && decided_ports.insert(port) && verdict == Verdict::Allow {
                    return true;
                }
            }
            None => {
                return verdict == Verdict::Allow && decided_ports.len() < usize::from(u16::MAX);
            }
        }
    }
    false
}

fn valid_name(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host.parse::<std::net::IpAddr>().is_err()
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn dns_requires_at_least_one_port_with_an_effective_allow() {
        for (tcp, http, expected) in [
            (json!([]), json!([]), false),
            (
                json!([]),
                json!([{"match":"*", "verdict":"deny"}, {"match":"api.example", "verdict":"allow"}]),
                false,
            ),
            (
                json!([{"match":"api.example:443", "verdict":"deny"}]),
                json!([{"match":"api.example:443", "verdict":"allow"}]),
                false,
            ),
            (
                json!([{"match":"api.example:443", "verdict":"deny"}]),
                json!([{"match":"api.example:80", "verdict":"allow"}]),
                true,
            ),
            (
                json!([{"match":"api.example:443", "verdict":"deny"}]),
                json!([{"match":"api.example", "verdict":"allow"}]),
                true,
            ),
            (
                json!([{"match":"api.example:443", "verdict":"allow"}]),
                json!([{"match":"*", "verdict":"deny"}]),
                true,
            ),
        ] {
            let policy =
                serde_json::from_value(json!({"network":{"egress":{"tcp":tcp,"http":http}}}))
                    .unwrap();
            assert_eq!(eligible(&policy, "API.EXAMPLE."), expected, "{policy:?}");
        }
    }

    #[test]
    fn invalid_names_do_not_inherit_a_wildcard_allow() {
        let policy = serde_json::from_value(
            json!({"network":{"egress":{"http":[{"match":"*", "verdict":"allow"}]}}}),
        )
        .unwrap();
        for host in [
            "",
            ".",
            "api.example..",
            "a..example",
            "a b.example",
            "*.example",
            "127.0.0.1",
            "::1",
        ] {
            assert!(!eligible(&policy, host), "{host}");
        }
        assert!(!eligible(&policy, &format!("{}.example", "a".repeat(64))));
        assert!(!eligible(&policy, &format!("{}a", "abc.".repeat(64))));
        assert!(eligible(&policy, "api.example"));
    }
}
