use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use lns_service::image_store::{
    self, Caches, Fs, ImageRecord, LayerRef, PruneReport, RemovedImage, RuntimeCacheEntryKind,
    RuntimeCacheFs, RuntimeCacheMetadata,
};

#[derive(Debug, Default, Clone)]
pub struct IndexFs {
    files: Arc<Mutex<HashMap<PathBuf, Vec<u8>>>>,
    fail_write: Arc<AtomicBool>,
}

impl Fs for IndexFs {
    async fn read_dir(&self, dir: &Path) -> std::io::Result<Vec<PathBuf>> {
        let g = self.files.lock().unwrap();
        if g.contains_key(dir) {
            return Err(std::io::Error::from(std::io::ErrorKind::NotADirectory));
        }
        let entries: Vec<PathBuf> = g
            .keys()
            .filter(|p| p.parent() == Some(dir))
            .cloned()
            .collect();
        if entries.is_empty() {
            Err(std::io::Error::from(std::io::ErrorKind::NotFound))
        } else {
            Ok(entries)
        }
    }
    async fn read(&self, p: &Path) -> std::io::Result<Vec<u8>> {
        self.files
            .lock()
            .unwrap()
            .get(p)
            .cloned()
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::NotFound))
    }
    async fn write(&self, p: &Path, bytes: &[u8]) -> std::io::Result<()> {
        if self.fail_write.load(Ordering::SeqCst) {
            return Err(std::io::Error::other("write boom"));
        }
        self.files
            .lock()
            .unwrap()
            .insert(p.to_path_buf(), bytes.to_vec());
        Ok(())
    }
    async fn remove_file(&self, p: &Path) -> std::io::Result<()> {
        if self.files.lock().unwrap().remove(p).is_none() {
            return Err(std::io::Error::from(std::io::ErrorKind::NotFound));
        }
        Ok(())
    }
}

impl RuntimeCacheFs for IndexFs {
    async fn metadata(&self, p: &Path) -> std::io::Result<RuntimeCacheMetadata> {
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
        Err(std::io::Error::from(std::io::ErrorKind::NotFound))
    }

    async fn remove_dir_all(&self, p: &Path) -> std::io::Result<()> {
        let mut files = self.files.lock().unwrap();
        let before = files.len();
        files.retain(|path, _| !path.starts_with(p));
        if files.len() == before {
            return Err(std::io::Error::from(std::io::ErrorKind::NotFound));
        }
        Ok(())
    }
}

#[derive(Debug, Default, Clone)]
pub struct CacheFake {
    layers: Arc<Mutex<HashMap<String, u64>>>,
    manifests: Arc<Mutex<HashSet<String>>>,
}

impl CacheFake {
    pub fn add_layer(&self, digest: &str, size: u64) {
        self.layers.lock().unwrap().insert(digest.to_string(), size);
    }
    pub fn has_layer(&self, digest: &str) -> bool {
        self.layers.lock().unwrap().contains_key(digest)
    }
}

impl Caches for CacheFake {
    fn sweep_layers(&self, keep: &HashSet<String>) -> anyhow::Result<u64> {
        let mut g = self.layers.lock().unwrap();
        let doomed: Vec<String> = g.keys().filter(|d| !keep.contains(*d)).cloned().collect();
        let mut freed = 0;
        for digest in doomed {
            freed += g.remove(&digest).unwrap_or(0);
        }
        Ok(freed)
    }
    fn remove_manifest(&self, reference: &str) -> anyhow::Result<()> {
        self.manifests.lock().unwrap().remove(reference);
        Ok(())
    }
}

#[derive(Debug)]
pub struct ImageRig {
    pub fs: IndexFs,
    pub caches: CacheFake,
    pub images_root: PathBuf,
    tool_cache_paths: Vec<PathBuf>,
    pub active: Vec<lns_ipc::RunSummary>,
    pub holder_run_id: Option<String>,
    next_run_id: u32,
    seeded: HashMap<String, Vec<(String, u64)>>,
    pub last_list: Option<Vec<lns_ipc::ImageInfo>>,
    pub last_removed: Option<RemovedImage>,
    pub last_prune: Option<PruneReport>,
    pub last_pull: Option<lns_ipc::ImageInfo>,
    pub last_error: Option<String>,
}

impl ImageRig {
    pub fn new() -> Self {
        Self {
            fs: IndexFs::default(),
            caches: CacheFake::default(),
            images_root: PathBuf::from("/images"),
            tool_cache_paths: Vec::new(),
            active: Vec::new(),
            holder_run_id: None,
            next_run_id: 1,
            seeded: HashMap::new(),
            last_list: None,
            last_removed: None,
            last_prune: None,
            last_pull: None,
            last_error: None,
        }
    }

