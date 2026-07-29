use serde::{Deserialize, Serialize};

use lns_policy::NetworkPolicy;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HostFrame {
    Policy(PolicyMessage),
    RequestDecision(RequestDecision),
    CredentialDecision(CredentialDecision),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GuestFrame {
    RequestPending(RequestPending),
    CredentialPending(CredentialPending),
    #[serde(other)]
    Other,
}

/// Outbound `policy` payload; the receiver tolerates extra fields, so we send only `network` and (when armed) `credentials`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyMessage {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<NetworkPolicy>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credentials: Option<Vec<Credential>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestPending {
    pub id: String,
    pub host: String,
    pub action: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestDecision {
    pub id: String,
    pub decision: Decision,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    AllowAlways,
    AllowOnce,
    DenyAlways,
    DenyOnce,
    Timeout,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Credential {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_var: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placeholder: Option<String>,
    #[serde(default)]
    pub injections: Vec<CredentialInjection>,
}

/// `UriPlaceholder` substitutes the parent `Credential.placeholder` inside matching outbound URLs, so unlike `Header` it carries no placeholder of its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "injectionType",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum CredentialInjection {
    Header {
        domain: String,
        header: String,
        value: String,
    },
    UriPlaceholder {
        domain: String,
        value: String,
    },
}

impl CredentialInjection {
    pub(crate) fn domain(&self) -> &str {
        match self {
            Self::Header { domain, .. } => domain,
            Self::UriPlaceholder { domain, .. } => domain,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialPending {
    pub id: String,
    pub credential_id: String,
    pub action: String,
    pub reason: String,
}

/// Outbound `credential_decision`; the real credential value, if any, follows in a later `policy` frame rather than here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CredentialDecision {
    pub id: String,
    pub decision: CredentialDecisionKind,
}

/// Unlike [`Decision`] there is no once/always — every credential decision is sticky, and `Timeout` is lns-service-initiated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialDecisionKind {
    Allow,
    Deny,
    Timeout,
}

#[cfg(test)]
mod tests {
    use super::*;
    use lns_policy::{NetworkPolicy, RouteRule, Transport, Verdict};
    use serde_json::json;

    #[test]
    fn policy_frame_serializes_with_type_discriminator() {
        let frame = HostFrame::Policy(PolicyMessage {
            network: Some(NetworkPolicy::default()),
            credentials: None,
        });
        let v: serde_json::Value = serde_json::to_value(&frame).unwrap();
        assert_eq!(v["type"], "policy");
        assert_eq!(v["network"]["defaultVerdict"], "ask");
        assert_eq!(
            v["network"]["defaultTransport"], "direct",
            "lens-sandbox-core fail-closes a non-deny verdict to deny when defaultTransport is missing"
        );
        assert_eq!(v["network"]["egress"]["http"], json!([]));
    }

    #[test]
    fn the_policy_frame_publishes_one_egress_table_and_not_the_deprecated_list() {
        let frame = HostFrame::Policy(PolicyMessage {
            network: Some(NetworkPolicy::default()),
            credentials: None,
        });
        let v: serde_json::Value = serde_json::to_value(&frame).unwrap();
        let network = v["network"].as_object().expect("network is an object");
        assert!(
            !network.contains_key("allowedRoutes"),
            "an egress block supersedes allowedRoutes in the guest, so a route sent in both is a route sent in neither: {network:?}"
        );
        let egress = network["egress"]
            .as_object()
            .expect("a null or scalar egress makes the guest force-deny every destination");
        assert_eq!(
            egress.keys().collect::<Vec<_>>(),
            ["http"],
            "core denies unknown fields under egress, so one stray key fails the whole policy closed: {egress:?}"
        );
    }

    #[test]
    fn every_route_in_the_frame_carries_its_transport_even_when_direct() {
        let mut net = NetworkPolicy::default();
        net.egress.http.push(RouteRule::allow_host("10.0.0.0/8"));
        let frame = HostFrame::Policy(PolicyMessage {
            network: Some(net),
            credentials: None,
        });
        let v: serde_json::Value = serde_json::to_value(&frame).unwrap();
        assert_eq!(
            v["network"]["egress"]["http"][0]["transport"], "direct",
            "core's route schema has no transport default — one missing transport fails the whole route parse and clears every route"
        );
    }

    #[test]
    fn policy_frame_round_trips() {
        let mut net = NetworkPolicy::default();
        net.egress
            .http
            .push(RouteRule::allow_host("api.linear.app"));
        let frame = HostFrame::Policy(PolicyMessage {
            network: Some(net),
            credentials: None,
        });

        let s = serde_json::to_string(&frame).unwrap();
        let parsed: HostFrame = serde_json::from_str(&s).unwrap();
        assert_eq!(frame, parsed);
    }

    #[test]
    fn request_decision_serializes_decision_in_camel_case() {
        let frame = HostFrame::RequestDecision(RequestDecision {
            id: "req-1".into(),
            decision: Decision::AllowAlways,
        });
        let v: serde_json::Value = serde_json::to_value(&frame).unwrap();
        assert_eq!(v["type"], "request_decision");
        assert_eq!(v["id"], "req-1");
        assert_eq!(v["decision"], "allow_always");
    }

    #[test]
    fn every_decision_variant_serializes_to_snake_case_token_matching_in_sandbox_proxy() {
        for (variant, expected) in [
            (Decision::AllowOnce, "allow_once"),
            (Decision::AllowAlways, "allow_always"),
            (Decision::DenyOnce, "deny_once"),
            (Decision::DenyAlways, "deny_always"),
            (Decision::Timeout, "timeout"),
        ] {
            let s = serde_json::to_string(&variant).unwrap();
            assert_eq!(s.trim_matches('"'), expected);
        }
    }

    #[test]
    fn request_pending_parses_from_gate_emitted_json() {
        let raw = r#"{
            "type": "request_pending",
            "id": "req-42",
            "host": "api.linear.app",
            "action": "CONNECT api.linear.app:443",
            "reason": "policy-ambiguous"
        }"#;
        let parsed: GuestFrame = serde_json::from_str(raw).unwrap();
        assert!(
            matches!(&parsed, GuestFrame::RequestPending(rp)
                if rp.id == "req-42"
                    && rp.host == "api.linear.app"
                    && rp.action == "CONNECT api.linear.app:443"
                    && rp.reason == "policy-ambiguous"),
            "got {parsed:?}"
        );
    }

    #[test]
    fn unrecognised_guest_frame_lands_in_other_not_error() {
        let raw = r#"{"type":"audit_event","payload":{"x":1}}"#;
        let parsed: GuestFrame = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed, GuestFrame::Other);
    }

    #[test]
    fn host_frame_text_envelope_matches_lens_sandbox_core_dispatch() {
        let policy_s = serde_json::to_value(HostFrame::Policy(PolicyMessage::default())).unwrap();
        let decision_s = serde_json::to_value(HostFrame::RequestDecision(RequestDecision {
            id: "x".into(),
            decision: Decision::DenyOnce,
        }))
        .unwrap();
        assert_eq!(policy_s["type"], "policy");
        assert_eq!(decision_s["type"], "request_decision");
    }

    #[test]
    fn empty_policy_frame_serializes_with_only_type_field() {
        let frame = HostFrame::Policy(PolicyMessage::default());
        let s = serde_json::to_string(&frame).unwrap();
        assert_eq!(s, r#"{"type":"policy"}"#);
    }

    #[test]
    fn network_policy_in_frame_serializes_route_rule_with_match_key() {
        let mut net = NetworkPolicy::default();
        net.egress.http.push(RouteRule {
            match_pattern: "api.linear.app".into(),
            verdict: Verdict::Allow,
            transport: Transport::Upstream,
            scheme: None,
            description: None,
            tls_terminate: false,
            rules: Vec::new(),
        });
        let frame = HostFrame::Policy(PolicyMessage {
            network: Some(net),
            credentials: None,
        });
        let s = serde_json::to_string(&frame).unwrap();
        assert!(s.contains(r#""match":"api.linear.app""#), "got: {s}");
        assert!(!s.contains("matchPattern"), "rust ident leaked: {s}");
    }

    fn some_credential(armed: bool) -> Credential {
        Credential {
            id: "some-provider".into(),
            env_var: Some("SOME_TOKEN".into()),
            placeholder: Some("some-placeholder-0000000000000000000000".into()),
            injections: if armed {
                vec![CredentialInjection::Header {
                    domain: "api.some-provider.example".into(),
                    header: "Authorization".into(),
                    value: "Bearer some-secret".into(),
                }]
            } else {
                Vec::new()
            },
        }
    }

    #[test]
    fn credential_in_policy_frame_serializes_with_camel_case_field_names() {
        let frame = HostFrame::Policy(PolicyMessage {
            network: None,
            credentials: Some(vec![some_credential(false)]),
        });
        let s = serde_json::to_string(&frame).unwrap();
        assert!(s.contains(r#""envVar":"SOME_TOKEN""#), "got: {s}");
        assert!(!s.contains("env_var"), "rust ident leaked into JSON: {s}");
        assert!(s.contains(r#""placeholder":"some-placeholder"#), "got: {s}");
        assert!(s.contains(r#""injections":[]"#), "got: {s}");
    }

    #[test]
    fn header_injection_serializes_with_injection_type_discriminator() {
        let frame = HostFrame::Policy(PolicyMessage {
            network: None,
            credentials: Some(vec![some_credential(true)]),
        });
        let s = serde_json::to_string(&frame).unwrap();
        assert!(
            s.contains(r#""injectionType":"header""#),
            "discriminator tag missing or wrong: {s}"
        );
        assert!(
            s.contains(r#""domain":"api.some-provider.example""#),
            "got: {s}"
        );
        assert!(s.contains(r#""header":"Authorization""#), "got: {s}");
    }

    #[test]
    fn policy_frame_with_credentials_round_trips() {
        let frame = HostFrame::Policy(PolicyMessage {
            network: Some(NetworkPolicy::default()),
            credentials: Some(vec![some_credential(true)]),
        });
        let s = serde_json::to_string(&frame).unwrap();
        let parsed: HostFrame = serde_json::from_str(&s).unwrap();
        assert_eq!(frame, parsed);
    }

    #[test]
    fn credential_pending_parses_from_proxy_emitted_json() {
        let raw = r#"{
            "type": "credential_pending",
            "id": "cred-42",
            "credentialId": "some-provider",
            "action": "POST https://api.some-provider.example/issues",
            "reason": "placeholder-unauthorized"
        }"#;
        let parsed: GuestFrame = serde_json::from_str(raw).unwrap();
        assert!(
            matches!(&parsed, GuestFrame::CredentialPending(cp)
                if cp.id == "cred-42"
                    && cp.credential_id == "some-provider"
                    && cp.action == "POST https://api.some-provider.example/issues"
                    && cp.reason == "placeholder-unauthorized"),
            "got {parsed:?}"
        );
    }

    #[test]
    fn credential_decision_serializes_with_envelope_type_and_snake_case_kind() {
        let frame = HostFrame::CredentialDecision(CredentialDecision {
            id: "cred-1".into(),
            decision: CredentialDecisionKind::Allow,
        });
        let v: serde_json::Value = serde_json::to_value(&frame).unwrap();
        assert_eq!(v["type"], "credential_decision");
        assert_eq!(v["id"], "cred-1");
        assert_eq!(v["decision"], "allow");
    }

    #[test]
    fn every_credential_decision_kind_serializes_to_snake_case_token() {
        for (variant, expected) in [
            (CredentialDecisionKind::Allow, "allow"),
            (CredentialDecisionKind::Deny, "deny"),
            (CredentialDecisionKind::Timeout, "timeout"),
        ] {
            let s = serde_json::to_string(&variant).unwrap();
            assert_eq!(s.trim_matches('"'), expected);
        }
    }

    #[test]
    fn host_frame_credential_decision_round_trips_through_json() {
        let frame = HostFrame::CredentialDecision(CredentialDecision {
            id: "cred-1".into(),
            decision: CredentialDecisionKind::Deny,
        });
        let s = serde_json::to_string(&frame).unwrap();
        let parsed: HostFrame = serde_json::from_str(&s).unwrap();
        assert_eq!(frame, parsed);
    }

    #[test]
    fn domain_accessor_returns_domain_for_both_injection_variants() {
        let header = CredentialInjection::Header {
            domain: "api.some-provider.example".into(),
            header: "Authorization".into(),
            value: "Bearer some-secret".into(),
        };
        assert_eq!(header.domain(), "api.some-provider.example");

        let uri = CredentialInjection::UriPlaceholder {
            domain: "api.some-provider.example".into(),
            value: "some-secret".into(),
        };
        assert_eq!(uri.domain(), "api.some-provider.example");
    }
}
