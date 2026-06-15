use anyhow::{Context, Result};
use lns_policy::artifact::Family;
use lns_policy::registry_auth::{
    JsonFileRegistryCredentialStore, RegistryAuthFile, RegistryCredentialStore,
    default_registry_auth_path,
};
use oci_client::Reference;
use oci_client::secrets::RegistryAuth;

use crate::image::RealRegistry;

pub struct ManifestHead {
    pub config_media_type: String,
    pub artifact_type: Option<String>,
    pub config_blob: Vec<u8>,
    pub digest: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Pulled {
    Artifact {
        artifact_type: String,
        config_blob: Vec<u8>,
        digest: String,
    },
    Image {
        digest: String,
    },
}

pub(crate) trait ArtifactRegistry: Send + Sync {
    fn push_artifact(
        &self,
        reference: &Reference,
        artifact_type: &str,
        config_media_type: &str,
        config_blob: &[u8],
        auth: &RegistryAuth,
    ) -> impl std::future::Future<Output = Result<String>> + Send;

    fn pull_head(
        &self,
        reference: &Reference,
        auth: &RegistryAuth,
    ) -> impl std::future::Future<Output = Result<ManifestHead>> + Send;

    fn pull_image_to_cache(
        &self,
        reference: &str,
    ) -> impl std::future::Future<Output = Result<String>> + Send;

    fn push_image_from_cache(
        &self,
        source_reference: &str,
        target: &Reference,
        auth: &RegistryAuth,
    ) -> impl std::future::Future<Output = Result<String>> + Send;
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

/// Best-effort auth for an image pull: resolves the stored credential for the reference's registry, falling back to anonymous on any parse/load failure (the pull surfaces its own error if the reference is bad).
pub(crate) fn resolve_auth(reference: &str) -> RegistryAuth {
    match reference.parse::<Reference>() {
        Ok(parsed) => auth_for(
            parsed.resolve_registry(),
            &store().load().unwrap_or_default(),
        ),
        Err(_) => RegistryAuth::Anonymous,
    }
}

async fn push_artifact_with<R: ArtifactRegistry>(
    client: &R,
    store: &dyn RegistryCredentialStore,
    reference: &str,
    artifact_type: &str,
    config_media_type: &str,
    config_blob: &[u8],
) -> Result<String> {
    let (reference, auth) = resolve(reference, store)?;
    client
        .push_artifact(
            &reference,
            artifact_type,
            config_media_type,
            config_blob,
            &auth,
        )
        .await
}

async fn pull_with<R: ArtifactRegistry>(
    client: &R,
    store: &dyn RegistryCredentialStore,
    reference: &str,
) -> Result<Pulled> {
    let (parsed, auth) = resolve(reference, store)?;
    let head = client.pull_head(&parsed, &auth).await?;
    if Family::from_config_media_type(&head.config_media_type).is_some() {
        Ok(Pulled::Artifact {
            artifact_type: head
                .artifact_type
                .unwrap_or_else(|| head.config_media_type.clone()),
            config_blob: head.config_blob,
            digest: head.digest,
        })
    } else {
        let digest = client.pull_image_to_cache(reference).await?;
        Ok(Pulled::Image { digest })
    }
}

async fn push_image_with<R: ArtifactRegistry>(
    client: &R,
    store: &dyn RegistryCredentialStore,
    source_reference: &str,
    target_reference: &str,
) -> Result<String> {
    let (target, auth) = resolve(target_reference, store)?;
    client
        .push_image_from_cache(source_reference, &target, &auth)
        .await
}

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

fn store() -> JsonFileRegistryCredentialStore {
    JsonFileRegistryCredentialStore::new(default_registry_auth_path())
}

pub async fn push_artifact(
    reference: &str,
    artifact_type: &str,
    config_media_type: &str,
    config_blob: &[u8],
) -> Result<String> {
    push_artifact_with(
        &registry_for(reference),
        &store(),
        reference,
        artifact_type,
        config_media_type,
        config_blob,
    )
    .await
}

pub async fn push_image(source_reference: &str, target_reference: &str) -> Result<String> {
    push_image_with(
        &registry_for(target_reference),
        &store(),
        source_reference,
        target_reference,
    )
    .await
}

pub async fn pull(reference: &str) -> Result<Pulled> {
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
        head: Option<ManifestHead>,
        image_digest: String,
        fail: bool,
        image_fail: bool,
        seen_auth: Mutex<Option<RegistryAuth>>,
        image_pulled: Mutex<Option<String>>,
        image_pushed: Mutex<Option<(String, String)>>,
    }

    impl ArtifactRegistry for FakeRegistry {
        async fn push_artifact(
            &self,
            _reference: &Reference,
            _artifact_type: &str,
            _config_media_type: &str,
            _config_blob: &[u8],
            auth: &RegistryAuth,
        ) -> Result<String> {
            *self.seen_auth.lock().unwrap() = Some(auth.clone());
            if self.fail {
                anyhow::bail!("registry refused the push");
            }
            Ok(self.digest.clone())
        }

