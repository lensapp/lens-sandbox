use crate::artifact::resolve::{FetchError, FetchedComponent};
use crate::artifact::spec::{self, Kind};
use crate::image::Registry;
use anyhow::{Context, Result};
use oci_client::Reference;

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
            fetched.resources = sandbox.spec.resources;
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
        Kind::Policy => {
            spec::validate_envelope(config_json, kind)?;
            fetched.policy = parse_network_policy(config_json)?;
        }
        Kind::AgentSystem | Kind::Integration => {
            spec::validate_envelope(config_json, kind)?;
        }
    }
    Ok(fetched)
}

/// Read a Policy component's `spec.network` into an `lns_policy::Policy`; integration refs are resolved separately, so this carries only the network rules. A missing `network` section is `Ok(None)`, but a present-but-malformed one is an error so a bundle's rules are never silently dropped (fail-closed).
fn parse_network_policy(config_json: &[u8]) -> Result<Option<lns_policy::Policy>> {
    let doc: serde_json::Value =
        serde_json::from_slice(config_json).context("parsing policy config")?;
    let Some(network) = doc.get("spec").and_then(|spec| spec.get("network")) else {
        return Ok(None);
    };
    let network: lns_policy::NetworkPolicy = serde_json::from_value(network.clone())
        .context("policy component has a malformed network section")?;
    Ok(Some(lns_policy::Policy {
        network,
        integrations: Vec::new(),
    }))
}

fn classify_fetch_error(host: &str, err: &anyhow::Error) -> FetchError {
    let msg = format!("{err:#}").to_lowercase();
    if msg.contains("not found") || msg.contains("404") || msg.contains("manifest unknown") {
        FetchError::NotFound
    } else if msg.contains("401")
        || msg.contains("403")
        || msg.contains("unauthorized")
        || msg.contains("denied")
    {
        FetchError::NeedsLogin {
            host: host.to_string(),
        }
    } else {
        FetchError::Invalid {
            reason: format!("{err:#}"),
        }
    }
}

// arch (base-image platform) is intentionally left unset here; the base image's arch is re-checked when it is pulled as the rootfs in `ingest::run`.
pub(crate) async fn fetch_component<R: Registry>(
    registry: &R,
    reference: &str,
) -> std::result::Result<FetchedComponent, FetchError> {
    let parsed: Reference = reference.parse().map_err(|e| FetchError::Invalid {
        reason: format!("invalid reference {reference}: {e}"),
    })?;
    let (manifest, _digest, config_json) = match registry.pull_manifest_and_config(&parsed).await {
        Ok(triple) => triple,
        Err(e) => return Err(classify_fetch_error(parsed.registry(), &e)),
    };
    let artifact_type = manifest.artifact_type.as_deref();
    let config_media_type = manifest.config.media_type.as_str();
    if Kind::from_artifact_type(artifact_type.unwrap_or_default())
        .or_else(|| Kind::from_config_media_type(config_media_type))
        .is_none()
    {
        return Err(FetchError::UnsupportedKind {
            media_type: artifact_type.unwrap_or(config_media_type).to_string(),
        });
    }
    let mut fetched = to_fetched(
        artifact_type,
        Some(config_media_type),
        config_json.as_bytes(),
    )
    .map_err(|e| FetchError::Invalid {
        reason: format!("{e:#}"),
    })?;
    if fetched.kind == Kind::Policy.as_str() {
        let policy =
            spec::parse_policy(config_json.as_bytes()).map_err(|e| FetchError::Invalid {
                reason: format!("{e:#}"),
            })?;
        fetched.references = policy
            .spec
            .integrations
            .into_iter()
            .map(|r| r.reference)
            .collect();
    }
    Ok(fetched)
}

#[cfg(test)]
mod tests {
    use super::*;
    use oci_client::manifest::{OciDescriptor, OciImageManifest};

    struct FakeReg {
        artifact_type: Option<String>,
        config_media_type: String,
        config_json: String,
        pull_error: Option<String>,
    }

    impl FakeReg {
        fn lens(kind: Kind, config_json: &str) -> Self {
            Self {
                artifact_type: Some(kind.artifact_type()),
                config_media_type: kind.config_media_type(),
                config_json: config_json.to_string(),
                pull_error: None,
            }
        }
    }

