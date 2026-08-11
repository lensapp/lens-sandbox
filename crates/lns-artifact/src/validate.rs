/// Validate a sandbox definition: the schema and cross-field guards `run`, `inspect` and `push` all load it through, plus the guards that belong to authoring only — whether this build can provision a declared tool is deliberately not part of parsing, since a consumer must still be able to inspect an artifact whose tool the shipped registry has since dropped.
pub fn validate(doc: &[u8]) -> Result<(), Vec<String>> {
    crate::sandbox::validate(doc).map_err(|e| vec![format!("schema: {e:#}")])?;
    refuse_unprovisionable_tools(doc).map_err(|problem| vec![problem])?;
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

    fn retired_group_sandbox(base_image: &str) -> Vec<u8> {
        format!(
            r#"{{"apiVersion":"lens.dev/v1alpha1","kind":"Sandbox","metadata":{{"name":"some-sandbox"}},"spec":{{"isolation":"microvm","baseImage":"{base_image}"}}}}"#
        )
        .into_bytes()
    }

    fn unsupported_backend_tool() -> String {
        crate::tools::registry::backends()
            .find(|(_, backend)| !crate::tools::registry::is_supported_backend(backend))
            .map(|(name, _)| name.to_string())
            .expect("the snapshot carries at least one unsupported-backend entry")
    }

    #[test]
    fn a_document_from_the_retired_group_is_not_a_definition() {
        // `run -f`, `inspect -f` and `push` all load a definition through `sandbox::parse`, so validating one against the older group's schema would pass a file none of them can run.
        for doc in [
            retired_group_sandbox(&format!("reg/base@sha256:{}", "a".repeat(64))),
            retired_group_sandbox("reg/base:1"),
        ] {
            let problems = validate(&doc).unwrap_err();
            assert!(
                problems
                    .iter()
                    .any(|p| p.contains("apiVersion") && p.contains(crate::sandbox::API_VERSION)),
                "the answer must name the group a definition is read as: {problems:?}"
            );
        }
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
        let unsupported = unsupported_backend_tool();
        let plugin_backed = format!(
            r#"{{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{{"name":"hermes"}},"spec":{{"image":"ghcr.io/team/base:1","tools":["{unsupported}@1"]}}}}"#
        );
        assert!(
            validate(plugin_backed.as_bytes())
                .unwrap_err()
                .iter()
                .any(|p| p.contains("bring it via spec.image") && p.contains(&unsupported)),
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
        let doc = br#"{"apiVersion":"lns.run/v1","kind":"Sorcery","metadata":{"name":"x"},"spec":{"image":"reg/base:1"}}"#;
        let problems = validate(doc).unwrap_err();
        assert!(
            problems.iter().any(|p| p.contains("kind")),
            "got: {problems:?}"
        );
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
