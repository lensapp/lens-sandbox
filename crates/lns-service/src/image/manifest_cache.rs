use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use lns_ipc::PullPolicy;
use oci_client::{Reference, manifest::OciImageManifest};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::Registry;

const TAG_FRESH_SECS: u64 = 60;

fn epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Serialize, Deserialize)]
pub(crate) struct CachedManifest {
    pub manifest: OciImageManifest,
    pub manifest_digest: String,
    pub config: String,
    #[serde(default)]
    pub tag_digest: Option<String>,
    #[serde(default)]
    pub cached_at: Option<u64>,
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

    pub fn remove(&self, reference: &str) -> Result<()> {
        match std::fs::remove_file(self.path_for(reference)) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                Err(e).context("removing manifest cache entry")
            }
            _ => Ok(()),
        }
    }
}

pub(crate) struct CachingRegistry<R: Registry> {
    inner: R,
    cache: ManifestCache,
    pull_policy: PullPolicy,
    now_secs: Arc<dyn Fn() -> u64 + Send + Sync>,
}

impl<R: Registry> CachingRegistry<R> {
    pub fn new(inner: R, cache: ManifestCache, pull_policy: PullPolicy) -> Self {
        Self {
            inner,
            cache,
            pull_policy,
            now_secs: Arc::new(epoch_secs),
        }
    }

    #[cfg(test)]
    fn with_clock(
        inner: R,
        cache: ManifestCache,
        pull_policy: PullPolicy,
        clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    ) -> Self {
        Self {
            inner,
            cache,
            pull_policy,
            now_secs: clock,
        }
    }
}

impl<R: Registry> Registry for CachingRegistry<R> {
    async fn pull_manifest_and_config(
        &self,
        reference: &Reference,
    ) -> Result<(OciImageManifest, String, String)> {
        let is_tag = reference.digest().is_none();
        let key = reference.whole();

        if self.pull_policy == PullPolicy::Always {
            let (manifest, manifest_digest, config) =
                self.inner.pull_manifest_and_config(reference).await?;
            let tag_digest = if is_tag {
                self.inner.fetch_tag_digest(reference).await.ok()
            } else {
                None
            };
            self.write_cache(&key, &manifest, &manifest_digest, &config, tag_digest);
            return Ok((manifest, manifest_digest, config));
        }

        if is_tag {
            return self.resolve_tag(reference, &key).await;
        }

        if let Some(c) = self.cache.get(&key) {
            return Ok((c.manifest, c.manifest_digest, c.config));
        }

        if self.pull_policy == PullPolicy::Never {
            anyhow::bail!("manifest for {key} is not cached and pull policy is `never`");
        }

        let (manifest, manifest_digest, config) =
            self.inner.pull_manifest_and_config(reference).await?;
        self.write_cache(&key, &manifest, &manifest_digest, &config, None);
        Ok((manifest, manifest_digest, config))
    }

    async fn pull_blob(
        &self,
        reference: &Reference,
        descriptor: &oci_client::manifest::OciDescriptor,
        on_chunk: &(dyn Fn(u64) + Send + Sync),
    ) -> Result<Vec<u8>> {
        self.inner.pull_blob(reference, descriptor, on_chunk).await
    }

    async fn fetch_tag_digest(&self, reference: &Reference) -> Result<String> {
        self.inner.fetch_tag_digest(reference).await
    }
}

