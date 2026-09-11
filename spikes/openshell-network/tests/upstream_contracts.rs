use std::collections::HashMap;

use openshell_core::proto::{
    NetworkBinary, NetworkEndpoint, NetworkPolicyRule, SandboxPolicy, StaticCredentialBinding,
    StaticCredentialEndpointBinding,
};
use openshell_core::provider_credentials::ProviderCredentialState;
use openshell_supervisor_network::opa::{NetworkAction, NetworkInput, OpaEngine};

#[test]
fn upstream_wildcard_is_not_the_lns_wildcard() {
    let policy = SandboxPolicy {
        network_policies: HashMap::from([(
            "api".into(),
            NetworkPolicyRule {
                endpoints: vec![NetworkEndpoint {
                    host: "*.example.com".into(),
                    ports: vec![443],
                    ..Default::default()
                }],
                binaries: vec![NetworkBinary {
                    path: "/usr/bin/curl".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
        )]),
        ..Default::default()
    };
    let engine = OpaEngine::from_proto(&policy).unwrap();
    for (host, allowed) in [
        ("api.example.com", true),
        ("example.com", false),
        ("a.b.example.com", false),
    ] {
        let input = NetworkInput {
            host: host.into(),
            port: 443,
            binary_path: "/usr/bin/curl".into(),
            binary_sha256: String::new(),
            ancestors: vec![],
            cmdline_paths: vec![],
        };
        assert_eq!(
            matches!(
                engine.authorize_egress(&input).unwrap().action,
                NetworkAction::Allow { .. }
            ),
            allowed,
            "{host}"
        );
        assert!(lns_policy::matching::domain_matches("*.example.com", host));
    }
}

fn binding() -> HashMap<String, StaticCredentialBinding> {
    HashMap::from([(
        "API_KEY".into(),
        StaticCredentialBinding {
            endpoints: vec![StaticCredentialEndpointBinding {
                host: "api.example.com".into(),
                port: 443,
                path: "/v1/**".into(),
            }],
            credential_identity: "spike:API_KEY".into(),
            workload_credential_handle: "a".repeat(64),
        },
    )])
}

#[test]
fn upstream_credentials_rotate_without_replacing_the_workload_marker() {
    let state = ProviderCredentialState::from_bound_environment(
        1,
        HashMap::from([("API_KEY".into(), "fake-spike-value-1".into())]),
        HashMap::new(),
        HashMap::new(),
        binding(),
        vec![],
    )
    .unwrap();
    let workload_env = state.snapshot().child_env.clone();
    let marker = &workload_env["API_KEY"];
    assert_ne!(marker, "fake-spike-value-1");
    state
        .install_bound_environment(
            2,
            HashMap::from([("API_KEY".into(), "fake-spike-value-2".into())]),
            HashMap::new(),
            HashMap::new(),
            binding(),
            vec![],
        )
        .unwrap();
    assert_eq!(state.snapshot().child_env, workload_env);
    let resolver = state
        .resolver_for_endpoint("api.example.com", 443, "/v1/messages")
        .unwrap();
    assert_eq!(
        resolver
            .rewrite_header_value(&format!("Bearer {marker}"))
            .unwrap(),
        Some("Bearer fake-spike-value-2".into())
    );
    for (host, port, path) in [
        ("other.example.com", 443, "/v1/messages"),
        ("api.example.com", 80, "/v1/messages"),
        ("api.example.com", 443, "/v2/messages"),
    ] {
        let resolver = state.resolver_for_endpoint(host, port, path).unwrap();
        assert!(
            resolver
                .rewrite_header_value(marker)
                .unwrap_err()
                .is_endpoint_mismatch()
        );
    }
}

#[test]
fn upstream_cannot_arm_an_existing_literal_lns_placeholder() {
    let marker = "LNSPLACEHOLDER_SPIKE_0000000000";
    let state = ProviderCredentialState::from_child_env_snapshot(
        0,
        HashMap::from([("API_KEY".into(), marker.into())]),
    );
    let running_workload_env = state.snapshot().child_env.clone();
    assert!(state.resolver().is_none());
    state
        .install_bound_environment(
            1,
            HashMap::from([("API_KEY".into(), "fake-spike-value".into())]),
            HashMap::new(),
            HashMap::new(),
            binding(),
            vec![],
        )
        .unwrap();
    assert_ne!(state.snapshot().child_env, running_workload_env);
    let resolver = state
        .resolver_for_endpoint("api.example.com", 443, "/v1/messages")
        .unwrap();
    assert_eq!(resolver.resolve_placeholder(marker), None);
    assert_eq!(
        resolver
            .rewrite_header_value(&format!("Bearer {marker}"))
            .unwrap(),
        None
    );
}
