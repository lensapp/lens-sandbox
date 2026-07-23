pub mod real;

use anyhow::{Context, Result, bail};
use lns_ipc::Response;
use lns_policy::connectors::Connector;
use lns_policy::credentials::{CredentialStateFile, has_armed_entry};
use lns_policy::pulled::{PulledCatalog, PulledConnector};
use oci_client::Reference;
use oci_client::manifest::OciImageManifest;

use crate::image::verify_digest_pin;

/// The single registry operation a connector pull needs — fetch the manifest and config blob — narrowed out of the full `Registry` so a test fakes exactly this and no unused blob stubs dilute the coverage floor; the blanket impl over every real `Registry` lives in `real.rs`.
pub(crate) trait ManifestSource {
    fn pull_manifest_and_config(
        &self,
        reference: &Reference,
    ) -> impl std::future::Future<Output = Result<(OciImageManifest, String, String)>> + Send;
}

/// The result of a connector pull: either it landed (possibly replacing a prior entry), or it would replace a connector the user has already consented to and needs explicit confirmation first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PullConnectorOutcome {
    Pulled {
        id: String,
        config_digest: String,
        replaced: bool,
    },
    NeedsConfirm {
        id: String,
        changes: Vec<String>,
    },
}

/// Pull a connector artifact and fold it into `pulled`, gating a replacement: if this would overwrite a connector the user has already armed a credential for, and its config-blob digest changed, refuse with a disclosed diff unless `confirm_replace` — the one explicit mutation channel stays honest, so a redefined connector can't silently widen where a consented credential flows.
pub(crate) async fn pull_connector_with<M: ManifestSource>(
    client: &M,
    reference: &str,
    now: &str,
    pulled: &mut PulledCatalog,
    credentials: &CredentialStateFile,
    confirm_replace: bool,
) -> Result<PullConnectorOutcome> {
    let parsed: Reference = reference
        .parse()
        .with_context(|| format!("invalid connector reference: {reference}"))?;
    let (manifest, manifest_digest, config_str) = client.pull_manifest_and_config(&parsed).await?;
    let kind = manifest
        .artifact_type
        .as_deref()
        .and_then(lns_artifact::spec::Kind::from_artifact_type)
        .or_else(|| lns_artifact::spec::Kind::from_config_media_type(&manifest.config.media_type));
    if kind != Some(lns_artifact::spec::Kind::Connector) {
        bail!("{reference} is not a connector artifact");
    }
    verify_digest_pin(&parsed, &manifest_digest, reference)?;
    let definition = lns_artifact::spec::parse_connector(config_str.as_bytes())
        .with_context(|| format!("parsing pulled connector {reference}"))?;
    let config_digest = manifest.config.digest.clone();
    let id = definition.id.clone();

    let existing = pulled
        .connectors
        .iter()
        .find(|c| c.definition.id == id)
        .map(|c| (c.config_digest.clone(), c.definition.clone()));
    let replaced = existing.is_some();
    if let Some((old_digest, old_definition)) = existing
        && old_digest != config_digest
        && has_armed_entry(credentials, &id)
        && !confirm_replace
    {
        return Ok(PullConnectorOutcome::NeedsConfirm {
            id,
            changes: connector_changes(&old_definition, &definition),
        });
    }

    pulled.upsert(PulledConnector {
        source: reference.to_string(),
        manifest_digest,
        config_digest: config_digest.clone(),
        pulled_at: now.to_string(),
        definition,
    });
    Ok(PullConnectorOutcome::Pulled {
        id,
        config_digest,
        replaced,
    })
}

/// Map a remove result to the wire response: removed → `ConnectorRemoved`, absent → an error naming the id.
pub fn connector_remove_response(result: Result<bool>, id: &str) -> Response {
    match result {
        Ok(true) => Response::ConnectorRemoved { id: id.to_string() },
        Ok(false) => Response::Error {
            message: format!("no pulled connector {id:?} to remove"),
        },
        Err(e) => Response::Error {
            message: format!("{e:#}"),
        },
    }
}

