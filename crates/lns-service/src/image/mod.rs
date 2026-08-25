use anyhow::{Context, Result};
use oci_client::{
    Reference,
    client::ClientConfig,
    manifest::{self, OciDescriptor, OciImageManifest},
};
use sha2::{Digest, Sha256};

use crate::log;
use crate::oci_layer_cache::LayerCache;

pub(crate) mod manifest_cache;
mod real;
pub(crate) use real::{RealRegistry, caching_registry_for, pull_dependency, registry_auth_for};
pub use real::{pull, pull_artifact, verify_login};

pub(crate) trait Registry: Send + Sync {
    fn pull_manifest_and_config(
        &self,
        reference: &Reference,
    ) -> impl std::future::Future<Output = Result<(OciImageManifest, String, String)>> + Send;

    fn pull_blob(
        &self,
        reference: &Reference,
        descriptor: &OciDescriptor,
        on_chunk: &(dyn Fn(u64) + Send + Sync),
    ) -> impl std::future::Future<Output = Result<Vec<u8>>> + Send;

    fn pull_blob_to_path(
        &self,
        reference: &Reference,
        descriptor: &OciDescriptor,
        max_bytes: u64,
        path: &std::path::Path,
        on_chunk: &(dyn Fn(u64) + Send + Sync),
    ) -> impl std::future::Future<Output = Result<()>> + Send;
}

pub(crate) struct CountingSink<'a> {
    buf: Vec<u8>,
    max_bytes: u64,
    on_chunk: &'a (dyn Fn(u64) + Send + Sync),
}

impl<'a> CountingSink<'a> {
    pub(crate) fn new(max_bytes: u64, on_chunk: &'a (dyn Fn(u64) + Send + Sync)) -> Self {
        Self {
            buf: Vec::new(),
            max_bytes,
            on_chunk,
        }
    }

    pub(crate) fn into_bytes(self) -> Vec<u8> {
        self.buf
    }
}

/// The in-memory ceiling for a buffered blob: its declared descriptor size, clamped to the global layer ceiling, so a registry that under-declares then over-streams can't OOM the service.
pub(crate) fn blob_byte_cap(declared_size: i64) -> u64 {
    if declared_size > 0 {
        (declared_size as u64).min(MAX_TOTAL_DECLARED_LAYER_BYTES)
    } else {
        MAX_TOTAL_DECLARED_LAYER_BYTES
    }
}

impl tokio::io::AsyncWrite for CountingSink<'_> {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let sink = self.get_mut();
        if !buf.is_empty() && sink.buf.len() as u64 >= sink.max_bytes {
            return std::task::Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("blob exceeds the {}-byte limit", sink.max_bytes),
            )));
        }
        let remaining = sink.max_bytes.saturating_sub(sink.buf.len() as u64);
        let allowed = usize::try_from(remaining)
            .unwrap_or(usize::MAX)
            .min(buf.len());
        (sink.on_chunk)(allowed as u64);
        sink.buf.extend_from_slice(&buf[..allowed]);
        std::task::Poll::Ready(Ok(allowed))
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }
}

const MIN_PROGRESS_STEP_BYTES: u64 = 256 * 1024;

struct PullProgress {
    total: u64,
    step: u64,
    received: std::sync::atomic::AtomicU64,
    logged: std::sync::Mutex<u64>,
}

impl PullProgress {
    fn start(total: u64) -> Self {
        log::progress("Pulling", "", 0, total);
        Self {
            total,
            step: (total / 100).max(MIN_PROGRESS_STEP_BYTES),
            received: std::sync::atomic::AtomicU64::new(0),
            logged: std::sync::Mutex::new(0),
        }
    }

    fn add(&self, n: u64) {
        let cur = self
            .received
            .fetch_add(n, std::sync::atomic::Ordering::Relaxed)
            + n;
        let mut logged = self.logged.lock().expect("pull-progress mutex");
        let crossed_step = cur / self.step > *logged / self.step;
        let reached_total = self.total > 0 && cur >= self.total && *logged < self.total;
        if cur > *logged && (crossed_step || reached_total) {
            *logged = cur;
            log::progress("Pulling", "", cur, self.total);
        }
    }
}

#[cfg(target_arch = "aarch64")]
pub(crate) fn want_arch() -> oci_spec::image::Arch {
    oci_spec::image::Arch::ARM64
}

