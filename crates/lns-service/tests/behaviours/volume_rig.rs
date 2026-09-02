use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use lns_service::vm::VolumeAttachment;
use lns_service::volume_store::{
    FileMeta, Fs, Holders, LeaseRegistry, PruneReport, VolumeClaims, VolumeLease,
};

pub const FAKE_CREATED_UNIX_SECS: u64 = 1_765_022_400;
pub const FAKE_ALLOCATED_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, Default, Clone)]
pub struct TrackingFs {
    existing: Arc<Mutex<HashSet<PathBuf>>>,
    created: Arc<Mutex<Vec<PathBuf>>>,
    touched: Arc<Mutex<Vec<PathBuf>>>,
    sized: Arc<Mutex<std::collections::HashMap<PathBuf, u64>>>,
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
    pub fn size_of(&self, p: &Path) -> Option<u64> {
        self.sized.lock().unwrap().get(p).copied()
    }
    pub fn preset_size(&self, p: &Path, size: u64) {
        self.existing.lock().unwrap().insert(p.to_path_buf());
        self.sized.lock().unwrap().insert(p.to_path_buf(), size);
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
    async fn create_ext4_image(&self, p: &Path, size: u64) -> std::io::Result<()> {
        self.touched.lock().unwrap().push(p.to_path_buf());
        self.existing.lock().unwrap().insert(p.to_path_buf());
        self.created.lock().unwrap().push(p.to_path_buf());
        self.sized.lock().unwrap().insert(p.to_path_buf(), size);
        Ok(())
    }
    async fn grow_ext4_image(&self, p: &Path, size: u64) -> std::io::Result<()> {
        let mut sized = self.sized.lock().unwrap();
        let held = sized.entry(p.to_path_buf()).or_insert(size);
        *held = (*held).max(size);
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
            size_bytes: self
                .size_of(p)
                .unwrap_or(lns_service::volume_store::VOLUME_DEFAULT_SIZE_BYTES),
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

/// The sandboxes a volume belongs to, as the run registry would report them.
#[derive(Debug, Default, Clone)]
pub struct FakeHolders {
    by_volume: Arc<Mutex<std::collections::HashMap<String, Vec<String>>>>,
    damaged: Arc<Mutex<std::collections::HashMap<String, Vec<String>>>>,
    unreadable: Arc<Mutex<Vec<String>>>,
}

impl FakeHolders {
    pub fn declare(&self, volume: &str, sandbox: &str) {
        let mut g = self.by_volume.lock().unwrap();
        let names = g.entry(volume.to_string()).or_default();
        names.push(sandbox.to_string());
        names.sort();
    }

    pub fn forget(&self, sandbox: &str) {
        for names in self.by_volume.lock().unwrap().values_mut() {
            names.retain(|n| n != sandbox);
        }
    }

    pub fn declare_damaged(&self, volume: &str, run_id: &str) {
        self.damaged
            .lock()
            .unwrap()
            .entry(volume.to_string())
            .or_default()
            .push(run_id.to_string());
    }

    pub fn unreadable(&self, run_id: &str) {
        self.unreadable.lock().unwrap().push(run_id.to_string());
    }
}

impl Holders for FakeHolders {
    fn claims_on(&self, name: &str) -> VolumeClaims {
        VolumeClaims {
            held_by: self
                .by_volume
                .lock()
                .unwrap()
                .get(name)
                .cloned()
                .unwrap_or_default(),
            damaged: self
                .damaged
                .lock()
                .unwrap()
                .get(name)
                .cloned()
                .unwrap_or_default(),
            unreadable: self.unreadable.lock().unwrap().clone(),
        }
    }
}

#[derive(Debug)]
pub struct VolumeRig {
    pub registry: Arc<LeaseRegistry>,
    pub holders: FakeHolders,
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
    pub holder_name: Option<String>,
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
            holders: FakeHolders::default(),
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
            holder_name: None,
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

    pub fn preexisting_image_sized(&self, name: &str, size_bytes: u64) {
        self.fs.preset_size(&self.image_path(name), size_bytes);
    }

    pub fn image_size(&self, name: &str) -> Option<u64> {
        self.fs.size_of(&self.image_path(name))
    }

    pub async fn hold(&mut self, name: &str) {
        self.hold_as("holding-run", name).await;
    }

    /// A live sandbox: it takes the boot-time lease and declares the volume, the way a running run does both.
    pub async fn hold_as(&mut self, sandbox: &str, name: &str) {
        let id = self.alloc_run_id();
        let acq = lns_service::volume_store::acquire_with(
            &self.fs,
            &self.registry,
            &self.store_root,
            name,
            &id,
            lns_service::volume_store::VOLUME_DEFAULT_SIZE_BYTES,
        )
        .await
        .expect("hold acquire");
        self.held_leases.push(acq.lease);
        self.holders.declare(name, sandbox);
        self.holder_run_id = Some(id);
        self.holder_name = Some(sandbox.to_string());
    }

    pub fn release_held(&mut self) {
        self.held_leases.clear();
        if let Some(sandbox) = self.holder_name.clone() {
            self.holders.forget(&sandbox);
        }
    }

    /// A run whose record parses but which this build will not run; it still claims what the record names.
    pub fn damaged_record_declaring(&mut self, run_id: &str, volume: &str) {
        self.preexisting_image(volume);
        self.holders.declare_damaged(volume, run_id);
    }

    /// A run whose record this build cannot read at all, so what it claims is unknown.
    pub fn unreadable_record(&mut self, run_id: &str) {
        self.holders.unreadable(run_id);
    }

    /// A sandbox that declares a volume without booting — what a stopped run is to the store.
    pub fn declare(&mut self, sandbox: &str, volume: &str) {
        self.preexisting_image(volume);
        self.holders.declare(volume, sandbox);
    }

    pub fn forget_sandbox(&mut self, sandbox: &str) {
        self.holders.forget(sandbox);
    }

    pub async fn request(&mut self, name: &str, target: &str, read_only: bool) {
        self.request_paths(name, &[target], read_only).await;
    }

    pub async fn request_sized(&mut self, name: &str, target: &str, size_bytes: u64) {
        self.request_mounts(&[(name, target, false, Some(size_bytes))])
            .await;
    }

    pub async fn request_paths(&mut self, name: &str, targets: &[&str], read_only: bool) {
        let mounts: Vec<(&str, &str, bool, Option<u64>)> = targets
            .iter()
            .map(|t| (name, *t, read_only, None))
            .collect();
        self.request_mounts(&mounts).await;
    }

    pub async fn request_mounts(&mut self, spec: &[(&str, &str, bool, Option<u64>)]) {
        let id = self.alloc_run_id();
        let mounts: Vec<lns_ipc::VolumeMount> = spec
            .iter()
            .map(
                |(name, target, read_only, size_bytes)| lns_ipc::VolumeMount {
                    name: name.to_string(),
                    target: target.to_string(),
                    read_only: *read_only,
                    size_bytes: *size_bytes,
                },
            )
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
        match lns_service::volume_store::list_with(&self.fs, &self.holders, &self.store_root).await
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
            &self.holders,
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
            &self.holders,
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
            &self.holders,
            &self.store_root,
            name,
        )
        .await
        {
            Ok(()) => self.last_error = None,
            Err(e) => self.last_error = Some(e.to_string()),
        }
    }

    pub async fn prune(&mut self, dry_run: bool) {
        match lns_service::volume_store::prune_with(
            &self.fs,
            &self.registry,
            &self.holders,
            &self.store_root,
            dry_run,
        )
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
        let cx = lns_service::ocsf_audit::OcsfCtx::at_unix(
            "test-run".into(),
            "calm-finch".into(),
            1_700_000_000,
        );
        lns_service::audit::record_volume_attached_at(&self.audit_file, &cx, name, target)
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