impl<R: Registry> CachingRegistry<R> {
    async fn resolve_tag(
        &self,
        reference: &Reference,
        key: &str,
    ) -> Result<(OciImageManifest, String, String)> {
        let cached = self.cache.get(key);

        if self.pull_policy == PullPolicy::Never {
            return match cached {
                Some(c) => Ok((c.manifest, c.manifest_digest, c.config)),
                None => {
                    anyhow::bail!("image {key} is not cached and pull policy is `never`")
                }
            };
        }

        if let Some(c) = cached {
            let now = (self.now_secs)();
            let age = c
                .cached_at
                .map(|t| now.saturating_sub(t))
                .unwrap_or(u64::MAX);

            if age < TAG_FRESH_SECS {
                return Ok((c.manifest, c.manifest_digest, c.config));
            }

            let current_digest = self.inner.fetch_tag_digest(reference).await?;
            if c.tag_digest.as_deref() == Some(&current_digest) {
                self.write_cache(
                    key,
                    &c.manifest,
                    &c.manifest_digest,
                    &c.config,
                    Some(current_digest),
                );
                return Ok((c.manifest, c.manifest_digest, c.config));
            }

            let (manifest, manifest_digest, config) =
                self.inner.pull_manifest_and_config(reference).await?;
            self.write_cache(
                key,
                &manifest,
                &manifest_digest,
                &config,
                Some(current_digest),
            );
            return Ok((manifest, manifest_digest, config));
        }

        let (manifest, manifest_digest, config) =
            self.inner.pull_manifest_and_config(reference).await?;
        let tag_digest = self.inner.fetch_tag_digest(reference).await.ok();
        self.write_cache(key, &manifest, &manifest_digest, &config, tag_digest);
        Ok((manifest, manifest_digest, config))
    }

