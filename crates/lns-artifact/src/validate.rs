use crate::spec;

/// Validate a single artifact document: schema + cross-field guards for its declared kind.
pub fn validate(doc: &[u8]) -> Result<(), Vec<String>> {
    let schema = if crate::sandbox::is_sandbox_definition(doc) {
        crate::sandbox::validate(doc)
    } else {
        spec::validate_any(doc)
    };
    schema.map_err(|e| vec![format!("schema: {e:#}")])
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
    fn a_non_json_document_surfaces_the_schema_error() {
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
}
