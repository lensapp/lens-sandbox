use crate::spec;
use serde_json::Value;

/// Validate a single artifact document: schema + cross-field guards for its declared kind, plus the secret guard. Returns every problem found, not just the first.
pub fn validate(doc: &[u8]) -> Result<(), Vec<String>> {
    let mut problems = Vec::new();
    let schema = if crate::sandbox::is_sandbox_definition(doc) {
        crate::sandbox::validate(doc)
    } else {
        spec::validate_any(doc)
    };
    if let Err(e) = schema {
        problems.push(format!("schema: {e:#}"));
    }
    problems.extend(scan_secrets(doc));
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems)
    }
}

fn scan_secrets(doc: &[u8]) -> Vec<String> {
    let Ok(value) = serde_json::from_slice::<Value>(doc) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    walk(&value, "", &mut found);
    found
}

fn walk(value: &Value, path: &str, out: &mut Vec<String>) {
    match value {
        Value::String(s) => {
            if let Some(kind) = secret_shape(s) {
                out.push(format!(
                    "secret: {path} looks like a real {kind}; artifacts must carry credential placeholders, never real secrets"
                ));
            }
        }
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                walk(item, &format!("{path}[{i}]"), out);
            }
        }
        Value::Object(map) => {
            for (key, item) in map {
                let child = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                walk(item, &child, out);
            }
        }
        _ => {}
    }
}

/// A string that self-identifies as a placeholder is always exempt; otherwise flag values shaped like well-known credential formats.
fn secret_shape(value: &str) -> Option<&'static str> {
    if value.to_ascii_lowercase().contains("placeholder") {
        return None;
    }
    if is_github_token(value) {
        return Some("GitHub token");
    }
    if is_openai_key(value) {
        return Some("OpenAI-style API key");
    }
    if is_aws_access_key_id(value) {
        return Some("AWS access key id");
    }
    None
}

fn is_github_token(value: &str) -> bool {
    let Some(rest) = value.strip_prefix("ghp_") else {
        return false;
    };
    rest.len() == 36 && rest.bytes().all(|b| b.is_ascii_alphanumeric())
}

fn is_openai_key(value: &str) -> bool {
    let Some(rest) = value.strip_prefix("sk-") else {
        return false;
    };
    rest.len() >= 20 && rest.bytes().all(|b| b.is_ascii_alphanumeric())
}

