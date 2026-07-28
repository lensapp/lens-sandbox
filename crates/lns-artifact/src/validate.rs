use crate::spec;

/// Validate a single artifact document: schema + cross-field guards for its declared kind, plus the guards that belong to authoring only — whether this build can provision a declared tool is deliberately not part of parsing, since a consumer must still be able to inspect an artifact whose tool the shipped registry has since dropped.
pub fn validate(doc: &[u8]) -> Result<(), Vec<String>> {
    let is_sandbox = crate::sandbox::is_sandbox_definition(doc);
    let schema = if is_sandbox {
        crate::sandbox::validate(doc)
    } else {
        spec::validate_any(doc)
    };
    schema.map_err(|e| vec![format!("schema: {e:#}")])?;
    if is_sandbox {
        refuse_unprovisionable_tools(doc).map_err(|problem| vec![problem])?;
    }
    Ok(())
}

/// The authoring-time half of the tool gate: an author is refused here rather than by a consumer's failed launch. Anything about the document's shape is the schema check's to report, so this reads the declared entries and judges only whether each names a tool this build can install.
pub fn refuse_unprovisionable_tools(doc: &[u8]) -> Result<(), String> {
    let Some(entries) = serde_json::from_slice::<serde_json::Value>(doc)
        .ok()
        .and_then(|doc| doc["spec"]["tools"].as_array().cloned())
    else {
        return Ok(());
    };
    for entry in entries {
        let Some(declared) = entry.as_str() else {
            continue;
        };
        let Ok(tool) = crate::tools::parse(declared) else {
            continue;
        };
        crate::tools::registry::refuse_unprovisionable(std::slice::from_ref(&tool))
            .map_err(|e| format!("tools: {e}"))?;
    }
    Ok(())
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
    fn a_tool_this_build_cannot_provision_is_an_authoring_problem() {
        let doc = br#"{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{"name":"hermes"},"spec":{"image":"ghcr.io/team/base:1","tools":["definitely-not-a-tool@1"]}}"#;
        let problems = validate(doc).unwrap_err();
        assert!(
            problems
                .iter()
                .any(|p| p.contains("not a tool lns can provision")),
            "got: {problems:?}"
        );
        let plugin_backed = br#"{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{"name":"hermes"},"spec":{"image":"ghcr.io/team/base:1","tools":["prettier@3"]}}"#;
        assert!(
            validate(plugin_backed)
                .unwrap_err()
                .iter()
                .any(|p| p.contains("bring it via spec.image")),
            "a plugin-backed tool is refused at authoring time too"
        );
    }

    #[test]
    fn the_tool_gate_leaves_every_other_shape_problem_to_the_schema_check() {
        for doc in [
            &b"not json at all"[..],
            br#"{"spec":{}}"#,
            br#"{"spec":{"tools":[42]}}"#,
            br#"{"spec":{"tools":["no-version"]}}"#,
        ] {
            let label = String::from_utf8_lossy(doc);
            assert_eq!(
                refuse_unprovisionable_tools(doc),
                Ok(()),
                "the gate judges provisionability only: {label}"
            );
        }
    }

    #[test]
    fn parsing_an_artifact_declaring_such_a_tool_still_succeeds() {
        // The shipped registry shrinks over time; a consumer must still be able to see what a published sandbox declares.
        let doc = br#"{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{"name":"hermes"},"spec":{"image":"ghcr.io/team/base:1","tools":["definitely-not-a-tool@1"]}}"#;
        let def = crate::sandbox::parse(doc).expect("the definition still parses and inspects");
        assert_eq!(def.spec.tools, vec!["definitely-not-a-tool@1".to_string()]);
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