        async fn pull_head(
            &self,
            _reference: &Reference,
            auth: &RegistryAuth,
        ) -> Result<ManifestHead> {
            *self.seen_auth.lock().unwrap() = Some(auth.clone());
            if self.fail {
                anyhow::bail!("registry refused the pull");
            }
            let head = self.head.as_ref().expect("canned head");
            Ok(ManifestHead {
                config_media_type: head.config_media_type.clone(),
                artifact_type: head.artifact_type.clone(),
                config_blob: head.config_blob.clone(),
                digest: head.digest.clone(),
            })
        }

        async fn pull_image_to_cache(&self, reference: &str) -> Result<String> {
            *self.image_pulled.lock().unwrap() = Some(reference.to_string());
            if self.image_fail {
                anyhow::bail!("image pull failed");
            }
            Ok(self.image_digest.clone())
        }

        async fn push_image_from_cache(
            &self,
            source_reference: &str,
            target: &Reference,
            auth: &RegistryAuth,
        ) -> Result<String> {
            *self.seen_auth.lock().unwrap() = Some(auth.clone());
            *self.image_pushed.lock().unwrap() =
                Some((source_reference.into(), target.to_string()));
            if self.fail {
                anyhow::bail!("registry refused the image push");
            }
            Ok(self.image_digest.clone())
        }
    }

    fn head(config_media_type: &str, artifact_type: Option<&str>) -> ManifestHead {
        ManifestHead {
            config_media_type: config_media_type.into(),
            artifact_type: artifact_type.map(str::to_string),
            config_blob: br#"{"network":{}}"#.to_vec(),
            digest: "sha256:abc".into(),
        }
    }

    /// A path that never exists, so the real store's `load()` returns an empty map (→ anonymous).
    fn empty_store() -> JsonFileRegistryCredentialStore {
        JsonFileRegistryCredentialStore::new(
            std::env::temp_dir().join("lns-artifact-cov-absent.json"),
        )
    }

    /// Pointing the real store at a directory makes `load()` surface a non-NotFound IO error.
    fn failing_store() -> JsonFileRegistryCredentialStore {
        JsonFileRegistryCredentialStore::new(std::env::temp_dir())
    }

    const REF: &str = "registry.example.test/org/acme/policies/pii:v1";
    const POLICY_CMT: &str = "application/vnd.lens.policy.config.v1+json";
    const POLICY_AT: &str = "application/vnd.lens.policy.v1+json";
    const OCI_CMT: &str = "application/vnd.oci.image.config.v1+json";

