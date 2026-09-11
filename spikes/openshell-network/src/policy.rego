package openshell.sandbox

network_middlewares := {}
endpoint_credential_guards := []

host_matches(matcher) if {
    matcher.kind == "any"
    input.network.host != ""
}

host_matches(matcher) if {
    matcher.kind == "exact"
    lower(input.network.host) == matcher.host
}

host_matches(matcher) if {
    matcher.kind == "suffix"
    lower(input.network.host) == matcher.suffix
}

host_matches(matcher) if {
    matcher.kind == "suffix"
    endswith(lower(input.network.host), concat("", [".", matcher.suffix]))
}

host_matches(matcher) if {
    matcher.kind == "middle"
    host := lower(input.network.host)
    startswith(host, matcher.prefix)
    endswith(host, matcher.suffix)
    count(host) > count(matcher.prefix) + count(matcher.suffix)
}

port_matches(rule) if { rule.port == null }
port_matches(rule) if { rule.port == input.network.port }

matching_indices contains i if {
    some i
    rule := data.lns_rules[i]
    host_matches(rule.matcher)
    port_matches(rule)
}

selected := data.lns_rules[min(matching_indices)] if { count(matching_indices) > 0 }

binary_matches(rule) if { rule.binaries == null }
binary_matches(rule) if { input.exec.path in rule.binaries }

default action := "deny"
action := "allow" if {
    selected.verdict == "allow"
    binary_matches(selected)
}

default reason := "lns:ask"
reason := "lns:deny" if { count(matching_indices) > 0 }

egress_authorization := {
    "action": action,
    "matched_policy": "lns:raw",
    "deny_reason": reason,
    "endpoint_configs": [{"tls": "skip"}],
    "matched_endpoints": [],
    "exact_declared_endpoint_host": false,
}