    impl Registry for FakeReg {
        async fn pull_manifest_and_config(
            &self,
            _reference: &Reference,
        ) -> Result<(OciImageManifest, String, String)> {
            if let Some(e) = &self.pull_error {
                anyhow::bail!("{e}");
            }
            let manifest = OciImageManifest {
                config: OciDescriptor {
                    media_type: self.config_media_type.clone(),
                    ..Default::default()
                },
                artifact_type: self.artifact_type.clone(),
                ..Default::default()
            };
            Ok((manifest, "sha256:whatever".into(), self.config_json.clone()))
        }

        async fn pull_blob(
            &self,
            _reference: &Reference,
            _descriptor: &OciDescriptor,
            _on_chunk: &(dyn Fn(u64) + Send + Sync),
        ) -> Result<Vec<u8>> {
            anyhow::bail!("a component fetch only reads the manifest+config, never layer blobs")
        }
    }

    async fn fetch(reg: &FakeReg) -> std::result::Result<FetchedComponent, FetchError> {
        fetch_component(reg, "reg.example.test/some/comp:1").await
    }

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
    fn maps_a_policy_config_to_its_network_rules() {
        let json = br#"{"apiVersion":"lens.dev/v1alpha1","kind":"Policy","metadata":{"name":"some-policy"},"spec":{"network":{"allowedRoutes":[{"match":"api.example.test","verdict":"deny","transport":"direct"}],"defaultVerdict":"ask"}}}"#;
        let fetched = to_fetched(Some(&Kind::Policy.artifact_type()), None, json).unwrap();
        assert_eq!(fetched.kind, "Policy");
        let policy = fetched
            .policy
            .expect("a Policy component carries its network rules");
        assert_eq!(policy.network.allowed_routes.len(), 1);
        assert_eq!(
            policy.network.allowed_routes[0].match_pattern,
            "api.example.test"
        );
        assert_eq!(
            policy.network.allowed_routes[0].verdict,
            lns_policy::Verdict::Deny
        );
    }

    #[test]
    fn a_policy_config_without_a_network_section_carries_no_rules() {
        let json = br#"{"apiVersion":"lens.dev/v1alpha1","kind":"Policy","metadata":{"name":"some-policy"},"spec":{}}"#;
        let fetched = to_fetched(Some(&Kind::Policy.artifact_type()), None, json).unwrap();
        assert!(fetched.policy.is_none());
    }

    #[test]
    fn a_policy_with_a_malformed_network_section_is_refused_not_silently_dropped() {
        let json = br#"{"apiVersion":"lens.dev/v1alpha1","kind":"Policy","metadata":{"name":"some-policy"},"spec":{"network":{"defaultVerdict":"maybe"}}}"#;
        let err = to_fetched(Some(&Kind::Policy.artifact_type()), None, json).unwrap_err();
        assert!(
            format!("{err:#}").contains("malformed network section"),
            "a bundle's policy must fail closed, not vanish: {err:#}"
        );
    }

