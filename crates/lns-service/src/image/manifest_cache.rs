use std::path::PathBuf;

use anyhow::{Context, Result};
use oci_client::{Reference, manifest::OciImageManifest};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::Registry;

#[derive(Serialize, Deserialize)]
pub(crate) struct CachedManifest {
    pub manifest: OciImageManifest,
    pub manifest_digest: String,
    pub config: String,
}

pub(crate) struct ManifestCache {
    root: PathBuf,
}

impl ManifestCache {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn path_for(&self, reference: &str) -> PathBuf {
        let key = hex::encode(Sha256::digest(reference.as_bytes()));
        self.root.join(format!("{key}.json"))
    }

    pub fn get(&self, reference: &str) -> Option<CachedManifest> {
        let bytes = std::fs::read(self.path_for(reference)).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    pub fn put(&self, reference: &str, entry: &CachedManifest) -> Result<()> {
        std::fs::create_dir_all(&self.root).context("creating manifest cache dir")?;
        let bytes = serde_json::to_vec(entry).context("serializing cached manifest")?;
        std::fs::write(self.path_for(reference), bytes).context("writing manifest cache entry")
    }
}

pub(crate) struct CachingRegistry<R: Registry> {
    inner: R,
    cache: ManifestCache,
}

impl<R: Registry> CachingRegistry<R> {
    pub fn new(inner: R, cache: ManifestCache) -> Self {
        Self { inner, cache }
    }
}

impl<R: Registry> Registry for CachingRegistry<R> {
    async fn pull_manifest_and_config(
        &self,
        reference: &Reference,
    ) -> Result<(OciImageManifest, String, String)> {
        // Only digest-pinned references are immutable; caching a tag would freeze it past upstream republishes.
        if reference.digest().is_none() {
            return self.inner.pull_manifest_and_config(reference).await;
        }
        let key = reference.whole();
        if let Some(c) = self.cache.get(&key) {
            return Ok((c.manifest, c.manifest_digest, c.config));
        }
        let (manifest, manifest_digest, config) =
            self.inner.pull_manifest_and_config(reference).await?;
        let entry = CachedManifest {
            manifest: manifest.clone(),
            manifest_digest: manifest_digest.clone(),
            config: config.clone(),
        };
        if let Err(e) = self.cache.put(&key, &entry) {
            crate::log::warn!("manifest cache write failed for {key} ({e:#}); continuing");
        }
        Ok((manifest, manifest_digest, config))
    }

