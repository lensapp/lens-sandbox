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
    pub fn with_protocol(protocol: oci_client::client::ClientProtocol) -> Self {
        let client = oci_client::Client::new(ClientConfig {
            platform_resolver: Some(Box::new(linux_platform_resolver)),
            protocol,
            ..Default::default()
        });
        Self {
            client,
            auth: RegistryAuth::Anonymous,
        }
    }

    /// Builds a client for pulling `reference`: loopback/`LNS_REGISTRY_PLAIN_HTTP` protocol and the stored credential for that registry (anonymous if none).
    pub fn for_reference(reference: &str) -> Self {
        let target = reference
            .parse::<Reference>()
            .ok()
            .map(|r| r.resolve_registry().to_string());
        let protocol = super::registry_protocol(
            std::env::var("LNS_REGISTRY_PLAIN_HTTP").ok().as_deref(),
            target.as_deref(),
        );
        let client = oci_client::Client::new(ClientConfig {
            platform_resolver: Some(Box::new(linux_platform_resolver)),
            protocol,
            ..Default::default()
        });
        Self {
            client,
            auth: crate::artifact::resolve_auth(reference),
        }
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

impl crate::artifact::ArtifactRegistry for RealRegistry {
    async fn push_artifact(
        &self,
        reference: &Reference,
        artifact_type: &str,
        config_media_type: &str,
        config_blob: &[u8],
        auth: &RegistryAuth,
    ) -> Result<String> {
        use oci_client::client::Config;
        use oci_client::manifest::OCI_IMAGE_MEDIA_TYPE;
        use sha2::{Digest, Sha256};

        let config = Config::new(config_blob.to_vec(), config_media_type.to_string(), None);
        let config_digest = format!("sha256:{}", hex::encode(Sha256::digest(config_blob)));
        let manifest = OciImageManifest {
            schema_version: 2,
            media_type: Some(OCI_IMAGE_MEDIA_TYPE.to_string()),
            config: OciDescriptor {
                media_type: config_media_type.to_string(),
                digest: config_digest,
                size: config_blob.len() as i64,
                ..Default::default()
            },
            layers: Vec::new(),
            artifact_type: Some(artifact_type.to_string()),
            ..Default::default()
        };
        self.client
            .push(reference, &[], config, auth, Some(manifest))
            .await
            .with_context(|| format!("pushing artifact to {reference}"))?;
        self.client
            .fetch_manifest_digest(reference, auth)
            .await
            .with_context(|| format!("reading pushed manifest digest for {reference}"))
    }

    async fn pull_head(
        &self,
        reference: &Reference,
        auth: &RegistryAuth,
    ) -> Result<crate::artifact::ManifestHead> {
        let (manifest, digest, config) = self
            .client
            .pull_manifest_and_config(reference, auth)
            .await
            .with_context(|| format!("pulling manifest for {reference}"))?;
        let manifest_bytes = serialized_len(&manifest).unwrap_or(usize::MAX);
        enforce_manifest_doc_size(&reference.to_string(), manifest_bytes, config.len())?;
        Ok(crate::artifact::ManifestHead {
            config_media_type: manifest.config.media_type,
            artifact_type: manifest.artifact_type,
            config_blob: config.into_bytes(),
            digest,
        })
    }

    async fn pull_image_to_cache(&self, reference: &str) -> Result<String> {
        Ok(crate::image_store::pull(reference).await?.digest)
    }

    async fn pull_artifact_layers(
        &self,
        reference: &Reference,
        auth: &RegistryAuth,
    ) -> Result<Vec<Vec<u8>>> {
        let (manifest, _digest, _config) = self
            .client
            .pull_manifest_and_config(reference, auth)
            .await
            .with_context(|| format!("pulling manifest for {reference}"))?;
        let mut layers = Vec::with_capacity(manifest.layers.len());
        for descriptor in &manifest.layers {
            let mut out = CountingSink::new(&|_| {});
            self.client
                .pull_blob(reference, descriptor, &mut out)
                .await
                .map_err(|e| anyhow::anyhow!("pull_blob {}: {e}", descriptor.digest))?;
            layers.push(out.into_bytes());
        }
        Ok(layers)
    }

    async fn push_image_from_cache(
        &self,
        source_reference: &str,
        target: &Reference,
        auth: &RegistryAuth,
    ) -> Result<String> {
        use oci_client::client::{Config, ImageLayer};

        let record = crate::image_store::read_record(source_reference)
            .await?
            .filter(|r| r.manifest.is_some());
        let record = record.with_context(|| {
            format!("{source_reference} is not in the local image cache; pull it first")
        })?;
        let manifest: OciImageManifest =
            serde_json::from_str(&record.manifest.expect("filtered to Some above"))
                .with_context(|| format!("parsing cached manifest for {source_reference}"))?;
        let config_blob = record.config.unwrap_or_default();

        let layer_cache = LayerCache::new(crate::cache::root()?.join("layers"));
        let mut layers = Vec::with_capacity(manifest.layers.len());
        for descriptor in &manifest.layers {
            let bytes = layer_cache
                .read(&descriptor.digest)
                .with_context(|| format!("reading cached layer {}", descriptor.digest))?;
            layers.push(ImageLayer::new(
                bytes,
                descriptor.media_type.clone(),
                descriptor.annotations.clone(),
            ));
        }
        let config = Config::new(
            config_blob.into_bytes(),
            manifest.config.media_type.clone(),
            manifest.config.annotations.clone(),
        );
        self.client
            .push(target, &layers, config, auth, Some(manifest))
            .await
            .with_context(|| format!("pushing image to {target}"))?;
        self.client
            .fetch_manifest_digest(target, auth)
            .await
            .with_context(|| format!("reading pushed manifest digest for {target}"))
    }
}

pub async fn pull(image: &str, layer_cache: &LayerCache) -> Result<PulledImage> {
    let _shared = crate::image_store::lock_shared().await;
    let manifests = crate::cache::root()?.join("manifests");
    let registry = CachingRegistry::new(
        RealRegistry::for_reference(image),
        ManifestCache::new(manifests),
    );
    let pulled = pull_inner(&registry, image, layer_cache).await?;
    if let Err(e) = crate::image_store::record(&pulled).await {
        crate::log::warn!("image index write failed for {image} ({e:#}); continuing");
    }
    Ok(pulled)
}