#[cfg(not(target_arch = "aarch64"))]
pub(crate) fn want_arch() -> oci_spec::image::Arch {
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

/// Every registry client this service builds, so no pull, push or login probe can pick its own transport.
pub(crate) fn client_config_for(registry: &str) -> ClientConfig {
    ClientConfig {
        protocol: lns_artifact::client_protocol_for(registry),
        platform_resolver: Some(Box::new(linux_platform_resolver)),
        ..Default::default()
    }
}

const LOGIN_PROBE_REPOSITORY: &str = "lns/login-check";

pub(crate) fn login_probe_reference(registry: &str) -> Result<Reference> {
    format!("{registry}/{LOGIN_PROBE_REPOSITORY}")
        .parse()
        .with_context(|| format!("invalid registry {registry:?}"))
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
    pub artifact_type: Option<String>,
    pub config_media_type: String,
}

#[derive(Debug)]
pub struct PulledSandbox {
    pub reference: Reference,
    pub digest: String,
    pub base_image: String,
    pub mixins: Vec<String>,
    pub tools: Vec<String>,
}

/// What a published artifact reference resolved to: a sandbox a run can boot, or a mixin something else layers on.
#[derive(Debug)]
pub enum PulledArtifact {
    Sandbox(PulledSandbox),
    Mixin(PulledMixin),
}

impl PulledArtifact {
    /// Answer as a sandbox, or not at all — so a caller that needs one says which kind it expected rather than matching a variant it has no use for.
    pub fn sandbox(self) -> Option<PulledSandbox> {
        match self {
            PulledArtifact::Sandbox(sandbox) => Some(sandbox),
            PulledArtifact::Mixin(_) => None,
        }
    }

    pub fn mixin(self) -> Option<PulledMixin> {
        match self {
            PulledArtifact::Mixin(mixin) => Some(mixin),
            PulledArtifact::Sandbox(_) => None,
        }
    }
}

/// A pulled mixin: config-only, so what it carries is the graph it names and the tools a consumer would install.
#[derive(Debug)]
pub struct PulledMixin {
    pub reference: Reference,
    pub digest: String,
    pub mixins: Vec<String>,
    pub tools: Vec<String>,
}

pub(crate) async fn pull_artifact_with<R: Registry>(
    client: &R,
    image: &str,
) -> Result<PulledArtifact> {
    let reference: Reference = image
        .parse()
        .with_context(|| format!("invalid image reference: {image}"))?;
    let (manifest, manifest_digest, config_str) =
        client.pull_manifest_and_config(&reference).await?;
    let path = crate::artifact::dispatch(
        manifest.artifact_type.as_deref(),
        Some(manifest.config.media_type.as_str()),
    )
    .map_err(sandbox_pull_error)?;
    verify_digest_pin(&reference, &manifest_digest, image)?;
    match path {
        Some(lns_artifact::spec::Kind::Mixin) => {
            let mixin = lns_artifact::sandbox::parse_mixin(config_str.as_bytes())
                .with_context(|| format!("parsing published mixin {image}"))?;
            return Ok(PulledArtifact::Mixin(PulledMixin {
                reference,
                digest: manifest_digest,
                mixins: mixin.spec.mixins.clone(),
                tools: mixin.spec.tools.clone(),
            }));
        }
        Some(lns_artifact::spec::Kind::Sandbox) => {}
        None => anyhow::bail!(
            "{image} is an OCI image, not a Lens Sandbox artifact; `lns pull` takes a published sandbox or mixin"
        ),
    }
    let def = lns_artifact::sandbox::parse(config_str.as_bytes())
        .with_context(|| format!("parsing published sandbox {image}"))?;
    Ok(PulledArtifact::Sandbox(PulledSandbox {
        reference,
        digest: manifest_digest,
        base_image: def.spec.image,
        mixins: def.spec.mixins,
        tools: def.spec.tools,
    }))
}

/// Pull a declared mixin's document. The pin is verified before the type, because for a mixin the bytes are the identity: a reference that does not name exactly these bytes has nothing to be typed.
pub(crate) async fn pull_mixin_with<R: Registry>(
    client: &R,
    reference: &str,
) -> Result<crate::artifact::mixin::FetchedMixin> {
    let parsed: Reference = reference
        .parse()
        .with_context(|| format!("invalid mixin reference {reference}"))?;
    let (manifest, manifest_digest, config_json) = client.pull_manifest_and_config(&parsed).await?;
    verify_digest_pin(&parsed, &manifest_digest, reference)?;
    if !crate::artifact::mixin::is_a_mixin_artifact(
        manifest.artifact_type.as_deref(),
        Some(manifest.config.media_type.as_str()),
    ) {
        anyhow::bail!("{reference} is not a mixin artifact");
    }
    Ok(crate::artifact::mixin::FetchedMixin {
        pinned: parsed.clone_with_digest(manifest_digest).to_string(),
        document: config_json,
        layers: crate::artifact::fileset::packed_layers(&manifest),
    })
}

fn sandbox_pull_error(e: anyhow::Error) -> anyhow::Error {
    anyhow::anyhow!("{e:#}").context("this reference is not a supported Lens Sandbox artifact")
}

pub(crate) fn verify_digest_pin(
    reference: &Reference,
    manifest_digest: &str,
    image: &str,
) -> Result<()> {
    if let Some(expected) = reference.digest()
        && !ct_digest_eq(manifest_digest, expected)
    {
        anyhow::bail!(
            "manifest digest mismatch for {image} — expected {expected}, \
             received {manifest_digest}"
        );
    }
    Ok(())
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

    // Compare against the registry-reported content digest, not a re-serialization of the parsed manifest — serde's byte layout differs from the registry's, so re-hashing would false-mismatch every real digest-pinned pull.
    verify_digest_pin(&reference, &manifest_digest, image)?;

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
        if descriptor.size <= 0 {
            anyhow::bail!(
                "layer {i} declares a non-positive size {} (digest {}); a registry that under-declares a layer then over-streams could evade the buffered-blob cap",
                descriptor.size,
                descriptor.digest
            );
        }
    }

    let total_bytes: u64 = manifest
        .layers
        .iter()
        .map(|d| d.size as u64)
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
    let missing_total: u64 = manifest
        .layers
        .iter()
        .zip(&was_cached)
        .filter(|(_, cached)| !**cached)
        .map(|(d, _)| d.size.max(0) as u64)
        .fold(0u64, u64::saturating_add);
    let progress = any_missing.then(|| PullProgress::start(missing_total));
    let progress = progress.as_ref();
    let pull_start = std::time::Instant::now();

    // `buffered` preserves manifest order — `buffer_unordered` caused the digest-mismatch bug fixed in 608bda56.
    use futures_util::stream::{self, StreamExt, TryStreamExt};
    const MAX_CONCURRENT_LAYER_FETCHES: usize = 4;
    let reference_ref = &reference;
    let layers_owned: Vec<(usize, oci_client::manifest::OciDescriptor)> =
        manifest.layers.iter().cloned().enumerate().collect();
    let bytes_per_layer: Vec<Vec<u8>> = stream::iter(layers_owned)
        .map(|(i, descriptor)| {
            // Cached layers were DiffID-verified on their cold pull; their decompressed content is pinned by the already-verified compressed digest, so re-decompressing them every run is pure waste.
            let expected_diff_id = (!was_cached[i]).then(|| config.rootfs.diff_ids[i].clone());
            async move {
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
                let media_type = descriptor.media_type.clone();
                let bytes = layer_cache
                    .get_or_install(&digest, || async move {
                        let on_chunk = move |bytes: u64| {
                            if let Some(p) = progress {
                                p.add(bytes);
                            }
                        };
                        client
                            .pull_blob(reference_ref, &descriptor, &on_chunk)
                            .await
                    })
                    .await
                    .with_context(|| format!("resolving layer {i} (digest {digest})"))?;
                let Some(expected_diff_id) = expected_diff_id else {
                    return Ok(bytes);
                };
                let (bytes, actual_diff_id) = tokio::task::spawn_blocking(move || {
                    let actual = compute_diff_id(&bytes, &media_type)
                        .with_context(|| format!("computing DiffID for layer {i}"))?;
                    Ok::<_, anyhow::Error>((bytes, actual))
                })
                .await??;
                if !ct_digest_eq(&actual_diff_id, &expected_diff_id) {
                    anyhow::bail!(
                        "layer {i} DiffID mismatch: image config says {expected_diff_id}, \
                         actual {actual_diff_id}"
                    );
                }
                Ok(bytes)
            }
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
        artifact_type: manifest.artifact_type.clone(),
        config_media_type: manifest.config.media_type.clone(),
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
    use oci_client::client::ClientProtocol;
    use oci_client::config::{ConfigFile, Rootfs};
    use oci_client::manifest::{OciDescriptor, OciImageManifest};
    use std::io::Write;
    use std::sync::Mutex;
    use tempfile::TempDir;

    fn speaks_http(registry: &str) -> bool {
        matches!(client_config_for(registry).protocol, ClientProtocol::Http)
    }

    #[test]
    fn a_loopback_registry_client_speaks_plaintext_http() {
        assert!(speaks_http("localhost:5000"));
        assert!(speaks_http("127.0.0.1:5000"));
        assert!(speaks_http("[::1]:5000"));
    }

    #[test]
    fn a_remote_registry_client_stays_on_https() {
        assert!(!speaks_http("ghcr.io"));
        assert!(!speaks_http("127.0.0.1.evil.com"));
    }

    #[test]
    fn every_registry_client_resolves_the_linux_host_arch_manifest() {
        let loopback = client_config_for("localhost:5000");
        assert!(loopback.platform_resolver.is_some());
        assert!(client_config_for("ghcr.io").platform_resolver.is_some());
    }

    #[test]
    fn the_login_probe_targets_a_throwaway_repository_on_the_named_registry() {
        let probe = login_probe_reference("localhost:5000").unwrap();
        assert_eq!(probe.registry(), "localhost:5000");
        assert_eq!(probe.repository(), "lns/login-check");
    }

    #[test]
    fn a_login_probe_reaches_a_loopback_registry_the_same_way_a_pull_does() {
        let probe = login_probe_reference("127.0.0.1:5000").unwrap();
        assert!(speaks_http(probe.registry()));
    }

    #[test]
    fn an_unparseable_registry_names_itself_in_the_login_probe_error() {
        let err = login_probe_reference("not a registry").unwrap_err();
        assert!(
            err.to_string().contains("not a registry"),
            "unexpected error: {err}"
        );
    }

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

    #[derive(Default)]
    struct ProgressCapture {
        points: Mutex<Vec<(u64, u64)>>,
    }

    struct ProgressPointVisitor {
        current: u64,
        total: u64,
    }

    impl tracing::field::Visit for ProgressPointVisitor {
        fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
            if field.name() == "current" {
                self.current = value;
            }
            if field.name() == "total" {
                self.total = value;
            }
        }
        fn record_debug(&mut self, _: &tracing::field::Field, _: &dyn std::fmt::Debug) {}
    }

    struct ProgressLayer(std::sync::Arc<ProgressCapture>);

    impl<S> tracing_subscriber::Layer<S> for ProgressLayer
    where
        S: tracing::Subscriber,
    {
        fn on_event(
            &self,
            event: &tracing::Event<'_>,
            _ctx: tracing_subscriber::layer::Context<'_, S>,
        ) {
            if event.metadata().target() != crate::log::PROGRESS_TARGET {
                return;
            }
            let mut v = ProgressPointVisitor {
                current: 0,
                total: 0,
            };
            event.record(&mut v);
            self.0.points.lock().unwrap().push((v.current, v.total));
        }
    }

    fn capture_progress_sync(f: impl FnOnce()) -> Vec<(u64, u64)> {
        use tracing_subscriber::layer::SubscriberExt;
        let capture = std::sync::Arc::new(ProgressCapture::default());
        let subscriber =
            tracing_subscriber::registry().with(ProgressLayer(std::sync::Arc::clone(&capture)));
        let guard = tracing::subscriber::set_default(subscriber);
        f();
        drop(guard);
        capture.points.lock().unwrap().clone()
    }

    async fn capture_progress<F, Fut, R>(f: F) -> (R, Vec<(u64, u64)>)
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = R>,
    {
        use tracing_subscriber::layer::SubscriberExt;
        let capture = std::sync::Arc::new(ProgressCapture::default());
        let subscriber =
            tracing_subscriber::registry().with(ProgressLayer(std::sync::Arc::clone(&capture)));
        let guard = tracing::subscriber::set_default(subscriber);
        let result = f().await;
        drop(guard);
        let points = capture.points.lock().unwrap().clone();
        (result, points)
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
            manifest_digest: format!("sha256:{}", "a".repeat(64)),
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
            on_chunk: &(dyn Fn(u64) + Send + Sync),
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
            let blob =
                blob.ok_or_else(|| anyhow::anyhow!("no canned blob for {}", descriptor.digest))?;
            let mid = blob.len() / 2;
            on_chunk(mid as u64);
            on_chunk((blob.len() - mid) as u64);
            Ok(blob)
        }

        async fn pull_blob_to_path(
            &self,
            reference: &Reference,
            descriptor: &OciDescriptor,
            _max_bytes: u64,
            path: &std::path::Path,
            on_chunk: &(dyn Fn(u64) + Send + Sync),
        ) -> Result<()> {
            let bytes = self.pull_blob(reference, descriptor, on_chunk).await?;
            tokio::fs::write(path, bytes).await?;
            Ok(())
        }
    }

    #[tokio::test]
    async fn registry_path_pull_writes_the_blob_and_reports_streamed_bytes() {
        let registry = build_two_layer_image().into_registry();
        let descriptor = registry.manifest.layers[0].clone();
        let expected = registry
            .blobs
            .iter()
            .find(|(digest, _)| digest == &descriptor.digest)
            .unwrap()
            .1
            .clone();
        let reference: Reference = "registry.example.test/team/image:1".parse().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("blob");
        let received = std::sync::atomic::AtomicU64::new(0);

        registry
            .pull_blob_to_path(
                &reference,
                &descriptor,
                expected.len() as u64,
                &path,
                &|bytes| {
                    received.fetch_add(bytes, std::sync::atomic::Ordering::Relaxed);
                },
            )
            .await
            .unwrap();

        assert_eq!(std::fs::read(path).unwrap(), expected);
        assert_eq!(
            received.load(std::sync::atomic::Ordering::Relaxed),
            expected.len() as u64
        );
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

    fn build_sandbox_artifact() -> FakeImage {
        let definition = format!(
            r#"{{"apiVersion":"lns.run/v1","kind":"sandbox","name":"some-sandbox","spec":{{"image":"registry.example.test/base@sha256:{}"}}}}"#,
            "b".repeat(64)
        );
        let manifest = OciImageManifest {
            artifact_type: Some("application/vnd.lens.sandbox.v1+json".into()),
            config: OciDescriptor {
                media_type: "application/vnd.lens.sandbox.config.v1+json".into(),
                digest: sha256_hex(definition.as_bytes()),
                size: definition.len() as i64,
                ..Default::default()
            },
            ..Default::default()
        };
        FakeImage {
            manifest,
            config_json: definition,
            manifest_digest: format!("sha256:{}", "c".repeat(64)),
            blobs: vec![],
        }
    }

    #[tokio::test]
    async fn pull_sandbox_accepts_a_published_sandbox_and_names_its_base_image() {
        ensure_global_trace_subscriber();
        let registry = build_sandbox_artifact().into_registry();
        let sandbox = pull_artifact_with(&registry, "registry.example.test/sb:1")
            .await
            .unwrap()
            .sandbox()
            .expect("a sandbox artifact pulls as a sandbox");
        assert_eq!(sandbox.digest, format!("sha256:{}", "c".repeat(64)));
        assert_eq!(
            sandbox.base_image,
            format!("registry.example.test/base@sha256:{}", "b".repeat(64)),
            "the definition's spec.image must surface so the store can prefetch it",
        );
        let calls: Vec<String> = registry.calls.lock().unwrap().clone();
        assert_eq!(
            calls,
            vec!["manifest"],
            "a config-only artifact must not pull layer blobs",
        );
    }

    fn build_mixin_artifact() -> FakeImage {
        let document = r#"{"apiVersion":"lns.run/v1","kind":"mixin","name":"postgres-tools","spec":{"tools":["node@22"]}}"#.to_string();
        let manifest = OciImageManifest {
            artifact_type: Some("application/vnd.lens.mixin.v1+json".into()),
            config: OciDescriptor {
                media_type: "application/vnd.lens.mixin.config.v1+json".into(),
                digest: sha256_hex(document.as_bytes()),
                size: document.len() as i64,
                ..Default::default()
            },
            ..Default::default()
        };
        FakeImage {
            manifest,
            config_json: document,
            manifest_digest: format!("sha256:{}", "c".repeat(64)),
            blobs: vec![],
        }
    }

    #[test]
    fn a_pulled_artifact_answers_only_as_the_kind_it_is() {
        let sandbox = PulledArtifact::Sandbox(PulledSandbox {
            reference: "registry.example.test/sb:1".parse().unwrap(),
            digest: format!("sha256:{}", "a".repeat(64)),
            base_image: "registry.example.test/base:1".into(),
            mixins: Vec::new(),
            tools: Vec::new(),
        });
        let mixin = PulledArtifact::Mixin(PulledMixin {
            reference: "registry.example.test/m:1".parse().unwrap(),
            digest: format!("sha256:{}", "b".repeat(64)),
            mixins: Vec::new(),
            tools: Vec::new(),
        });
        assert!(
            PulledArtifact::Sandbox(PulledSandbox {
                reference: "registry.example.test/sb:1".parse().unwrap(),
                digest: String::new(),
                base_image: String::new(),
                mixins: Vec::new(),
                tools: Vec::new(),
            })
            .mixin()
            .is_none(),
            "a sandbox answering as a mixin would merge a document nothing can boot"
        );
        assert!(
            PulledArtifact::Mixin(PulledMixin {
                reference: "registry.example.test/m:1".parse().unwrap(),
                digest: String::new(),
                mixins: Vec::new(),
                tools: Vec::new(),
            })
            .sandbox()
            .is_none(),
            "a mixin describes no launch, so answering as a sandbox would hand a run a document with no image"
        );
        assert!(sandbox.sandbox().is_some());
        assert!(mixin.mixin().is_some());
    }

    #[tokio::test]
    async fn pull_accepts_a_published_mixin_and_names_the_graph_it_declares() {
        ensure_global_trace_subscriber();
        let registry = build_mixin_artifact().into_registry();
        let mixin = pull_artifact_with(&registry, "registry.example.test/m:1")
            .await
            .unwrap()
            .mixin()
            .expect("a mixin artifact pulls as a mixin");
        assert_eq!(mixin.digest, format!("sha256:{}", "c".repeat(64)));
        assert_eq!(
            registry.calls.lock().unwrap().as_slice(),
            ["manifest"],
            "a mixin is config-only, so no layer blob is fetched"
        );
    }

    #[tokio::test]
    async fn pull_mixin_returns_the_document_a_matching_pin_names() {
        ensure_global_trace_subscriber();
        let registry = build_mixin_artifact().into_registry();
        let pinned = format!("registry.example.test/m@sha256:{}", "c".repeat(64));
        let fetched = pull_mixin_with(&registry, &pinned).await.unwrap();
        assert!(
            fetched.document.contains(r#""kind":"mixin""#),
            "got: {}",
            fetched.document
        );
        assert_eq!(
            fetched.pinned, pinned,
            "a reference that already names the bytes answers as itself"
        );
        assert_eq!(
            registry.calls.lock().unwrap().as_slice(),
            ["manifest"],
            "a mixin is config-only, so no layer blob is fetched",
        );
    }

    #[tokio::test]
    async fn pull_mixin_refuses_a_pin_the_registry_did_not_answer_with() {
        ensure_global_trace_subscriber();
        let registry = build_mixin_artifact().into_registry();
        let pinned = format!("registry.example.test/m@sha256:{}", "d".repeat(64));
        let err = pull_mixin_with(&registry, &pinned).await.unwrap_err();
        assert!(
            format!("{err:#}").contains("manifest digest mismatch"),
            "for a mixin the bytes are the identity, so the pin has to gate their use before anything reads them; got: {err:#}"
        );
    }

    #[tokio::test]
    async fn pull_mixin_refuses_an_artifact_of_another_kind() {
        ensure_global_trace_subscriber();
        let registry = build_sandbox_artifact().into_registry();
        let pinned = format!("registry.example.test/m@sha256:{}", "c".repeat(64));
        let err = pull_mixin_with(&registry, &pinned).await.unwrap_err();
        assert!(
            format!("{err:#}").contains("is not a mixin artifact"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn pull_mixin_answers_with_the_pin_when_the_type_is_wrong_too() {
        ensure_global_trace_subscriber();
        let registry = build_sandbox_artifact().into_registry();
        let pinned = format!("registry.example.test/m@sha256:{}", "d".repeat(64));
        let err = pull_mixin_with(&registry, &pinned).await.unwrap_err();
        assert!(
            format!("{err:#}").contains("manifest digest mismatch"),
            "for a mixin the bytes are the identity, so a reference that names other bytes has nothing to be typed and the pin has to answer first; got: {err:#}"
        );
    }

    #[tokio::test]
    async fn pull_mixin_refuses_a_reference_that_does_not_parse() {
        ensure_global_trace_subscriber();
        let registry = build_mixin_artifact().into_registry();
        let err = pull_mixin_with(&registry, "not a reference")
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("invalid mixin reference"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn pull_sandbox_rejects_a_plain_oci_image_before_pulling_layers() {
        ensure_global_trace_subscriber();
        let registry = build_two_layer_image().into_registry();
        let err = pull_artifact_with(&registry, "alpine:3.20")
            .await
            .unwrap_err();
        let rendered = format!("{err:#}");
        assert!(
            rendered.contains("OCI image, not a Lens Sandbox artifact"),
            "got: {rendered}"
        );
        assert_eq!(
            registry.calls.lock().unwrap().as_slice(),
            ["manifest"],
            "rejection must happen before any image layer is fetched",
        );
    }

    #[tokio::test]
    async fn pull_sandbox_refuses_a_digest_pinned_artifact_on_mismatch() {
        ensure_global_trace_subscriber();
        let registry = build_sandbox_artifact().into_registry();
        let pinned = format!("registry.example.test/sb@sha256:{}", "d".repeat(64));
        let err = pull_artifact_with(&registry, &pinned).await.unwrap_err();
        assert!(
            format!("{err:#}").contains("manifest digest mismatch"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn pull_sandbox_accepts_a_digest_pinned_artifact_that_matches() {
        ensure_global_trace_subscriber();
        let registry = build_sandbox_artifact().into_registry();
        let pinned = format!("registry.example.test/sb@sha256:{}", "c".repeat(64));
        pull_artifact_with(&registry, &pinned).await.unwrap();
    }

    #[tokio::test]
    async fn pull_sandbox_rejects_a_corrupt_definition() {
        ensure_global_trace_subscriber();
        let mut artifact = build_sandbox_artifact();
        artifact.config_json = "{}".into();
        let registry = artifact.into_registry();
        let err = pull_artifact_with(&registry, "registry.example.test/sb:1")
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("parsing published sandbox"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn pull_sandbox_refuses_an_unpullable_artifact_kind_naming_the_type() {
        ensure_global_trace_subscriber();
        let mut artifact = build_sandbox_artifact();
        artifact.manifest.artifact_type = Some("application/vnd.acme.surprise.v1+json".into());
        let registry = artifact.into_registry();
        let err = pull_artifact_with(&registry, "registry.example.test/odd:1")
            .await
            .unwrap_err();
        let rendered = format!("{err:#}");
        assert!(
            rendered.contains("not a supported Lens Sandbox artifact")
                && rendered.contains("vnd.acme.surprise"),
            "got: {rendered}"
        );
    }

    #[tokio::test]
    async fn pull_sandbox_refuses_an_unparseable_reference() {
        ensure_global_trace_subscriber();
        let registry = build_sandbox_artifact().into_registry();
        let err = pull_artifact_with(&registry, "###").await.unwrap_err();
        assert!(
            format!("{err:#}").contains("invalid image reference"),
            "got: {err:#}"
        );
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
    async fn pull_inner_threads_manifest_artifact_type_and_config_media_type() {
        ensure_global_trace_subscriber();
        let mut img = build_two_layer_image();
        img.manifest.artifact_type = Some("application/vnd.lens.sandbox.v1+json".into());
        img.manifest.config.media_type = "application/vnd.lens.sandbox.config.v1+json".into();
        let registry = img.into_registry();
        let (_dir, cache) = cache();
        let pulled = pull_inner(&registry, "reg/sandbox:1", &cache)
            .await
            .unwrap();
        assert_eq!(
            pulled.artifact_type.as_deref(),
            Some("application/vnd.lens.sandbox.v1+json"),
            "the manifest's artifactType must be carried on the pulled image so run can dispatch",
        );
        assert_eq!(
            pulled.config_media_type, "application/vnd.lens.sandbox.config.v1+json",
            "the config descriptor's mediaType must be carried for the oras empty-artifactType fallback",
        );
    }

    #[tokio::test]
    async fn pull_inner_reports_no_artifact_type_for_a_plain_image() {
        ensure_global_trace_subscriber();
        let registry = build_two_layer_image().into_registry();
        let (_dir, cache) = cache();
        let pulled = pull_inner(&registry, "alpine:3.20", &cache).await.unwrap();
        assert!(
            pulled.artifact_type.is_none(),
            "a plain image manifest declares no artifactType",
        );
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
    async fn pull_inner_rejects_a_non_positive_declared_layer_size_before_fetching() {
        for size in [0i64, -1] {
            let mut img = build_two_layer_image();
            img.manifest.layers[0].size = size;
            let registry = img.into_registry();
            let (_dir, cache) = cache();
            let err = pull_inner(&registry, "alpine:3.20", &cache)
                .await
                .unwrap_err();
            let msg = format!("{err:#}");
            assert!(msg.contains("non-positive size"), "size {size}: got: {msg}");
            let calls = registry.calls.lock().unwrap();
            assert!(
                calls.iter().all(|c| !c.starts_with("blob:")),
                "a layer declaring size {size} must be refused before any blob fetch; calls={calls:?}"
            );
        }
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
        // The pinned digest is checked against the registry-reported content digest, not a re-serialization of the parsed manifest (which would never match a real registry's byte layout).
        let image = build_two_layer_image();
        let reported = image.manifest_digest.clone();
        let registry = image.into_registry();
        let (_dir, cache) = cache();
        pull_inner(&registry, &format!("alpine@{reported}"), &cache)
            .await
            .unwrap();
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

    #[test]
    fn pull_progress_emits_start_then_step_crossings_then_completion() {
        let total = MIN_PROGRESS_STEP_BYTES * 4;
        let points = capture_progress_sync(|| {
            let p = PullProgress::start(total);
            p.add(MIN_PROGRESS_STEP_BYTES - 1);
            p.add(2);
            p.add(MIN_PROGRESS_STEP_BYTES);
            p.add(2 * MIN_PROGRESS_STEP_BYTES - 1);
        });
        assert_eq!(
            points,
            vec![
                (0, total),
                (MIN_PROGRESS_STEP_BYTES + 1, total),
                (2 * MIN_PROGRESS_STEP_BYTES + 1, total),
                (total, total),
            ],
            "sub-step chunks must stay silent; boundary crossings and completion must emit",
        );
    }

    #[test]
    fn pull_progress_small_pull_emits_only_start_and_completion() {
        let points = capture_progress_sync(|| {
            let p = PullProgress::start(10);
            p.add(4);
            p.add(6);
        });
        assert_eq!(points, vec![(0, 10), (10, 10)]);
    }

    #[test]
    fn pull_progress_overshoot_beyond_declared_total_still_reports_monotonically() {
        let points = capture_progress_sync(|| {
            let p = PullProgress::start(10);
            p.add(25);
        });
        assert_eq!(
            points,
            vec![(0, 10), (25, 10)],
            "a registry sending more bytes than the manifest declared must not wedge progress",
        );
    }

    #[test]
    fn pull_progress_zero_total_reports_byte_counter_on_step_crossings() {
        let points = capture_progress_sync(|| {
            let p = PullProgress::start(0);
            p.add(MIN_PROGRESS_STEP_BYTES + 5);
        });
        assert_eq!(points, vec![(0, 0), (MIN_PROGRESS_STEP_BYTES + 5, 0)]);
    }

    #[tokio::test]
    async fn counting_sink_reports_each_chunk_and_accumulates_bytes() {
        use tokio::io::AsyncWriteExt;
        let seen = Mutex::new(Vec::<u64>::new());
        let on_chunk = |n: u64| seen.lock().unwrap().push(n);
        let mut sink = CountingSink::new(64, &on_chunk);
        sink.write_all(b"hello ").await.unwrap();
        sink.write_all(b"world").await.unwrap();
        sink.flush().await.unwrap();
        sink.shutdown().await.unwrap();
        assert_eq!(sink.into_bytes(), b"hello world");
        assert_eq!(*seen.lock().unwrap(), vec![6, 5]);
    }

    #[tokio::test]
    async fn counting_sink_stops_before_buffering_beyond_its_declared_cap() {
        use tokio::io::AsyncWriteExt;
        let mut sink = CountingSink::new(3, &|_| {});
        let err = sink.write_all(b"four").await.unwrap_err();
        assert_eq!(
            err.kind(),
            std::io::ErrorKind::InvalidData,
            "a blob that overruns its declared size must be refused, not buffered whole"
        );
        assert_eq!(sink.into_bytes(), b"fou");
    }

    #[test]
    fn blob_byte_cap_uses_the_declared_size_and_backstops_an_unknown_one() {
        assert_eq!(blob_byte_cap(42), 42);
        assert_eq!(blob_byte_cap(0), MAX_TOTAL_DECLARED_LAYER_BYTES);
        assert_eq!(blob_byte_cap(-1), MAX_TOTAL_DECLARED_LAYER_BYTES);
        assert_eq!(
            blob_byte_cap(i64::MAX),
            MAX_TOTAL_DECLARED_LAYER_BYTES,
            "a declared size over the global ceiling is clamped to it"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pull_inner_cold_cache_streams_progress_from_zero_to_missing_total() {
        let image = build_two_layer_image();
        let total: u64 = image.manifest.layers.iter().map(|l| l.size as u64).sum();
        let (_dir, cache) = cache();
        let registry = image.into_registry();
        let (result, points) =
            capture_progress(|| pull_inner(&registry, "alpine:3.20", &cache)).await;
        result.expect("cold pull");
        assert_eq!(
            points.first(),
            Some(&(0, total)),
            "the bar must appear at 0% as soon as the pull starts",
        );
        assert_eq!(
            points.last(),
            Some(&(total, total)),
            "the bar must reach 100% when the last byte lands",
        );
        assert!(
            points.windows(2).all(|w| w[0].0 < w[1].0),
            "progress must be monotone: {points:?}",
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pull_inner_partial_cache_reports_progress_against_missing_bytes_only() {
        let image = build_two_layer_image();
        let (_dir, cache) = cache();
        let (digest0, bytes0) = image.blobs[0].clone();
        cache.install_from_bytes(&digest0, &bytes0).unwrap();
        let missing = image.manifest.layers[1].size as u64;
        let registry = image.into_registry();
        let (result, points) =
            capture_progress(|| pull_inner(&registry, "alpine:3.20", &cache)).await;
        result.expect("partial pull");
        assert!(
            points.iter().all(|(_, t)| *t == missing),
            "total must count only bytes that actually need fetching: {points:?}",
        );
        assert_eq!(points.last(), Some(&(missing, missing)));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pull_inner_warm_cache_emits_no_progress_frames() {
        let image = build_two_layer_image();
        let (_dir, cache) = cache();
        for (digest, bytes) in &image.blobs {
            cache.install_from_bytes(digest, bytes).unwrap();
        }
        let registry = image.into_registry();
        let (result, points) =
            capture_progress(|| pull_inner(&registry, "alpine:3.20", &cache)).await;
        result.expect("warm pull");
        assert!(
            points.is_empty(),
            "a fully cached image has no wait to report: {points:?}",
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