    fn write_cache(
        &self,
        key: &str,
        manifest: &OciImageManifest,
        manifest_digest: &str,
        config: &str,
        tag_digest: Option<String>,
    ) {
        let entry = CachedManifest {
            manifest: manifest.clone(),
            manifest_digest: manifest_digest.to_string(),
            config: config.to_string(),
            tag_digest,
            cached_at: Some((self.now_secs)()),
        };
        if let Err(e) = self.cache.put(key, &entry) {
            crate::log::warn!("manifest cache write failed for {key} ({e:#}); continuing");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oci_client::manifest::{OciDescriptor, OciImageManifest};
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, Ordering};

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
            tag_digest: None,
            cached_at: None,
        }
    }

    fn tag_entry(tag_digest: &str, cached_at: u64) -> CachedManifest {
        CachedManifest {
            manifest: sample_manifest(),
            manifest_digest: "sha256:manifest".into(),
            config: r#"{"architecture":"arm64"}"#.into(),
            tag_digest: Some(tag_digest.to_string()),
            cached_at: Some(cached_at),
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
    fn remove_deletes_a_stored_entry() {
        let d = tempfile::tempdir().unwrap();
        let cache = ManifestCache::new(d.path());
        cache.put("img:a", &entry()).unwrap();
        cache.remove("img:a").unwrap();
        assert!(cache.get("img:a").is_none());
    }

    #[test]
    fn remove_of_an_absent_entry_is_benign() {
        let d = tempfile::tempdir().unwrap();
        let cache = ManifestCache::new(d.path());
        cache.remove("img:never-stored").unwrap();
    }

    #[test]
    fn remove_surfaces_a_non_not_found_failure() {
        let d = tempfile::tempdir().unwrap();
        let cache = ManifestCache::new(d.path());
        std::fs::create_dir_all(cache.path_for("img:a").join("nested")).unwrap();
        let err = cache.remove("img:a").unwrap_err();
        assert!(
            format!("{err:#}").contains("removing manifest cache entry"),
            "got: {err:#}"
        );
    }

    #[test]
    fn corrupt_cache_file_reads_as_a_miss() {
        let d = tempfile::tempdir().unwrap();
        let cache = ManifestCache::new(d.path());
        std::fs::write(cache.path_for("img:a"), b"not json").unwrap();
        assert!(cache.get("img:a").is_none());
    }

    #[test]
    fn cached_manifest_without_tag_digest_or_cached_at_deserializes_with_defaults() {
        let json = serde_json::json!({
            "manifest": sample_manifest(),
            "manifest_digest": "sha256:old",
            "config": "{}"
        });
        let c: CachedManifest = serde_json::from_value(json).unwrap();
        assert!(c.tag_digest.is_none());
        assert!(c.cached_at.is_none());
    }

    struct FakeRegistry {
        manifest_calls: Mutex<usize>,
        digest_calls: Mutex<usize>,
        tag_digests: Mutex<VecDeque<String>>,
        manifests: Mutex<VecDeque<(OciImageManifest, String, String)>>,
    }

    impl FakeRegistry {
        fn new() -> Self {
            Self {
                manifest_calls: Mutex::new(0),
                digest_calls: Mutex::new(0),
                tag_digests: Mutex::new(VecDeque::new()),
                manifests: Mutex::new(VecDeque::new()),
            }
        }

        fn with_tag_digests(digests: impl IntoIterator<Item = &'static str>) -> Self {
            let r = Self::new();
            *r.tag_digests.lock().unwrap() = digests.into_iter().map(String::from).collect();
            r
        }
    }

    impl Registry for FakeRegistry {
        async fn pull_manifest_and_config(
            &self,
            _reference: &Reference,
        ) -> Result<(OciImageManifest, String, String)> {
            *self.manifest_calls.lock().unwrap() += 1;
            if let Some(m) = self.manifests.lock().unwrap().pop_front() {
                return Ok(m);
            }
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
            _on_chunk: &(dyn Fn(u64) + Send + Sync),
        ) -> Result<Vec<u8>> {
            Ok(vec![])
        }

        async fn fetch_tag_digest(&self, _reference: &Reference) -> Result<String> {
            *self.digest_calls.lock().unwrap() += 1;
            self.tag_digests
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| anyhow::anyhow!("no scripted tag digest"))
        }
    }

    fn fake_clock(secs: u64) -> Arc<dyn Fn() -> u64 + Send + Sync> {
        let clock = Arc::new(AtomicU64::new(secs));
        let c = Arc::clone(&clock);
        Arc::new(move || c.load(Ordering::Relaxed))
    }

    fn settable_clock() -> (Arc<AtomicU64>, Arc<dyn Fn() -> u64 + Send + Sync>) {
        let clock = Arc::new(AtomicU64::new(1000));
        let c = Arc::clone(&clock);
        (clock, Arc::new(move || c.load(Ordering::Relaxed)))
    }

    const PINNED: &str =
        "ghcr.io/x/y@sha256:1111111111111111111111111111111111111111111111111111111111111111";

    #[tokio::test]
    async fn first_pull_of_a_digest_ref_hits_the_registry_then_warm_pull_skips_it() {
        let d = tempfile::tempdir().unwrap();
        let caching = CachingRegistry::new(
            FakeRegistry::with_tag_digests(["sha256:idx"]),
            ManifestCache::new(d.path()),
            PullPolicy::Auto,
        );
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
    async fn tag_auto_cold_miss_pulls_and_caches_with_tag_digest() {
        let d = tempfile::tempdir().unwrap();
        let caching = CachingRegistry::with_clock(
            FakeRegistry::with_tag_digests(["sha256:idx"]),
            ManifestCache::new(d.path()),
            PullPolicy::Auto,
            fake_clock(1000),
        );
        let reference: Reference = "ghcr.io/x/y:latest".parse().unwrap();

        let (_, digest, _) = caching.pull_manifest_and_config(&reference).await.unwrap();
        assert_eq!(digest, "sha256:manifest");
        assert_eq!(*caching.inner.manifest_calls.lock().unwrap(), 1);
        assert_eq!(*caching.inner.digest_calls.lock().unwrap(), 1);

        let cached = caching.cache.get(&reference.whole()).unwrap();
        assert_eq!(cached.tag_digest.as_deref(), Some("sha256:idx"));
        assert_eq!(cached.cached_at, Some(1000));
    }

    #[tokio::test]
    async fn tag_auto_fresh_serves_cache_zero_network() {
        let d = tempfile::tempdir().unwrap();
        let (clock, clock_fn) = settable_clock();
        let reference: Reference = "ghcr.io/x/y:latest".parse().unwrap();
        let cache = ManifestCache::new(d.path());
        cache
            .put(&reference.whole(), &tag_entry("sha256:idx", 1000))
            .unwrap();

        let caching =
            CachingRegistry::with_clock(FakeRegistry::new(), cache, PullPolicy::Auto, clock_fn);
        clock.store(1030, Ordering::Relaxed);

        let (_, digest, _) = caching.pull_manifest_and_config(&reference).await.unwrap();
        assert_eq!(digest, "sha256:manifest");
        assert_eq!(
            *caching.inner.manifest_calls.lock().unwrap(),
            0,
            "fresh tag must serve from cache without pulling"
        );
        assert_eq!(
            *caching.inner.digest_calls.lock().unwrap(),
            0,
            "fresh tag must not call fetch_tag_digest"
        );
    }

    #[tokio::test]
    async fn tag_auto_stale_unchanged_revalidates_with_head_only() {
        let d = tempfile::tempdir().unwrap();
        let (clock, clock_fn) = settable_clock();
        let reference: Reference = "ghcr.io/x/y:latest".parse().unwrap();
        let cache = ManifestCache::new(d.path());
        cache
            .put(&reference.whole(), &tag_entry("sha256:idx", 1000))
            .unwrap();

        let caching = CachingRegistry::with_clock(
            FakeRegistry::with_tag_digests(["sha256:idx"]),
            cache,
            PullPolicy::Auto,
            clock_fn,
        );
        clock.store(1061, Ordering::Relaxed);

        let (_, digest, _) = caching.pull_manifest_and_config(&reference).await.unwrap();
        assert_eq!(digest, "sha256:manifest");
        assert_eq!(
            *caching.inner.digest_calls.lock().unwrap(),
            1,
            "stale tag must call fetch_tag_digest exactly once"
        );
        assert_eq!(
            *caching.inner.manifest_calls.lock().unwrap(),
            0,
            "unchanged digest must not trigger a full pull"
        );

        let refreshed = caching.cache.get(&reference.whole()).unwrap();
        assert_eq!(
            refreshed.cached_at,
            Some(1061),
            "cached_at must be refreshed after successful revalidation"
        );
    }

    #[tokio::test]
    async fn tag_auto_stale_changed_does_head_then_full_pull() {
        let d = tempfile::tempdir().unwrap();
        let (clock, clock_fn) = settable_clock();
        let reference: Reference = "ghcr.io/x/y:latest".parse().unwrap();
        let cache = ManifestCache::new(d.path());
        cache
            .put(&reference.whole(), &tag_entry("sha256:old-idx", 1000))
            .unwrap();

        let mut fresh_manifest = sample_manifest();
        fresh_manifest.layers[0].digest = "sha256:bbbb".into();

        let fake = FakeRegistry::with_tag_digests(["sha256:new-idx"]);
        *fake.manifests.lock().unwrap() = VecDeque::from([(
            fresh_manifest.clone(),
            "sha256:fresh-manifest".to_string(),
            r#"{"architecture":"amd64"}"#.to_string(),
        )]);

        let caching = CachingRegistry::with_clock(fake, cache, PullPolicy::Auto, clock_fn);
        clock.store(1061, Ordering::Relaxed);

        let (manifest, digest, config) =
            caching.pull_manifest_and_config(&reference).await.unwrap();
        assert_eq!(digest, "sha256:fresh-manifest");
        assert_eq!(config, r#"{"architecture":"amd64"}"#);
        assert_eq!(manifest.layers[0].digest, "sha256:bbbb");
        assert_eq!(
            *caching.inner.digest_calls.lock().unwrap(),
            1,
            "must call fetch_tag_digest once to check staleness"
        );
        assert_eq!(
            *caching.inner.manifest_calls.lock().unwrap(),
            1,
            "changed digest must trigger exactly one full pull"
        );

        let updated = caching.cache.get(&reference.whole()).unwrap();
        assert_eq!(updated.tag_digest.as_deref(), Some("sha256:new-idx"));
        assert_eq!(updated.cached_at, Some(1061));
    }

    #[tokio::test]
    async fn tag_always_always_pulls_and_writes_cache() {
        let d = tempfile::tempdir().unwrap();
        let reference: Reference = "ghcr.io/x/y:latest".parse().unwrap();
        let cache = ManifestCache::new(d.path());
        cache
            .put(&reference.whole(), &tag_entry("sha256:idx", 1000))
            .unwrap();

        let caching = CachingRegistry::with_clock(
            FakeRegistry::with_tag_digests(["sha256:new-idx", "sha256:new-idx2"]),
            cache,
            PullPolicy::Always,
            fake_clock(2000),
        );

        caching.pull_manifest_and_config(&reference).await.unwrap();
        caching.pull_manifest_and_config(&reference).await.unwrap();
        assert_eq!(
            *caching.inner.manifest_calls.lock().unwrap(),
            2,
            "policy=always must hit the registry every time for tags"
        );

        let cached = caching.cache.get(&reference.whole()).unwrap();
        assert!(cached.tag_digest.is_some());
        assert_eq!(cached.cached_at, Some(2000));
    }

    #[tokio::test]
    async fn tag_never_uncached_bails_with_clear_error() {
        let d = tempfile::tempdir().unwrap();
        let caching = CachingRegistry::new(
            FakeRegistry::new(),
            ManifestCache::new(d.path()),
            PullPolicy::Never,
        );
        let reference: Reference = "ghcr.io/x/y:latest".parse().unwrap();
        let err = caching
            .pull_manifest_and_config(&reference)
            .await
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("not cached") && msg.contains("never"),
            "expected clear error, got: {msg}"
        );
        assert_eq!(*caching.inner.manifest_calls.lock().unwrap(), 0);
        assert_eq!(*caching.inner.digest_calls.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn tag_never_cached_serves_from_cache_zero_network() {
        let d = tempfile::tempdir().unwrap();
        let reference: Reference = "ghcr.io/x/y:latest".parse().unwrap();
        let cache = ManifestCache::new(d.path());
        cache
            .put(&reference.whole(), &tag_entry("sha256:idx", 500))
            .unwrap();

        let caching = CachingRegistry::new(FakeRegistry::new(), cache, PullPolicy::Never);

        let (_, digest, _) = caching.pull_manifest_and_config(&reference).await.unwrap();
        assert_eq!(digest, "sha256:manifest");
        assert_eq!(
            *caching.inner.manifest_calls.lock().unwrap(),
            0,
            "policy=never must not pull"
        );
        assert_eq!(
            *caching.inner.digest_calls.lock().unwrap(),
            0,
            "policy=never must not call fetch_tag_digest"
        );
    }

    #[tokio::test]
    async fn digest_pin_auto_never_calls_fetch_tag_digest_even_when_old() {
        let d = tempfile::tempdir().unwrap();
        let reference: Reference = PINNED.parse().unwrap();
        let cache = ManifestCache::new(d.path());
        let mut old_entry = entry();
        old_entry.cached_at = Some(0);
        cache.put(&reference.whole(), &old_entry).unwrap();

        let caching = CachingRegistry::with_clock(
            FakeRegistry::new(),
            cache,
            PullPolicy::Auto,
            fake_clock(999_999),
        );

        let (_, digest, _) = caching.pull_manifest_and_config(&reference).await.unwrap();
        assert_eq!(digest, "sha256:manifest");
        assert_eq!(
            *caching.inner.manifest_calls.lock().unwrap(),
            0,
            "digest-pinned ref must always serve from cache under Auto"
        );
        assert_eq!(
            *caching.inner.digest_calls.lock().unwrap(),
            0,
            "digest-pinned ref must never call fetch_tag_digest"
        );
    }

    #[tokio::test]
    async fn legacy_cache_entry_without_tag_digest_or_cached_at_treated_as_stale() {
        let d = tempfile::tempdir().unwrap();
        let reference: Reference = "ghcr.io/x/y:latest".parse().unwrap();
        let cache = ManifestCache::new(d.path());
        let legacy = CachedManifest {
            manifest: sample_manifest(),
            manifest_digest: "sha256:manifest".into(),
            config: r#"{"architecture":"arm64"}"#.into(),
            tag_digest: None,
            cached_at: None,
        };
        cache.put(&reference.whole(), &legacy).unwrap();

        let caching = CachingRegistry::with_clock(
            FakeRegistry::with_tag_digests(["sha256:fresh-idx"]),
            cache,
            PullPolicy::Auto,
            fake_clock(5000),
        );

        let (_, digest, _) = caching.pull_manifest_and_config(&reference).await.unwrap();
        assert_eq!(digest, "sha256:manifest");
        assert_eq!(
            *caching.inner.digest_calls.lock().unwrap(),
            1,
            "legacy entry (no cached_at) must be treated as stale and revalidated"
        );
        // tag_digest was None so comparison fails → full pull
        assert_eq!(
            *caching.inner.manifest_calls.lock().unwrap(),
            1,
            "legacy entry with no tag_digest cannot match HEAD → full pull"
        );
    }

    #[tokio::test]
    async fn policy_never_bails_when_digest_ref_is_not_cached() {
        let d = tempfile::tempdir().unwrap();
        let caching = CachingRegistry::new(
            FakeRegistry::new(),
            ManifestCache::new(d.path()),
            PullPolicy::Never,
        );
        let reference: Reference = PINNED.parse().unwrap();
        let err = caching
            .pull_manifest_and_config(&reference)
            .await
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("not cached") && msg.contains("never"),
            "expected a `not cached` + `never` error, got: {msg}"
        );
        assert_eq!(
            *caching.inner.manifest_calls.lock().unwrap(),
            0,
            "policy=never must not hit the registry"
        );
    }

    #[tokio::test]
    async fn policy_never_serves_a_cached_digest_ref() {
        let d = tempfile::tempdir().unwrap();
        let cache = ManifestCache::new(d.path());
        cache
            .put(&PINNED.parse::<Reference>().unwrap().whole(), &entry())
            .unwrap();
        let caching = CachingRegistry::new(FakeRegistry::new(), cache, PullPolicy::Never);
        let reference: Reference = PINNED.parse().unwrap();
        let (_, digest, _) = caching.pull_manifest_and_config(&reference).await.unwrap();
        assert_eq!(digest, "sha256:manifest");
        assert_eq!(
            *caching.inner.manifest_calls.lock().unwrap(),
            0,
            "policy=never with warm cache must not hit the registry"
        );
    }

    #[tokio::test]
    async fn policy_always_bypasses_cache_and_re_pulls_digest_ref() {
        let d = tempfile::tempdir().unwrap();
        let cache = ManifestCache::new(d.path());
        cache
            .put(&PINNED.parse::<Reference>().unwrap().whole(), &entry())
            .unwrap();
        let caching = CachingRegistry::new(
            FakeRegistry::with_tag_digests(["sha256:x"]),
            cache,
            PullPolicy::Always,
        );
        let reference: Reference = PINNED.parse().unwrap();
        let (_, digest, _) = caching.pull_manifest_and_config(&reference).await.unwrap();
        assert_eq!(digest, "sha256:manifest");
        assert_eq!(
            *caching.inner.manifest_calls.lock().unwrap(),
            1,
            "policy=always must hit the registry even when a cache entry exists"
        );
    }

    #[tokio::test]
    async fn a_cache_write_failure_does_not_break_the_pull() {
        let d = tempfile::tempdir().unwrap();
        let blocked = d.path().join("blocked");
        std::fs::write(&blocked, b"i am a file, not a dir").unwrap();
        let caching = CachingRegistry::new(
            FakeRegistry::new(),
            ManifestCache::new(&blocked),
            PullPolicy::Auto,
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
            FakeRegistry::new(),
            ManifestCache::new(d.path()),
            PullPolicy::Auto,
        );
        let reference: Reference = "ghcr.io/x/y:latest".parse().unwrap();
        let descriptor = OciDescriptor {
            digest: "sha256:aaaa".into(),
            ..Default::default()
        };
        assert!(
            caching
                .pull_blob(&reference, &descriptor, &|_| {})
                .await
                .unwrap()
                .is_empty()
        );
    }
}