/// Map a pull result to the wire response the CLI renders.
pub fn connector_pull_response(result: Result<PullConnectorOutcome>) -> Response {
    match result {
        Ok(PullConnectorOutcome::Pulled {
            id,
            config_digest,
            replaced,
        }) => Response::ConnectorPulled {
            id,
            config_digest,
            replaced,
        },
        Ok(PullConnectorOutcome::NeedsConfirm { id, changes }) => {
            Response::ConnectorReplaceNeedsConfirm { id, changes }
        }
        Err(e) => Response::Error {
            message: format!("{e:#}"),
        },
    }
}

fn env_var(connector: &Connector) -> Option<&str> {
    connector
        .credential
        .as_ref()
        .map(|c| c.env_var.as_str())
        .or_else(|| connector.oauth.as_ref().map(|o| o.env_var.as_str()))
}

fn injection_domains(connector: &Connector) -> Vec<String> {
    let mut domains: Vec<String> = connector
        .credential
        .iter()
        .flat_map(|c| &c.injections)
        .chain(connector.oauth.iter().flat_map(|o| &o.injections))
        .map(|i| i.domain.clone())
        .collect();
    domains.sort();
    domains.dedup();
    domains
}

fn oauth_endpoints(connector: &Connector) -> Vec<String> {
    let mut endpoints = Vec::new();
    if let Some(oauth) = &connector.oauth {
        endpoints.push(format!("token={}", oauth.token_endpoint));
        for (label, value) in [
            ("userinfo", &oauth.userinfo_endpoint),
            ("device", &oauth.device_authorization_endpoint),
            ("authorization", &oauth.authorization_endpoint),
        ] {
            if let Some(url) = value {
                endpoints.push(format!("{label}={url}"));
            }
        }
    }
    endpoints
}

fn route_patterns(connector: &Connector) -> Vec<String> {
    let mut patterns: Vec<String> = connector
        .routes
        .iter()
        .map(|r| r.match_pattern.clone())
        .collect();
    patterns.sort();
    patterns
}

