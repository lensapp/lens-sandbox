use anyhow::{Context, Result};
use lns_policy::registry_auth::{
    JsonFileRegistryAuthStore, RegistryAuthStore, credential_for, default_registry_auth_path,
};
use oci_client::{
    Reference, RegistryOperation,
    manifest::{OciDescriptor, OciImageManifest},
    secrets::RegistryAuth,
};

use crate::oci_layer_cache::LayerCache;

use super::manifest_cache::{CachingRegistry, ManifestCache};
use super::{
    CountingSink, PulledImage, Registry, client_config_for, enforce_manifest_doc_size,
    login_probe_reference, pull_artifact_with, pull_inner, serialized_len,
};

pub struct RealRegistry {
    client: oci_client::Client,
    auth: RegistryAuth,
}

impl RealRegistry {
    pub fn for_registry(registry: &str, auth: RegistryAuth) -> Self {
        Self {
            client: oci_client::Client::new(client_config_for(registry)),
            auth,
        }
    }

    pub fn for_reference(reference: &Reference, auth: RegistryAuth) -> Self {
        Self::for_registry(reference.registry(), auth)
    }
}

/// The stored login for `image`'s registry, or anonymous when none is recorded (or the reference / store is unreadable — the pull then fails with the registry's own auth error).
pub(crate) fn registry_auth_for(image: &str) -> RegistryAuth {
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

/// Verifies a registry login by running the exact pull-auth handshake a later pull would, against a throwaway repository scope: an accepted credential resolves, a rejected one errors.
pub async fn verify_login(registry: &str, username: &str, secret: &str) -> Result<()> {
    let reference = login_probe_reference(registry)?;
    let auth = RegistryAuth::Basic(username.to_string(), secret.to_string());
    let probe = RealRegistry::for_reference(&reference, auth);
    probe
        .client
        .auth(&reference, &probe.auth, RegistryOperation::Pull)
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
        let mut out = CountingSink::new(super::blob_byte_cap(descriptor.size), on_chunk);
        self.client
            .pull_blob(reference, descriptor, &mut out)
            .await
            .map_err(|e| anyhow::anyhow!("pull_blob {}: {e}", descriptor.digest))?;
        Ok(out.into_bytes())
    }

    async fn pull_blob_to_path(
        &self,
        reference: &Reference,
        descriptor: &OciDescriptor,
        max_bytes: u64,
        path: &std::path::Path,
        on_chunk: &(dyn Fn(u64) + Send + Sync),
    ) -> Result<()> {
        let file = tokio::fs::File::create(path)
            .await
            .with_context(|| format!("creating streamed blob {}", path.display()))?;
        let mut out = CountingWriter::new(file, max_bytes, on_chunk);
        self.client
            .pull_blob(reference, descriptor, &mut out)
            .await
            .map_err(|e| anyhow::anyhow!("pull_blob {}: {e}", descriptor.digest))?;
        tokio::io::AsyncWriteExt::shutdown(&mut out)
            .await
            .with_context(|| format!("closing streamed blob {}", path.display()))
    }
}

struct CountingWriter<'a, W> {
    inner: W,
    written: u64,
    max_bytes: u64,
    on_chunk: &'a (dyn Fn(u64) + Send + Sync),
}

impl<'a, W> CountingWriter<'a, W> {
    fn new(inner: W, max_bytes: u64, on_chunk: &'a (dyn Fn(u64) + Send + Sync)) -> Self {
        Self {
            inner,
            written: 0,
            max_bytes,
            on_chunk,
        }
    }
}

impl<W: tokio::io::AsyncWrite + Unpin> tokio::io::AsyncWrite for CountingWriter<'_, W> {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        if !buf.is_empty() && this.written >= this.max_bytes {
            return std::task::Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("blob exceeds the {}-byte limit", this.max_bytes),
            )));
        }
        let remaining = this.max_bytes.saturating_sub(this.written);
        let allowed = usize::try_from(remaining)
            .unwrap_or(usize::MAX)
            .min(buf.len());
        match std::pin::Pin::new(&mut this.inner).poll_write(cx, &buf[..allowed]) {
            std::task::Poll::Ready(Ok(written)) => {
                this.written += written as u64;
                (this.on_chunk)(written as u64);
                std::task::Poll::Ready(Ok(written))
            }
            other => other,
        }
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

pub async fn pull(image: &str, layer_cache: &LayerCache) -> Result<PulledImage> {
    let _shared = crate::image_store::lock_shared().await;
    let registry = caching_registry_for(image)?;
    let pulled = pull_inner(&registry, image, layer_cache).await?;
    crate::image_store::record(&pulled)
        .await
        .with_context(|| format!("recording the image index for {image}"))?;
    Ok(pulled)
}

pub(crate) async fn pull_dependency(image: &str, layer_cache: &LayerCache) -> Result<PulledImage> {
    let registry = caching_registry_for(image)?;
    let pulled = pull_inner(&registry, image, layer_cache).await?;
    crate::image_store::record(&pulled).await?;
    Ok(pulled)
}

pub async fn pull_artifact(image: &str) -> Result<super::PulledArtifact> {
    let _shared = crate::image_store::lock_shared().await;
    let registry = caching_registry_for(image)?;
    pull_artifact_with(&registry, image).await
}

pub(crate) fn caching_registry_for(image: &str) -> Result<CachingRegistry<RealRegistry>> {
    let manifests = crate::cache::root()?.join("manifests");
    let auth = registry_auth_for(image);
    let inner = match image.parse::<Reference>() {
        Ok(parsed) => RealRegistry::for_reference(&parsed, auth),
        Err(_) => RealRegistry::for_registry(image, auth),
    };
    Ok(CachingRegistry::new(inner, ManifestCache::new(manifests)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn counting_writer_stops_before_writing_beyond_its_limit() {
        let mut writer = CountingWriter::new(Vec::new(), 3, &|_| {});
        let err = writer.write_all(b"four").await.unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(writer.inner, b"fou");
    }
}
