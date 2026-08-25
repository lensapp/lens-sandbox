mod real;
pub(crate) use real::RealFs;
mod traits;

pub use traits::{Caches, Fs, RuntimeCacheEntryKind, RuntimeCacheFs, RuntimeCacheMetadata};

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::image::PulledImage;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RecordKind {
    Image,
    Sandbox,
    Mixin,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageRecord {
    pub reference: String,
    pub digest: String,
    pub kind: RecordKind,
    #[serde(default)]
    pub dependencies: Vec<String>,
    pub layers: Vec<LayerRef>,
    pub pulled_unix_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LayerRef {
    pub digest: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemovedImage {
    pub reference: String,
    pub reclaimed_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PruneReport {
    pub removed: Vec<String>,
    pub reclaimed_bytes: u64,
}

pub fn normalize_reference(image: &str) -> Result<String> {
    let reference: oci_client::Reference = image
        .parse()
        .with_context(|| format!("invalid image reference: {image}"))?;
    Ok(reference.whole())
}

fn record_path(images_root: &Path, reference: &str) -> PathBuf {
    let key = hex::encode(Sha256::digest(reference.as_bytes()));
    images_root.join(format!("{key}.json"))
}

fn pinned_reference(reference: &str, digest: &str) -> Result<String> {
    let parsed: oci_client::Reference = reference
        .parse()
        .with_context(|| format!("invalid image reference: {reference}"))?;
    Ok(parsed.clone_with_digest(digest.to_string()).whole())
}

pub fn artifact_record_for(
    sandbox: &crate::image::PulledSandbox,
    base_image: &PulledImage,
    pulled_unix_secs: u64,
) -> Result<ImageRecord> {
    let mut dependencies = vec![base_image.reference.whole()];
    for mixin in &sandbox.mixins {
        dependencies.push(normalize_reference(mixin)?);
    }
    Ok(ImageRecord {
        reference: sandbox.reference.whole(),
        digest: sandbox.digest.clone(),
        kind: RecordKind::Sandbox,
        dependencies,
        layers: Vec::new(),
        pulled_unix_secs,
    })
}

/// The dependency-only index record a sandbox *run* writes so its base image is protected from `rm`/`prune` while the sandbox is live or cached — the same base linkage `lns pull` persists, minus the base's own layer record (boot writes that).
fn artifact_run_record(
    reference: &str,
    digest: &str,
    base_image: &str,
    pulled_unix_secs: u64,
) -> Result<ImageRecord> {
    Ok(ImageRecord {
        reference: normalize_reference(reference)?,
        digest: digest.to_string(),
        kind: RecordKind::Sandbox,
        dependencies: vec![normalize_reference(base_image)?],
        layers: Vec::new(),
        pulled_unix_secs,
    })
}

pub fn record_for(pulled: &PulledImage, pulled_unix_secs: u64) -> ImageRecord {
    ImageRecord {
        reference: pulled.reference.whole(),
        digest: pulled.digest.clone(),
        kind: RecordKind::Image,
        dependencies: Vec::new(),
        layers: pulled
            .layer_digests
            .iter()
            .zip(&pulled.layers)
            .map(|(digest, layer)| LayerRef {
                digest: digest.clone(),
                size_bytes: layer.data.len() as u64,
            })
            .collect(),
        pulled_unix_secs,
    }
}

pub async fn record_with<F: Fs>(fs: &F, images_root: &Path, record: &ImageRecord) -> Result<()> {
    let bytes = serde_json::to_vec(record).context("serializing image record")?;
    fs.write(&record_path(images_root, &record.reference), &bytes)
        .await
        .with_context(|| format!("writing image record for {}", record.reference))?;
    Ok(())
}

async fn load_records<F: Fs>(fs: &F, images_root: &Path) -> Result<Vec<ImageRecord>> {
    let entries = match fs.read_dir(images_root).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut records: Vec<ImageRecord> = Vec::with_capacity(entries.len());
    for path in entries
        .iter()
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("json"))
    {
        if let Ok(bytes) = fs.read(path).await
            && let Ok(record) = serde_json::from_slice(&bytes)
        {
            records.push(record);
        }
    }
    records.sort_by(|a, b| a.reference.cmp(&b.reference));
    Ok(records)
}

/// Whether a run's registered image and a record's reference name the same image, bridging the by-reference case where the run holds the tag it was given while the record is keyed by the digest that tag resolved to.
fn same_image(run_image: &str, reference: &str) -> bool {
    let (Ok(run), Ok(record)) = (
        run_image.parse::<oci_client::Reference>(),
        reference.parse::<oci_client::Reference>(),
    ) else {
        return false;
    };
    if run.whole() == record.whole() {
        return true;
    }
    run.registry() == record.registry()
        && run.repository() == record.repository()
        && (run.digest().is_some() != record.digest().is_some())
}

/// A stopped run pins its image until removal, so removal asks every listed run — the Running filter belongs to display only.
fn pinning_holder(runs: &[lns_ipc::RunSummary], reference: &str) -> Option<String> {
    runs.iter()
        .find(|r| same_image(&r.image, reference))
        .map(|r| r.id.clone())
}

fn holder(active: &[lns_ipc::RunSummary], reference: &str) -> Option<String> {
    active
        .iter()
        .filter(|r| matches!(r.status, lns_ipc::RunStatus::Running))
        .find(|r| same_image(&r.image, reference))
        .map(|r| r.id.clone())
}

fn kind_of(record: &ImageRecord) -> lns_ipc::CachedKind {
    match record.kind {
        RecordKind::Image => lns_ipc::CachedKind::Image,
        RecordKind::Sandbox => lns_ipc::CachedKind::Sandbox,
        RecordKind::Mixin => lns_ipc::CachedKind::Mixin,
    }
}

fn info_from(record: &ImageRecord, active: &[lns_ipc::RunSummary]) -> lns_ipc::ImageInfo {
    lns_ipc::ImageInfo {
        reference: record.reference.clone(),
        kind: kind_of(record),
        digest: record.digest.clone(),
        size_bytes: record.layers.iter().map(|l| l.size_bytes).sum(),
        layers: record.layers.len() as u32,
        pulled: crate::time_fmt::rfc3339_from_unix(record.pulled_unix_secs),
        in_use_by: holder(active, &record.reference),
    }
}

fn layer_keep_set<'a>(records: impl Iterator<Item = &'a ImageRecord>) -> HashSet<String> {
    records
        .flat_map(|r| r.layers.iter().map(|l| l.digest.clone()))
        .collect()
}

fn dependency_closure(records: &[ImageRecord], roots: &HashSet<String>) -> HashSet<String> {
    let mut keep = roots.clone();
    let mut pending: Vec<String> = roots.iter().cloned().collect();
    while let Some(reference) = pending.pop() {
        let Some(record) = records.iter().find(|record| record.reference == reference) else {
            continue;
        };
        for dependency in &record.dependencies {
            if keep.insert(dependency.clone()) {
                pending.push(dependency.clone());
            }
        }
    }
    keep
}

fn removal_closure(
    records: &[ImageRecord],
    root: &str,
    retained: &HashSet<String>,
) -> HashSet<String> {
    let mut remove = HashSet::from([root.to_string()]);
    loop {
        let candidates: Vec<String> = records
            .iter()
            .filter(|record| remove.contains(&record.reference))
            .flat_map(|record| record.dependencies.iter().cloned())
            .collect();
        let mut changed = false;
        for dependency in candidates {
            if retained.contains(&dependency) {
                continue;
            }
            let retained_owner = records.iter().any(|record| {
                !remove.contains(&record.reference) && record.dependencies.contains(&dependency)
            });
            if !retained_owner && records.iter().any(|record| record.reference == dependency) {
                changed |= remove.insert(dependency);
            }
        }
        if !changed {
            return remove;
        }
    }
}

fn manifest_keep_set<'a>(
    records: impl Iterator<Item = &'a ImageRecord>,
) -> Result<HashSet<String>> {
    records
        .map(|record| pinned_reference(&record.reference, &record.digest))
        .collect()
}

pub async fn list_with<F: Fs>(
    fs: &F,
    images_root: &Path,
    active: &[lns_ipc::RunSummary],
) -> Result<Vec<lns_ipc::ImageInfo>> {
    Ok(load_records(fs, images_root)
        .await?
        .iter()
        .map(|r| info_from(r, active))
        .collect())
}

pub async fn remove_with<F: Fs, C: Caches>(
    fs: &F,
    caches: &C,
    images_root: &Path,
    pinned_layers: &HashSet<String>,
    active: &[lns_ipc::RunSummary],
    image: &str,
) -> Result<RemovedImage> {
    let reference = normalize_reference(image)?;
    let records = load_records(fs, images_root).await?;
    if !records.iter().any(|record| record.reference == reference) {
        bail!("no such image: {reference}");
    }
    if let Some(run_id) = pinning_holder(active, &reference) {
        bail!(
            "image {reference:?} in use by run {}",
            lns_ipc::short_run_id(&run_id)
        );
    }
    if let Some(owner) = records.iter().find(|candidate| {
        candidate.reference != reference && candidate.dependencies.contains(&reference)
    }) {
        bail!(
            "image {reference:?} is required by cached sandbox {:?}",
            owner.reference
        );
    }
    let active_references: HashSet<String> = records
        .iter()
        .filter(|record| pinning_holder(active, &record.reference).is_some())
        .map(|record| record.reference.clone())
        .collect();
    let removed_references = removal_closure(&records, &reference, &active_references);
    for removed in records
        .iter()
        .filter(|candidate| removed_references.contains(&candidate.reference))
    {
        fs.remove_file(&record_path(images_root, &removed.reference))
            .await
            .with_context(|| format!("removing image record for {}", removed.reference))?;
    }
    let surviving: Vec<&ImageRecord> = records
        .iter()
        .filter(|candidate| !removed_references.contains(&candidate.reference))
        .collect();
    let surviving_manifests = manifest_keep_set(surviving.iter().copied())?;
    for removed in records
        .iter()
        .filter(|candidate| removed_references.contains(&candidate.reference))
    {
        let pinned = pinned_reference(&removed.reference, &removed.digest)?;
        if pinned != removed.reference {
            caches.remove_manifest(&removed.reference)?;
        }
        if !surviving_manifests.contains(&pinned) {
            caches.remove_manifest(&pinned)?;
        }
    }
    let mut keep = layer_keep_set(surviving.into_iter());
    keep.extend(pinned_layers.iter().cloned());
    let reclaimed_bytes = caches.sweep_layers(&keep)?;
    Ok(RemovedImage {
        reference,
        reclaimed_bytes,
    })
}

pub async fn tag_with<F: Fs>(fs: &F, images_root: &Path, from: &str, to: &str) -> Result<()> {
    let from_parsed: oci_client::Reference = from
        .parse()
        .with_context(|| format!("invalid image reference: {from}"))?;
    let to_parsed: oci_client::Reference = to
        .parse()
        .with_context(|| format!("invalid image reference: {to}"))?;
    if from_parsed.registry() != to_parsed.registry()
        || from_parsed.repository() != to_parsed.repository()
    {
        bail!(
            "cross-repository tagging isn't supported; cross-repository publication requires `lns sandbox push`"
        );
    }
    let from_ref = from_parsed.whole();
    let to_ref = to_parsed.whole();
    let bytes = match fs.read(&record_path(images_root, &from_ref)).await {
        Ok(bytes) => bytes,
        Err(_) => bail!("no such cached sandbox: {from_ref}"),
    };
    let mut record: ImageRecord =
        serde_json::from_slice(&bytes).context("parsing cached sandbox record")?;
    record.reference = to_ref;
    record_with(fs, images_root, &record).await
}

fn removable_partition(
    records: Vec<ImageRecord>,
    active: &[lns_ipc::RunSummary],
) -> (Vec<ImageRecord>, Vec<ImageRecord>) {
    let active_roots: HashSet<String> = records
        .iter()
        .filter(|record| pinning_holder(active, &record.reference).is_some())
        .map(|record| record.reference.clone())
        .collect();
    let kept_references = dependency_closure(&records, &active_roots);
    records
        .into_iter()
        .partition(|record| kept_references.contains(&record.reference))
}

pub async fn list_prunable_with<F: Fs>(
    fs: &F,
    images_root: &Path,
    active: &[lns_ipc::RunSummary],
) -> Result<Vec<lns_ipc::ImageInfo>> {
    let records = load_records(fs, images_root).await?;
    let (_, removable) = removable_partition(records, active);
    Ok(removable
        .iter()
        .map(|record| info_from(record, active))
        .collect())
}

pub async fn prune_with<F: RuntimeCacheFs, C: Caches>(
    fs: &F,
    caches: &C,
    images_root: &Path,
    pinned_layers: &HashSet<String>,
    active: &[lns_ipc::RunSummary],
) -> Result<PruneReport> {
    let records = load_records(fs, images_root).await?;
    let (kept, removable) = removable_partition(records, active);
    let kept_manifests = manifest_keep_set(kept.iter())?;
    let mut removable_manifests = HashSet::new();
    let mut removed = Vec::with_capacity(removable.len());
    for record in &removable {
        fs.remove_file(&record_path(images_root, &record.reference))
            .await
            .with_context(|| format!("removing image record for {}", record.reference))?;
        let pinned = pinned_reference(&record.reference, &record.digest)?;
        if pinned != record.reference {
            caches.remove_manifest(&record.reference)?;
        }
        removable_manifests.insert(pinned);
        removed.push(record.reference.clone());
    }
    for pinned in removable_manifests.difference(&kept_manifests) {
        caches.remove_manifest(pinned)?;
    }
    let mut keep = layer_keep_set(kept.iter());
    keep.extend(pinned_layers.iter().cloned());
    let mut reclaimed_bytes = caches.sweep_layers(&keep)?;
    if active.is_empty() {
        let cache_root = images_root.parent().unwrap_or_else(|| Path::new(""));
        reclaimed_bytes += clear_runtime_cache(fs, cache_root).await?;
    }
    Ok(PruneReport {
        removed,
        reclaimed_bytes,
    })
}

async fn clear_runtime_cache<F: RuntimeCacheFs>(fs: &F, cache_root: &Path) -> Result<u64> {
    let mut reclaimed = 0;
    for root in [
        cache_root.join("composefs"),
        cache_root
            .join("tools")
            .join(crate::tools::cache::TREES_DIR),
        cache_root.join("content"),
    ] {
        let bytes = tree_bytes(fs, &root).await?;
        match fs.remove_dir_all(&root).await {
            Ok(()) => reclaimed += bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(e)
                    .with_context(|| format!("removing runtime cache {}", root.display()));
            }
        }
    }
    Ok(reclaimed)
}