    pub fn seed_tool_cache(&mut self, bytes: u64) {
        let cache_root = self.images_root.parent().unwrap_or_else(|| Path::new(""));
        let portions = [bytes / 3, bytes / 3, bytes - 2 * (bytes / 3)];
        self.tool_cache_paths = ["tools", "content", "composefs"]
            .into_iter()
            .zip(portions)
            .map(|(name, size)| {
                let path = cache_root.join(name).join("cached");
                self.fs
                    .files
                    .lock()
                    .unwrap()
                    .insert(path.clone(), vec![0; size as usize]);
                path
            })
            .collect();
    }

    pub fn has_tool_cache(&self) -> bool {
        let files = self.fs.files.lock().unwrap();
        self.tool_cache_paths
            .iter()
            .any(|path| files.contains_key(path))
    }

    pub fn fail_index_writes(&self) {
        self.fs.fail_write.store(true, Ordering::SeqCst);
    }

    pub async fn pull(&mut self, reference: &str, digest: &str, size: u64) {
        let record = ImageRecord {
            reference: reference.to_string(),
            digest: format!("sha256:{}", "f".repeat(64)),
            dependencies: Vec::new(),
            layers: vec![LayerRef {
                digest: digest.to_string(),
                size_bytes: size,
            }],
            pulled_unix_secs: 1_765_022_400,
        };
        match image_store::pull_with(&self.fs, &self.images_root, &record, &self.active).await {
            Ok(info) => {
                self.last_pull = Some(info);
                self.last_error = None;
            }
            Err(e) => self.last_error = Some(e.to_string()),
        }
    }

    pub async fn seed_layer(&mut self, reference: &str, digest: &str, size: u64) {
        let layers = self.seeded.entry(reference.to_string()).or_default();
        layers.push((digest.to_string(), size));
        let record = ImageRecord {
            reference: reference.to_string(),
            digest: format!("sha256:{}", "f".repeat(64)),
            dependencies: Vec::new(),
            layers: layers
                .iter()
                .map(|(digest, size)| LayerRef {
                    digest: digest.clone(),
                    size_bytes: *size,
                })
                .collect(),
            pulled_unix_secs: 1_765_022_400,
        };
        self.caches.add_layer(digest, size);
        image_store::record_with(&self.fs, &self.images_root, &record)
            .await
            .expect("seeding an image record");
    }

    pub fn hold(&mut self, reference: &str) {
        let run_id = format!("{:032x}", self.next_run_id);
        self.next_run_id += 1;
        self.holder_run_id = Some(run_id.clone());
        self.active.push(lns_ipc::RunSummary {
            id: run_id,
            name: String::new(),
            image: reference.to_string(),
            command: String::new(),
            status: lns_ipc::RunStatus::Running,
            started: String::new(),
        });
    }

    pub async fn list(&mut self) {
        match image_store::list_with(&self.fs, &self.images_root, &self.active).await {
            Ok(images) => {
                self.last_list = Some(images);
                self.last_error = None;
            }
            Err(e) => self.last_error = Some(e.to_string()),
        }
    }

    pub async fn remove(&mut self, reference: &str) {
        match image_store::remove_with(
            &self.fs,
            &self.caches,
            &self.images_root,
            &self.active,
            reference,
        )
        .await
        {
            Ok(removed) => {
                self.last_removed = Some(removed);
                self.last_error = None;
            }
            Err(e) => self.last_error = Some(e.to_string()),
        }
    }

    pub async fn tag(&mut self, from: &str, to: &str) {
        match image_store::tag_with(&self.fs, &self.images_root, from, to).await {
            Ok(()) => self.last_error = None,
            Err(e) => self.last_error = Some(e.to_string()),
        }
    }

    pub async fn prune(&mut self) {
        match image_store::prune_with(&self.fs, &self.caches, &self.images_root, &self.active).await
        {
            Ok(report) => {
                self.last_prune = Some(report);
                self.last_error = None;
            }
            Err(e) => self.last_error = Some(e.to_string()),
        }
    }

    pub async fn record_in_index(&self, reference: &str) -> bool {
        let listed = image_store::list_with(&self.fs, &self.images_root, &self.active)
            .await
            .expect("listing the index");
        listed.iter().any(|i| i.reference == reference)
    }

    pub async fn recorded_digest(&self, reference: &str) -> Option<String> {
        let listed = image_store::list_with(&self.fs, &self.images_root, &self.active)
            .await
            .expect("listing the index");
        listed
            .iter()
            .find(|image| image.reference == reference)
            .map(|image| image.digest.clone())
    }
}
