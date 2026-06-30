use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use lns_service::vm::VolumeAttachment;
use lns_service::volume_store::{FileMeta, Fs, LeaseRegistry, PruneReport, VolumeLease};

pub const FAKE_CREATED_UNIX_SECS: u64 = 1_765_022_400;
pub const FAKE_ALLOCATED_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, Default, Clone)]
pub struct TrackingFs {
    existing: Arc<Mutex<HashSet<PathBuf>>>,
    created: Arc<Mutex<Vec<PathBuf>>>,
    touched: Arc<Mutex<Vec<PathBuf>>>,
}

impl TrackingFs {
    pub fn preexist(&self, p: &Path) {
        self.existing.lock().unwrap().insert(p.to_path_buf());
    }
    pub fn created(&self) -> Vec<PathBuf> {
        self.created.lock().unwrap().clone()
    }
    pub fn touched(&self) -> Vec<PathBuf> {
        self.touched.lock().unwrap().clone()
    }
    pub fn has(&self, p: &Path) -> bool {
        self.existing.lock().unwrap().contains(p)
    }
}

impl Fs for TrackingFs {
    async fn exists(&self, p: &Path) -> bool {
        self.has(p)
    }
    async fn create_dir_all(&self, p: &Path) -> std::io::Result<()> {
        self.touched.lock().unwrap().push(p.to_path_buf());
        Ok(())
    }
    async fn create_ext4_image(&self, p: &Path, _size: u64) -> std::io::Result<()> {
        self.touched.lock().unwrap().push(p.to_path_buf());
        self.existing.lock().unwrap().insert(p.to_path_buf());
        self.created.lock().unwrap().push(p.to_path_buf());
        Ok(())
    }
    async fn read_dir(&self, dir: &Path) -> std::io::Result<Vec<PathBuf>> {
        let g = self.existing.lock().unwrap();
        Ok(g.iter()
            .filter(|p| p.parent() == Some(dir))
            .cloned()
            .collect())
    }
    async fn metadata(&self, p: &Path) -> std::io::Result<FileMeta> {
        if !self.has(p) {
            return Err(std::io::Error::from(std::io::ErrorKind::NotFound));
        }
        Ok(FileMeta {
            size_bytes: lns_service::volume_store::VOLUME_DEFAULT_SIZE_BYTES,
            allocated_bytes: FAKE_ALLOCATED_BYTES,
            created_unix_secs: FAKE_CREATED_UNIX_SECS,
        })
    }
    async fn remove_file(&self, p: &Path) -> std::io::Result<()> {
        if !self.existing.lock().unwrap().remove(p) {
            return Err(std::io::Error::from(std::io::ErrorKind::NotFound));
        }
        self.touched.lock().unwrap().push(p.to_path_buf());
        Ok(())
    }
}

#[derive(Debug)]
pub struct VolumeRig {
    pub registry: Arc<LeaseRegistry>,
    pub fs: TrackingFs,
    pub store_root: PathBuf,
    pub audit_file: PathBuf,
    _tmp: tempfile::TempDir,
    held_leases: Vec<VolumeLease>,
    pub last_attachments: Vec<VolumeAttachment>,
    last_leases: Vec<VolumeLease>,
    pub last_error: Option<String>,
    next_run_id: u32,
    pub holder_run_id: Option<String>,
    pub last_list: Option<Vec<lns_ipc::VolumeInfo>>,
    pub last_inspect: Option<lns_ipc::VolumeInfo>,
    pub last_prune: Option<PruneReport>,
}

