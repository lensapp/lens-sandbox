use anyhow::{Context, Result};
use lns_policy::artifact::POLICY_ARTIFACT_TYPE;
use lns_policy::registry_auth::{
    JsonFileRegistryCredentialStore, RegistryAuthFile, RegistryCredentialStore,
    default_registry_auth_path,
};
use oci_client::Reference;
use oci_client::secrets::RegistryAuth;

use crate::image::RealRegistry;

pub(crate) trait PolicyArtifactRegistry: Send + Sync {
    fn push_policy_artifact(
        &self,
        reference: &Reference,
        config_blob: &[u8],
        auth: &RegistryAuth,
    ) -> impl std::future::Future<Output = Result<String>> + Send;

    fn pull_artifact(
        &self,
        reference: &Reference,
        auth: &RegistryAuth,
    ) -> impl std::future::Future<Output = Result<(Option<String>, Vec<u8>, String)>> + Send;
}

fn auth_for(registry: &str, file: &RegistryAuthFile) -> RegistryAuth {
    match file.get(registry) {
        Some(cred) => RegistryAuth::Basic(
            cred.username.clone().unwrap_or_else(|| "any".to_string()),
            cred.token.clone(),
        ),
        None => RegistryAuth::Anonymous,
    }
}

fn resolve(
    reference: &str,
    store: &dyn RegistryCredentialStore,
) -> Result<(Reference, RegistryAuth)> {
    let reference: Reference = reference
        .parse()
        .with_context(|| format!("invalid registry reference: {reference}"))?;
    let file = store.load().context("loading registry credentials")?;
    let auth = auth_for(reference.resolve_registry(), &file);
    Ok((reference, auth))
}

async fn push_with<R: PolicyArtifactRegistry>(
    client: &R,
    store: &dyn RegistryCredentialStore,
    reference: &str,
    config_blob: &[u8],
) -> Result<String> {
    let (reference, auth) = resolve(reference, store)?;
    client
        .push_policy_artifact(&reference, config_blob, &auth)
        .await
}

async fn pull_with<R: PolicyArtifactRegistry>(
    client: &R,
    store: &dyn RegistryCredentialStore,
    reference: &str,
) -> Result<(Vec<u8>, String)> {
    let (reference, auth) = resolve(reference, store)?;
    let (artifact_type, config_blob, digest) = client.pull_artifact(&reference, &auth).await?;
    if artifact_type.as_deref() != Some(POLICY_ARTIFACT_TYPE) {
        anyhow::bail!(
            "{reference} is not a policy artifact (artifactType {}, expected {POLICY_ARTIFACT_TYPE})",
            artifact_type.as_deref().unwrap_or("<none>")
        );
    }
    Ok((config_blob, digest))
}

fn store() -> JsonFileRegistryCredentialStore {
    JsonFileRegistryCredentialStore::new(default_registry_auth_path())
}

/// Builds the registry client with a protocol derived from the target host: loopback registries (and any in `LNS_REGISTRY_PLAIN_HTTP`) use plain HTTP, everything else HTTPS.
fn registry_for(reference: &str) -> RealRegistry {
    let target = reference
        .parse::<Reference>()
        .ok()
        .map(|r| r.resolve_registry().to_string());
    let protocol = crate::image::registry_protocol(
        std::env::var("LNS_REGISTRY_PLAIN_HTTP").ok().as_deref(),
        target.as_deref(),
    );
    RealRegistry::with_protocol(protocol)
}

pub async fn push(reference: &str, config_blob: &[u8]) -> Result<String> {
    push_with(&registry_for(reference), &store(), reference, config_blob).await
}