    #[test]
    fn auth_for_builds_basic_defaulting_username_else_anonymous() {
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
        assert_eq!(auth_for("absent", &f), RegistryAuth::Anonymous);
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn resolve_auth_returns_basic_for_a_stored_credential() {
        let dir = tempfile::tempdir().unwrap();
        let _g = crate::test_env::EnvVarGuard::set(
            "LNS_REGISTRY_AUTH_PATH",
            dir.path().join("auth.json"),
        );
        let store = JsonFileRegistryCredentialStore::new(dir.path().join("auth.json"));
        let mut file = RegistryAuthFile::new();
        file.insert(
            "registry.example.test".into(),
            RegistryCredential {
                username: None,
                token: "lns_tok".into(),
            },
        );
        store.save(&file).unwrap();
        assert_eq!(
            resolve_auth("registry.example.test/org/x/agents/a:v1"),
            RegistryAuth::Basic("any".into(), "lns_tok".into())
        );
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn resolve_auth_is_anonymous_without_a_credential_or_for_a_bad_reference() {
        let dir = tempfile::tempdir().unwrap();
        let _g = crate::test_env::EnvVarGuard::set(
            "LNS_REGISTRY_AUTH_PATH",
            dir.path().join("absent.json"),
        );
        assert_eq!(
            resolve_auth("registry.example.test/org/x/agents/a:v1"),
            RegistryAuth::Anonymous
        );
        assert_eq!(resolve_auth("::bad::"), RegistryAuth::Anonymous);
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

    #[tokio::test]
    async fn push_sends_basic_auth_from_the_store_and_returns_the_digest() {
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

        let digest = push_artifact_with(&client, &store, REF, POLICY_AT, POLICY_CMT, b"{}")
            .await
            .unwrap();
        assert_eq!(digest, "sha256:abc");
        assert_eq!(
            *client.seen_auth.lock().unwrap(),
            Some(RegistryAuth::Basic("any".into(), "lns_secret".into()))
        );
    }

    #[tokio::test]
    async fn push_falls_back_to_anonymous_without_a_stored_credential() {
        let client = FakeRegistry::default();
        push_artifact_with(&client, &empty_store(), REF, POLICY_AT, POLICY_CMT, b"{}")
            .await
            .unwrap();
        assert_eq!(
            *client.seen_auth.lock().unwrap(),
            Some(RegistryAuth::Anonymous)
        );
    }

    #[tokio::test]
    async fn push_rejects_an_invalid_reference_before_touching_the_registry() {
        let client = FakeRegistry::default();
        let err = push_artifact_with(
            &client,
            &empty_store(),
            "::bad::",
            POLICY_AT,
            POLICY_CMT,
            b"{}",
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("invalid registry reference"),
            "got: {err:#}"
        );
        assert!(client.seen_auth.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn push_surfaces_a_credential_store_load_error() {
        let client = FakeRegistry::default();
        let err = push_artifact_with(&client, &failing_store(), REF, POLICY_AT, POLICY_CMT, b"{}")
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("loading registry credentials"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn push_propagates_a_registry_failure() {
        let client = FakeRegistry {
            fail: true,
            ..Default::default()
        };
        let err = push_artifact_with(&client, &empty_store(), REF, POLICY_AT, POLICY_CMT, b"{}")
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("refused the push"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn push_image_resolves_target_auth_and_returns_the_digest() {
        let client = FakeRegistry {
            image_digest: "sha256:img".into(),
            ..Default::default()
        };
        let dir = tempfile::tempdir().unwrap();
        let store = JsonFileRegistryCredentialStore::new(dir.path().join("auth.json"));
        let mut file = RegistryAuthFile::new();
        file.insert(
            "registry.example.test".into(),
            RegistryCredential {
                username: None,
                token: "lns_tok".into(),
            },
        );
        store.save(&file).unwrap();

        let digest = push_image_with(&client, &store, "docker.io/library/alpine:3.20", REF)
            .await
            .unwrap();
        assert_eq!(digest, "sha256:img");
        assert_eq!(
            client.image_pushed.lock().unwrap().as_ref().unwrap().0,
            "docker.io/library/alpine:3.20"
        );
        assert_eq!(
            *client.seen_auth.lock().unwrap(),
            Some(RegistryAuth::Basic("any".into(), "lns_tok".into()))
        );
    }

    #[tokio::test]
    async fn push_image_rejects_an_invalid_target_reference() {
        let client = FakeRegistry::default();
        let err = push_image_with(&client, &empty_store(), "alpine:3.20", "::bad::")
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("invalid registry reference"),
            "got: {err:#}"
        );
        assert!(client.image_pushed.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn push_image_propagates_a_registry_failure() {
        let client = FakeRegistry {
            fail: true,
            ..Default::default()
        };
        let err = push_image_with(&client, &empty_store(), "alpine:3.20", REF)
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("refused the image push"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn pull_returns_an_artifact_when_the_config_media_type_is_a_lens_family() {
        let client = FakeRegistry {
            head: Some(head(POLICY_CMT, Some(POLICY_AT))),
            ..Default::default()
        };
        let pulled = pull_with(&client, &empty_store(), REF).await.unwrap();
        assert_eq!(
            pulled,
            Pulled::Artifact {
                artifact_type: POLICY_AT.into(),
                config_blob: br#"{"network":{}}"#.to_vec(),
                digest: "sha256:abc".into(),
            }
        );
        assert!(client.image_pulled.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn pull_falls_back_to_the_config_media_type_when_artifact_type_is_absent() {
        let client = FakeRegistry {
            head: Some(head(POLICY_CMT, None)),
            ..Default::default()
        };
        let pulled = pull_with(&client, &empty_store(), REF).await.unwrap();
        assert!(
            matches!(pulled, Pulled::Artifact { artifact_type, .. } if artifact_type == POLICY_CMT)
        );
    }

    #[tokio::test]
    async fn pull_pulls_an_image_into_the_cache_when_the_config_is_an_oci_image() {
        let client = FakeRegistry {
            head: Some(head(OCI_CMT, None)),
            image_digest: "sha256:img".into(),
            ..Default::default()
        };
        let pulled = pull_with(&client, &empty_store(), REF).await.unwrap();
        assert!(matches!(pulled, Pulled::Image { digest } if digest == "sha256:img"));
        assert_eq!(client.image_pulled.lock().unwrap().as_deref(), Some(REF));
    }

    #[tokio::test]
    async fn pull_propagates_a_head_fetch_error() {
        let client = FakeRegistry {
            fail: true,
            head: Some(head(OCI_CMT, None)),
            ..Default::default()
        };
        let err = pull_with(&client, &empty_store(), REF).await.unwrap_err();
        assert!(
            format!("{err:#}").contains("refused the pull"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn pull_propagates_an_image_pull_failure() {
        let client = FakeRegistry {
            head: Some(head(OCI_CMT, None)),
            image_fail: true,
            ..Default::default()
        };
        let err = pull_with(&client, &empty_store(), REF).await.unwrap_err();
        assert!(
            format!("{err:#}").contains("image pull failed"),
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
        let err = push_artifact("::bad::", POLICY_AT, POLICY_CMT, b"{}")
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("invalid registry reference"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn public_push_image_wires_real_registry_and_fails_fast_on_a_bad_reference() {
        let dir = tempfile::tempdir().unwrap();
        let _g = crate::test_env::EnvVarGuard::set(
            "LNS_REGISTRY_AUTH_PATH",
            dir.path().join("auth.json"),
        );
        let err = push_image("alpine:3.20", "::bad::").await.unwrap_err();
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