impl VolumeRig {
    pub fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store_root = tmp.path().join("volumes");
        let audit_file = tmp.path().join("audit.jsonl");
        Self {
            registry: Arc::new(LeaseRegistry::new()),
            fs: TrackingFs::default(),
            store_root,
            audit_file,
            _tmp: tmp,
            held_leases: Vec::new(),
            last_attachments: Vec::new(),
            last_leases: Vec::new(),
            last_error: None,
            next_run_id: 1,
            holder_run_id: None,
            last_list: None,
            last_inspect: None,
            last_prune: None,
        }
    }

    fn alloc_run_id(&mut self) -> String {
        let id = format!("{:032x}", self.next_run_id);
        self.next_run_id += 1;
        id
    }

    pub fn image_path(&self, name: &str) -> PathBuf {
        self.store_root.join(format!("{name}.img"))
    }

    pub fn preexisting_image(&self, name: &str) {
        self.fs.preexist(&self.image_path(name));
    }

    pub async fn hold(&mut self, name: &str) {
        let id = self.alloc_run_id();
        let acq = lns_service::volume_store::acquire_with(
            &self.fs,
            &self.registry,
            &self.store_root,
            name,
            &id,
        )
        .await
        .expect("hold acquire");
        self.held_leases.push(acq.lease);
        self.holder_run_id = Some(id);
    }

    pub fn release_held(&mut self) {
        self.held_leases.clear();
    }

    pub async fn request(&mut self, name: &str, target: &str, read_only: bool) {
        self.request_paths(name, &[target], read_only).await;
    }

    pub async fn request_paths(&mut self, name: &str, targets: &[&str], read_only: bool) {
        let id = self.alloc_run_id();
        let mounts: Vec<lns_ipc::VolumeMount> = targets
            .iter()
            .map(|t| lns_ipc::VolumeMount {
                name: name.to_string(),
                target: t.to_string(),
                read_only,
            })
            .collect();
        match lns_service::volume_store::resolve_with(
            &self.fs,
            &self.registry,
            &self.store_root,
            &mounts,
            &id,
        )
        .await
        {
            Ok((attachments, leases)) => {
                self.last_attachments = attachments;
                self.last_leases = leases;
                self.last_error = None;
            }
            Err(e) => {
                self.last_error = Some(e.to_string());
                self.last_attachments.clear();
                self.last_leases.clear();
            }
        }
    }

    pub async fn list(&mut self) {
        match lns_service::volume_store::list_with(&self.fs, &self.registry, &self.store_root).await
        {
            Ok(volumes) => {
                self.last_list = Some(volumes);
                self.last_error = None;
            }
            Err(e) => self.last_error = Some(e.to_string()),
        }
    }

    pub async fn create(&mut self, name: &str) {
        match lns_service::volume_store::create_with(
            &self.fs,
            &self.registry,
            &self.store_root,
            name,
        )
        .await
        {
            Ok(_) => self.last_error = None,
            Err(e) => self.last_error = Some(e.to_string()),
        }
    }

    pub async fn inspect(&mut self, name: &str) {
        match lns_service::volume_store::inspect_with(
            &self.fs,
            &self.registry,
            &self.store_root,
            name,
        )
        .await
        {
            Ok(info) => {
                self.last_inspect = Some(info);
                self.last_error = None;
            }
            Err(e) => self.last_error = Some(e.to_string()),
        }
    }

    pub async fn remove(&mut self, name: &str) {
        match lns_service::volume_store::remove_with(
            &self.fs,
            &self.registry,
            &self.store_root,
            name,
        )
        .await
        {
            Ok(()) => self.last_error = None,
            Err(e) => self.last_error = Some(e.to_string()),
        }
    }

    pub async fn prune(&mut self) {
        match lns_service::volume_store::prune_with(&self.fs, &self.registry, &self.store_root)
            .await
        {
            Ok(report) => {
                self.last_prune = Some(report);
                self.last_error = None;
            }
            Err(e) => self.last_error = Some(e.to_string()),
        }
    }

    pub fn image_in_store(&self, name: &str) -> bool {
        self.fs.has(&self.image_path(name))
    }

    pub fn record_attach(&self, name: &str, target: &str) {
        lns_service::audit::record_volume_attached_at(
            &self.audit_file,
            name,
            target,
            &lns_service::oauth::RealClock,
        )
        .expect("record audit event");
    }

    pub fn audit_contents(&self) -> String {
        std::fs::read_to_string(&self.audit_file).unwrap_or_default()
    }

    pub fn attachment(&self, name: &str) -> Option<&VolumeAttachment> {
        self.last_attachments
            .iter()
            .find(|a| a.host_image == self.image_path(name))
    }

    pub fn attachment_targets(&self, name: &str) -> Vec<String> {
        self.last_attachments
            .iter()
            .filter(|a| a.host_image == self.image_path(name))
            .map(|a| a.target.clone())
            .collect()
    }

    pub fn created_count(&self, name: &str) -> usize {
        let want = self.image_path(name);
        self.fs.created().iter().filter(|p| **p == want).count()
    }

    pub fn touched_outside_store(&self) -> Vec<PathBuf> {
        self.fs
            .touched()
            .into_iter()
            .chain(self.fs.created())
            .filter(|p| !p.starts_with(&self.store_root))
            .collect()
    }
}