/// The security-relevant fields whose change the user must re-approve: which env var carries the token, where it is injected, the oauth endpoints it flows to, and the routes it opens.
fn connector_changes(old: &Connector, new: &Connector) -> Vec<String> {
    let mut changes = Vec::new();
    if env_var(old) != env_var(new) {
        changes.push(format!("env var {:?} → {:?}", env_var(old), env_var(new)));
    }
    if injection_domains(old) != injection_domains(new) {
        changes.push(format!(
            "injection domains {:?} → {:?}",
            injection_domains(old),
            injection_domains(new)
        ));
    }
    if oauth_endpoints(old) != oauth_endpoints(new) {
        changes.push(format!(
            "oauth endpoints {:?} → {:?}",
            oauth_endpoints(old),
            oauth_endpoints(new)
        ));
    }
    if route_patterns(old) != route_patterns(new) {
        changes.push(format!(
            "routes {:?} → {:?}",
            route_patterns(old),
            route_patterns(new)
        ));
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;
    use lns_policy::credentials::CredentialEntry;
    use oci_client::manifest::OciDescriptor;

    struct FakeRegistry {
        artifact_type: Option<String>,
        config_media_type: String,
        config_digest: String,
        manifest_digest: String,
        config_json: String,
    }

    impl FakeRegistry {
        fn connector(config_json: &str, config_digest: &str) -> Self {
            Self {
                artifact_type: Some(lns_artifact::spec::Kind::Connector.artifact_type()),
                config_media_type: lns_artifact::spec::Kind::Connector.config_media_type(),
                config_digest: config_digest.into(),
                manifest_digest: format!("sha256:{}", "e".repeat(64)),
                config_json: config_json.into(),
            }
        }
    }

    impl ManifestSource for FakeRegistry {
        async fn pull_manifest_and_config(
            &self,
            _reference: &Reference,
        ) -> Result<(OciImageManifest, String, String)> {
            let manifest = OciImageManifest {
                schema_version: 2,
                media_type: None,
                artifact_type: self.artifact_type.clone(),
                config: OciDescriptor {
                    media_type: self.config_media_type.clone(),
                    digest: self.config_digest.clone(),
                    size: self.config_json.len() as i64,
                    ..Default::default()
                },
                layers: Vec::new(),
                annotations: None,
                subject: None,
            };
            Ok((
                manifest,
                self.manifest_digest.clone(),
                self.config_json.clone(),
            ))
        }
    }

    fn credential_envelope(env_var: &str, domain: &str, route: &str) -> String {
        format!(
            r#"{{"apiVersion":"lens.dev/v1alpha1","kind":"Connector","metadata":{{"name":"some-provider"}},"spec":{{"authKind":"credential","routes":[{{"match":"{route}"}}],"credential":{{"envVar":"{env_var}","placeholder":"lns-placeholder","injections":[{{"kind":"bearer_header","domain":"{domain}"}}]}}}}}}"#
        )
    }

    fn armed_state(id: &str) -> CredentialStateFile {
        let mut state = CredentialStateFile::new();
        state.insert(
            id.to_string(),
            CredentialEntry::Stored {
                value: "a-real-token".into(),
            },
        );
        state
    }

    async fn pull(
        registry: &FakeRegistry,
        reference: &str,
        pulled: &mut PulledCatalog,
        credentials: &CredentialStateFile,
        confirm_replace: bool,
    ) -> Result<PullConnectorOutcome> {
        pull_connector_with(
            registry,
            reference,
            "2026-07-23T10:00:00Z",
            pulled,
            credentials,
            confirm_replace,
        )
        .await
    }

    #[tokio::test]
    async fn a_fresh_pull_records_the_connector_with_its_provenance() {
        let registry = FakeRegistry::connector(
            &credential_envelope(
                "SOME_TOKEN",
                "api.some-provider.example",
                "api.some-provider.example",
            ),
            "sha256:cfg1",
        );
        let mut pulled = PulledCatalog::default();
        let outcome = pull(
            &registry,
            "registry.lns.run/connectors/some-provider:0.1.0",
            &mut pulled,
            &CredentialStateFile::new(),
            false,
        )
        .await
        .unwrap();
        assert_eq!(
            outcome,
            PullConnectorOutcome::Pulled {
                id: "some-provider".into(),
                config_digest: "sha256:cfg1".into(),
                replaced: false,
            }
        );
        let entry = &pulled.connectors[0];
        assert_eq!(
            entry.source,
            "registry.lns.run/connectors/some-provider:0.1.0"
        );
        assert_eq!(entry.config_digest, "sha256:cfg1");
        assert_eq!(entry.pulled_at, "2026-07-23T10:00:00Z");
        assert_eq!(entry.definition.id, "some-provider");
    }

    #[tokio::test]
    async fn re_pulling_an_unconsented_connector_replaces_it_without_confirmation() {
        let mut pulled = PulledCatalog::default();
        pull(
            &FakeRegistry::connector(
                &credential_envelope(
                    "SOME_TOKEN",
                    "api.some-provider.example",
                    "api.some-provider.example",
                ),
                "sha256:cfg1",
            ),
            "registry.lns.run/connectors/some-provider:0.1.0",
            &mut pulled,
            &CredentialStateFile::new(),
            false,
        )
        .await
        .unwrap();
        let outcome = pull(
            &FakeRegistry::connector(
                &credential_envelope("OTHER_TOKEN", "api.evil.example", "api.evil.example"),
                "sha256:cfg2",
            ),
            "registry.lns.run/connectors/some-provider:0.2.0",
            &mut pulled,
            &CredentialStateFile::new(),
            false,
        )
        .await
        .unwrap();
        assert!(
            matches!(outcome, PullConnectorOutcome::Pulled { replaced: true, .. }),
            "an unconsented connector re-pulls freely: {outcome:?}"
        );
        assert_eq!(pulled.connectors.len(), 1);
        assert_eq!(pulled.digest_for("some-provider"), Some("sha256:cfg2"));
    }

    #[tokio::test]
    async fn re_pulling_a_consented_connector_with_the_same_digest_is_not_gated() {
        let mut pulled = PulledCatalog::default();
        let envelope = credential_envelope(
            "SOME_TOKEN",
            "api.some-provider.example",
            "api.some-provider.example",
        );
        pull(
            &FakeRegistry::connector(&envelope, "sha256:cfg1"),
            "r/some-provider:0.1.0",
            &mut pulled,
            &CredentialStateFile::new(),
            false,
        )
        .await
        .unwrap();
        let outcome = pull(
            &FakeRegistry::connector(&envelope, "sha256:cfg1"),
            "r/some-provider:0.1.0",
            &mut pulled,
            &armed_state("some-provider"),
            false,
        )
        .await
        .unwrap();
        assert!(
            matches!(outcome, PullConnectorOutcome::Pulled { replaced: true, .. }),
            "an unchanged definition never gates, even when consented: {outcome:?}"
        );
    }

    #[tokio::test]
    async fn replacing_a_consented_connector_whose_definition_changed_needs_confirmation() {
        let mut pulled = PulledCatalog::default();
        pull(
            &FakeRegistry::connector(
                &credential_envelope(
                    "SOME_TOKEN",
                    "api.some-provider.example",
                    "api.some-provider.example",
                ),
                "sha256:cfg1",
            ),
            "r/some-provider:0.1.0",
            &mut pulled,
            &CredentialStateFile::new(),
            false,
        )
        .await
        .unwrap();
        let outcome = pull(
            &FakeRegistry::connector(
                &credential_envelope("SOME_TOKEN", "api.attacker.example", "api.attacker.example"),
                "sha256:cfg2",
            ),
            "r/some-provider:0.2.0",
            &mut pulled,
            &armed_state("some-provider"),
            false,
        )
        .await
        .unwrap();
        match outcome {
            PullConnectorOutcome::NeedsConfirm { id, changes } => {
                assert_eq!(id, "some-provider");
                assert!(
                    changes.iter().any(|c| c.contains("injection domains")),
                    "the disclosed diff must name the widened injection domain: {changes:?}"
                );
                assert!(changes.iter().any(|c| c.contains("routes")), "{changes:?}");
            }
            other => panic!("a changed consented connector must need confirmation, got {other:?}"),
        }
        assert_eq!(
            pulled.digest_for("some-provider"),
            Some("sha256:cfg1"),
            "a gated replacement must not have overwritten the stored entry"
        );
    }

    #[tokio::test]
    async fn a_confirmed_replacement_of_a_consented_connector_lands() {
        let mut pulled = PulledCatalog::default();
        pull(
            &FakeRegistry::connector(
                &credential_envelope(
                    "SOME_TOKEN",
                    "api.some-provider.example",
                    "api.some-provider.example",
                ),
                "sha256:cfg1",
            ),
            "r/some-provider:0.1.0",
            &mut pulled,
            &CredentialStateFile::new(),
            false,
        )
        .await
        .unwrap();
        let outcome = pull(
            &FakeRegistry::connector(
                &credential_envelope("SOME_TOKEN", "api.attacker.example", "api.attacker.example"),
                "sha256:cfg2",
            ),
            "r/some-provider:0.2.0",
            &mut pulled,
            &armed_state("some-provider"),
            true,
        )
        .await
        .unwrap();
        assert!(matches!(
            outcome,
            PullConnectorOutcome::Pulled { replaced: true, .. }
        ));
        assert_eq!(pulled.digest_for("some-provider"), Some("sha256:cfg2"));
    }

    #[tokio::test]
    async fn detects_a_connector_by_config_media_type_when_artifact_type_is_absent() {
        let mut registry = FakeRegistry::connector(
            &credential_envelope(
                "SOME_TOKEN",
                "api.some-provider.example",
                "api.some-provider.example",
            ),
            "sha256:cfg1",
        );
        registry.artifact_type = None;
        let outcome = pull(
            &registry,
            "r/some-provider:1",
            &mut PulledCatalog::default(),
            &CredentialStateFile::new(),
            false,
        )
        .await
        .unwrap();
        assert!(
            matches!(outcome, PullConnectorOutcome::Pulled { .. }),
            "with no artifactType, the config media type identifies the connector: {outcome:?}"
        );
    }

    #[tokio::test]
    async fn a_non_connector_artifact_is_refused() {
        let mut registry = FakeRegistry::connector("{}", "sha256:cfg1");
        registry.artifact_type = Some("application/vnd.lens.sandbox.v1+json".into());
        registry.config_media_type = "application/vnd.lens.sandbox.config.v1+json".into();
        let err = pull(
            &registry,
            "r/x:1",
            &mut PulledCatalog::default(),
            &CredentialStateFile::new(),
            false,
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("not a connector artifact"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn an_invalid_reference_surfaces_a_parse_error() {
        let err = pull(
            &FakeRegistry::connector("{}", "sha256:cfg1"),
            "###",
            &mut PulledCatalog::default(),
            &CredentialStateFile::new(),
            false,
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("invalid connector reference"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn a_digest_pinned_reference_that_mismatches_is_refused() {
        let reference = format!(
            "registry.lns.run/connectors/some-provider@sha256:{}",
            "f".repeat(64)
        );
        let err = pull(
            &FakeRegistry::connector(
                &credential_envelope(
                    "SOME_TOKEN",
                    "api.some-provider.example",
                    "api.some-provider.example",
                ),
                "sha256:cfg1",
            ),
            &reference,
            &mut PulledCatalog::default(),
            &CredentialStateFile::new(),
            false,
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("manifest digest mismatch"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn a_malformed_connector_config_is_refused() {
        let err = pull(
            &FakeRegistry::connector(r#"{"apiVersion":"lens.dev/v1alpha1","kind":"Connector","metadata":{"name":"some-provider"},"spec":{"authKind":"credential"}}"#, "sha256:cfg1"),
            "r/some-provider:1",
            &mut PulledCatalog::default(),
            &CredentialStateFile::new(),
            false,
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("no `credential:` block"),
            "got: {err:#}"
        );
    }

    #[test]
    fn connector_remove_response_maps_present_absent_and_error() {
        assert!(matches!(
            connector_remove_response(Ok(true), "some-provider"),
            Response::ConnectorRemoved { .. }
        ));
        let absent = connector_remove_response(Ok(false), "some-provider");
        assert!(
            matches!(&absent, Response::Error { message } if message.contains("no pulled connector")),
            "an absent id must be an error naming it, got {absent:?}"
        );
        assert!(matches!(
            connector_remove_response(Err(anyhow::anyhow!("boom")), "x"),
            Response::Error { .. }
        ));
    }

    #[test]
    fn connector_pull_response_maps_each_outcome() {
        let pulled = connector_pull_response(Ok(PullConnectorOutcome::Pulled {
            id: "some-provider".into(),
            config_digest: "sha256:cfg1".into(),
            replaced: true,
        }));
        assert!(matches!(
            pulled,
            Response::ConnectorPulled { replaced: true, .. }
        ));
        let needs = connector_pull_response(Ok(PullConnectorOutcome::NeedsConfirm {
            id: "some-provider".into(),
            changes: vec!["routes [..] → [..]".into()],
        }));
        assert!(matches!(
            needs,
            Response::ConnectorReplaceNeedsConfirm { .. }
        ));
        let err = connector_pull_response(Err(anyhow::anyhow!("boom")));
        assert!(matches!(err, Response::Error { .. }));
    }

    #[test]
    fn connector_changes_reports_oauth_endpoint_and_env_changes() {
        let old: Connector = serde_json::from_value(serde_json::json!({
            "id": "some-oauth", "authKind": "oauth",
            "oauth": {"clientId": "c", "deviceAuthorizationEndpoint": "https://api.some-oauth.example/device", "tokenEndpoint": "https://api.some-oauth.example/token", "userinfoEndpoint": "https://api.some-oauth.example/me", "envVar": "SOME_OAUTH_TOKEN", "placeholder": "lns"}
        })).unwrap();
        let new: Connector = serde_json::from_value(serde_json::json!({
            "id": "some-oauth", "authKind": "oauth",
            "oauth": {"clientId": "c", "deviceAuthorizationEndpoint": "https://api.some-oauth.example/device", "tokenEndpoint": "https://api.attacker.example/token", "userinfoEndpoint": "https://api.some-oauth.example/me", "envVar": "OTHER_TOKEN", "placeholder": "lns"}
        })).unwrap();
        let changes = connector_changes(&old, &new);
        assert!(changes.iter().any(|c| c.contains("env var")), "{changes:?}");
        assert!(
            changes.iter().any(|c| c.contains("oauth endpoints")),
            "{changes:?}"
        );
    }
}
