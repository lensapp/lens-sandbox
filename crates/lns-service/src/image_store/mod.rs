mod real;
mod traits;

pub use traits::{Caches, Fs};

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::image::PulledImage;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageRecord {
    pub reference: String,
    pub digest: String,
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

pub fn record_for(pulled: &PulledImage, pulled_unix_secs: u64) -> ImageRecord {
    ImageRecord {
        reference: pulled.reference.whole(),
        digest: pulled.digest.clone(),
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

fn holder(active: &[lns_ipc::RunSummary], reference: &str) -> Option<String> {
    active
        .iter()
        .filter(|r| matches!(r.status, lns_ipc::RunStatus::Running))
        .find(|r| normalize_reference(&r.image).is_ok_and(|whole| whole == reference))
        .map(|r| r.id.clone())
}

fn info_from(record: &ImageRecord, active: &[lns_ipc::RunSummary]) -> lns_ipc::ImageInfo {
    lns_ipc::ImageInfo {
        reference: record.reference.clone(),
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
    active: &[lns_ipc::RunSummary],
    image: &str,
) -> Result<RemovedImage> {
    let reference = normalize_reference(image)?;
    let records = load_records(fs, images_root).await?;
    if !records.iter().any(|r| r.reference == reference) {
        bail!("no such image: {reference}");
    }
    if let Some(run_id) = holder(active, &reference) {
        bail!(
            "image {reference:?} in use by run {}",
            lns_ipc::short_run_id(&run_id)
        );
    }
    fs.remove_file(&record_path(images_root, &reference))
        .await
        .with_context(|| format!("removing image record for {reference}"))?;
    caches.remove_manifest(&reference)?;
    let keep = layer_keep_set(records.iter().filter(|r| r.reference != reference));
    let reclaimed_bytes = caches.sweep_layers(&keep)?;
    Ok(RemovedImage {
        reference,
        reclaimed_bytes,
    })
}

pub async fn prune_with<F: Fs, C: Caches>(
    fs: &F,
    caches: &C,
    images_root: &Path,
    active: &[lns_ipc::RunSummary],
) -> Result<PruneReport> {
    let records = load_records(fs, images_root).await?;
    let (kept, removable): (Vec<_>, Vec<_>) = records
        .into_iter()
        .partition(|r| holder(active, &r.reference).is_some());
    let mut removed = Vec::with_capacity(removable.len());
    for record in &removable {
        fs.remove_file(&record_path(images_root, &record.reference))
            .await
            .with_context(|| format!("removing image record for {}", record.reference))?;
        caches.remove_manifest(&record.reference)?;
        removed.push(record.reference.clone());
    }
    let reclaimed_bytes = caches.sweep_layers(&layer_keep_set(kept.iter()))?;
    Ok(PruneReport {
        removed,
        reclaimed_bytes,
    })
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

/// An in-flight pull holds this shared guard so a concurrent rm/prune can't sweep the layers it has installed but not yet recorded.
pub(crate) async fn lock_shared() -> tokio::sync::RwLockReadGuard<'static, ()> {
    cache_lock().read().await
}

pub async fn record(pulled: &PulledImage) -> Result<()> {
    record_with(
        &real::RealFs,
        &images_root()?,
        &record_for(pulled, now_unix_secs()),
    )
    .await
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

pub async fn pull(image: &str) -> Result<lns_ipc::ImageInfo> {
    let layer_cache = crate::oci_layer_cache::LayerCache::new(crate::cache::root()?.join("layers"));
    let pulled = crate::image::pull(image, &layer_cache).await?;
    pull_with(
        &real::RealFs,
        &images_root()?,
        &record_for(&pulled, now_unix_secs()),
        &crate::run_registry::snapshot(),
    )
    .await
}

pub async fn list() -> Result<Vec<lns_ipc::ImageInfo>> {
    list_with(
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
        &crate::run_registry::snapshot(),
        image,
    )
    .await
}

pub async fn prune() -> Result<PruneReport> {
    let _exclusive = cache_lock().write().await;
    let mut report = prune_with(
        &real::RealFs,
        &real::RealCaches::new(&crate::cache::root()?),
        &images_root()?,
        &crate::run_registry::snapshot(),
    )
    .await?;
    report.reclaimed_bytes +=
        crate::build_cache::sweep_with(&real::RealFs, &lns_ipc::build_cache_root()?).await?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::io;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeFs {
        files: Mutex<HashMap<PathBuf, Vec<u8>>>,
        fail_read_dir: bool,
        read_dir_missing: bool,
        fail_read: bool,
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
    }

    impl Fs for FakeFs {
        async fn read_dir(&self, dir: &Path) -> io::Result<Vec<PathBuf>> {
            if self.fail_read_dir {
                return Err(io::Error::other("read_dir boom"));
            }
            if self.read_dir_missing {
                return Err(io::Error::from(io::ErrorKind::NotFound));
            }
            let g = self.files.lock().unwrap();
            Ok(g.keys()
                .filter(|p| p.parent() == Some(dir))
                .cloned()
                .collect())
        }

        async fn read(&self, p: &Path) -> io::Result<Vec<u8>> {
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

    fn rec(reference: &str, layers: &[(&str, u64)]) -> ImageRecord {
        ImageRecord {
            reference: reference.to_string(),
            digest: format!("sha256:{}", "d".repeat(64)),
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
        let err = remove_with(&fs, &caches, Path::new(ROOT), &[], "###")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("invalid image reference"), "got: {err}");
        assert!(caches.swept_with.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn remove_of_an_unknown_image_names_its_normalized_reference() {
        let fs = FakeFs::default();
        let err = remove_with(&fs, &FakeCaches::default(), Path::new(ROOT), &[], "absent")
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
            vec!["registry.example.test/gone:1".to_string()]
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
    async fn prune_of_an_empty_index_still_sweeps_orphaned_layer_blobs() {
        let fs = FakeFs::default();
        let caches = FakeCaches {
            freed: 11,
            ..Default::default()
        };
        let report = prune_with(&fs, &caches, Path::new(ROOT), &[])
            .await
            .unwrap();
        assert!(report.removed.is_empty());
        assert_eq!(report.reclaimed_bytes, 11);
        assert_eq!(*caches.swept_with.lock().unwrap(), vec![HashSet::new()]);
    }

    #[tokio::test]
    async fn prune_propagates_a_record_delete_failure() {
        let fs = FakeFs::with_records(&[rec("registry.example.test/idle:1", &[])]);
        let failing = FakeFs {
            files: Mutex::new(fs.files.lock().unwrap().clone()),
            fail_remove: true,
            ..Default::default()
        };
        let err = prune_with(&failing, &FakeCaches::default(), Path::new(ROOT), &[])
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
        let err = prune_with(&fs, &caches, Path::new(ROOT), &[])
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
        let err = real::RealFs
            .read_dir(&dir.path().join("absent"))
            .await
            .unwrap_err();
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
        let _x = crate::test_env::EnvVarGuard::set("XDG_CACHE_HOME", d.path().join("cache"));

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
