use anyhow::{Context, Result};
use oci_client::{
    Reference,
    manifest::{self, OciDescriptor, OciImageManifest},
};
use sha2::{Digest, Sha256};

use crate::log;
use crate::oci_layer_cache::LayerCache;

pub(crate) mod manifest_cache;
mod real;
pub use real::pull;

pub(crate) trait Registry: Send + Sync {
    fn pull_manifest_and_config(
        &self,
        reference: &Reference,
    ) -> impl std::future::Future<Output = Result<(OciImageManifest, String, String)>> + Send;

    fn pull_blob(
        &self,
        reference: &Reference,
        descriptor: &OciDescriptor,
    ) -> impl std::future::Future<Output = Result<Vec<u8>>> + Send;
}

#[cfg(target_arch = "aarch64")]
fn want_arch() -> oci_spec::image::Arch {
    oci_spec::image::Arch::ARM64
}

#[cfg(not(target_arch = "aarch64"))]
fn want_arch() -> oci_spec::image::Arch {
    oci_spec::image::Arch::Amd64
}

pub(crate) fn linux_platform_resolver(
    manifests: &[oci_client::manifest::ImageIndexEntry],
) -> Option<String> {
    let want = want_arch();
    manifests
        .iter()
        .find(|m| {
            !m.media_type.contains("attestation")
                && m.platform.as_ref().is_some_and(|p| {
                    matches!(p.os, oci_spec::image::Os::Linux) && p.architecture == want
                })
        })
        .map(|m| m.digest.clone())
}

const GIB: u64 = 1024 * 1024 * 1024;

// Reject untrusted manifests whose summed declared layer sizes alone would exhaust host disk, before any blob is fetched.
const MAX_TOTAL_DECLARED_LAYER_BYTES: u64 = 64 * GIB;

// An honest image manifest/config is kilobytes; we refuse to handle a multi-MiB document post-fetch, since that's a DoS attempt rather than a real image.
const MAX_MANIFEST_DOC_BYTES: usize = 4 * 1024 * 1024;

#[derive(Default)]
struct ByteCounter(usize);

impl std::io::Write for ByteCounter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0 += buf.len();
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(crate) fn serialized_len<T: serde::Serialize>(value: &T) -> Result<usize> {
    let mut counter = ByteCounter::default();
    serde_json::to_writer(&mut counter, value).context("measuring serialized size")?;
    Ok(counter.0)
}

pub(crate) fn enforce_manifest_doc_size(
    reference: &str,
    manifest_bytes: usize,
    config_bytes: usize,
) -> Result<()> {
    if manifest_bytes > MAX_MANIFEST_DOC_BYTES {
        anyhow::bail!(
            "manifest for {reference} is {manifest_bytes} bytes, over the \
             {MAX_MANIFEST_DOC_BYTES}-byte cap"
        );
    }
    if config_bytes > MAX_MANIFEST_DOC_BYTES {
        anyhow::bail!(
            "image config for {reference} is {config_bytes} bytes, over the \
             {MAX_MANIFEST_DOC_BYTES}-byte cap"
        );
    }
    Ok(())
}

#[derive(Debug)]
pub struct PulledImage {
    pub reference: Reference,
    pub digest: String,
    pub layers: Vec<oci_client::client::ImageLayer>,
    pub config: oci_client::config::ConfigFile,
    pub layer_digests: Vec<String>,
}

