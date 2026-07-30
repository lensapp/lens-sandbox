//! Pins `lns-policy`'s host-side copy of the gate's matcher to the gate itself; this crate is the only one that depends on both.

const CASES: &[(&str, &str)] = &[
    ("*", "api.example.test"),
    ("*", "1.2.3.4"),
    ("*", ""),
    ("*.example.test", "example.test"),
    ("*.example.test", "api.example.test"),
    ("*.example.test", "a.b.example.test"),
    ("*.example.test", "notexample.test"),
    ("*.example.test", "us-east-1example.test"),
    ("*.example.test", "example.test.evil.test"),
    ("api.*.example.test", "api.eu.example.test"),
    ("api.*.example.test", "api.example.test"),
    ("api.*.example.test", "other.eu.example.test"),
    ("api.*.example.test", "api.eu.example.test.evil.test"),
    ("api.*.example.test", "api.eu.exampleXtest"),
    ("*-api.example.test", "eu-api.example.test"),
    ("API.Example.Test", "api.example.test"),
    ("api.example.test", "API.EXAMPLE.TEST"),
    ("api.example.test", "other.example.test"),
    ("api.example.test", "api.example.test"),
    ("api.example.test", ""),
    ("10.0.0.1", "10.0.0.1"),
];

#[test]
fn the_ported_matcher_agrees_with_the_gate_it_was_ported_from() {
    for (pattern, host) in CASES {
        assert_eq!(
            lns_policy::matching::domain_matches(pattern, host),
            lens_sandbox_core::routing::domain_matches(pattern, host),
            "the CLI decides rule placement with its own copy of this matcher, so a divergence on ({pattern:?}, {host:?}) means it reasons about an order the gate does not enforce"
        );
    }
}
