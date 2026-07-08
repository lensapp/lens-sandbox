use anyhow::{Context, Result};
use lns_policy::registry_auth::{
    JsonFileRegistryAuthStore, RegistryAuthStore, credential_for, default_registry_auth_path,
};
use oci_client::{
    Reference, RegistryOperation,
    client::ClientConfig,
    manifest::{OciDescriptor, OciImageManifest},
    secrets::RegistryAuth,
};

use crate::oci_layer_cache::LayerCache;

use super::client_pool::{ClientPool, pool_key};
use super::manifest_cache::{CachingRegistry, ManifestCache};
use super::{
    CountingSink, PulledImage, Registry, enforce_manifest_doc_size, linux_platform_resolver,
    pull_inner, serialized_len,
};

pub struct RealRegistry {
    client: oci_client::Client,
    auth: RegistryAuth,
}

impl RealRegistry {
    fn with_client(client: oci_client::Client, auth: RegistryAuth) -> Self {
        Self { client, auth }
    }
}

fn new_client() -> oci_client::Client {
    oci_client::Client::new(ClientConfig {
        platform_resolver: Some(Box::new(linux_platform_resolver)),
        ..Default::default()
    })
}

/// Reuse one registry client per (registry, credential) across pulls so the bearer token and connection pool survive — a fresh client re-runs the registry auth handshake on every pull, the dominant cost of a warm tag-image boot.
fn pooled_client(registry: &str, auth: &RegistryAuth) -> oci_client::Client {
    static POOL: std::sync::OnceLock<ClientPool<oci_client::Client>> = std::sync::OnceLock::new();
    POOL.get_or_init(ClientPool::new)
        .get_or_create(pool_key(registry, auth), new_client)
}

/// The stored login for `image`'s registry, or anonymous when none is recorded (or the reference / store is unreadable — the pull then fails with the registry's own auth error).
fn registry_auth_for(image: &str) -> RegistryAuth {
    let Ok(reference) = image.parse::<Reference>() else {
        return RegistryAuth::Anonymous;
    };
    let store = JsonFileRegistryAuthStore::new(default_registry_auth_path());
    let Ok(file) = store.load() else {
        return RegistryAuth::Anonymous;
    };
    match credential_for(&file, reference.registry()) {
        Some(cred) => RegistryAuth::Basic(cred.username.clone(), cred.secret.clone()),
        None => RegistryAuth::Anonymous,
    }
}

const LOGIN_PROBE_REPOSITORY: &str = "lns/login-check";

/// Verifies a registry login by running the exact pull-auth handshake a later pull would, against a throwaway repository scope: an accepted credential resolves, a rejected one errors.
pub async fn verify_login(registry: &str, username: &str, secret: &str) -> Result<()> {
    let reference: Reference = format!("{registry}/{LOGIN_PROBE_REPOSITORY}")
        .parse()
        .with_context(|| format!("invalid registry {registry:?}"))?;
    let client = new_client();
    let auth = RegistryAuth::Basic(username.to_string(), secret.to_string());
    client
        .auth(&reference, &auth, RegistryOperation::Pull)
        .await
        .map(|_| ())
        .map_err(|e| anyhow::anyhow!("{e}"))
}

impl Registry for RealRegistry {
    async fn pull_manifest_and_config(
        &self,
        reference: &Reference,
    ) -> Result<(OciImageManifest, String, String)> {
        let (manifest, manifest_digest, config) = self
            .client
            .pull_manifest_and_config(reference, &self.auth)
            .await
            .with_context(|| format!("failed to pull manifest/config for {reference}"))?;
        let manifest_bytes = serialized_len(&manifest).unwrap_or(usize::MAX);
        enforce_manifest_doc_size(&reference.to_string(), manifest_bytes, config.len())?;
        Ok((manifest, manifest_digest, config))
    }

    async fn pull_blob(
        &self,
        reference: &Reference,
        descriptor: &OciDescriptor,
        on_chunk: &(dyn Fn(u64) + Send + Sync),
    ) -> Result<Vec<u8>> {
        let mut out = CountingSink::new(on_chunk);
        self.client
            .pull_blob(reference, descriptor, &mut out)
            .await
            .map_err(|e| anyhow::anyhow!("pull_blob {}: {e}", descriptor.digest))?;
        Ok(out.into_bytes())
    }
}

pub async fn pull(image: &str, layer_cache: &LayerCache) -> Result<PulledImage> {
    let _shared = crate::image_store::lock_shared().await;
    let manifests = crate::cache::root()?.join("manifests");
    let auth = registry_auth_for(image);
    let client = match image.parse::<Reference>() {
        Ok(reference) => pooled_client(reference.resolve_registry(), &auth),
        Err(_) => new_client(),
    };
    let registry = CachingRegistry::new(
        RealRegistry::with_client(client, auth),
        ManifestCache::new(manifests),
    );
    let pulled = pull_inner(&registry, image, layer_cache).await?;
    if let Err(e) = crate::image_store::record(&pulled).await {
        crate::log::warn!("image index write failed for {image} ({e:#}); continuing");
    }
    Ok(pulled)
}