async fn tree_bytes<F: RuntimeCacheFs>(fs: &F, root: &Path) -> Result<u64> {
    let mut total = 0u64;
    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        let metadata = match fs.metadata(&path).await {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => {
                return Err(e)
                    .with_context(|| format!("inspecting runtime cache {}", path.display()));
            }
        };
        match metadata.kind {
            RuntimeCacheEntryKind::Directory => match fs.read_dir(&path).await {
                Ok(entries) => pending.extend(entries),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(e)
                        .with_context(|| format!("listing runtime cache {}", path.display()));
                }
            },
            RuntimeCacheEntryKind::RegularFile | RuntimeCacheEntryKind::Symlink => {
                total = total.saturating_add(metadata.len);
            }
            RuntimeCacheEntryKind::Other => {
                bail!(
                    "runtime cache entry {} has an unsupported file type",
                    path.display()
                );
            }
        }
    }
    Ok(total)
}

fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn images_root() -> Result<PathBuf> {
    Ok(crate::cache::root()?.join("images"))
}

fn cache_lock() -> &'static tokio::sync::RwLock<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::RwLock<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::RwLock::new(()))
}

fn runtime_cache_lock() -> &'static tokio::sync::RwLock<()> {
    static LOCK: std::sync::OnceLock<tokio::sync::RwLock<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::RwLock::new(()))
}

/// An in-flight pull holds this shared guard so a concurrent rm/prune can't sweep the layers it has installed but not yet recorded.
pub(crate) async fn lock_shared() -> tokio::sync::RwLockReadGuard<'static, ()> {
    cache_lock().read().await
}

pub(crate) async fn lock_runtime_cache_shared() -> tokio::sync::RwLockReadGuard<'static, ()> {
    runtime_cache_lock().read().await
}

pub async fn record(pulled: &PulledImage) -> Result<()> {
    record_with(
        &real::RealFs,
        &images_root()?,
        &record_for(pulled, now_unix_secs()),
    )
    .await
}

/// Persist the sandbox→base dependency for a live run so a concurrent `rm`/`prune` can't delete the base out from under a sandbox launched by reference (auto-pull-on-run, never explicitly pulled).
pub async fn record_artifact_run(reference: &str, digest: &str, base_image: &str) -> Result<()> {
    let record = artifact_run_record(reference, digest, base_image, now_unix_secs())?;
    let _shared = lock_shared().await;
    record_artifact_run_with(&real::RealFs, &images_root()?, record).await
}

/// A run's record must not displace what a pull already knew, so an existing record's edges carry over into the rewrite.
async fn record_artifact_run_with<F: Fs>(
    fs: &F,
    images_root: &Path,
    mut record: ImageRecord,
) -> Result<()> {
    if let Ok(bytes) = fs.read(&record_path(images_root, &record.reference)).await
        && let Ok(existing) = serde_json::from_slice::<ImageRecord>(&bytes)
    {
        for dependency in existing.dependencies {
            if !record.dependencies.contains(&dependency) {
                record.dependencies.push(dependency);
            }
        }
    }
    record_with(fs, images_root, &record).await
}

pub async fn pull_with<F: Fs>(
    fs: &F,
    images_root: &Path,
    record: &ImageRecord,
    active: &[lns_ipc::RunSummary],
) -> Result<lns_ipc::ImageInfo> {
    record_with(fs, images_root, record).await?;
    Ok(info_from(record, active))
}

/// Pre-provisioning only buys an offline first start and the run path provisions anyway, so a failure must not throw away a pull whose layers already landed.
fn warn_if_tools_unprovisioned(
    image: &str,
    outcome: Result<(), crate::tools::ProvisionError>,
) -> Vec<String> {
    let Err(e) = outcome else {
        return Vec::new();
    };
    let warning = match e.pull_disposition() {
        crate::tools::PullProvisionDisposition::RetryOnRun => format!(
            "Could not pre-provision the declared tools of {image} ({e}); the sandbox is cached, and the first run will retry tool provisioning."
        ),
        crate::tools::PullProvisionDisposition::PermanentRefusal => {
            format!("the declared tools of {image} cannot be provisioned on this machine: {e}")
        }
    };
    crate::log::warn!("{warning}");
    vec![warning]
}

pub enum PullOutcome {
    Sandbox {
        image: lns_ipc::ImageInfo,
        warnings: Vec<String>,
    },
    Mixin {
        reference: String,
        digest: String,
        cached_mixins: usize,
    },
}

fn verify_consented_digest(image: &str, expected: &str, actual: &str) -> Result<()> {
    if expected != actual {
        bail!(
            "{image} changed after consent: expected {expected}, got {actual}; inspect it and try again"
        );
    }
    Ok(())
}

async fn finish_pull_with<F: Fs>(
    fs: &F,
    images_root: &Path,
    record: &ImageRecord,
    active: &[lns_ipc::RunSummary],
    image: &str,
    shared: tokio::sync::RwLockReadGuard<'_, ()>,
    pre_provision: impl std::future::Future<Output = Result<(), crate::tools::ProvisionError>>,
) -> Result<(lns_ipc::ImageInfo, Vec<String>)> {
    let image_info = pull_with(fs, images_root, record, active).await?;
    drop(shared);
    let warnings = warn_if_tools_unprovisioned(image, pre_provision.await);
    Ok((image_info, warnings))
}

pub async fn pull(image: &str, expected_digest: &str) -> Result<PullOutcome> {
    let layer_cache = crate::oci_layer_cache::LayerCache::new(crate::cache::root()?.join("layers"));
    let sandbox = match crate::image::pull_artifact(image).await? {
        crate::image::PulledArtifact::Sandbox(sandbox) => sandbox,
        crate::image::PulledArtifact::Mixin(mixin) => {
            verify_consented_digest(image, expected_digest, &mixin.digest)?;
            return pull_mixin_graph(image, mixin).await;
        }
    };
    verify_consented_digest(image, expected_digest, &sandbox.digest)?;
    let shared = lock_shared().await;
    let base_image = crate::image::pull_dependency(&sandbox.base_image, &layer_cache)
        .await
        .with_context(|| format!("fetching the sandbox's base image {}", sandbox.base_image))?;
    let record = artifact_record_for(&sandbox, &base_image, now_unix_secs())?;
    let (image_info, warnings) = finish_pull_with(
        &real::RealFs,
        &images_root()?,
        &record,
        &crate::run_registry::snapshot(),
        image,
        shared,
        crate::tools::real::pre_provision_for_pull(&sandbox, &base_image),
    )
    .await?;
    Ok(PullOutcome::Sandbox {
        image: image_info,
        warnings,
    })
}