pub async fn pull(reference: &str) -> Result<(Vec<u8>, String)> {
    pull_with(&registry_for(reference), &store(), reference).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use lns_policy::registry_auth::RegistryCredential;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeRegistry {
        digest: String,
        pull: Option<(Option<String>, Vec<u8>, String)>,
        fail: bool,
        seen_auth: Mutex<Option<RegistryAuth>>,
    }

    impl PolicyArtifactRegistry for FakeRegistry {
        async fn push_policy_artifact(
            &self,
            _reference: &Reference,
            _config_blob: &[u8],
            auth: &RegistryAuth,
        ) -> Result<String> {
            *self.seen_auth.lock().unwrap() = Some(auth.clone());
            if self.fail {
                anyhow::bail!("registry refused the push");
            }
            Ok(self.digest.clone())
        }

        async fn pull_artifact(
            &self,
            _reference: &Reference,
            auth: &RegistryAuth,
        ) -> Result<(Option<String>, Vec<u8>, String)> {
            *self.seen_auth.lock().unwrap() = Some(auth.clone());
            if self.fail {
                anyhow::bail!("registry refused the pull");
            }
            Ok(self.pull.clone().expect("canned pull result"))
        }
    }

    /// A path that never exists, so the real store's `load()` returns an empty map (→ anonymous auth) without a temp dir to keep alive.
    fn empty_store() -> JsonFileRegistryCredentialStore {
        JsonFileRegistryCredentialStore::new(
            std::env::temp_dir().join("lns-policy-artifact-cov-absent.json"),
        )
    }

    /// Pointing the real store at a directory makes `load()` surface a non-NotFound IO error.
    fn failing_store() -> JsonFileRegistryCredentialStore {
        JsonFileRegistryCredentialStore::new(std::env::temp_dir())
    }

    const REF: &str = "registry.example.test/org/acme/policies/pii:v1";

    #[test]
    fn auth_for_builds_basic_from_a_stored_credential_defaulting_the_username() {
        let mut f = RegistryAuthFile::new();
        f.insert(
            "reg.example".into(),
            RegistryCredential {
                username: None,
                token: "lns_tok".into(),
            },
        );
        assert_eq!(
            auth_for("reg.example", &f),
            RegistryAuth::Basic("any".into(), "lns_tok".into())
        );
    }

    #[test]
    fn auth_for_keeps_an_explicit_username() {
        let mut f = RegistryAuthFile::new();
        f.insert(
            "reg.example".into(),
            RegistryCredential {
                username: Some("ci-bot".into()),
                token: "lns_tok".into(),
            },
        );
        assert!(matches!(auth_for("reg.example", &f), RegistryAuth::Basic(u, _) if u == "ci-bot"));
    }

    #[test]
    fn auth_for_is_anonymous_when_no_credential_is_stored() {
        let f = RegistryAuthFile::new();
        assert!(matches!(
            auth_for("reg.example", &f),
            RegistryAuth::Anonymous
        ));
    }

    #[tokio::test]
    async fn push_with_sends_the_stored_credential_as_basic_auth_and_returns_the_digest() {
        let client = FakeRegistry {
            digest: "sha256:abc".into(),
            ..Default::default()
        };
        let dir = tempfile::tempdir().unwrap();
        let store = JsonFileRegistryCredentialStore::new(dir.path().join("auth.json"));
        let mut file = RegistryAuthFile::new();
        file.insert(
            "registry.example.test".into(),
            RegistryCredential {
                username: Some("any".into()),
                token: "lns_secret".into(),
            },
        );
        store.save(&file).unwrap();

        let digest = push_with(&client, &store, REF, b"{}").await.unwrap();
        assert_eq!(digest, "sha256:abc");
        assert_eq!(
            *client.seen_auth.lock().unwrap(),
            Some(RegistryAuth::Basic("any".into(), "lns_secret".into()))
        );
    }

    #[tokio::test]
    async fn push_with_falls_back_to_anonymous_when_no_credential_is_stored() {
        let client = FakeRegistry {
            digest: "sha256:def".into(),
            ..Default::default()
        };
        push_with(&client, &empty_store(), REF, b"{}")
            .await
            .unwrap();
        assert_eq!(
            *client.seen_auth.lock().unwrap(),
            Some(RegistryAuth::Anonymous)
        );
    }

    #[tokio::test]
    async fn push_with_rejects_an_invalid_reference_before_touching_the_registry() {
        let client = FakeRegistry::default();
        let err = push_with(&client, &empty_store(), "::bad::", b"{}")
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("invalid registry reference"),
            "got: {err:#}"
        );
        assert!(
            client.seen_auth.lock().unwrap().is_none(),
            "registry must not be called"
        );
    }

    #[tokio::test]
    async fn push_with_surfaces_a_credential_store_load_error() {
        let client = FakeRegistry::default();
        let err = push_with(&client, &failing_store(), REF, b"{}")
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("loading registry credentials"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn push_with_propagates_a_registry_push_failure() {
        let client = FakeRegistry {
            fail: true,
            ..Default::default()
        };
        let err = push_with(&client, &empty_store(), REF, b"{}")
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("refused the push"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn pull_with_returns_the_config_blob_and_digest_for_a_policy_artifact() {
        let client = FakeRegistry {
            pull: Some((
                Some(POLICY_ARTIFACT_TYPE.into()),
                br#"{"network":{}}"#.to_vec(),
                "sha256:abc".into(),
            )),
            ..Default::default()
        };
        let (blob, digest) = pull_with(&client, &empty_store(), REF).await.unwrap();
        assert_eq!(blob, br#"{"network":{}}"#);
        assert_eq!(digest, "sha256:abc");
    }

    #[tokio::test]
    async fn pull_with_rejects_an_artifact_that_is_not_a_policy() {
        let client = FakeRegistry {
            pull: Some((
                Some("application/vnd.lens.agent.v1+json".into()),
                b"{}".to_vec(),
                "sha256:abc".into(),
            )),
            ..Default::default()
        };
        let err = pull_with(&client, &empty_store(), REF).await.unwrap_err();
        assert!(
            format!("{err:#}").contains("not a policy artifact"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn pull_with_reports_a_missing_artifact_type_as_not_a_policy() {
        let client = FakeRegistry {
            pull: Some((None, b"{}".to_vec(), "sha256:abc".into())),
            ..Default::default()
        };
        let err = pull_with(&client, &empty_store(), REF).await.unwrap_err();
        assert!(format!("{err:#}").contains("<none>"), "got: {err:#}");
    }

    #[tokio::test]
    async fn pull_with_propagates_a_registry_pull_failure() {
        let client = FakeRegistry {
            fail: true,
            ..Default::default()
        };
        let err = pull_with(&client, &empty_store(), REF).await.unwrap_err();
        assert!(
            format!("{err:#}").contains("refused the pull"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn public_push_wires_real_registry_and_fails_fast_on_a_bad_reference() {
        let dir = tempfile::tempdir().unwrap();
        let _g = crate::test_env::EnvVarGuard::set(
            "LNS_REGISTRY_AUTH_PATH",
            dir.path().join("auth.json"),
        );
        let err = push("::bad::", b"{}").await.unwrap_err();
        assert!(
            format!("{err:#}").contains("invalid registry reference"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn public_pull_wires_real_registry_and_fails_fast_on_a_bad_reference() {
        let dir = tempfile::tempdir().unwrap();
        let _g = crate::test_env::EnvVarGuard::set(
            "LNS_REGISTRY_AUTH_PATH",
            dir.path().join("auth.json"),
        );
        let err = pull("::bad::").await.unwrap_err();
        assert!(
            format!("{err:#}").contains("invalid registry reference"),
            "got: {err:#}"
        );
    }
}