    async fn pull_blob(
        &self,
        reference: &Reference,
        descriptor: &oci_client::manifest::OciDescriptor,
    ) -> Result<Vec<u8>> {
        self.inner.pull_blob(reference, descriptor).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oci_client::manifest::{OciDescriptor, OciImageManifest};
    use std::sync::Mutex;

    fn sample_manifest() -> OciImageManifest {
        OciImageManifest {
            layers: vec![OciDescriptor {
                digest: "sha256:aaaa".into(),
                size: 1,
                media_type: "application/vnd.oci.image.layer.v1.tar+gzip".into(),
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn entry() -> CachedManifest {
        CachedManifest {
            manifest: sample_manifest(),
            manifest_digest: "sha256:manifest".into(),
            config: r#"{"architecture":"arm64"}"#.into(),
        }
    }

    #[test]
    fn get_is_none_for_a_reference_never_stored() {
        let d = tempfile::tempdir().unwrap();
        let cache = ManifestCache::new(d.path());
        assert!(cache.get("ghcr.io/x/y:latest").is_none());
    }

    #[test]
    fn put_then_get_round_trips_manifest_digest_and_config() {
        let d = tempfile::tempdir().unwrap();
        let cache = ManifestCache::new(d.path());
        cache.put("ghcr.io/x/y:latest", &entry()).unwrap();
        let got = cache.get("ghcr.io/x/y:latest").expect("cached entry");
        assert_eq!(got.manifest_digest, "sha256:manifest");
        assert_eq!(got.config, r#"{"architecture":"arm64"}"#);
        assert_eq!(got.manifest.layers[0].digest, "sha256:aaaa");
    }

    #[test]
    fn distinct_references_do_not_collide() {
        let d = tempfile::tempdir().unwrap();
        let cache = ManifestCache::new(d.path());
        cache.put("img:a", &entry()).unwrap();
        let mut other = entry();
        other.manifest_digest = "sha256:other".into();
        cache.put("img:b", &other).unwrap();
        assert_eq!(
            cache.get("img:a").unwrap().manifest_digest,
            "sha256:manifest"
        );
        assert_eq!(cache.get("img:b").unwrap().manifest_digest, "sha256:other");
    }

    #[test]
    fn corrupt_cache_file_reads_as_a_miss() {
        let d = tempfile::tempdir().unwrap();
        let cache = ManifestCache::new(d.path());
        std::fs::write(cache.path_for("img:a"), b"not json").unwrap();
        assert!(cache.get("img:a").is_none());
    }

    struct CountingRegistry {
        manifest_calls: Mutex<usize>,
    }

    impl Registry for CountingRegistry {
        async fn pull_manifest_and_config(
            &self,
            _reference: &Reference,
        ) -> Result<(OciImageManifest, String, String)> {
            *self.manifest_calls.lock().unwrap() += 1;
            Ok((
                sample_manifest(),
                "sha256:manifest".into(),
                r#"{"architecture":"arm64"}"#.into(),
            ))
        }

        async fn pull_blob(
            &self,
            _reference: &Reference,
            _descriptor: &OciDescriptor,
        ) -> Result<Vec<u8>> {
            Ok(vec![])
        }
    }

    const PINNED: &str =
        "ghcr.io/x/y@sha256:1111111111111111111111111111111111111111111111111111111111111111";

    #[tokio::test]
    async fn first_pull_of_a_digest_ref_hits_the_registry_then_warm_pull_skips_it() {
        let d = tempfile::tempdir().unwrap();
        let inner = CountingRegistry {
            manifest_calls: Mutex::new(0),
        };
        let caching = CachingRegistry::new(inner, ManifestCache::new(d.path()));
        let reference: Reference = PINNED.parse().unwrap();

        let (_, digest, _) = caching.pull_manifest_and_config(&reference).await.unwrap();
        assert_eq!(digest, "sha256:manifest");
        assert_eq!(*caching.inner.manifest_calls.lock().unwrap(), 1);

        let (_, digest2, _) = caching.pull_manifest_and_config(&reference).await.unwrap();
        assert_eq!(digest2, "sha256:manifest");
        assert_eq!(
            *caching.inner.manifest_calls.lock().unwrap(),
            1,
            "warm pull of a digest-pinned ref must be served from cache without hitting the registry"
        );
    }

    #[tokio::test]
    async fn tag_references_are_never_cached_and_always_re_resolve() {
        let d = tempfile::tempdir().unwrap();
        let caching = CachingRegistry::new(
            CountingRegistry {
                manifest_calls: Mutex::new(0),
            },
            ManifestCache::new(d.path()),
        );
        let reference: Reference = "ghcr.io/x/y:latest".parse().unwrap();

        caching.pull_manifest_and_config(&reference).await.unwrap();
        caching.pull_manifest_and_config(&reference).await.unwrap();
        assert_eq!(
            *caching.inner.manifest_calls.lock().unwrap(),
            2,
            "a mutable tag must re-resolve every pull so upstream republishes are picked up"
        );
        assert!(
            std::fs::read_dir(d.path()).unwrap().next().is_none(),
            "no manifest cache file may be written for a tag reference"
        );
    }

    #[tokio::test]
    async fn a_cache_write_failure_does_not_break_the_pull() {
        let d = tempfile::tempdir().unwrap();
        let blocked = d.path().join("blocked");
        std::fs::write(&blocked, b"i am a file, not a dir").unwrap();
        let caching = CachingRegistry::new(
            CountingRegistry {
                manifest_calls: Mutex::new(0),
            },
            ManifestCache::new(&blocked),
        );
        let reference: Reference = PINNED.parse().unwrap();
        let (_, digest, _) = caching
            .pull_manifest_and_config(&reference)
            .await
            .expect("an uncacheable manifest must still pull successfully");
        assert_eq!(digest, "sha256:manifest");
    }

    #[tokio::test]
    async fn pull_blob_always_delegates_to_the_inner_registry() {
        let d = tempfile::tempdir().unwrap();
        let caching = CachingRegistry::new(
            CountingRegistry {
                manifest_calls: Mutex::new(0),
            },
            ManifestCache::new(d.path()),
        );
        let reference: Reference = "ghcr.io/x/y:latest".parse().unwrap();
        let descriptor = OciDescriptor {
            digest: "sha256:aaaa".into(),
            ..Default::default()
        };
        assert!(
            caching
                .pull_blob(&reference, &descriptor)
                .await
                .unwrap()
                .is_empty()
        );
    }
}