fn is_aws_access_key_id(value: &str) -> bool {
    let Some(rest) = value.strip_prefix("AKIA") else {
        return false;
    };
    rest.len() == 16
        && rest
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox(base_image: &str) -> Vec<u8> {
        format!(
            r#"{{"apiVersion":"lens.dev/v1alpha1","kind":"Sandbox","metadata":{{"name":"some-sandbox"}},"spec":{{"isolation":"microvm","baseImage":"{base_image}"}}}}"#
        )
        .into_bytes()
    }

    #[test]
    fn a_well_formed_sandbox_validates() {
        let doc = sandbox(&format!("reg/base@sha256:{}", "a".repeat(64)));
        validate(&doc).unwrap();
    }

    #[test]
    fn a_schema_violation_is_reported() {
        let doc = sandbox("reg/base:1");
        let problems = validate(&doc).unwrap_err();
        assert!(
            problems.iter().any(|p| p.contains("digest-pinned")),
            "a floating base image must be reported: {problems:?}"
        );
    }

    #[test]
    fn an_unknown_kind_is_reported() {
        let doc = br#"{"apiVersion":"lens.dev/v1alpha1","kind":"Sorcery","metadata":{"name":"x"},"spec":{}}"#;
        let problems = validate(doc).unwrap_err();
        assert!(problems.iter().any(|p| p.contains("unknown artifact kind")));
    }

    #[test]
    fn a_real_looking_token_in_an_agent_env_is_flagged_as_a_secret() {
        let doc = format!(
            r#"{{"apiVersion":"lens.dev/v1alpha1","kind":"Agent","metadata":{{"name":"some-agent"}},"spec":{{"command":"agent","env":{{"GH_TOKEN":"ghp_{}"}}}}}}"#,
            "a".repeat(36)
        )
        .into_bytes();
        let problems = validate(&doc).unwrap_err();
        assert!(
            problems
                .iter()
                .any(|p| p.contains("GitHub token") && p.contains("spec.env.GH_TOKEN")),
            "a github token must be caught and located: {problems:?}"
        );
    }

    #[test]
    fn a_self_identifying_placeholder_is_not_flagged() {
        let doc = format!(
            r#"{{"apiVersion":"lens.dev/v1alpha1","kind":"Agent","metadata":{{"name":"some-agent"}},"spec":{{"command":"agent","env":{{"GH_TOKEN":"ghp_PLACEHOLDER{}"}}}}}}"#,
            "0".repeat(27)
        )
        .into_bytes();
        validate(&doc).expect("a placeholder-shaped value must pass the secret guard");
    }

    #[test]
    fn secret_shape_recognizes_known_formats_and_ignores_benign_strings() {
        assert_eq!(
            secret_shape(&format!("ghp_{}", "a".repeat(36))),
            Some("GitHub token")
        );
        assert_eq!(
            secret_shape(&format!("sk-{}", "a".repeat(20))),
            Some("OpenAI-style API key")
        );
        assert_eq!(
            secret_shape(&format!("AKIA{}", "A".repeat(16))),
            Some("AWS access key id")
        );
        assert_eq!(secret_shape("some-secret"), None);
        assert_eq!(secret_shape("ghp_short"), None);
        assert_eq!(secret_shape("sk-short"), None);
        assert_eq!(secret_shape("AKIAtooshort"), None);
        assert_eq!(secret_shape(&format!("ghp_{}", "!".repeat(36))), None);
        assert_eq!(secret_shape(&format!("sk-{}", "!".repeat(20))), None);
        assert_eq!(secret_shape(&format!("AKIA{}", "a".repeat(16))), None);
    }

    #[test]
    fn the_secret_scan_walks_nested_arrays_and_ignores_non_strings() {
        let doc = format!(
            r#"{{"apiVersion":"lens.dev/v1alpha1","kind":"Agent","metadata":{{"name":"some-agent"}},"spec":{{"command":"agent","ports":[{{"container":8080}}],"env":{{"K":"sk-{}"}}}}}}"#,
            "b".repeat(24)
        )
        .into_bytes();
        let problems = validate(&doc).unwrap_err();
        assert!(problems.iter().any(|p| p.contains("OpenAI-style API key")));
    }

    #[test]
    fn a_non_json_document_surfaces_only_the_schema_error() {
        let problems = validate(b"not json at all").unwrap_err();
        assert_eq!(problems.len(), 1);
        assert!(problems[0].starts_with("schema:"));
    }

    #[test]
    fn a_well_formed_flat_sandbox_definition_validates() {
        let doc = br#"{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{"name":"hermes"},"spec":{"image":"ghcr.io/team/base:1"}}"#;
        validate(doc).expect("a lns.run/v1 sandbox routes to the flat validator");
    }

    #[test]
    fn a_flat_sandbox_schema_violation_is_reported() {
        let doc = br#"{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{"name":"hermes"},"spec":{}}"#;
        let problems = validate(doc).unwrap_err();
        assert!(
            problems.iter().any(|p| p.contains("must carry an image")),
            "a flat sandbox missing its image must be reported: {problems:?}"
        );
    }

    #[test]
    fn a_real_looking_token_in_a_flat_sandbox_env_is_flagged() {
        let doc = format!(
            r#"{{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{{"name":"hermes"}},"spec":{{"image":"x:1","env":{{"GH_TOKEN":"ghp_{}"}}}}}}"#,
            "a".repeat(36)
        )
        .into_bytes();
        let problems = validate(&doc).unwrap_err();
        assert!(
            problems
                .iter()
                .any(|p| p.contains("GitHub token") && p.contains("spec.env.GH_TOKEN")),
            "the secret guard must run on a flat sandbox too: {problems:?}"
        );
    }
}
