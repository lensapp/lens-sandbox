use anyhow::{Context, Result};
use oci_client::{
    Reference,
    client::ClientConfig,
    manifest::{OciDescriptor, OciImageManifest},
    secrets::RegistryAuth,
};

use crate::oci_layer_cache::LayerCache;

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
    pub fn new() -> Self {
        let client = oci_client::Client::new(ClientConfig {
            platform_resolver: Some(Box::new(linux_platform_resolver)),
            ..Default::default()
        });
        Self {
            client,
            auth: RegistryAuth::Anonymous,
        }
    }
}

impl Default for RealRegistry {
    fn default() -> Self {
        Self::new()
    }
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
    let registry = CachingRegistry::new(RealRegistry::new(), ManifestCache::new(manifests));
    let pulled = pull_inner(&registry, image, layer_cache).await?;
    if let Err(e) = crate::image_store::record(&pulled).await {
        crate::log::warn!("image index write failed for {image} ({e:#}); continuing");
    }
    Ok(pulled)
}
