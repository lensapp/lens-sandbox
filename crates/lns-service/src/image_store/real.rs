use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};

use super::traits::{Caches, Fs, RuntimeCacheFs};
use crate::image::manifest_cache::ManifestCache;
use crate::oci_layer_cache::LayerCache;

pub(super) struct RealFs;

impl Fs for RealFs {
    async fn read_dir(&self, dir: &Path) -> io::Result<Vec<PathBuf>> {
        let mut entries = tokio::fs::read_dir(dir).await?;
        let mut out = Vec::new();
        while let Some(entry) = entries.next_entry().await? {
            out.push(entry.path());
        }
        Ok(out)
    }

    async fn read(&self, p: &Path) -> io::Result<Vec<u8>> {
        tokio::fs::read(p).await
    }

    async fn write(&self, p: &Path, bytes: &[u8]) -> io::Result<()> {
        if let Some(parent) = p.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let tmp = p.with_extension(format!("{:016x}.tmp", rand::random::<u64>()));
        tokio::fs::write(&tmp, bytes).await?;
        tokio::fs::rename(&tmp, p).await
    }

    async fn remove_file(&self, p: &Path) -> io::Result<()> {
        tokio::fs::remove_file(p).await
    }
}

impl RuntimeCacheFs for RealFs {
    async fn remove_dir_all(&self, p: &Path) -> io::Result<()> {
        tokio::fs::remove_dir_all(p).await
    }
}

pub(super) struct RealCaches {
    layers: LayerCache,
    manifests: ManifestCache,
}

impl RealCaches {
    pub(super) fn new(cache_root: &Path) -> Self {
        Self {
            layers: LayerCache::new(cache_root.join("layers")),
            manifests: ManifestCache::new(cache_root.join("manifests")),
        }
    }
}

impl Caches for RealCaches {
    fn sweep_layers(&self, keep: &HashSet<String>) -> anyhow::Result<u64> {
        self.layers.sweep(keep)
    }

    fn remove_manifest(&self, reference: &str) -> anyhow::Result<()> {
        self.manifests.remove(reference)
    }
}
