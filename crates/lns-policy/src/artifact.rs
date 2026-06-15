//! Encoding of a `Policy` as the registry's typed-artifact config blob; the wire shape mirrors the `policy` JSON Schema the registry serves at `GET /ext/v1/types`.

use serde::{Deserialize, Serialize};

use crate::Policy;

pub const POLICY_ARTIFACT_TYPE: &str = "application/vnd.lens.policy.v1+json";
pub const POLICY_CONFIG_MEDIA_TYPE: &str = "application/vnd.lens.policy.config.v1+json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(flatten)]
    pub policy: Policy,
}

pub fn encode(
    policy: &Policy,
    name: Option<&str>,
    version: Option<&str>,
) -> serde_json::Result<Vec<u8>> {
    let config = PolicyConfig {
        name: name.map(str::to_string),
        version: version.map(str::to_string),
        policy: policy.clone(),
    };
    serde_json::to_vec(&config)
}

pub fn decode(bytes: &[u8]) -> serde_json::Result<(Policy, Option<String>, Option<String>)> {
    let config: PolicyConfig = serde_json::from_slice(bytes)?;
    Ok((config.policy, config.name, config.version))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NetworkPolicy, RouteRule, Transport, Verdict};
    use serde_json::{Value, json};

    fn sample_policy() -> Policy {
        Policy {
            network: NetworkPolicy {
                allowed_routes: vec![RouteRule {
                    match_pattern: "api.example.test".into(),
                    verdict: Verdict::Allow,
                    transport: Transport::Upstream,
                    scheme: None,
                    description: Some("an allowed host".into()),
                    tls_terminate: false,
                    rules: Vec::new(),
                }],
                default_verdict: Verdict::Ask,
                default_transport: Transport::Direct,
            },
            integrations: vec!["some-integration".into()],
        }
    }

    fn policy_schema() -> Value {
        json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": {
                "name": { "type": "string" },
                "version": { "type": "string" },
                "network": {
                    "type": "object",
                    "properties": {
                        "allowedRoutes": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "required": ["match", "verdict"],
                                "properties": {
                                    "match": { "type": "string", "minLength": 1 },
                                    "verdict": { "enum": ["allow", "deny", "ask"] },
                                    "transport": { "enum": ["upstream", "direct"] },
                                    "scheme": { "enum": ["http", "https"] },
                                    "description": { "type": "string" },
                                    "tlsTerminate": { "type": "boolean" },
                                    "rules": {
                                        "type": "array",
                                        "items": {
                                            "type": "object",
                                            "properties": {
                                                "method": { "type": "string" },
                                                "path": { "type": "string" }
                                            }
                                        }
                                    }
                                }
                            }
                        },
                        "defaultVerdict": { "enum": ["allow", "deny", "ask"] },
                        "defaultTransport": { "enum": ["upstream", "direct"] }
                    }
                },
                "integrations": { "type": "array", "items": { "type": "string" } }
            }
        })
    }

    fn assert_schema_valid(blob: &[u8]) {
        let schema = policy_schema();
        let instance: Value = serde_json::from_slice(blob).expect("blob is JSON");
        let validator = jsonschema::validator_for(&schema).expect("schema compiles");
        let errors: Vec<String> = validator
            .iter_errors(&instance)
            .map(|e| e.to_string())
            .collect();
        assert!(
            errors.is_empty(),
            "config blob must satisfy the registry policy schema: {errors:?}"
        );
    }

    #[test]
    fn encode_produces_a_blob_that_satisfies_the_registry_policy_schema() {
        let blob = encode(&sample_policy(), Some("pii"), Some("v1")).unwrap();
        assert_schema_valid(&blob);
    }

    #[test]
    fn default_policy_blob_is_also_schema_valid() {
        let blob = encode(&Policy::default(), None, None).unwrap();
        assert_schema_valid(&blob);
    }

    #[test]
    fn encode_flattens_network_and_integrations_to_the_top_level_alongside_name_and_version() {
        let blob = encode(&sample_policy(), Some("pii"), Some("v1")).unwrap();
        let v: Value = serde_json::from_slice(&blob).unwrap();
        assert_eq!(v["name"], json!("pii"));
        assert_eq!(v["version"], json!("v1"));
        assert!(v.get("network").is_some(), "network at top level: {v}");
        assert_eq!(
            v["network"]["allowedRoutes"][0]["match"],
            json!("api.example.test")
        );
        assert_eq!(v["network"]["allowedRoutes"][0]["verdict"], json!("allow"));
        assert_eq!(v["integrations"], json!(["some-integration"]));
    }

    #[test]
    fn encode_omits_name_and_version_when_absent() {
        let blob = encode(&sample_policy(), None, None).unwrap();
        let v: Value = serde_json::from_slice(&blob).unwrap();
        assert!(v.get("name").is_none(), "name must be omitted: {v}");
        assert!(v.get("version").is_none(), "version must be omitted: {v}");
    }

    #[test]
    fn encode_then_decode_round_trips_the_policy_and_metadata() {
        let policy = sample_policy();
        let blob = encode(&policy, Some("pii"), Some("v1")).unwrap();
        let (decoded, name, version) = decode(&blob).unwrap();
        assert_eq!(decoded, policy);
        assert_eq!(name.as_deref(), Some("pii"));
        assert_eq!(version.as_deref(), Some("v1"));
    }

    #[test]
    fn decode_defaults_metadata_to_none_when_the_blob_carries_only_a_policy() {
        let blob = encode(&sample_policy(), None, None).unwrap();
        let (_, name, version) = decode(&blob).unwrap();
        assert!(name.is_none());
        assert!(version.is_none());
    }

    #[test]
    fn decode_rejects_a_malformed_blob() {
        let err = decode(b"not json at all").unwrap_err();
        assert!(err.is_syntax() || err.is_data(), "got: {err}");
    }

    #[test]
    fn media_type_constants_match_the_registry_contract() {
        assert_eq!(POLICY_ARTIFACT_TYPE, "application/vnd.lens.policy.v1+json");
        assert_eq!(
            POLICY_CONFIG_MEDIA_TYPE,
            "application/vnd.lens.policy.config.v1+json"
        );
    }
}
