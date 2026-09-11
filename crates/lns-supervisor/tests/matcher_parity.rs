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
fn the_host_matcher_agrees_with_the_openshell_gate() {
    for (pattern, host) in CASES {
        let (tx, _) = tokio::sync::mpsc::unbounded_channel();
        let gate = lns_openshell_spike::Gate::new(tx).unwrap();
        let policy = serde_json::from_value(serde_json::json!({
            "network": {"egress": {"http": [{"match": pattern, "verdict": "allow"}]}}
        }))
        .unwrap();
        gate.reload(&policy).unwrap();
        let authorization = gate
            .engine
            .authorize_egress(&openshell_supervisor_network::opa::NetworkInput {
                host: host.to_string(),
                port: 443,
                binary_path: "/usr/bin/curl".into(),
                binary_sha256: String::new(),
                ancestors: Vec::new(),
                cmdline_paths: Vec::new(),
            })
            .unwrap();
        assert_eq!(
            lns_policy::matching::domain_matches(pattern, host),
            matches!(
                authorization.action,
                openshell_supervisor_network::opa::NetworkAction::Allow { .. }
            ),
            "the CLI decides rule placement with its own copy of this matcher, so a divergence on ({pattern:?}, {host:?}) means it reasons about an order the gate does not enforce"
        );
    }
}