/// A mixin is config-only, so pulling one warms the manifest cache for it and every mixin it names — that is what lets a digest-pinned graph resolve offline afterwards — and records the graph in the index so `ls`, `rm`, and `prune` see it.
async fn pull_mixin_graph(image: &str, mixin: crate::image::PulledMixin) -> Result<PullOutcome> {
    let warmed =
        crate::artifact::mixin::warm(&mixin.mixins, &crate::artifact::real::RegistryMixins)
            .await
            .with_context(|| format!("caching the mixins {image} layers on"))?;
    let records = mixin_graph_records(image, &mixin.digest, &warmed, now_unix_secs())?;
    let images_root = images_root()?;
    let _shared = lock_shared().await;
    for record in &records {
        record_with(&real::RealFs, &images_root, record).await?;
    }
    Ok(PullOutcome::Mixin {
        reference: image.to_string(),
        digest: mixin.digest,
        cached_mixins: warmed.nodes.len(),
    })
}

/// One index record per document of the pulled graph, edges pinned, so holder tracking and reclamation treat a mixin like any other cached artifact.
fn mixin_graph_records(
    reference: &str,
    digest: &str,
    warmed: &crate::artifact::mixin::WarmedGraph,
    pulled_unix_secs: u64,
) -> Result<Vec<ImageRecord>> {
    let mut records = vec![ImageRecord {
        reference: normalize_reference(reference)?,
        digest: digest.to_string(),
        kind: RecordKind::Mixin,
        dependencies: warmed.roots.clone(),
        layers: Vec::new(),
        pulled_unix_secs,
    }];
    for node in &warmed.nodes {
        records.push(ImageRecord {
            reference: normalize_reference(&node.pinned)?,
            digest: digest_of_pinned(&node.pinned)?,
            kind: RecordKind::Mixin,
            dependencies: node.mixins.clone(),
            layers: Vec::new(),
            pulled_unix_secs,
        });
    }
    Ok(records)
}

fn digest_of_pinned(pinned: &str) -> Result<String> {
    let parsed: oci_client::Reference = pinned
        .parse()
        .with_context(|| format!("invalid mixin reference: {pinned}"))?;
    parsed
        .digest()
        .map(str::to_string)
        .with_context(|| format!("mixin {pinned} resolved without a digest pin"))
}

#[cfg(test)]
mod consent_tests {
    use super::verify_consented_digest;

    #[test]
    fn pull_accepts_the_digest_the_user_inspected() {
        let digest = format!("sha256:{}", "a".repeat(64));
        verify_consented_digest("ghcr.io/team/hermes:1", &digest, &digest).unwrap();
    }

    #[test]
    fn pull_refuses_a_tag_that_changed_after_consent() {
        let expected = format!("sha256:{}", "a".repeat(64));
        let actual = format!("sha256:{}", "b".repeat(64));

        let err = verify_consented_digest("ghcr.io/team/hermes:1", &expected, &actual).unwrap_err();

        assert!(
            err.to_string().contains("changed after consent"),
            "got: {err}"
        );
        assert!(err.to_string().contains(&expected), "got: {err}");
        assert!(err.to_string().contains(&actual), "got: {err}");
    }
}

pub async fn list() -> Result<Vec<lns_ipc::ImageInfo>> {
    list_with(
        &real::RealFs,
        &images_root()?,
        &crate::run_registry::snapshot(),
    )
    .await
}

pub async fn list_prunable() -> Result<Vec<lns_ipc::ImageInfo>> {
    list_prunable_with(
        &real::RealFs,
        &images_root()?,
        &crate::run_registry::snapshot(),
    )
    .await
}

pub async fn remove(image: &str) -> Result<RemovedImage> {
    let _exclusive = cache_lock().write().await;
    remove_with(
        &real::RealFs,
        &real::RealCaches::new(&crate::cache::root()?),
        &images_root()?,
        &recorded_run_pins().await?,
        &crate::run_registry::snapshot(),
        image,
    )
    .await
}

async fn recorded_run_pins() -> Result<HashSet<String>> {
    let scan = crate::run_record::load_all_with(&real::RealFs, &crate::cache::root()?).await?;
    Ok(crate::run_record::pinned_digests(&scan.records))
}

pub async fn tag(from: &str, to: &str) -> Result<()> {
    let _exclusive = cache_lock().write().await;
    tag_with(&real::RealFs, &images_root()?, from, to).await
}