    #[test]
    fn parse_network_policy_distinguishes_absent_from_malformed_from_unparseable() {
        assert!(parse_network_policy(b"not json").is_err());
        assert!(
            parse_network_policy(br#"{"spec":{"network":{"defaultVerdict":"nope"}}}"#).is_err()
        );
        assert!(parse_network_policy(br#"{"spec":{}}"#).unwrap().is_none());
        assert!(
            parse_network_policy(br#"{"spec":{"network":{"defaultVerdict":"deny"}}}"#)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn maps_an_integration_config_by_validating_its_envelope_only() {
        let json = br#"{"apiVersion":"lens.dev/v1alpha1","kind":"Integration","metadata":{"name":"some-integration"},"spec":{}}"#;
        let fetched = to_fetched(Some(&Kind::Integration.artifact_type()), None, json).unwrap();
        assert_eq!(fetched.kind, "Integration");
        assert!(fetched.policy.is_none());
        assert!(fetched.base_image.is_none());
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

    #[tokio::test]
    async fn fetch_component_maps_a_pulled_sandbox_to_its_base_image() {
        let reg = FakeReg::lens(Kind::Sandbox, std::str::from_utf8(&sandbox_json()).unwrap());
        let fetched = fetch(&reg).await.unwrap();
        assert_eq!(fetched.kind, "Sandbox");
        assert!(fetched.base_image.unwrap().contains("@sha256:"));
        assert!(
            fetched.arch.is_none(),
            "arch is left to the ingest arch check"
        );
    }

    #[tokio::test]
    async fn fetch_component_populates_policy_integration_references() {
        let policy = r#"{"apiVersion":"lens.dev/v1alpha1","kind":"Policy","metadata":{"name":"some-policy"},"spec":{"integrations":[{"ref":"reg/some-integration:1"},{"ref":"reg/other:1"}]}}"#;
        let reg = FakeReg::lens(Kind::Policy, policy);
        let fetched = fetch(&reg).await.unwrap();
        assert_eq!(fetched.kind, "Policy");
        assert_eq!(
            fetched.references,
            vec![
                "reg/some-integration:1".to_string(),
                "reg/other:1".to_string()
            ],
            "resolve must be able to walk a policy's integration refs",
        );
    }

    #[tokio::test]
    async fn fetch_component_refuses_an_unrecognized_artifact_type() {
        let reg = FakeReg {
            artifact_type: Some("application/vnd.oci.image.config.v1+json".into()),
            config_media_type: "application/vnd.oci.image.config.v1+json".into(),
            config_json: "{}".into(),
            pull_error: None,
        };
        match fetch(&reg).await.unwrap_err() {
            FetchError::UnsupportedKind { media_type } => {
                assert_eq!(media_type, "application/vnd.oci.image.config.v1+json")
            }
            other => panic!("expected UnsupportedKind, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn fetch_component_maps_a_policy_with_a_bad_integration_ref_to_invalid() {
        let policy = r#"{"apiVersion":"lens.dev/v1alpha1","kind":"Policy","metadata":{"name":"some-policy"},"spec":{"integrations":[{"ref":"reg/x:1","digest":"sha256:bad"}]}}"#;
        let reg = FakeReg::lens(Kind::Policy, policy);
        match fetch(&reg).await.unwrap_err() {
            FetchError::Invalid { reason } => assert!(reason.contains("not a sha256 digest")),
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn the_component_registry_fake_never_serves_layer_blobs() {
        let reg = FakeReg::lens(Kind::Sandbox, "{}");
        let reference: Reference = "reg.example.test/x:1".parse().unwrap();
        let err = reg
            .pull_blob(&reference, &OciDescriptor::default(), &|_| {})
            .await;
        assert!(err.is_err(), "component fetch must not pull blobs");
    }

    #[tokio::test]
    async fn fetch_component_maps_a_malformed_config_to_invalid() {
        let floating = r#"{"apiVersion":"lens.dev/v1alpha1","kind":"Sandbox","metadata":{"name":"some-sandbox"},"spec":{"isolation":"microvm","baseImage":"reg/base:1"}}"#;
        let reg = FakeReg::lens(Kind::Sandbox, floating);
        match fetch(&reg).await.unwrap_err() {
            FetchError::Invalid { reason } => assert!(reason.contains("digest-pinned")),
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn fetch_component_classifies_pull_failures() {
        let not_found = FakeReg {
            artifact_type: None,
            config_media_type: String::new(),
            config_json: String::new(),
            pull_error: Some("manifest unknown: 404".into()),
        };
        assert!(matches!(fetch(&not_found).await, Err(FetchError::NotFound)));

        let unauthorized = FakeReg {
            artifact_type: None,
            config_media_type: String::new(),
            config_json: String::new(),
            pull_error: Some("401 Unauthorized".into()),
        };
        match fetch(&unauthorized).await.unwrap_err() {
            FetchError::NeedsLogin { host } => assert_eq!(host, "reg.example.test"),
            other => panic!("expected NeedsLogin, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn fetch_component_rejects_an_unparseable_reference() {
        let reg = FakeReg::lens(Kind::Sandbox, "{}");
        match fetch_component(&reg, "::not a ref::").await.unwrap_err() {
            FetchError::Invalid { reason } => assert!(reason.contains("invalid reference")),
            other => panic!("expected Invalid, got {other:?}"),
        }
    }

    #[test]
    fn classify_fetch_error_maps_transport_failures_to_kinds() {
        assert!(matches!(
            classify_fetch_error("h", &anyhow::anyhow!("500 internal")),
            FetchError::Invalid { .. }
        ));
        assert!(matches!(
            classify_fetch_error("h", &anyhow::anyhow!("HTTP 404 not found")),
            FetchError::NotFound
        ));
        assert!(matches!(
            classify_fetch_error("h", &anyhow::anyhow!("access denied")),
            FetchError::NeedsLogin { .. }
        ));
    }
}