#[allow(clippy::cognitive_complexity)] // manifest fetch → digest verify → per-layer parallel pull → diff_id check
pub(crate) async fn pull_inner<R: Registry>(
    client: &R,
    image: &str,
    layer_cache: &LayerCache,
) -> Result<PulledImage> {
    let reference: Reference = image
        .parse()
        .with_context(|| format!("invalid image reference: {image}"))?;

    let (manifest, manifest_digest, config_str) =
        client.pull_manifest_and_config(&reference).await?;

    let config: oci_client::config::ConfigFile =
        serde_json::from_str(&config_str).context("parsing image config")?;

    if let Some(expected) = reference.digest() {
        let raw = manifest_bytes(&manifest)?;
        let actual = format!("sha256:{}", hex::encode(Sha256::digest(&raw)));
        if !ct_digest_eq(&actual, expected) {
            anyhow::bail!(
                "manifest digest mismatch for {image} — expected {expected}, \
                 received {actual}"
            );
        }
    }

    if config.rootfs.diff_ids.len() != manifest.layers.len() {
        anyhow::bail!(
            "image config declares {} diff_ids but manifest has {} layers",
            config.rootfs.diff_ids.len(),
            manifest.layers.len()
        );
    }

    let accepted: &[&str] = &[
        manifest::IMAGE_LAYER_GZIP_MEDIA_TYPE,
        manifest::IMAGE_DOCKER_LAYER_GZIP_MEDIA_TYPE,
        manifest::IMAGE_LAYER_MEDIA_TYPE,
    ];

    for (i, descriptor) in manifest.layers.iter().enumerate() {
        if !accepted.contains(&descriptor.media_type.as_str()) {
            anyhow::bail!(
                "layer {i} has unsupported media type {} (digest {})",
                descriptor.media_type,
                descriptor.digest
            );
        }
    }

    let total_bytes: u64 = manifest
        .layers
        .iter()
        .map(|d| d.size.max(0) as u64)
        .fold(0u64, u64::saturating_add);
    if total_bytes > MAX_TOTAL_DECLARED_LAYER_BYTES {
        anyhow::bail!(
            "image {image} declares {total_bytes} bytes of layers, over the \
             {MAX_TOTAL_DECLARED_LAYER_BYTES}-byte ceiling"
        );
    }
    let mut was_cached = Vec::with_capacity(manifest.layers.len());
    for d in &manifest.layers {
        was_cached.push(layer_cache.contains(&d.digest)?);
    }
    let any_missing = was_cached.iter().any(|cached| !cached);
    let n = manifest.layers.len();
    let plural = if n == 1 { "" } else { "s" };
    let total_bytes_fmt = format_bytes(total_bytes);
    if any_missing {
        log::info!("Resolved", "{image} @ {manifest_digest}");
    } else {
        log::info!(
            "ImageCached",
            "{image} ({n} layer{plural}, {total_bytes_fmt})"
        );
    }
    let pull_start = std::time::Instant::now();

    // `buffered` preserves manifest order — `buffer_unordered` caused the digest-mismatch bug fixed in 608bda56.
    use futures_util::stream::{self, StreamExt, TryStreamExt};
    const MAX_CONCURRENT_LAYER_FETCHES: usize = 4;
    let reference_ref = &reference;
    let layers_owned: Vec<(usize, oci_client::manifest::OciDescriptor)> =
        manifest.layers.iter().cloned().enumerate().collect();
    let bytes_per_layer: Vec<Vec<u8>> = stream::iter(layers_owned)
        .map(|(i, descriptor)| async move {
            let layer_idx = i + 1;
            let short = short_digest(&descriptor.digest);
            let size = descriptor.size;
            log::debug!(
                layer = layer_idx,
                of = n,
                digest = short,
                size = size,
                "fetch-or-cache",
            );
            let digest = descriptor.digest.clone();
            layer_cache
                .get_or_install(&digest, || async move {
                    client.pull_blob(reference_ref, &descriptor).await
                })
                .await
                .with_context(|| format!("resolving layer {i} (digest {digest})"))
        })
        .buffered(MAX_CONCURRENT_LAYER_FETCHES)
        .try_collect()
        .await?;

    if any_missing {
        let elapsed_s = pull_start.elapsed().as_secs_f64();
        log::info!(
            "Pulled",
            "{n} layer{plural}   ({elapsed_s:.2}s · {total_bytes_fmt})"
        );
    }

    // Cached layers were DiffID-verified on their cold pull; their decompressed content is pinned by the already-verified compressed digest, so re-decompressing them every run is pure waste.
    let diff_id_futures = bytes_per_layer
        .iter()
        .enumerate()
        .filter(|(i, _)| !was_cached[*i])
        .map(|(i, bytes)| {
            let descriptor = &manifest.layers[i];
            let expected_diff_id = config.rootfs.diff_ids[i].clone();
            async move {
                let actual_diff_id = compute_diff_id(bytes, &descriptor.media_type)
                    .with_context(|| format!("computing DiffID for layer {i}"))?;
                if !ct_digest_eq(&actual_diff_id, &expected_diff_id) {
                    anyhow::bail!(
                        "layer {i} DiffID mismatch: image config says {expected_diff_id}, \
                     actual {actual_diff_id}"
                    );
                }
                Ok::<(), anyhow::Error>(())
            }
        });
    futures_util::future::try_join_all(diff_id_futures).await?;

    let mut layers: Vec<oci_client::client::ImageLayer> = Vec::with_capacity(n);
    let mut layer_digests: Vec<String> = Vec::with_capacity(n);
    for (descriptor, bytes) in manifest.layers.iter().zip(bytes_per_layer) {
        layers.push(oci_client::client::ImageLayer::new(
            bytes,
            descriptor.media_type.clone(),
            descriptor.annotations.clone(),
        ));
        layer_digests.push(descriptor.digest.clone());
    }

    Ok(PulledImage {
        reference,
        digest: manifest_digest,
        layers,
        config,
        layer_digests,
    })
}

