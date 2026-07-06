use crate::artifact::resolve::FetchedComponent;
use crate::artifact::spec::{self, Kind};
use anyhow::{Context, Result};

// `arch` (base-image platform) and `references` (Policy integrations) are populated by the Layer-1 registry adapter, not here.
pub fn to_fetched(
    artifact_type: Option<&str>,
    config_media_type: Option<&str>,
    config_json: &[u8],
) -> Result<FetchedComponent> {
    let kind = artifact_type
        .and_then(Kind::from_artifact_type)
        .or_else(|| config_media_type.and_then(Kind::from_config_media_type))
        .context("component is not a recognized lens artifact")?;
    let mut fetched = FetchedComponent {
        kind: kind.as_str().to_string(),
        ..Default::default()
    };
    match kind {
        Kind::Sandbox => {
            let sandbox = spec::parse_sandbox(config_json)?;
            fetched.name = sandbox.metadata.name;
            fetched.base_image = sandbox.spec.base_image;
        }
        Kind::Agent => {
            let agent = spec::parse_agent(config_json)?;
            fetched.name = agent.metadata.name;
            fetched.command = agent.spec.command;
            fetched.env = agent.spec.env;
        }
        Kind::FileSet => {
            let fileset = spec::parse_fileset(config_json)?;
            fetched.name = fileset.metadata.name;
            fetched.mount_path = Some(fileset.mount.path);
        }
        Kind::AgentSystem | Kind::Policy | Kind::Integration => {
            spec::validate_envelope(config_json, kind)?;
        }
    }
    Ok(fetched)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sandbox_json() -> Vec<u8> {
        format!(
            r#"{{"apiVersion":"lens.dev/v1alpha1","kind":"Sandbox","metadata":{{"name":"some-sandbox"}},"spec":{{"isolation":"microvm","baseImage":"reg/base@sha256:{}"}}}}"#,
            "a".repeat(64)
        )
        .into_bytes()
    }

    fn agent_json() -> &'static [u8] {
        br#"{"apiVersion":"lens.dev/v1alpha1","kind":"Agent","metadata":{"name":"some-agent"},"spec":{"command":"agent --serve","env":{"MODE":"research"}}}"#
    }

    fn policy_json() -> &'static [u8] {
        br#"{"apiVersion":"lens.dev/v1alpha1","kind":"Policy","metadata":{"name":"some-policy"},"spec":{"network":{"defaultVerdict":"ask"}}}"#
    }

    #[test]
    fn maps_a_sandbox_config_to_its_base_image() {
        let fetched =
            to_fetched(Some(&Kind::Sandbox.artifact_type()), None, &sandbox_json()).unwrap();
        assert_eq!(fetched.kind, "Sandbox");
        assert_eq!(fetched.name, "some-sandbox");
        assert!(
            fetched
                .base_image
                .unwrap()
                .ends_with(&format!("@sha256:{}", "a".repeat(64)))
        );
    }

    #[test]
    fn maps_an_agent_config_to_its_invocation() {
        let fetched = to_fetched(Some(&Kind::Agent.artifact_type()), None, agent_json()).unwrap();
        assert_eq!(fetched.kind, "Agent");
        assert_eq!(fetched.name, "some-agent");
        assert_eq!(fetched.command.as_deref(), Some("agent --serve"));
        assert_eq!(
            fetched.env.get("MODE").map(String::as_str),
            Some("research")
        );
    }

    #[test]
    fn maps_a_fileset_config_to_its_mount_path() {
        let json = br#"{"apiVersion":"lens.dev/v1alpha1","kind":"FileSet","metadata":{"name":"skills"},"mount":{"path":"/root/.some-agent/skills"},"spec":{}}"#;
        let fetched = to_fetched(Some(&Kind::FileSet.artifact_type()), None, json).unwrap();
        assert_eq!(fetched.kind, "FileSet");
        assert_eq!(fetched.name, "skills");
        assert_eq!(
            fetched.mount_path.as_deref(),
            Some("/root/.some-agent/skills")
        );
    }

    #[test]
    fn a_runtime_only_kind_validates_its_envelope_but_surfaces_no_payload() {
        let fetched = to_fetched(Some(&Kind::Policy.artifact_type()), None, policy_json()).unwrap();
        assert_eq!(fetched.kind, "Policy");
        assert_eq!(fetched.name, "");
        assert!(fetched.base_image.is_none());
        assert!(fetched.command.is_none());
        assert!(fetched.env.is_empty());
        assert!(fetched.mount_path.is_none());
    }

    #[test]
    fn a_runtime_only_kind_with_a_mislabeled_body_is_rejected() {
        let err = to_fetched(Some(&Kind::Policy.artifact_type()), None, agent_json()).unwrap_err();
        assert!(format!("{err:#}").contains("expected kind"), "got: {err:#}");
    }

    #[test]
    fn artifact_type_wins_over_a_disagreeing_config_media_type() {
        let fetched = to_fetched(
            Some(&Kind::Agent.artifact_type()),
            Some(&Kind::Sandbox.config_media_type()),
            agent_json(),
        )
        .unwrap();
        assert_eq!(fetched.kind, "Agent");
        assert_eq!(fetched.command.as_deref(), Some("agent --serve"));
    }

    #[test]
    fn falls_back_to_the_config_media_type_when_artifact_type_is_absent() {
        let fetched = to_fetched(
            None,
            Some(&Kind::Sandbox.config_media_type()),
            &sandbox_json(),
        )
        .unwrap();
        assert_eq!(fetched.kind, "Sandbox");
    }

    #[test]
    fn arch_and_references_are_not_populated_by_the_mapping() {
        let fetched =
            to_fetched(Some(&Kind::Sandbox.artifact_type()), None, &sandbox_json()).unwrap();
        assert!(fetched.arch.is_none());
        assert!(fetched.references.is_empty());
    }

    #[test]
    fn rejects_a_component_that_is_not_a_lens_artifact() {
        let err = to_fetched(
            Some("application/vnd.oci.image.config.v1+json"),
            None,
            b"{}",
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("not a recognized lens artifact"));
    }

    #[test]
    fn propagates_a_sandbox_parse_failure() {
        let floating = br#"{"apiVersion":"lens.dev/v1alpha1","kind":"Sandbox","metadata":{"name":"some-sandbox"},"spec":{"isolation":"microvm","baseImage":"reg/base:1"}}"#;
        let err = to_fetched(Some(&Kind::Sandbox.artifact_type()), None, floating).unwrap_err();
        assert!(format!("{err:#}").contains("digest-pinned"));
    }

    #[test]
    fn propagates_the_agent_image_rejection() {
        let with_image = br#"{"apiVersion":"lens.dev/v1alpha1","kind":"Agent","metadata":{"name":"some-agent"},"spec":{"image":"reg/x:1"}}"#;
        let err = to_fetched(Some(&Kind::Agent.artifact_type()), None, with_image).unwrap_err();
        assert!(
            format!("{err:#}").contains("must not carry `image`"),
            "got: {err:#}"
        );
    }

    #[test]
    fn rejects_a_media_type_whose_envelope_kind_disagrees() {
        let err = to_fetched(Some(&Kind::Sandbox.artifact_type()), None, agent_json()).unwrap_err();
        assert!(format!("{err:#}").contains("expected kind"), "got: {err:#}");
    }
}