pub async fn prune() -> Result<PruneReport> {
    let _runtime_exclusive = runtime_cache_lock().write().await;
    let _exclusive = cache_lock().write().await;
    let mut report = prune_with(
        &real::RealFs,
        &real::RealCaches::new(&crate::cache::root()?),
        &images_root()?,
        &recorded_run_pins().await?,
        &crate::run_registry::snapshot(),
    )
    .await?;
    report.reclaimed_bytes +=
        crate::build_cache::sweep_with(&real::RealFs, &lns_ipc::build_cache_root()?).await?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    fn warnings_from(body: impl FnOnce()) -> String {
        use tracing_subscriber::layer::SubscriberExt;
        #[derive(Clone, Default)]
        struct Capture(std::sync::Arc<Mutex<Vec<String>>>);
        impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for Capture {
            fn on_event(
                &self,
                event: &tracing::Event<'_>,
                _: tracing_subscriber::layer::Context<'_, S>,
            ) {
                struct Message(String);
                impl tracing::field::Visit for Message {
                    fn record_debug(
                        &mut self,
                        field: &tracing::field::Field,
                        value: &dyn std::fmt::Debug,
                    ) {
                        if field.name() == "message" {
                            self.0 = format!("{value:?}");
                        }
                    }
                }
                let mut message = Message(String::new());
                event.record(&mut message);
                self.0.lock().unwrap().push(message.0);
            }
        }
        let capture = Capture::default();
        let subscriber = tracing_subscriber::registry().with(capture.clone());
        tracing::subscriber::with_default(subscriber, body);
        capture.0.lock().unwrap().join("\n")
    }

    #[test]
    fn a_refusal_no_network_can_fix_does_not_read_as_an_offline_note() {
        let warned = warnings_from(|| {
            warn_if_tools_unprovisioned(
                "ghcr.io/team/hermes:1.4.0",
                Err(crate::tools::ProvisionError::LibcUnsupported {
                    tool: "deno@2".into(),
                    name: "deno".into(),
                    image: "alpine:3.20".into(),
                    reason: "Deno publishes no musl builds".into(),
                }),
            );
        });
        assert!(
            warned.contains("cannot be provisioned on this machine")
                && warned.contains("no musl builds"),
            "the operator is told the real answer: {warned}"
        );
        assert!(
            !warned.contains("needs the network"),
            "no network will fix it: {warned}"
        );
    }

    #[test]
    fn a_local_tool_pre_provision_failure_promises_a_retry_without_prescribing_network() {
        let warned = warnings_from(|| {
            warn_if_tools_unprovisioned(
                "ghcr.io/team/hermes:1.4.0",
                Err(crate::tools::ProvisionError::Engine(
                    "virtiofsd does not support read-only shares".into(),
                )),
            );
        });
        assert!(
            warned.contains("ghcr.io/team/hermes:1.4.0")
                && warned.contains("virtiofsd does not support read-only shares")
                && warned.contains("the first run will retry tool provisioning"),
            "the operator learns which sandbox will retry and why: {warned}"
        );
        assert!(
            !warned.contains("network"),
            "networking cannot repair this local failure: {warned}"
        );
    }

    #[test]
    fn a_provisioned_pull_says_nothing() {
        let quiet = warnings_from(|| {
            warn_if_tools_unprovisioned("ghcr.io/team/hermes:1.4.0", Ok(()));
        });
        assert!(quiet.is_empty(), "got: {quiet}");
    }

    use super::*;
    use std::collections::HashMap;
    use std::io;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeFs {
        files: Mutex<HashMap<PathBuf, Vec<u8>>>,
        metadata_overrides: Mutex<HashMap<PathBuf, RuntimeCacheMetadata>>,
        read_calls: Mutex<Vec<PathBuf>>,
        fail_read_dir: bool,
        read_dir_missing: bool,
        fail_read: bool,
        fail_metadata: bool,
        fail_write: bool,
        fail_remove: bool,
    }

    impl FakeFs {
        fn with_records(records: &[ImageRecord]) -> Self {
            let fs = Self::default();
            for r in records {
                fs.files.lock().unwrap().insert(
                    record_path(Path::new(ROOT), &r.reference),
                    serde_json::to_vec(r).unwrap(),
                );
            }
            fs
        }

        fn put(&self, p: &Path, bytes: &[u8]) {
            self.files
                .lock()
                .unwrap()
                .insert(p.to_path_buf(), bytes.to_vec());
        }

        fn has(&self, p: &Path) -> bool {
            self.files.lock().unwrap().contains_key(p)
        }

        fn put_metadata(&self, p: &Path, metadata: RuntimeCacheMetadata) {
            self.metadata_overrides
                .lock()
                .unwrap()
                .insert(p.to_path_buf(), metadata);
        }
    }

    #[tokio::test]
    async fn pull_records_the_image_before_pre_provisioning() {
        let lock = tokio::sync::RwLock::new(());
        let shared = lock.read().await;
        let fs = FakeFs::default();
        let record = rec("registry.example.test/team/agent:1", &[]);

        finish_pull_with(
            &fs,
            Path::new(ROOT),
            &record,
            &[],
            &record.reference,
            shared,
            async {
                assert!(
                    fs.has(&record_path(Path::new(ROOT), &record.reference)),
                    "the image must be committed before optional tool provisioning starts"
                );
                Ok(())
            },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn pre_provisioning_does_not_hold_the_image_cache_lock() {
        let lock = std::sync::Arc::new(tokio::sync::RwLock::new(()));
        let shared = lock.read().await;
        let writer_lock = lock.clone();
        let writer = tokio::spawn(async move {
            let _exclusive = writer_lock.write().await;
        });
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while lock.try_read().is_ok() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the test writer must queue before the simulated provisioner rootfs pull");

        let fs = FakeFs::default();
        let record = rec("registry.example.test/team/agent:1", &[]);
        finish_pull_with(
            &fs,
            Path::new(ROOT),
            &record,
            &[],
            &record.reference,
            shared,
            async {
                let _nested =
                    tokio::time::timeout(std::time::Duration::from_millis(100), lock.read())
                        .await
                        .expect(
                            "the provisioner rootfs pull must not deadlock behind a queued writer",
                        );
                Ok(())
            },
        )
        .await
        .unwrap();
        writer.await.unwrap();
    }

    #[tokio::test]
    async fn failed_pre_provisioning_keeps_the_committed_image() {
        let lock = tokio::sync::RwLock::new(());
        let shared = lock.read().await;
        let fs = FakeFs::default();
        let record = rec("registry.example.test/team/agent:1", &[]);

        let outcome = finish_pull_with(
            &fs,
            Path::new(ROOT),
            &record,
            &[],
            &record.reference,
            shared,
            async {
                Err(crate::tools::ProvisionError::Engine(
                    "the version index is unreachable".into(),
                ))
            },
        )
        .await
        .unwrap();

        let (image, warnings) = outcome;
        assert_eq!(image.reference, record.reference);
        assert!(
            warnings.iter().any(
                |warning| warning.contains("the version index is unreachable")
                    && warning.contains("the first run will retry tool provisioning")
                    && !warning.contains("needs the network")
            ),
            "the successful pull must carry its offline-readiness warning: {warnings:?}",
        );
        assert!(
            fs.has(&record_path(Path::new(ROOT), &record.reference)),
            "optional tool provisioning must not roll back a completed image pull"
        );
    }

    impl Fs for FakeFs {
        async fn read_dir(&self, dir: &Path) -> io::Result<Vec<PathBuf>> {
            if self.fail_read_dir {
                return Err(io::Error::other("read_dir boom"));
            }
            if self.read_dir_missing {
                return Err(io::Error::from(io::ErrorKind::NotFound));
            }
            let files = self.files.lock().unwrap();
            if files.contains_key(dir) {
                return Err(io::Error::from(io::ErrorKind::NotADirectory));
            }
            let mut entries: Vec<PathBuf> = files
                .keys()
                .filter(|p| p.parent() == Some(dir))
                .cloned()
                .collect();
            entries.extend(
                self.metadata_overrides
                    .lock()
                    .unwrap()
                    .keys()
                    .filter(|p| p.parent() == Some(dir))
                    .cloned(),
            );
            entries.sort();
            entries.dedup();
            if entries.is_empty() {
                Err(io::Error::from(io::ErrorKind::NotFound))
            } else {
                Ok(entries)
            }
        }

        async fn read(&self, p: &Path) -> io::Result<Vec<u8>> {
            self.read_calls.lock().unwrap().push(p.to_path_buf());
            if self.fail_read {
                return Err(io::Error::other("read boom"));
            }
            self.files
                .lock()
                .unwrap()
                .get(p)
                .cloned()
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
        }

        async fn write(&self, p: &Path, bytes: &[u8]) -> io::Result<()> {
            if self.fail_write {
                return Err(io::Error::other("write boom"));
            }
            self.put(p, bytes);
            Ok(())
        }

        async fn remove_file(&self, p: &Path) -> io::Result<()> {
            if self.fail_remove {
                return Err(io::Error::other("remove boom"));
            }
            self.files.lock().unwrap().remove(p);
            Ok(())
        }
    }

    impl RuntimeCacheFs for FakeFs {
        async fn metadata(&self, p: &Path) -> io::Result<RuntimeCacheMetadata> {
            if self.fail_metadata {
                return Err(io::Error::other("metadata boom"));
            }
            {
                let overrides = self.metadata_overrides.lock().unwrap();
                if let Some(metadata) = overrides.get(p) {
                    return Ok(*metadata);
                }
                if overrides.keys().any(|path| path.starts_with(p)) {
                    return Ok(RuntimeCacheMetadata {
                        kind: RuntimeCacheEntryKind::Directory,
                        len: 0,
                    });
                }
            }
            let files = self.files.lock().unwrap();
            if let Some(bytes) = files.get(p) {
                return Ok(RuntimeCacheMetadata {
                    kind: RuntimeCacheEntryKind::RegularFile,
                    len: bytes.len() as u64,
                });
            }
            if files.keys().any(|path| path.starts_with(p)) {
                return Ok(RuntimeCacheMetadata {
                    kind: RuntimeCacheEntryKind::Directory,
                    len: 0,
                });
            }
            Err(io::Error::from(io::ErrorKind::NotFound))
        }

        async fn remove_dir_all(&self, p: &Path) -> io::Result<()> {
            if self.fail_remove {
                return Err(io::Error::other("remove boom"));
            }
            let mut files = self.files.lock().unwrap();
            let before = files.len();
            files.retain(|path, _| !path.starts_with(p));
            if files.len() == before {
                Err(io::Error::from(io::ErrorKind::NotFound))
            } else {
                Ok(())
            }
        }
    }

    #[derive(Default)]
    struct FakeCaches {
        swept_with: Mutex<Vec<HashSet<String>>>,
        removed_manifests: Mutex<Vec<String>>,
        freed: u64,
        fail_sweep: bool,
        fail_remove_manifest: bool,
    }

    impl Caches for FakeCaches {
        fn sweep_layers(&self, keep: &HashSet<String>) -> Result<u64> {
            if self.fail_sweep {
                bail!("sweep boom");
            }
            self.swept_with.lock().unwrap().push(keep.clone());
            Ok(self.freed)
        }

        fn remove_manifest(&self, reference: &str) -> Result<()> {
            if self.fail_remove_manifest {
                bail!("manifest remove boom");
            }
            self.removed_manifests
                .lock()
                .unwrap()
                .push(reference.to_string());
            Ok(())
        }
    }

    const ROOT: &str = "/images";

    fn no_pins() -> HashSet<String> {
        HashSet::new()
    }

    #[tokio::test]
    async fn remove_refuses_an_image_a_stopped_run_still_needs() {
        let sandbox = "registry.example.test/some-sandbox:1";
        let fs = FakeFs::with_records(&[rec(sandbox, &[])]);
        let stopped = lns_ipc::RunSummary {
            status: lns_ipc::RunStatus::Exited { code: 0 },
            ..running("aa07", sandbox)
        };
        let err = remove_with(
            &fs,
            &FakeCaches::default(),
            Path::new(ROOT),
            &no_pins(),
            &[stopped],
            sandbox,
        )
        .await
        .unwrap_err();
        assert!(
            err.to_string().contains("in use by run aa07"),
            "a stopped run pins its image until the run is removed, got: {err}"
        );
    }

    #[tokio::test]
    async fn prune_keeps_the_image_and_runtime_cache_a_stopped_run_still_needs() {
        let sandbox = "registry.example.test/some-sandbox:1";
        let fs = FakeFs::with_records(&[rec(sandbox, &[])]);
        fs.put(
            &Path::new(ROOT)
                .parent()
                .unwrap()
                .join("composefs")
                .join("d"),
            b"descriptor",
        );
        let caches = FakeCaches::default();
        let stopped = lns_ipc::RunSummary {
            status: lns_ipc::RunStatus::Exited { code: 0 },
            ..running("aa07", sandbox)
        };
        let report = prune_with(&fs, &caches, Path::new(ROOT), &no_pins(), &[stopped])
            .await
            .unwrap();
        assert!(
            report.removed.is_empty(),
            "a stopped run's image is not prunable: {:?}",
            report.removed
        );
        assert!(
            fs.has(
                &Path::new(ROOT)
                    .parent()
                    .unwrap()
                    .join("composefs")
                    .join("d")
            ),
            "the composefs descriptors a stopped run boots from must survive prune"
        );
    }

    #[tokio::test]
    async fn sweep_never_drops_a_layer_a_run_record_pins() {
        let sandbox = "registry.example.test/some-sandbox:1";
        let fs = FakeFs::with_records(&[rec(sandbox, &[])]);
        let caches = FakeCaches::default();
        let pins: HashSet<String> = ["sha256:pinned-by-a-plain-image-run".to_string()].into();
        prune_with(&fs, &caches, Path::new(ROOT), &pins, &[])
            .await
            .unwrap();
        let swept = caches.swept_with.lock().unwrap();
        assert!(
            swept[0].contains("sha256:pinned-by-a-plain-image-run"),
            "recorded layer digests ride in the keep set: {:?}",
            swept[0]
        );
    }

    fn rec(reference: &str, layers: &[(&str, u64)]) -> ImageRecord {
        ImageRecord {
            reference: reference.to_string(),
            digest: format!("sha256:{}", "d".repeat(64)),
            kind: RecordKind::Image,
            dependencies: Vec::new(),
            layers: layers
                .iter()
                .map(|(digest, size)| LayerRef {
                    digest: digest.to_string(),
                    size_bytes: *size,
                })
                .collect(),
            pulled_unix_secs: 1_765_022_400,
        }
    }

    fn running(id: &str, image: &str) -> lns_ipc::RunSummary {
        lns_ipc::RunSummary {
            id: id.to_string(),
            name: String::new(),
            image: image.to_string(),
            command: String::new(),
            status: lns_ipc::RunStatus::Running,
            started: String::new(),
        }
    }

    #[test]
    fn normalize_reference_fills_in_registry_namespace_and_tag() {
        assert_eq!(
            normalize_reference("some-image").unwrap(),
            "docker.io/library/some-image:latest"
        );
        assert_eq!(
            normalize_reference("registry.example.test/some/image:1.0").unwrap(),
            "registry.example.test/some/image:1.0"
        );
    }

    #[test]
    fn normalize_reference_keeps_a_digest_pin() {
        let pinned = format!("registry.example.test/some/image@sha256:{}", "a".repeat(64));
        assert_eq!(normalize_reference(&pinned).unwrap(), pinned);
    }

    #[test]
    fn normalize_reference_rejects_garbage() {
        let err = normalize_reference("###").unwrap_err().to_string();
        assert!(err.contains("invalid image reference"), "got: {err}");
    }

    #[tokio::test]
    async fn tag_with_allows_a_new_tag_in_the_same_registry_and_repository() {
        let from = "registry.example.test/team/hermes:1.4.0";
        let fs = FakeFs::with_records(&[rec(from, &[("sha256:layer", 10)])]);
        tag_with(
            &fs,
            Path::new(ROOT),
            from,
            "registry.example.test/team/hermes:latest",
        )
        .await
        .unwrap();
        let listed = list_with(&fs, Path::new(ROOT), &[]).await.unwrap();
        let digests: Vec<&str> = listed.iter().map(|i| i.digest.as_str()).collect();
        assert_eq!(listed.len(), 2, "both the original and the tag are listed");
        assert!(
            digests
                .iter()
                .all(|d| *d == format!("sha256:{}", "d".repeat(64))),
            "the tag points at the same cached artifact: {digests:?}"
        );
    }

    #[tokio::test]
    async fn tag_with_rejects_a_different_repository_without_creating_a_record() {
        let from = "registry.example.test/team/hermes:1.4.0";
        let to = "registry.example.test/other/hermes:latest";
        let fs = FakeFs::with_records(&[rec(from, &[])]);

        let err = tag_with(&fs, Path::new(ROOT), from, to)
            .await
            .unwrap_err()
            .to_string();

        assert!(
            err.contains("cross-repository publication requires `lns sandbox push`"),
            "got: {err}"
        );
        assert!(!fs.has(&record_path(Path::new(ROOT), to)));
    }

    #[tokio::test]
    async fn tag_with_rejects_a_different_registry_without_creating_a_record() {
        let from = "registry.example.test/team/hermes:1.4.0";
        let to = "other.example.test/team/hermes:latest";
        let fs = FakeFs::with_records(&[rec(from, &[])]);

        let err = tag_with(&fs, Path::new(ROOT), from, to)
            .await
            .unwrap_err()
            .to_string();

        assert!(
            err.contains("cross-repository publication requires `lns sandbox push`"),
            "got: {err}"
        );
        assert!(!fs.has(&record_path(Path::new(ROOT), to)));
    }

    #[tokio::test]
    async fn tag_with_refuses_an_uncached_source() {
        let fs = FakeFs::default();
        let err = tag_with(&fs, Path::new(ROOT), "absent:1", "absent:2")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("no such cached sandbox"), "got: {err}");
    }

    #[test]
    fn artifact_record_for_normalizes_the_reference_and_links_its_base_image_and_mixins() {
        let base_reference = format!("registry.example.test/base@sha256:{}", "a".repeat(64));
        let mixin_reference = format!("registry.example.test/team/lint@sha256:{}", "b".repeat(64));
        let sandbox = crate::image::PulledSandbox {
            reference: "some-sandbox:1.0".parse().unwrap(),
            digest: "sha256:manifest".into(),
            base_image: base_reference.clone(),
            mixins: vec![mixin_reference.clone()],
            tools: Vec::new(),
        };
        let base_image = PulledImage {
            reference: base_reference.parse().unwrap(),
            digest: format!("sha256:{}", "a".repeat(64)),
            layers: Vec::new(),
            config: oci_client::config::ConfigFile {
                architecture: "arm64".into(),
                os: "linux".into(),
                ..Default::default()
            },
            layer_digests: Vec::new(),
            artifact_type: None,
            config_media_type: "application/vnd.oci.image.config.v1+json".into(),
        };
        let record = artifact_record_for(&sandbox, &base_image, 42).unwrap();
        assert_eq!(record.reference, "docker.io/library/some-sandbox:1.0");
        assert_eq!(record.digest, "sha256:manifest");
        assert_eq!(record.kind, RecordKind::Sandbox);
        assert_eq!(record.pulled_unix_secs, 42);
        assert_eq!(record.dependencies, vec![base_reference, mixin_reference]);
        assert_eq!(
            record.layers,
            vec![],
            "a config-only artifact holds no reclaimable layers",
        );
    }

    #[test]
    fn record_for_normalizes_the_reference_and_measures_layers() {
        let reference: oci_client::Reference = "some-image:1.0".parse().unwrap();
        let pulled = PulledImage {
            reference,
            digest: "sha256:manifest".into(),
            layers: vec![oci_client::client::ImageLayer::new(
                vec![0u8; 7],
                "application/vnd.oci.image.layer.v1.tar".into(),
                None,
            )],
            config: oci_client::config::ConfigFile {
                architecture: "arm64".into(),
                os: "linux".into(),
                ..Default::default()
            },
            layer_digests: vec![format!("sha256:{}", "a".repeat(64))],
            artifact_type: None,
            config_media_type: "application/vnd.oci.image.config.v1+json".into(),
        };
        let record = record_for(&pulled, 42);
        assert_eq!(record.reference, "docker.io/library/some-image:1.0");
        assert_eq!(record.digest, "sha256:manifest");
        assert_eq!(record.pulled_unix_secs, 42);
        assert_eq!(
            record.layers,
            vec![LayerRef {
                digest: format!("sha256:{}", "a".repeat(64)),
                size_bytes: 7,
            }]
        );
    }

    #[tokio::test]
    async fn record_then_list_round_trips_through_the_index() {
        let fs = FakeFs::default();
        let record = rec("registry.example.test/some/image:1.0", &[("sha256:aa", 3)]);
        record_with(&fs, Path::new(ROOT), &record).await.unwrap();
        let listed = list_with(&fs, Path::new(ROOT), &[]).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].reference, "registry.example.test/some/image:1.0");
        assert_eq!(listed[0].size_bytes, 3);
        assert_eq!(listed[0].layers, 1);
        assert_eq!(listed[0].pulled, "2025-12-06T12:00:00Z");
        assert_eq!(listed[0].in_use_by, None);
    }

    #[tokio::test]
    async fn record_write_failure_names_the_reference() {
        let fs = FakeFs {
            fail_write: true,
            ..Default::default()
        };
        let err = record_with(
            &fs,
            Path::new(ROOT),
            &rec("registry.example.test/some/image:1.0", &[]),
        )
        .await
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("registry.example.test/some/image:1.0"),
            "got: {msg}"
        );
    }

    #[tokio::test]
    async fn list_of_a_missing_index_is_empty_not_an_error() {
        let fs = FakeFs {
            read_dir_missing: true,
            ..Default::default()
        };
        assert!(
            list_with(&fs, Path::new(ROOT), &[])
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn list_propagates_a_read_dir_failure() {
        let fs = FakeFs {
            fail_read_dir: true,
            ..Default::default()
        };
        let err = list_with(&fs, Path::new(ROOT), &[])
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("read_dir boom"), "got: {err}");
    }

    #[tokio::test]
    async fn list_sorts_by_reference_and_skips_non_index_files() {
        let fs = FakeFs::with_records(&[
            rec("registry.example.test/zeta:1", &[]),
            rec("registry.example.test/alpha:1", &[]),
        ]);
        fs.put(&Path::new(ROOT).join("notes.txt"), b"not a record");
        let names: Vec<String> = list_with(&fs, Path::new(ROOT), &[])
            .await
            .unwrap()
            .into_iter()
            .map(|i| i.reference)
            .collect();
        assert_eq!(
            names,
            vec![
                "registry.example.test/alpha:1".to_string(),
                "registry.example.test/zeta:1".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn list_skips_a_corrupt_record_rather_than_failing() {
        let fs = FakeFs::with_records(&[rec("registry.example.test/ok:1", &[])]);
        fs.put(&Path::new(ROOT).join("corrupt.json"), b"not json");
        let listed = list_with(&fs, Path::new(ROOT), &[]).await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].reference, "registry.example.test/ok:1");
    }

    #[tokio::test]
    async fn list_skips_an_unreadable_record_rather_than_failing() {
        let fs = FakeFs {
            fail_read: true,
            ..Default::default()
        };
        fs.put(&Path::new(ROOT).join("entry.json"), b"{}");
        assert!(
            list_with(&fs, Path::new(ROOT), &[])
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn list_names_the_running_holder_matching_its_raw_cli_reference() {
        let fs = FakeFs::with_records(&[rec("docker.io/library/some-image:1.0", &[])]);
        let listed = list_with(&fs, Path::new(ROOT), &[running("aa07", "some-image:1.0")])
            .await
            .unwrap();
        assert_eq!(listed[0].in_use_by, Some("aa07".to_string()));
    }

    #[tokio::test]
    async fn list_treats_an_exited_run_as_idle() {
        let fs = FakeFs::with_records(&[rec("docker.io/library/some-image:1.0", &[])]);
        let exited = lns_ipc::RunSummary {
            status: lns_ipc::RunStatus::Exited { code: 0 },
            ..running("aa07", "some-image:1.0")
        };
        let listed = list_with(&fs, Path::new(ROOT), &[exited]).await.unwrap();
        assert_eq!(listed[0].in_use_by, None);
    }

    #[tokio::test]
    async fn list_ignores_an_imageless_run() {
        let fs = FakeFs::with_records(&[rec("docker.io/library/some-image:1.0", &[])]);
        let listed = list_with(&fs, Path::new(ROOT), &[running("aa07", "<imageless>")])
            .await
            .unwrap();
        assert_eq!(listed[0].in_use_by, None);
    }

    #[tokio::test]
    async fn remove_rejects_an_invalid_reference_without_touching_anything() {
        let fs = FakeFs::default();
        let caches = FakeCaches::default();
        let err = remove_with(&fs, &caches, Path::new(ROOT), &no_pins(), &[], "###")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("invalid image reference"), "got: {err}");
        assert!(caches.swept_with.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn remove_of_an_unknown_image_names_its_normalized_reference() {
        let fs = FakeFs::default();
        let err = remove_with(
            &fs,
            &FakeCaches::default(),
            Path::new(ROOT),
            &no_pins(),
            &[],
            "absent",
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("no such image: docker.io/library/absent:latest"),
            "got: {err}"
        );
    }

    #[tokio::test]
    async fn remove_of_an_in_use_image_is_refused_naming_the_holder() {
        let fs = FakeFs::with_records(&[rec("docker.io/library/some-image:1.0", &[])]);
        let caches = FakeCaches::default();
        let err = remove_with(
            &fs,
            &caches,
            Path::new(ROOT),
            &no_pins(),
            &[running("aa07", "some-image:1.0")],
            "some-image:1.0",
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("in use by run aa07"), "got: {err}");
        assert!(fs.has(&record_path(
            Path::new(ROOT),
            "docker.io/library/some-image:1.0"
        )));
    }

    #[tokio::test]
    async fn remove_drops_the_record_and_manifest_and_keeps_only_surviving_layers() {
        let fs = FakeFs::with_records(&[
            rec(
                "registry.example.test/gone:1",
                &[("sha256:shared", 5), ("sha256:doomed", 7)],
            ),
            rec("registry.example.test/stays:1", &[("sha256:shared", 5)]),
        ]);
        let caches = FakeCaches {
            freed: 7,
            ..Default::default()
        };
        let removed = remove_with(
            &fs,
            &caches,
            Path::new(ROOT),
            &no_pins(),
            &[],
            "registry.example.test/gone:1",
        )
        .await
        .unwrap();
        assert_eq!(removed.reference, "registry.example.test/gone:1");
        assert_eq!(removed.reclaimed_bytes, 7);
        assert!(!fs.has(&record_path(
            Path::new(ROOT),
            "registry.example.test/gone:1"
        )));
        assert_eq!(
            *caches.removed_manifests.lock().unwrap(),
            vec![
                "registry.example.test/gone:1".to_string(),
                format!("registry.example.test/gone@sha256:{}", "d".repeat(64))
            ]
        );
        let swept = caches.swept_with.lock().unwrap();
        assert_eq!(swept.len(), 1);
        assert_eq!(
            swept[0],
            HashSet::from(["sha256:shared".to_string()]),
            "the surviving image's layer must be kept; the doomed-only layer must not"
        );
    }

    #[tokio::test]
    async fn remove_keeps_a_digest_manifest_used_by_another_tag() {
        let fs = FakeFs::with_records(&[
            rec("registry.example.test/team/image:old", &[]),
            rec("registry.example.test/team/image:current", &[]),
        ]);
        let caches = FakeCaches::default();

        remove_with(
            &fs,
            &caches,
            Path::new(ROOT),
            &no_pins(),
            &[],
            "registry.example.test/team/image:old",
        )
        .await
        .unwrap();

        assert_eq!(
            *caches.removed_manifests.lock().unwrap(),
            vec!["registry.example.test/team/image:old".to_string()]
        );
    }

    #[tokio::test]
    async fn remove_propagates_a_record_delete_failure() {
        let fs = FakeFs::with_records(&[rec("registry.example.test/gone:1", &[])]);
        fs.put(&Path::new(ROOT).join("sentinel.json"), b"x");
        let failing = FakeFs {
            files: Mutex::new(fs.files.lock().unwrap().clone()),
            fail_remove: true,
            ..Default::default()
        };
        let err = remove_with(
            &failing,
            &FakeCaches::default(),
            Path::new(ROOT),
            &no_pins(),
            &[],
            "registry.example.test/gone:1",
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("remove boom"), "got: {err:#}");
    }

    #[tokio::test]
    async fn remove_propagates_a_manifest_cache_failure() {
        let fs = FakeFs::with_records(&[rec("registry.example.test/gone:1", &[])]);
        let caches = FakeCaches {
            fail_remove_manifest: true,
            ..Default::default()
        };
        let err = remove_with(
            &fs,
            &caches,
            Path::new(ROOT),
            &no_pins(),
            &[],
            "registry.example.test/gone:1",
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("manifest remove boom"), "got: {err}");
    }

    #[tokio::test]
    async fn remove_propagates_a_sweep_failure() {
        let fs = FakeFs::with_records(&[rec("registry.example.test/gone:1", &[])]);
        let caches = FakeCaches {
            fail_sweep: true,
            ..Default::default()
        };
        let err = remove_with(
            &fs,
            &caches,
            Path::new(ROOT),
            &no_pins(),
            &[],
            "registry.example.test/gone:1",
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("sweep boom"), "got: {err}");
    }

    #[tokio::test]
    async fn prune_removes_idle_images_and_keeps_layers_of_in_use_ones() {
        let fs = FakeFs::with_records(&[
            rec("docker.io/library/held:1.0", &[("sha256:held-layer", 9)]),
            rec("registry.example.test/idle:1", &[("sha256:idle-layer", 4)]),
        ]);
        let caches = FakeCaches {
            freed: 4,
            ..Default::default()
        };
        let report = prune_with(
            &fs,
            &caches,
            Path::new(ROOT),
            &no_pins(),
            &[running("aa03", "held:1.0")],
        )
        .await
        .unwrap();
        assert_eq!(report.removed, vec!["registry.example.test/idle:1"]);
        assert_eq!(report.reclaimed_bytes, 4);
        assert!(fs.has(&record_path(Path::new(ROOT), "docker.io/library/held:1.0")));
        let swept = caches.swept_with.lock().unwrap();
        assert_eq!(swept[0], HashSet::from(["sha256:held-layer".to_string()]));
    }

    #[tokio::test]
    async fn prune_keeps_the_base_image_record_and_layers_of_an_active_sandbox() {
        let sandbox = "registry.example.test/team/sandbox:1";
        let base = "registry.example.test/team/base:1";
        let mut sandbox_record = rec(sandbox, &[]);
        sandbox_record.dependencies.push(base.to_string());
        let fs = FakeFs::with_records(&[sandbox_record, rec(base, &[("sha256:base-layer", 9)])]);
        let caches = FakeCaches {
            freed: 4,
            ..Default::default()
        };

        let report = prune_with(
            &fs,
            &caches,
            Path::new(ROOT),
            &no_pins(),
            &[running("aa03", sandbox)],
        )
        .await
        .unwrap();

        assert!(report.removed.is_empty());
        assert!(fs.has(&record_path(Path::new(ROOT), base)));
        assert_eq!(
            caches.swept_with.lock().unwrap()[0],
            HashSet::from(["sha256:base-layer".to_string()])
        );
    }

    #[test]
    fn same_image_bridges_a_by_reference_run_tag_to_its_resolved_digest_record() {
        let tag = "registry.example.test/team/sandbox:1";
        let digest = format!(
            "registry.example.test/team/sandbox@sha256:{}",
            "c".repeat(64)
        );
        assert!(same_image(tag, tag), "an exact reference is the same image");
        assert!(
            same_image(tag, &digest),
            "a run registered by tag holds the record keyed by the digest that tag resolved to"
        );
        assert!(
            same_image(&digest, tag),
            "the bridge holds in both directions"
        );
        assert!(
            !same_image(tag, "registry.example.test/team/sandbox:2"),
            "two distinct tags of one repository are not the same image"
        );
        assert!(
            !same_image(
                &digest,
                &format!(
                    "registry.example.test/team/sandbox@sha256:{}",
                    "d".repeat(64)
                )
            ),
            "two distinct digests of one repository are not the same image"
        );
        assert!(
            !same_image(tag, "registry.example.test/team/base:1"),
            "different repositories are never the same image"
        );
        assert!(
            !same_image("", tag),
            "an unparseable reference matches nothing"
        );
    }

    #[tokio::test]
    async fn prune_keeps_the_base_of_a_by_reference_sandbox_run_by_tag() {
        let sandbox_tag = "registry.example.test/team/sandbox:1";
        let sandbox_pinned = format!(
            "registry.example.test/team/sandbox@sha256:{}",
            "b".repeat(64)
        );
        let base = "registry.example.test/team/base:1";
        let mut sandbox_record = rec(&sandbox_pinned, &[]);
        sandbox_record.dependencies.push(base.to_string());
        let fs = FakeFs::with_records(&[sandbox_record, rec(base, &[("sha256:base-layer", 9)])]);
        let caches = FakeCaches {
            freed: 4,
            ..Default::default()
        };

        let report = prune_with(
            &fs,
            &caches,
            Path::new(ROOT),
            &no_pins(),
            &[running("aa03", sandbox_tag)],
        )
        .await
        .unwrap();

        assert!(
            fs.has(&record_path(Path::new(ROOT), base)),
            "an auto-pulled sandbox registers its run under the raw tag but records base retention under the resolved digest; prune must still recognize the run as the holder and keep the base, but it was reclaimed: {report:?}"
        );
        assert_eq!(
            caches.swept_with.lock().unwrap()[0],
            HashSet::from(["sha256:base-layer".to_string()]),
            "the base image's layer blobs must be kept while the sandbox is running"
        );
    }

    #[tokio::test]
    async fn remove_of_a_sandbox_removes_its_unshared_base_image_and_layers() {
        let sandbox = "registry.example.test/team/sandbox:1";
        let base = "registry.example.test/team/base:1";
        let mut sandbox_record = rec(sandbox, &[]);
        sandbox_record.dependencies.push(base.to_string());
        let fs = FakeFs::with_records(&[sandbox_record, rec(base, &[("sha256:base-layer", 9)])]);
        let caches = FakeCaches {
            freed: 9,
            ..Default::default()
        };

        let removed = remove_with(&fs, &caches, Path::new(ROOT), &no_pins(), &[], sandbox)
            .await
            .unwrap();

        assert_eq!(removed.reclaimed_bytes, 9);
        assert!(!fs.has(&record_path(Path::new(ROOT), sandbox)));
        assert!(!fs.has(&record_path(Path::new(ROOT), base)));
        assert_eq!(caches.swept_with.lock().unwrap()[0], HashSet::new());
    }

    #[tokio::test]
    async fn remove_of_a_sandbox_keeps_a_base_image_shared_by_another_sandbox() {
        let first = "registry.example.test/team/first:1";
        let second = "registry.example.test/team/second:1";
        let base = "registry.example.test/team/base:1";
        let mut first_record = rec(first, &[]);
        first_record.dependencies.push(base.to_string());
        let mut second_record = rec(second, &[]);
        second_record.dependencies.push(base.to_string());
        let fs = FakeFs::with_records(&[
            first_record,
            second_record,
            rec(base, &[("sha256:base-layer", 9)]),
        ]);

        remove_with(
            &fs,
            &FakeCaches::default(),
            Path::new(ROOT),
            &no_pins(),
            &[],
            first,
        )
        .await
        .unwrap();

        assert!(fs.has(&record_path(Path::new(ROOT), second)));
        assert!(fs.has(&record_path(Path::new(ROOT), base)));
    }

    #[tokio::test]
    async fn remove_of_a_sandbox_keeps_a_base_image_held_by_a_running_workload() {
        let sandbox = "registry.example.test/team/sandbox:1";
        let base = "registry.example.test/team/base:1";
        let mut sandbox_record = rec(sandbox, &[]);
        sandbox_record.dependencies.push(base.to_string());
        let fs = FakeFs::with_records(&[sandbox_record, rec(base, &[("sha256:base-layer", 9)])]);

        remove_with(
            &fs,
            &FakeCaches::default(),
            Path::new(ROOT),
            &no_pins(),
            &[running("aa04", base)],
            sandbox,
        )
        .await
        .unwrap();

        assert!(fs.has(&record_path(Path::new(ROOT), base)));
    }

    #[test]
    fn artifact_run_record_links_the_running_sandbox_to_its_normalized_base() {
        let base = format!("registry.example.test/team/base@sha256:{}", "a".repeat(64));
        let record =
            artifact_run_record("ghcr.io/team/agent:1", "sha256:manifest", &base, 42).unwrap();
        assert_eq!(record.reference, "ghcr.io/team/agent:1");
        assert_eq!(record.digest, "sha256:manifest");
        assert_eq!(record.dependencies, vec![base]);
        assert!(record.layers.is_empty());
        assert_eq!(record.pulled_unix_secs, 42);
    }

    #[tokio::test]
    async fn a_recorded_artifact_run_protects_its_base_from_removal() {
        let base = "registry.example.test/team/base:1";
        let sandbox_record =
            artifact_run_record("registry.example.test/team/agent:1", "sha256:m", base, 7).unwrap();
        let fs = FakeFs::with_records(&[sandbox_record, rec(base, &[])]);

        let err = remove_with(
            &fs,
            &FakeCaches::default(),
            Path::new(ROOT),
            &no_pins(),
            &[],
            base,
        )
        .await
        .unwrap_err()
        .to_string();

        assert!(
            err.contains("required by cached sandbox"),
            "a base recorded by an auto-pull-on-run sandbox must not be removable: {err}"
        );
    }

    #[tokio::test]
    async fn remove_refuses_a_base_image_required_by_a_cached_sandbox() {
        let sandbox = "registry.example.test/team/sandbox:1";
        let base = "registry.example.test/team/base:1";
        let mut sandbox_record = rec(sandbox, &[]);
        sandbox_record.dependencies.push(base.to_string());
        let fs = FakeFs::with_records(&[sandbox_record, rec(base, &[])]);

        let err = remove_with(
            &fs,
            &FakeCaches::default(),
            Path::new(ROOT),
            &no_pins(),
            &[],
            base,
        )
        .await
        .unwrap_err()
        .to_string();

        assert!(err.contains("required by cached sandbox"), "got: {err}");
        assert!(fs.has(&record_path(Path::new(ROOT), base)));
    }

    #[test]
    fn mixin_graph_records_index_the_pulled_mixin_and_every_document_it_reaches() {
        let child = format!("registry.example.test/team/lint@sha256:{}", "b".repeat(64));
        let warmed = crate::artifact::mixin::WarmedGraph {
            roots: vec![child.clone()],
            nodes: vec![crate::artifact::mixin::WarmedMixin {
                pinned: child.clone(),
                mixins: Vec::new(),
            }],
        };
        let records = mixin_graph_records(
            "registry.example.test/team/bundle:1",
            "sha256:r",
            &warmed,
            42,
        )
        .unwrap();

        assert_eq!(records.len(), 2);
        assert_eq!(records[0].reference, "registry.example.test/team/bundle:1");
        assert_eq!(records[0].digest, "sha256:r");
        assert_eq!(records[0].kind, RecordKind::Mixin);
        assert_eq!(records[0].dependencies, vec![child.clone()]);
        assert!(
            records[0].layers.is_empty(),
            "a config-only artifact holds no reclaimable layers"
        );
        assert_eq!(records[0].pulled_unix_secs, 42);
        assert_eq!(records[1].reference, child);
        assert_eq!(records[1].digest, format!("sha256:{}", "b".repeat(64)));
        assert_eq!(records[1].kind, RecordKind::Mixin);
        assert!(records[1].dependencies.is_empty());
    }

    #[test]
    fn an_invalid_mixin_reference_cannot_be_indexed() {
        let warmed = crate::artifact::mixin::WarmedGraph {
            roots: Vec::new(),
            nodes: Vec::new(),
        };
        let err = mixin_graph_records("###", "sha256:r", &warmed, 42)
            .unwrap_err()
            .to_string();
        assert!(err.contains("invalid image reference"), "got: {err}");
    }

    #[test]
    fn a_warmed_document_without_a_digest_pin_cannot_be_indexed() {
        let err = digest_of_pinned("registry.example.test/team/lint:1")
            .unwrap_err()
            .to_string();
        assert!(err.contains("resolved without a digest pin"), "got: {err}");

        let parse_err = digest_of_pinned("###").unwrap_err().to_string();
        assert!(
            parse_err.contains("invalid mixin reference"),
            "got: {parse_err}"
        );
    }

    #[tokio::test]
    async fn a_run_record_keeps_the_mixin_edges_a_pull_already_recorded() {
        let reference = "ghcr.io/team/agent:1";
        let base = "registry.example.test/team/base:1";
        let mixin = format!("registry.example.test/team/lint@sha256:{}", "b".repeat(64));
        let mut pulled = rec(reference, &[]);
        pulled.kind = RecordKind::Sandbox;
        pulled.dependencies = vec![mixin.clone()];
        let fs = FakeFs::with_records(&[pulled]);
        let run_record = artifact_run_record(reference, "sha256:m", base, 7).unwrap();

        record_artifact_run_with(&fs, Path::new(ROOT), run_record)
            .await
            .unwrap();

        let bytes = fs
            .read(&record_path(Path::new(ROOT), reference))
            .await
            .unwrap();
        let stored: ImageRecord = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(stored.kind, RecordKind::Sandbox);
        assert_eq!(stored.dependencies, vec![base.to_string(), mixin]);
    }

    #[tokio::test]
    async fn a_run_record_with_no_prior_pull_writes_its_base_edge_as_is() {
        let fs = FakeFs::default();
        let run_record = artifact_run_record(
            "ghcr.io/team/agent:1",
            "sha256:m",
            "registry.example.test/team/base:1",
            7,
        )
        .unwrap();

        record_artifact_run_with(&fs, Path::new(ROOT), run_record)
            .await
            .unwrap();

        let bytes = fs
            .read(&record_path(Path::new(ROOT), "ghcr.io/team/agent:1"))
            .await
            .unwrap();
        let stored: ImageRecord = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            stored.dependencies,
            vec!["registry.example.test/team/base:1".to_string()]
        );
    }

    #[test]
    fn a_record_written_before_kinds_were_explicit_no_longer_parses() {
        serde_json::from_str::<ImageRecord>(
            r#"{"reference":"registry.example.test/team/old:1","digest":"sha256:old","layers":[],"pulled_unix_secs":1}"#,
        )
        .expect_err("a kindless record is pre-format; the loader skips it and the artifact re-pulls");
    }

    #[tokio::test]
    async fn a_listing_skips_a_kindless_record_instead_of_failing() {
        let fs = FakeFs::default();
        fs.put(
            &record_path(Path::new(ROOT), "registry.example.test/team/old:1"),
            br#"{"reference":"registry.example.test/team/old:1","digest":"sha256:old","layers":[],"pulled_unix_secs":1}"#,
        );
        let listed = list_with(&fs, Path::new(ROOT), &[]).await.unwrap();
        assert!(listed.is_empty(), "got {listed:?}");
    }

    #[test]
    fn dependency_closure_tolerates_a_dependency_whose_record_is_missing() {
        let sandbox = "registry.example.test/team/sandbox:1";
        let missing_base = "registry.example.test/team/missing-base:1";
        let mut sandbox_record = rec(sandbox, &[]);
        sandbox_record.dependencies.push(missing_base.to_string());

        let kept = dependency_closure(&[sandbox_record], &HashSet::from([sandbox.to_string()]));

        assert_eq!(
            kept,
            HashSet::from([sandbox.to_string(), missing_base.to_string()])
        );
    }

    #[tokio::test]
    async fn prune_keeps_a_digest_manifest_used_by_a_held_alias() {
        let fs = FakeFs::with_records(&[
            rec("registry.example.test/team/image:held", &[]),
            rec("registry.example.test/team/image:idle", &[]),
        ]);
        let caches = FakeCaches::default();

        let report = prune_with(
            &fs,
            &caches,
            Path::new(ROOT),
            &no_pins(),
            &[running("aa03", "registry.example.test/team/image:held")],
        )
        .await
        .unwrap();

        assert_eq!(
            report.removed,
            vec!["registry.example.test/team/image:idle"]
        );
        assert_eq!(
            *caches.removed_manifests.lock().unwrap(),
            vec!["registry.example.test/team/image:idle".to_string()]
        );
    }

    #[tokio::test]
    async fn prune_of_an_empty_index_still_sweeps_orphaned_layer_blobs() {
        let fs = FakeFs::default();
        let caches = FakeCaches {
            freed: 11,
            ..Default::default()
        };
        let report = prune_with(&fs, &caches, Path::new(ROOT), &no_pins(), &[])
            .await
            .unwrap();
        assert!(report.removed.is_empty());
        assert_eq!(report.reclaimed_bytes, 11);
        assert_eq!(*caches.swept_with.lock().unwrap(), vec![HashSet::new()]);
    }

    #[tokio::test]
    async fn runtime_cache_listing_failure_aborts_before_removal() {
        let fs = FakeFs {
            fail_read_dir: true,
            ..Default::default()
        };
        fs.put(Path::new("/cache/composefs/descriptor"), b"descriptor");
        let err = clear_runtime_cache(&fs, Path::new("/cache"))
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("listing runtime cache /cache/composefs"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn missing_runtime_cache_roots_are_already_clean() {
        assert_eq!(
            clear_runtime_cache(&FakeFs::default(), Path::new("/cache"))
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn runtime_cache_clear_counts_and_removes_each_coordinated_root() {
        let fs = FakeFs::default();
        fs.put(Path::new("/cache/composefs/descriptor"), b"meta");
        fs.put(Path::new("/cache/tools/trees/tree"), b"tool");
        fs.put(Path::new("/cache/content/blob"), b"content");
        let reclaimed = clear_runtime_cache(&fs, Path::new("/cache")).await.unwrap();
        assert_eq!(reclaimed, 15);
        assert!(!fs.has(Path::new("/cache/composefs/descriptor")));
        assert!(!fs.has(Path::new("/cache/tools/tree")));
        assert!(!fs.has(Path::new("/cache/content/blob")));
    }

    #[tokio::test]
    async fn clearing_the_tool_cache_keeps_the_resolution_record() {
        let fs = FakeFs::default();
        fs.put(Path::new("/cache/tools/trees/node"), b"binary");
        fs.put(Path::new("/cache/tools/resolved.json"), b"{}");

        let reclaimed = clear_runtime_cache(&fs, Path::new("/cache")).await.unwrap();

        assert!(
            fs.has(Path::new("/cache/tools/resolved.json")),
            "reclaiming disk must not unpin what this machine already resolved"
        );
        assert!(!fs.has(Path::new("/cache/tools/trees/node")));
        assert_eq!(reclaimed, 6, "the record is not reclaimable space");
    }

    #[tokio::test]
    async fn runtime_cache_size_uses_metadata_without_reading_file_contents() {
        let fs = FakeFs {
            fail_read: true,
            ..Default::default()
        };
        fs.put(Path::new("/cache/content/blob"), b"content");

        let reclaimed = tree_bytes(&fs, Path::new("/cache/content"))
            .await
            .expect("metadata must be sufficient to size the cache");

        assert_eq!(reclaimed, 7);
        assert!(
            fs.read_calls.lock().unwrap().is_empty(),
            "cache sizing must not read file bodies"
        );
    }

    #[tokio::test]
    async fn runtime_cache_symlink_is_counted_without_traversing_its_target() {
        let fs = FakeFs::default();
        fs.put_metadata(
            Path::new("/cache/content/link"),
            RuntimeCacheMetadata {
                kind: RuntimeCacheEntryKind::Symlink,
                len: 14,
            },
        );
        fs.put(Path::new("/outside/large-blob"), &[0; 100]);

        let reclaimed = tree_bytes(&fs, Path::new("/cache/content")).await.unwrap();

        assert_eq!(reclaimed, 14);
    }

    #[tokio::test]
    async fn runtime_cache_special_entry_is_refused_with_its_path() {
        let fs = FakeFs::default();
        fs.put_metadata(
            Path::new("/cache/content/socket"),
            RuntimeCacheMetadata {
                kind: RuntimeCacheEntryKind::Other,
                len: 0,
            },
        );

        let err = tree_bytes(&fs, Path::new("/cache/content"))
            .await
            .unwrap_err();

        assert!(
            format!("{err:#}").contains("/cache/content/socket")
                && format!("{err:#}").contains("unsupported file type"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn fake_fs_read_dir_of_a_file_is_not_a_directory() {
        let fs = FakeFs::default();
        let path = Path::new("/cache/content/blob");
        fs.put(path, b"x");

        let error = fs.read_dir(path).await.unwrap_err();

        assert_eq!(error.kind(), io::ErrorKind::NotADirectory);
    }

    #[tokio::test]
    async fn pull_time_runtime_cache_leases_can_overlap() {
        let first = lock_runtime_cache_shared().await;
        let second = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            lock_runtime_cache_shared(),
        )
        .await
        .expect("pull-time cache readers must not serialize");
        drop((first, second));
    }

    #[tokio::test]
    async fn runtime_cache_metadata_failure_names_the_entry() {
        let fs = FakeFs {
            fail_metadata: true,
            ..Default::default()
        };
        fs.put(Path::new("/cache/composefs/descriptor"), b"descriptor");
        let err = clear_runtime_cache(&fs, Path::new("/cache"))
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("inspecting runtime cache /cache/composefs"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn runtime_cache_removal_failure_keeps_later_roots_intact() {
        let fs = FakeFs {
            fail_remove: true,
            ..Default::default()
        };
        fs.put(Path::new("/cache/composefs/descriptor"), b"descriptor");
        fs.put(Path::new("/cache/content/blob"), b"content");
        let err = clear_runtime_cache(&fs, Path::new("/cache"))
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("removing runtime cache /cache/composefs"),
            "got: {err:#}"
        );
        assert!(fs.has(Path::new("/cache/content/blob")));
    }

    #[tokio::test]
    async fn prune_propagates_a_record_delete_failure() {
        let fs = FakeFs::with_records(&[rec("registry.example.test/idle:1", &[])]);
        let failing = FakeFs {
            files: Mutex::new(fs.files.lock().unwrap().clone()),
            fail_remove: true,
            ..Default::default()
        };
        let err = prune_with(
            &failing,
            &FakeCaches::default(),
            Path::new(ROOT),
            &no_pins(),
            &[],
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("remove boom"), "got: {err:#}");
    }

    #[tokio::test]
    async fn prune_propagates_a_manifest_cache_failure() {
        let fs = FakeFs::with_records(&[rec("registry.example.test/idle:1", &[])]);
        let caches = FakeCaches {
            fail_remove_manifest: true,
            ..Default::default()
        };
        let err = prune_with(&fs, &caches, Path::new(ROOT), &no_pins(), &[])
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("manifest remove boom"), "got: {err}");
    }

    #[tokio::test]
    async fn real_fs_write_read_dir_read_and_remove_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index").join("entry.json");
        real::RealFs.write(&path, b"{\"k\":1}").await.unwrap();
        assert_eq!(
            real::RealFs.metadata(path.parent().unwrap()).await.unwrap(),
            RuntimeCacheMetadata {
                kind: RuntimeCacheEntryKind::Directory,
                len: std::fs::symlink_metadata(path.parent().unwrap())
                    .unwrap()
                    .len(),
            }
        );
        assert_eq!(
            real::RealFs.metadata(&path).await.unwrap(),
            RuntimeCacheMetadata {
                kind: RuntimeCacheEntryKind::RegularFile,
                len: 7,
            }
        );
        let link = dir.path().join("entry-link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert_eq!(
            real::RealFs.metadata(&link).await.unwrap().kind,
            RuntimeCacheEntryKind::Symlink,
            "metadata must describe the link itself rather than following its target"
        );
        let listed = real::RealFs.read_dir(path.parent().unwrap()).await.unwrap();
        assert_eq!(listed, vec![path.clone()]);
        assert_eq!(real::RealFs.read(&path).await.unwrap(), b"{\"k\":1}");
        real::RealFs.remove_file(&path).await.unwrap();
        assert!(
            real::RealFs
                .read_dir(path.parent().unwrap())
                .await
                .unwrap()
                .is_empty()
        );
        real::RealFs
            .remove_dir_all(path.parent().unwrap())
            .await
            .unwrap();
        assert!(!path.parent().unwrap().exists());
    }

    #[test]
    fn runtime_cache_kind_classifies_every_host_file_type() {
        assert_eq!(
            real::runtime_cache_kind(true, false, false),
            RuntimeCacheEntryKind::Directory
        );
        assert_eq!(
            real::runtime_cache_kind(false, true, false),
            RuntimeCacheEntryKind::RegularFile
        );
        assert_eq!(
            real::runtime_cache_kind(false, false, true),
            RuntimeCacheEntryKind::Symlink
        );
        assert_eq!(
            real::runtime_cache_kind(false, false, false),
            RuntimeCacheEntryKind::Other
        );
    }

    #[tokio::test]
    async fn real_fs_write_replaces_an_existing_record_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("entry.json");
        real::RealFs.write(&path, b"old").await.unwrap();
        real::RealFs.write(&path, b"new").await.unwrap();
        assert_eq!(real::RealFs.read(&path).await.unwrap(), b"new");
        let listed = real::RealFs.read_dir(dir.path()).await.unwrap();
        assert_eq!(listed, vec![path], "no tmp file may be left behind");
    }

    #[tokio::test]
    async fn real_fs_read_dir_of_a_missing_dir_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let absent = dir.path().join("absent");
        let err = real::RealFs.read_dir(&absent).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
        let err = real::RealFs.metadata(&absent).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[tokio::test]
    async fn real_caches_sweep_and_manifest_remove_operate_under_the_cache_root() {
        let dir = tempfile::tempdir().unwrap();
        let caches = real::RealCaches::new(dir.path());

        let layer_dir = dir.path().join("layers").join("sha256");
        std::fs::create_dir_all(&layer_dir).unwrap();
        use sha2::{Digest, Sha256};
        let bytes = b"doomed layer";
        let hex = format!("{:x}", Sha256::digest(bytes));
        std::fs::write(layer_dir.join(&hex), bytes).unwrap();

        let freed = caches.sweep_layers(&HashSet::new()).unwrap();
        assert_eq!(freed, bytes.len() as u64);

        caches
            .remove_manifest("registry.example.test/absent:1")
            .expect("removing a manifest entry that never existed is benign");
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn lifecycle_production_wrappers_round_trip_under_the_cache_root() {
        let d = tempfile::tempdir().unwrap();
        let _h = crate::test_env::EnvVarGuard::set("HOME", d.path());

        let reference: oci_client::Reference =
            "registry.example.test/cov/lifecycle:1".parse().unwrap();
        let pulled = PulledImage {
            reference,
            digest: format!("sha256:{}", "c".repeat(64)),
            layers: vec![oci_client::client::ImageLayer::new(
                b"layer".to_vec(),
                "application/vnd.oci.image.layer.v1.tar".into(),
                None,
            )],
            config: oci_client::config::ConfigFile {
                architecture: "arm64".into(),
                os: "linux".into(),
                ..Default::default()
            },
            layer_digests: vec![format!("sha256:{}", "e".repeat(64))],
            artifact_type: None,
            config_media_type: "application/vnd.oci.image.config.v1+json".into(),
        };
        record(&pulled).await.unwrap();

        let listed = list().await.unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].reference, "registry.example.test/cov/lifecycle:1");

        let removed = remove("registry.example.test/cov/lifecycle:1")
            .await
            .unwrap();
        assert_eq!(removed.reference, "registry.example.test/cov/lifecycle:1");

        record(&pulled).await.unwrap();
        let report = prune().await.unwrap();
        assert_eq!(
            report.removed,
            vec!["registry.example.test/cov/lifecycle:1".to_string()]
        );
        assert!(list().await.unwrap().is_empty());
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn record_artifact_run_persists_the_sandbox_dependency_under_the_cache_root() {
        let d = tempfile::tempdir().unwrap();
        let _h = crate::test_env::EnvVarGuard::set("HOME", d.path());
        let base = format!("registry.example.test/team/base@sha256:{}", "a".repeat(64));

        record_artifact_run("registry.example.test/team/agent:1", "sha256:m", &base)
            .await
            .unwrap();

        let listed = list().await.unwrap();
        assert!(
            listed
                .iter()
                .any(|i| i.reference == "registry.example.test/team/agent:1"),
            "an auto-pull-on-run sandbox must land in the index so its base is protected"
        );
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn concurrent_pulls_share_the_cache_lock() {
        let first = lock_shared().await;
        assert!(
            cache_lock().try_read().is_ok(),
            "two in-flight pulls must not block each other"
        );
        drop(first);
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn an_in_flight_pull_blocks_a_sweep_until_it_finishes() {
        let in_flight = lock_shared().await;
        assert!(
            cache_lock().try_write().is_err(),
            "rm/prune must wait while a pull is still installing layers"
        );
        drop(in_flight);
        assert!(
            cache_lock().try_write().is_ok(),
            "once the pull finishes the sweep may proceed"
        );
    }
}