fn format_bytes(n: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;
    if n >= GIB {
        format!("{:.1} GiB", n as f64 / GIB as f64)
    } else if n >= MIB {
        format!("{:.1} MiB", n as f64 / MIB as f64)
    } else if n >= KIB {
        format!("{:.1} KiB", n as f64 / KIB as f64)
    } else {
        format!("{n} B")
    }
}

fn short_digest(digest: &str) -> String {
    let prefix_end = digest.find(':').map(|i| i + 1).unwrap_or(0);
    let hex_part = &digest[prefix_end..];
    let take = hex_part.len().min(10);
    format!("{}{}…", &digest[..prefix_end], &hex_part[..take])
}

fn manifest_bytes(manifest: &oci_client::manifest::OciImageManifest) -> Result<Vec<u8>> {
    serde_json::to_vec(manifest).context("serializing manifest for digest check")
}

fn compute_diff_id(compressed: &[u8], media_type: &str) -> Result<String> {
    let mut hasher = Sha256::new();
    let is_gzip = media_type.contains("gzip");
    if is_gzip {
        use std::io::Read;
        let mut decoder = flate2::read::GzDecoder::new(std::io::Cursor::new(compressed));
        let mut buf = [0u8; 64 * 1024];
        loop {
            let n = decoder.read(&mut buf).context("decompressing layer")?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
    } else {
        hasher.update(compressed);
    }
    Ok(format!("sha256:{}", hex::encode(hasher.finalize())))
}

fn ct_digest_eq(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use oci_client::config::{ConfigFile, Rootfs};
    use oci_client::manifest::{OciDescriptor, OciImageManifest};
    use std::io::Write;
    use std::sync::Mutex;
    use tempfile::TempDir;

    fn gzip(bytes: &[u8]) -> Vec<u8> {
        let mut e = GzEncoder::new(Vec::new(), Compression::default());
        e.write_all(bytes).unwrap();
        e.finish().unwrap()
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
    }

    fn cache() -> (TempDir, LayerCache) {
        let dir = tempfile::tempdir().unwrap();
        let c = LayerCache::new(dir.path().to_path_buf());
        (dir, c)
    }

    fn ensure_global_trace_subscriber() {
        use std::sync::Once;
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            let subscriber = tracing_subscriber::fmt()
                .with_writer(std::io::sink)
                .with_ansi(false)
                .with_max_level(tracing::Level::TRACE)
                .finish();
            let _ = tracing::subscriber::set_global_default(subscriber);
        });
    }

    #[derive(Default)]
    struct VerbCapture {
        verbs: Mutex<Vec<String>>,
    }

    impl tracing::field::Visit for &VerbCapture {
        fn record_debug(&mut self, _: &tracing::field::Field, _: &dyn std::fmt::Debug) {}
        fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
            if field.name() == "verb" {
                self.verbs.lock().unwrap().push(value.to_string());
            }
        }
    }

    struct VerbLayer(std::sync::Arc<VerbCapture>);

    impl<S> tracing_subscriber::Layer<S> for VerbLayer
    where
        S: tracing::Subscriber,
    {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            if event.metadata().target() != crate::log::TARGET {
                return;
            }
            let mut visitor = &*self.0;
            event.record(&mut visitor);
        }
    }

    async fn capture_verbs<F, Fut, R>(f: F) -> (R, Vec<String>)
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = R>,
    {
        use tracing_subscriber::layer::SubscriberExt;
        let capture = std::sync::Arc::new(VerbCapture::default());
        let subscriber =
            tracing_subscriber::registry().with(VerbLayer(std::sync::Arc::clone(&capture)));
        let guard = tracing::subscriber::set_default(subscriber);
        let result = f().await;
        drop(guard);
        let verbs = capture.verbs.lock().unwrap().clone();
        (result, verbs)
    }

    fn build_two_layer_image() -> FakeImage {
        let layer_a_raw = b"layer-a-tarball-bytes".to_vec();
        let layer_b_raw = b"layer-b-tarball-bytes".to_vec();
        let layer_a_gz = gzip(&layer_a_raw);
        let layer_b_gz = gzip(&layer_b_raw);
        let layer_a_digest = sha256_hex(&layer_a_gz);
        let layer_b_digest = sha256_hex(&layer_b_gz);
        let diff_a = sha256_hex(&layer_a_raw);
        let diff_b = sha256_hex(&layer_b_raw);
        let manifest = OciImageManifest {
            layers: vec![
                OciDescriptor {
                    digest: layer_a_digest.clone(),
                    size: layer_a_gz.len() as i64,
                    media_type: manifest::IMAGE_LAYER_GZIP_MEDIA_TYPE.into(),
                    ..Default::default()
                },
                OciDescriptor {
                    digest: layer_b_digest.clone(),
                    size: layer_b_gz.len() as i64,
                    media_type: manifest::IMAGE_LAYER_GZIP_MEDIA_TYPE.into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let config = ConfigFile {
            architecture: "arm64".into(),
            os: "linux".into(),
            rootfs: Rootfs {
                r#type: "layers".into(),
                diff_ids: vec![diff_a, diff_b],
            },
            ..Default::default()
        };
        FakeImage {
            manifest,
            config_json: serde_json::to_string(&config).unwrap(),
            manifest_digest: "sha256:not-used-unless-pull-by-digest".to_string(),
            blobs: vec![(layer_a_digest, layer_a_gz), (layer_b_digest, layer_b_gz)],
        }
    }

    struct FakeImage {
        manifest: OciImageManifest,
        config_json: String,
        manifest_digest: String,
        blobs: Vec<(String, Vec<u8>)>,
    }

    impl FakeImage {
        fn into_registry(self) -> FakeRegistry {
            FakeRegistry {
                manifest: self.manifest,
                config_json: self.config_json,
                manifest_digest: self.manifest_digest,
                blobs: self.blobs,
                calls: Mutex::new(Vec::new()),
            }
        }
    }

    struct FakeRegistry {
        manifest: OciImageManifest,
        config_json: String,
        manifest_digest: String,
        blobs: Vec<(String, Vec<u8>)>,
        calls: Mutex<Vec<String>>,
    }

    impl Registry for FakeRegistry {
        async fn pull_manifest_and_config(
            &self,
            _reference: &Reference,
        ) -> Result<(OciImageManifest, String, String)> {
            self.calls.lock().unwrap().push("manifest".into());
            Ok((
                self.manifest.clone(),
                self.manifest_digest.clone(),
                self.config_json.clone(),
            ))
        }

        async fn pull_blob(
            &self,
            _reference: &Reference,
            descriptor: &OciDescriptor,
        ) -> Result<Vec<u8>> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("blob:{}", descriptor.digest));
            let blob = self
                .blobs
                .iter()
                .find(|(d, _)| d == &descriptor.digest)
                .map(|(_, b)| b.clone());
            blob.ok_or_else(|| anyhow::anyhow!("no canned blob for {}", descriptor.digest))
        }
    }

    #[test]
    fn want_arch_returns_arm64_or_amd64_for_known_hosts() {
        let arch = want_arch();
        assert!(matches!(
            arch,
            oci_spec::image::Arch::ARM64 | oci_spec::image::Arch::Amd64
        ));
    }

    #[test]
    fn linux_platform_resolver_picks_linux_host_arch_skipping_attestations() {
        use oci_client::manifest::{ImageIndexEntry, Platform};
        use oci_spec::image::Os;
        let want = want_arch();
        fn entry(digest: &str, media_type: &str, platform: Option<Platform>) -> ImageIndexEntry {
            ImageIndexEntry {
                digest: digest.into(),
                size: 1,
                media_type: media_type.into(),
                platform,
                annotations: None,
            }
        }
        fn linux_with(arch: oci_spec::image::Arch) -> Platform {
            Platform {
                architecture: arch,
                os: Os::Linux,
                os_version: None,
                os_features: None,
                variant: None,
                features: None,
            }
        }
        let entries = vec![
            entry(
                "sha256:attest",
                "application/vnd.in-toto+json+attestation",
                Some(linux_with(want.clone())),
            ),
            entry(
                "sha256:windows",
                "application/vnd.oci.image.manifest.v1+json",
                Some(Platform {
                    architecture: want.clone(),
                    os: Os::Windows,
                    os_version: None,
                    os_features: None,
                    variant: None,
                    features: None,
                }),
            ),
            entry(
                "sha256:linux-otherarch",
                "application/vnd.oci.image.manifest.v1+json",
                Some(linux_with(oci_spec::image::Arch::Other("riscv64".into()))),
            ),
            entry(
                "sha256:linux-correct",
                "application/vnd.oci.image.manifest.v1+json",
                Some(linux_with(want)),
            ),
        ];
        assert_eq!(
            linux_platform_resolver(&entries),
            Some("sha256:linux-correct".to_string())
        );
    }

    #[test]
    fn linux_platform_resolver_returns_none_when_no_match() {
        assert_eq!(linux_platform_resolver(&[]), None);
    }

    #[test]
    fn linux_platform_resolver_skips_entries_without_platform() {
        use oci_client::manifest::ImageIndexEntry;
        let entries = vec![ImageIndexEntry {
            digest: "sha256:no-platform".into(),
            size: 1,
            media_type: "application/vnd.oci.image.manifest.v1+json".into(),
            platform: None,
            annotations: None,
        }];
        assert_eq!(linux_platform_resolver(&entries), None);
    }

    #[test]
    fn format_bytes_picks_largest_unit_at_1024_boundary() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1024), "1.0 KiB");
        assert_eq!(format_bytes(1024 * 1024 - 1), "1024.0 KiB");
        assert_eq!(format_bytes(1024 * 1024), "1.0 MiB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.0 GiB");
        assert_eq!(format_bytes(5_400_000_000), "5.0 GiB");
    }

    #[test]
    fn enforce_manifest_doc_size_accepts_small_documents() {
        enforce_manifest_doc_size("alpine:3.20", 2048, 4096).unwrap();
        enforce_manifest_doc_size(
            "alpine:3.20",
            MAX_MANIFEST_DOC_BYTES,
            MAX_MANIFEST_DOC_BYTES,
        )
        .unwrap();
    }

    #[test]
    fn enforce_manifest_doc_size_rejects_an_oversized_manifest() {
        let err =
            enforce_manifest_doc_size("alpine:3.20", MAX_MANIFEST_DOC_BYTES + 1, 10).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("manifest for alpine:3.20"), "got: {msg}");
        assert!(msg.contains("over the"), "got: {msg}");
    }

    #[test]
    fn enforce_manifest_doc_size_rejects_an_oversized_config() {
        let err =
            enforce_manifest_doc_size("alpine:3.20", 10, MAX_MANIFEST_DOC_BYTES + 1).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("image config for alpine:3.20"), "got: {msg}");
        assert!(msg.contains("over the"), "got: {msg}");
    }

    #[test]
    fn manifest_bytes_round_trips_through_serde() {
        let manifest = OciImageManifest {
            layers: vec![OciDescriptor {
                digest: "sha256:abcd".into(),
                size: 7,
                media_type: manifest::IMAGE_LAYER_GZIP_MEDIA_TYPE.into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let bytes = manifest_bytes(&manifest).unwrap();
        let back: OciImageManifest = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back.layers.len(), 1);
        assert_eq!(back.layers[0].digest, "sha256:abcd");
    }

    #[test]
    fn serialized_len_matches_a_full_serialization_without_allocating_it() {
        let manifest = OciImageManifest {
            layers: vec![OciDescriptor {
                digest: "sha256:abcd".into(),
                size: 7,
                media_type: manifest::IMAGE_LAYER_GZIP_MEDIA_TYPE.into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let counted = serialized_len(&manifest).unwrap();
        let allocated = serde_json::to_vec(&manifest).unwrap().len();
        assert_eq!(counted, allocated);
    }

    #[test]
    fn byte_counter_tallies_writes_and_flushes_clean() {
        use std::io::Write;
        let mut counter = ByteCounter::default();
        assert_eq!(counter.write(b"hello").unwrap(), 5);
        assert_eq!(counter.write(b" world").unwrap(), 6);
        counter.flush().unwrap();
        assert_eq!(counter.0, 11);
    }

    #[test]
    fn compute_diff_id_returns_uncompressed_sha256_for_gzip() {
        let body = b"hello composefs DiffID";
        let compressed = gzip(body);
        let expected = sha256_hex(body);
        let actual =
            compute_diff_id(&compressed, "application/vnd.oci.image.layer.v1.tar+gzip").unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn compute_diff_id_returns_raw_sha256_for_non_gzip() {
        let body = b"already uncompressed tarball";
        let expected = sha256_hex(body);
        let actual = compute_diff_id(body, "application/vnd.oci.image.layer.v1.tar").unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn compute_diff_id_streams_large_body_in_chunks() {
        let body: Vec<u8> = (0..200_000).map(|i| (i % 251) as u8).collect();
        let compressed = gzip(&body);
        let expected = sha256_hex(&body);
        let actual =
            compute_diff_id(&compressed, "application/vnd.oci.image.layer.v1.tar+gzip").unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn compute_diff_id_rejects_corrupt_gzip() {
        let body = b"the quick brown fox jumps over the lazy dog";
        let mut compressed = gzip(body);
        compressed.truncate(compressed.len() - 5);
        let err = compute_diff_id(&compressed, "application/vnd.oci.image.layer.v1.tar+gzip")
            .unwrap_err();
        assert!(format!("{err:#}").contains("decompressing"));
    }

    #[test]
    fn ct_digest_eq_length_sensitive_and_value_correct() {
        assert!(ct_digest_eq("sha256:abc", "sha256:abc"));
        assert!(!ct_digest_eq("sha256:abc", "sha256:abd"));
        assert!(!ct_digest_eq("sha256:abc", "sha256:abcd"));
        assert!(ct_digest_eq("", ""));
    }

    #[test]
    fn short_digest_truncates_to_10_hex_chars() {
        assert_eq!(
            short_digest("sha256:c34f34b93598ddcd4235ae902f7a59bed95"),
            "sha256:c34f34b935…"
        );
        assert_eq!(short_digest("sha256:abc"), "sha256:abc…");
        assert_eq!(short_digest("c34f34b9359800"), "c34f34b935…");
    }

    #[tokio::test]
    async fn pull_inner_happy_path_returns_layers_in_manifest_order() {
        ensure_global_trace_subscriber();
        let image = build_two_layer_image();
        let registry = image.into_registry();
        let (_dir, cache) = cache();
        let pulled = pull_inner(&registry, "alpine:3.20", &cache).await.unwrap();
        let calls: Vec<String> = registry.calls.lock().unwrap().clone();
        assert_eq!(pulled.layers.len(), 2, "both layers present");
        assert_eq!(pulled.layer_digests.len(), 2, "digests parallel to layers");
        assert_eq!(calls[0], "manifest", "manifest fetch happens first");
        assert!(calls[1].starts_with("blob:"));
        assert!(calls[2].starts_with("blob:"));
    }

    #[tokio::test]
    async fn pull_inner_handles_single_layer_image_with_singular_log_phrasing() {
        ensure_global_trace_subscriber();
        let layer_raw = b"only-layer-bytes".to_vec();
        let layer_gz = gzip(&layer_raw);
        let layer_digest = sha256_hex(&layer_gz);
        let diff = sha256_hex(&layer_raw);
        let manifest = OciImageManifest {
            layers: vec![OciDescriptor {
                digest: layer_digest.clone(),
                size: layer_gz.len() as i64,
                media_type: manifest::IMAGE_LAYER_GZIP_MEDIA_TYPE.into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let config = ConfigFile {
            architecture: "arm64".into(),
            os: "linux".into(),
            rootfs: Rootfs {
                r#type: "layers".into(),
                diff_ids: vec![diff],
            },
            ..Default::default()
        };
        let img = FakeImage {
            manifest,
            config_json: serde_json::to_string(&config).unwrap(),
            manifest_digest: "sha256:not-used".to_string(),
            blobs: vec![(layer_digest, layer_gz)],
        };
        let (_dir, cache) = cache();
        let registry = img.into_registry();
        let pulled = pull_inner(&registry, "alpine:3.20", &cache).await.unwrap();
        assert_eq!(pulled.layers.len(), 1);

        let layer_raw = b"only-layer-bytes".to_vec();
        let layer_gz = gzip(&layer_raw);
        let layer_digest = sha256_hex(&layer_gz);
        let diff = sha256_hex(&layer_raw);
        let manifest = OciImageManifest {
            layers: vec![OciDescriptor {
                digest: layer_digest.clone(),
                size: layer_gz.len() as i64,
                media_type: manifest::IMAGE_LAYER_GZIP_MEDIA_TYPE.into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let config = ConfigFile {
            architecture: "arm64".into(),
            os: "linux".into(),
            rootfs: Rootfs {
                r#type: "layers".into(),
                diff_ids: vec![diff],
            },
            ..Default::default()
        };
        let img2 = FakeImage {
            manifest,
            config_json: serde_json::to_string(&config).unwrap(),
            manifest_digest: "sha256:not-used".to_string(),
            blobs: vec![(layer_digest, layer_gz)],
        };
        let registry2 = img2.into_registry();
        pull_inner(&registry2, "alpine:3.20", &cache).await.unwrap();
    }

    #[tokio::test]
    async fn pull_inner_rejects_when_diff_id_in_config_does_not_match_layer() {
        let mut img = build_two_layer_image();
        let cfg: ConfigFile = serde_json::from_str(&img.config_json).unwrap();
        let bad_cfg = ConfigFile {
            rootfs: Rootfs {
                r#type: cfg.rootfs.r#type.clone(),
                diff_ids: vec![
                    "sha256:0000000000000000000000000000000000000000000000000000000000000000"
                        .into(),
                    cfg.rootfs.diff_ids[1].clone(),
                ],
            },
            ..cfg
        };
        img.config_json = serde_json::to_string(&bad_cfg).unwrap();
        let registry = img.into_registry();
        let (_dir, cache) = cache();
        let err = pull_inner(&registry, "alpine:3.20", &cache)
            .await
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("DiffID mismatch"),
            "DiffID check must fire when config and layer bytes diverge: {msg}"
        );
    }

    #[tokio::test]
    async fn pull_inner_rejects_unsupported_media_type_before_fetching_blobs() {
        let mut img = build_two_layer_image();
        img.manifest.layers[0].media_type = "application/vnd.oci.image.layer.v1.tar+zstd".into();
        let registry = img.into_registry();
        let (_dir, cache) = cache();
        let err = pull_inner(&registry, "alpine:3.20", &cache)
            .await
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("unsupported media type"),
            "must explicitly bail on +zstd / other unsupported types: {msg}"
        );
        let calls = registry.calls.lock().unwrap();
        assert!(
            calls.iter().all(|c| !c.starts_with("blob:")),
            "no blobs should have been fetched; calls={calls:?}"
        );
    }

    #[tokio::test]
    async fn pull_inner_rejects_summed_declared_layer_size_over_ceiling_before_fetching() {
        let mut img = build_two_layer_image();
        img.manifest.layers[0].size = (MAX_TOTAL_DECLARED_LAYER_BYTES + 1) as i64;
        let registry = img.into_registry();
        let (_dir, cache) = cache();
        let err = pull_inner(&registry, "alpine:3.20", &cache)
            .await
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("over the"), "got: {msg}");
        let calls = registry.calls.lock().unwrap();
        assert!(
            calls.iter().all(|c| !c.starts_with("blob:")),
            "no blob may be fetched once the declared-size ceiling is exceeded; calls={calls:?}"
        );
    }

    #[tokio::test]
    async fn pull_inner_rejects_diff_id_count_mismatch() {
        let mut img = build_two_layer_image();
        let cfg: ConfigFile = serde_json::from_str(&img.config_json).unwrap();
        let bad_cfg = ConfigFile {
            rootfs: Rootfs {
                r#type: cfg.rootfs.r#type.clone(),
                diff_ids: vec![cfg.rootfs.diff_ids[0].clone()],
            },
            ..cfg
        };
        img.config_json = serde_json::to_string(&bad_cfg).unwrap();
        let registry = img.into_registry();
        let (_dir, cache) = cache();
        let err = pull_inner(&registry, "alpine:3.20", &cache)
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("diff_ids but manifest has"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn pull_inner_rejects_diff_id_mismatch_on_layer_bytes() {
        let mut img = build_two_layer_image();
        let bogus = gzip(b"totally different bytes");
        img.blobs[0].1 = bogus;
        let registry = img.into_registry();
        let (_dir, cache) = cache();
        let err = pull_inner(&registry, "alpine:3.20", &cache)
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("resolving layer"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn pull_inner_returns_using_cached_path_when_all_layers_already_in_cache() {
        let image = build_two_layer_image();
        let (_dir, cache) = cache();
        for (digest, bytes) in &image.blobs {
            cache.install_from_bytes(digest, bytes).unwrap();
        }
        let registry = image.into_registry();
        let result = pull_inner(&registry, "alpine:3.20", &cache).await.unwrap();
        assert_eq!(result.layers.len(), 2);
        let calls = registry.calls.lock().unwrap();
        assert!(
            calls.iter().all(|c| !c.starts_with("blob:")),
            "cached path must not hit the registry; calls={calls:?}"
        );
    }

    #[tokio::test]
    async fn pull_inner_propagates_invalid_image_reference() {
        let image = build_two_layer_image();
        let registry = image.into_registry();
        let (_dir, cache) = cache();
        let err = pull_inner(&registry, "::not-a-ref::", &cache)
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("invalid image reference"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn pull_inner_verifies_manifest_digest_when_pinned_by_digest() {
        let image = build_two_layer_image();
        let manifest_canonical = serde_json::to_vec(&image.manifest).unwrap();
        let actual_digest = sha256_hex(&manifest_canonical);
        let registry = image.into_registry();
        let (_dir, cache) = cache();
        let by_digest = format!("alpine@{actual_digest}");
        pull_inner(&registry, &by_digest, &cache).await.unwrap();
        let wrong =
            "alpine@sha256:0000000000000000000000000000000000000000000000000000000000000000";
        let err = pull_inner(&registry, wrong, &cache).await.unwrap_err();
        assert!(
            format!("{err:#}").contains("manifest digest mismatch"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn pull_inner_propagates_unparseable_config_json() {
        let mut img = build_two_layer_image();
        img.config_json = "not actually json".into();
        let registry = img.into_registry();
        let (_dir, cache) = cache();
        let err = pull_inner(&registry, "alpine:3.20", &cache)
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("parsing image config"),
            "got: {err:#}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pull_inner_cold_cache_emits_resolved_then_pulled_with_no_image_cached() {
        let image = build_two_layer_image();
        let (_dir, cache) = cache();
        let registry = image.into_registry();
        let (result, verbs) = capture_verbs(|| pull_inner(&registry, "alpine:3.20", &cache)).await;
        result.expect("cold pull");
        assert_eq!(
            verbs,
            vec!["Resolved".to_string(), "Pulled".to_string()],
            "cold cache must emit Resolved → Pulled and nothing else",
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pull_inner_warm_cache_emits_image_cached_only_no_resolved_or_pulled() {
        let image = build_two_layer_image();
        let (_dir, cache) = cache();
        for (digest, bytes) in &image.blobs {
            cache.install_from_bytes(digest, bytes).unwrap();
        }
        let registry = image.into_registry();
        let (result, verbs) = capture_verbs(|| pull_inner(&registry, "alpine:3.20", &cache)).await;
        result.expect("warm pull");
        assert_eq!(
            verbs,
            vec!["ImageCached".to_string()],
            "warm cache must emit only ImageCached (no Resolved, no Pulled)",
        );
    }

    #[tokio::test]
    async fn cached_layers_skip_diff_id_reverification() {
        let mut img = build_two_layer_image();
        let cfg: ConfigFile = serde_json::from_str(&img.config_json).unwrap();
        let bad_cfg = ConfigFile {
            rootfs: Rootfs {
                r#type: cfg.rootfs.r#type.clone(),
                diff_ids: vec![
                    "sha256:0000000000000000000000000000000000000000000000000000000000000000"
                        .into(),
                    cfg.rootfs.diff_ids[1].clone(),
                ],
            },
            ..cfg
        };
        img.config_json = serde_json::to_string(&bad_cfg).unwrap();
        let (_dir, cache) = cache();
        for (digest, bytes) in &img.blobs {
            cache.install_from_bytes(digest, bytes).unwrap();
        }
        let registry = img.into_registry();
        pull_inner(&registry, "alpine:3.20", &cache)
            .await
            .expect("a layer already in cache must not be re-verified against the config DiffID");
    }
}
