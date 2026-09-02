mod real;
mod traits;

pub use traits::{FileMeta, Fs, Holders, VolumeClaims};

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Context, Result, bail};

pub const VOLUME_DEFAULT_SIZE_BYTES: u64 = crate::upperfs::DEFAULT_SIZE_BYTES;

pub fn store_root() -> Result<PathBuf> {
    Ok(crate::cache::root()?.join("volumes"))
}

pub fn validate_name(name: &str) -> Result<()> {
    lns_ipc::validate_volume_name(name).map_err(|e| anyhow::anyhow!(e))
}

pub fn validate_target(target: &str) -> Result<()> {
    lns_ipc::validate_volume_target(target).map_err(|e| anyhow::anyhow!(e))
}

#[derive(Debug)]
pub struct LeaseRegistry {
    active: Mutex<HashMap<String, String>>,
}

impl LeaseRegistry {
    pub fn new() -> Self {
        Self {
            active: Mutex::new(HashMap::new()),
        }
    }

    fn try_acquire(&self, name: &str, run_id: &str) -> std::result::Result<(), String> {
        let mut g = self.active.lock().expect("lease registry poisoned");
        match g.get(name) {
            Some(holder) => Err(holder.clone()),
            None => {
                g.insert(name.to_string(), run_id.to_string());
                Ok(())
            }
        }
    }

    fn release(&self, name: &str) {
        self.active
            .lock()
            .expect("lease registry poisoned")
            .remove(name);
    }

    pub fn holder(&self, name: &str) -> Option<String> {
        self.active
            .lock()
            .expect("lease registry poisoned")
            .get(name)
            .cloned()
    }
}

impl Default for LeaseRegistry {
    fn default() -> Self {
        Self::new()
    }
}

pub fn global() -> Arc<LeaseRegistry> {
    static GLOBAL: OnceLock<Arc<LeaseRegistry>> = OnceLock::new();
    GLOBAL
        .get_or_init(|| Arc::new(LeaseRegistry::new()))
        .clone()
}

#[derive(Debug)]
pub struct VolumeLease {
    registry: Arc<LeaseRegistry>,
    name: String,
}

impl Drop for VolumeLease {
    fn drop(&mut self) {
        self.registry.release(&self.name);
    }
}

#[derive(Debug)]
pub struct Acquired {
    pub image_path: PathBuf,
    pub lease: VolumeLease,
}

pub async fn acquire(name: &str, run_id: &str) -> Result<Acquired> {
    acquire_with(
        &real::RealFs,
        &global(),
        &store_root()?,
        name,
        run_id,
        VOLUME_DEFAULT_SIZE_BYTES,
    )
    .await
}

pub async fn resolve(
    mounts: &[lns_ipc::VolumeMount],
    run_id: &str,
) -> Result<(Vec<crate::vm::VolumeAttachment>, Vec<VolumeLease>)> {
    resolve_with(&real::RealFs, &global(), &store_root()?, mounts, run_id).await
}

pub async fn resolve_with<F: Fs>(
    fs: &F,
    registry: &Arc<LeaseRegistry>,
    store_root: &Path,
    mounts: &[lns_ipc::VolumeMount],
    run_id: &str,
) -> Result<(Vec<crate::vm::VolumeAttachment>, Vec<VolumeLease>)> {
    let mut attachments = Vec::with_capacity(mounts.len());
    let mut leases = Vec::new();
    let mut image_by_name: HashMap<&str, PathBuf> = HashMap::new();
    let sizes = declared_sizes(mounts);
    for m in mounts {
        validate_target(&m.target)?;
        let image_path = if let Some(p) = image_by_name.get(m.name.as_str()) {
            p.clone()
        } else {
            let size = sizes
                .get(m.name.as_str())
                .copied()
                .unwrap_or(VOLUME_DEFAULT_SIZE_BYTES);
            let acq = acquire_with(fs, registry, store_root, &m.name, run_id, size).await?;
            image_by_name.insert(m.name.as_str(), acq.image_path.clone());
            leases.push(acq.lease);
            acq.image_path
        };
        attachments.push(crate::vm::VolumeAttachment {
            host_image: image_path,
            target: m.target.clone(),
            read_only: m.read_only,
        });
    }
    Ok((attachments, leases))
}

/// One volume may be mounted at several targets, each declaring its own size, so the volume has to satisfy the largest of them.
fn declared_sizes(mounts: &[lns_ipc::VolumeMount]) -> HashMap<&str, u64> {
    let mut sizes: HashMap<&str, u64> = HashMap::new();
    for m in mounts {
        let asked = m.size_bytes.unwrap_or(VOLUME_DEFAULT_SIZE_BYTES);
        let entry = sizes.entry(m.name.as_str()).or_insert(asked);
        *entry = (*entry).max(asked);
    }
    sizes
}

pub async fn acquire_with<F: Fs>(
    fs: &F,
    registry: &Arc<LeaseRegistry>,
    store_root: &Path,
    name: &str,
    run_id: &str,
    size_bytes: u64,
) -> Result<Acquired> {
    validate_name(name)?;
    let lease = take_lease(registry, name, run_id)?;
    let image_path = ensure_image(fs, store_root, name, size_bytes).await?;
    Ok(Acquired { image_path, lease })
}

fn image_path_in(store_root: &Path, name: &str) -> PathBuf {
    store_root.join(format!("{name}.img"))
}

fn take_lease(registry: &Arc<LeaseRegistry>, name: &str, run_id: &str) -> Result<VolumeLease> {
    if let Err(holder) = registry.try_acquire(name, run_id) {
        bail!(
            "volume {name:?} in use by run {}",
            lns_ipc::short_run_id(&holder)
        );
    }
    Ok(VolumeLease {
        registry: registry.clone(),
        name: name.to_string(),
    })
}

const MAINTENANCE_HOLDER_RUN_ID: &str = "maintenance";

/// What stands between a volume and its removal, split by the remedy: `lns rm` clears a sandbox, only repairing a run dir clears the rest.
enum Blocked {
    Sandboxes(String),
    Repair(String),
}

impl Blocked {
    fn message(&self) -> &str {
        match self {
            Blocked::Sandboxes(m) | Blocked::Repair(m) => m,
        }
    }
}

/// A volume's data belongs to the sandboxes that declare it, so it outlives every one of them and not one boot less.
fn blocking_claim<H: Holders>(holders: &H, name: &str) -> Option<Blocked> {
    let claims = holders.claims_on(name);
    if let Some(run_id) = claims.unreadable.first() {
        let short = lns_ipc::short_run_id(run_id);
        return Some(Blocked::Repair(format!(
            "volume {name:?} may be in use: run {short}'s record cannot be read; repair or delete ~/.lns/runs/{run_id}, then restart the service"
        )));
    }
    if let Some(run_id) = claims.damaged.first() {
        let short = lns_ipc::short_run_id(run_id);
        return Some(Blocked::Repair(format!(
            "volume {name:?} is in use by run {short}, which this build cannot run; repair or delete ~/.lns/runs/{run_id}, then restart the service"
        )));
    }
    match claims.held_by.len() {
        0 => None,
        1 => Some(Blocked::Sandboxes(format!(
            "volume {name:?} is in use by sandbox {:?}; remove the sandbox first",
            claims.held_by[0]
        ))),
        n => Some(Blocked::Sandboxes(format!(
            "volume {name:?} is in use by {n} sandboxes: {}; remove them first",
            claims
                .held_by
                .iter()
                .map(|h| format!("{h:?}"))
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

fn refuse_while_held<H: Holders>(holders: &H, name: &str) -> Result<()> {
    match blocking_claim(holders, name) {
        None => Ok(()),
        Some(blocked) => bail!("{}", blocked.message()),
    }
}

/// A declared size is a floor: an absent volume is created at it, a smaller one grows to it, and a larger one is already past it and is left alone.
async fn ensure_image<F: Fs>(
    fs: &F,
    store_root: &Path,
    name: &str,
    size_bytes: u64,
) -> Result<PathBuf> {
    let image_path = image_path_in(store_root, name);
    if !fs.exists(&image_path).await {
        fs.create_dir_all(store_root).await?;
        fs.create_ext4_image(&image_path, size_bytes).await?;
        return Ok(image_path);
    }
    fs.grow_ext4_image(&image_path, size_bytes)
        .await
        .with_context(|| format!("growing volume {name:?} to {size_bytes} bytes"))?;
    Ok(image_path)
}

async fn info_for<F: Fs, H: Holders>(
    fs: &F,
    holders: &H,
    image_path: &Path,
    name: &str,
) -> Result<lns_ipc::VolumeInfo> {
    let meta = fs.metadata(image_path).await?;
    Ok(lns_ipc::VolumeInfo {
        name: name.to_string(),
        size_bytes: meta.size_bytes,
        disk_bytes: meta.allocated_bytes,
        created: crate::time_fmt::rfc3339_from_unix(meta.created_unix_secs),
        in_use_by: holders.claims_on(name).held_by,
    })
}

fn volume_name_of(path: &Path) -> Option<String> {
    let file = path.file_name()?.to_str()?;
    file.strip_suffix(".img").map(str::to_string)
}

fn is_not_found(err: &anyhow::Error) -> bool {
    err.downcast_ref::<std::io::Error>()
        .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound)
}

pub async fn list_with<F: Fs, H: Holders>(
    fs: &F,
    holders: &H,
    store_root: &Path,
) -> Result<Vec<lns_ipc::VolumeInfo>> {
    let entries = match fs.read_dir(store_root).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut names: Vec<String> = entries.iter().filter_map(|p| volume_name_of(p)).collect();
    names.sort();
    let mut out = Vec::with_capacity(names.len());
    for name in &names {
        match info_for(fs, holders, &image_path_in(store_root, name), name).await {
            Ok(info) => out.push(info),
            Err(e) if is_not_found(&e) => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(out)
}

pub async fn create_with<F: Fs, H: Holders>(
    fs: &F,
    registry: &Arc<LeaseRegistry>,
    holders: &H,
    store_root: &Path,
    name: &str,
) -> Result<lns_ipc::VolumeInfo> {
    validate_name(name)?;
    let image_path = image_path_in(store_root, name);
    if !fs.exists(&image_path).await {
        let _guard = take_lease(registry, name, MAINTENANCE_HOLDER_RUN_ID)?;
        ensure_image(fs, store_root, name, VOLUME_DEFAULT_SIZE_BYTES).await?;
    }
    info_for(fs, holders, &image_path, name).await
}

pub async fn inspect_with<F: Fs, H: Holders>(
    fs: &F,
    holders: &H,
    store_root: &Path,
    name: &str,
) -> Result<lns_ipc::VolumeInfo> {
    validate_name(name)?;
    let image_path = image_path_in(store_root, name);
    if !fs.exists(&image_path).await {
        bail!("no such volume: {name}");
    }
    info_for(fs, holders, &image_path, name).await
}

pub async fn remove_with<F: Fs, H: Holders>(
    fs: &F,
    registry: &Arc<LeaseRegistry>,
    holders: &H,
    store_root: &Path,
    name: &str,
) -> Result<()> {
    validate_name(name)?;
    refuse_while_held(holders, name)?;
    let _guard = take_lease(registry, name, MAINTENANCE_HOLDER_RUN_ID)?;
    let image_path = image_path_in(store_root, name);
    if !fs.exists(&image_path).await {
        bail!("no such volume: {name}");
    }
    fs.remove_file(&image_path).await?;
    // A volume recreated under the same name must not inherit the marker that says an interrupted grow left the old one unclean.
    let marker = image_path.with_extension("img.growing");
    if fs.exists(&marker).await {
        fs.remove_file(&marker).await?;
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PruneReport {
    pub removed: Vec<String>,
    pub reclaimed_bytes: u64,
    pub failed: Vec<lns_ipc::VolumePruneFailure>,
}

/// A dry run answers what the sweep would do without doing it, so the question and the sweep cannot disagree.
pub async fn prune_with<F: Fs, H: Holders>(
    fs: &F,
    registry: &Arc<LeaseRegistry>,
    holders: &H,
    store_root: &Path,
    dry_run: bool,
) -> Result<PruneReport> {
    let mut report = PruneReport::default();
    for info in list_with(fs, holders, store_root).await? {
        // A sandbox holding a volume is the ordinary case prune skips quietly; anything else is a failure the user must hear about.
        match blocking_claim(holders, &info.name) {
            None => {}
            Some(Blocked::Sandboxes(_)) => continue,
            Some(blocked @ Blocked::Repair(_)) => {
                report.failed.push(lns_ipc::VolumePruneFailure {
                    name: info.name,
                    error: blocked.message().to_string(),
                });
                continue;
            }
        }
        // The dry run takes the lease too, so an open image is classified the same way in the question and in the sweep.
        let _guard = match take_lease(registry, &info.name, MAINTENANCE_HOLDER_RUN_ID) {
            Ok(guard) => guard,
            Err(e) => {
                report.failed.push(lns_ipc::VolumePruneFailure {
                    name: info.name,
                    error: e.to_string(),
                });
                continue;
            }
        };
        if dry_run {
            report.reclaimed_bytes += info.disk_bytes;
            report.removed.push(info.name);
            continue;
        }
        match fs.remove_file(&image_path_in(store_root, &info.name)).await {
            Ok(()) => {
                report.reclaimed_bytes += info.disk_bytes;
                report.removed.push(info.name);
            }
            Err(e) => report.failed.push(lns_ipc::VolumePruneFailure {
                name: info.name,
                error: e.to_string(),
            }),
        }
    }
    Ok(report)
}

pub async fn list() -> Result<Vec<lns_ipc::VolumeInfo>> {
    list_with(&real::RealFs, &real::RegistryHolders, &store_root()?).await
}

pub async fn create(name: &str) -> Result<lns_ipc::VolumeInfo> {
    create_with(
        &real::RealFs,
        &global(),
        &real::RegistryHolders,
        &store_root()?,
        name,
    )
    .await
}

pub async fn inspect(name: &str) -> Result<lns_ipc::VolumeInfo> {
    inspect_with(&real::RealFs, &real::RegistryHolders, &store_root()?, name).await
}

pub async fn remove(name: &str) -> Result<()> {
    remove_with(
        &real::RealFs,
        &global(),
        &real::RegistryHolders,
        &store_root()?,
        name,
    )
    .await
}

pub async fn prune(dry_run: bool) -> Result<PruneReport> {
    prune_with(
        &real::RealFs,
        &global(),
        &real::RegistryHolders,
        &store_root()?,
        dry_run,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::io;

    struct Idle;
    impl Holders for Idle {
        fn claims_on(&self, _: &str) -> VolumeClaims {
            VolumeClaims::default()
        }
    }

    struct HeldBy(&'static [&'static str]);
    impl Holders for HeldBy {
        fn claims_on(&self, _: &str) -> VolumeClaims {
            VolumeClaims {
                held_by: self.0.iter().map(|s| s.to_string()).collect(),
                ..VolumeClaims::default()
            }
        }
    }

    const FAKE_CREATED_UNIX_SECS: u64 = 1_765_022_400;
    const FAKE_ALLOCATED_BYTES: u64 = 4 * 1024 * 1024;

    #[derive(Default)]
    struct FakeFs {
        existing: Mutex<HashSet<PathBuf>>,
        created: Mutex<Vec<PathBuf>>,
        created_sizes: Mutex<Vec<u64>>,
        grown_to: Mutex<Vec<(PathBuf, u64)>>,
        fail_grow: bool,
        allocated: Mutex<HashMap<PathBuf, u64>>,
        vanished: Mutex<HashSet<PathBuf>>,
        unremovable: Mutex<HashSet<PathBuf>>,
        fail_create: bool,
        fail_read_dir: bool,
        read_dir_missing: bool,
        fail_metadata: bool,
        fail_remove: bool,
    }

    impl FakeFs {
        fn with(existing: &[&str]) -> Self {
            Self {
                existing: Mutex::new(existing.iter().map(PathBuf::from).collect()),
                ..Default::default()
            }
        }
        fn created_images(&self) -> Vec<PathBuf> {
            self.created.lock().unwrap().clone()
        }
        fn created_sizes(&self) -> Vec<u64> {
            self.created_sizes.lock().unwrap().clone()
        }
        fn grown_to(&self) -> Vec<(PathBuf, u64)> {
            self.grown_to.lock().unwrap().clone()
        }
        fn set_allocated(&self, p: &str, bytes: u64) {
            self.allocated
                .lock()
                .unwrap()
                .insert(PathBuf::from(p), bytes);
        }
        fn vanish(&self, p: &str) {
            self.vanished.lock().unwrap().insert(PathBuf::from(p));
        }
        fn refuse_remove(&self, p: &str) {
            self.unremovable.lock().unwrap().insert(PathBuf::from(p));
        }
    }

    impl Fs for FakeFs {
        async fn exists(&self, p: &Path) -> bool {
            self.existing.lock().unwrap().contains(p)
        }
        async fn create_dir_all(&self, _p: &Path) -> io::Result<()> {
            Ok(())
        }
        async fn create_ext4_image(&self, p: &Path, size: u64) -> io::Result<()> {
            if self.fail_create {
                return Err(io::Error::other("boom"));
            }
            self.created_sizes.lock().unwrap().push(size);
            self.existing.lock().unwrap().insert(p.to_path_buf());
            self.created.lock().unwrap().push(p.to_path_buf());
            Ok(())
        }
        async fn read_dir(&self, dir: &Path) -> io::Result<Vec<PathBuf>> {
            if self.fail_read_dir {
                return Err(io::Error::other("read_dir boom"));
            }
            if self.read_dir_missing {
                return Err(io::Error::from(io::ErrorKind::NotFound));
            }
            let g = self.existing.lock().unwrap();
            Ok(g.iter()
                .filter(|p| p.parent() == Some(dir))
                .cloned()
                .collect())
        }
        async fn metadata(&self, p: &Path) -> io::Result<FileMeta> {
            if self.fail_metadata {
                return Err(io::Error::other("metadata boom"));
            }
            if self.vanished.lock().unwrap().contains(p) {
                return Err(io::Error::from(io::ErrorKind::NotFound));
            }
            let allocated = self.allocated.lock().unwrap().get(p).copied();
            Ok(FileMeta {
                size_bytes: VOLUME_DEFAULT_SIZE_BYTES,
                allocated_bytes: allocated.unwrap_or(FAKE_ALLOCATED_BYTES),
                created_unix_secs: FAKE_CREATED_UNIX_SECS,
            })
        }
        async fn grow_ext4_image(&self, p: &Path, size: u64) -> io::Result<()> {
            if self.fail_grow {
                return Err(io::Error::other("no room"));
            }
            self.grown_to.lock().unwrap().push((p.to_path_buf(), size));
            Ok(())
        }
        async fn remove_file(&self, p: &Path) -> io::Result<()> {
            if self.fail_remove || self.unremovable.lock().unwrap().contains(p) {
                return Err(io::Error::other("remove boom"));
            }
            self.existing.lock().unwrap().remove(p);
            Ok(())
        }
    }

    fn reg() -> Arc<LeaseRegistry> {
        Arc::new(LeaseRegistry::new())
    }

    /// `holder` serves the restart preflight (`ipc::adapter`), which refuses a start while a live run has the image open.
    #[test]
    fn the_lease_holder_is_the_live_run_that_took_it_and_nobody_once_it_ends() {
        let registry = reg();
        assert_eq!(registry.holder("prism-data"), None);
        let lease = take_lease(&registry, "prism-data", "aa07").expect("a free volume");
        assert_eq!(registry.holder("prism-data"), Some("aa07".to_string()));
        drop(lease);
        assert_eq!(
            registry.holder("prism-data"),
            None,
            "a run that ends releases the image for the next boot"
        );
    }

    #[tokio::test]
    async fn acquiring_unknown_name_creates_the_backing_image() {
        let fs = FakeFs::default();
        let got = acquire_with(
            &fs,
            &reg(),
            Path::new("/store"),
            "prism-data",
            "aa01",
            VOLUME_DEFAULT_SIZE_BYTES,
        )
        .await
        .unwrap();
        assert_eq!(got.image_path, Path::new("/store/prism-data.img"));
        assert_eq!(
            fs.created_images(),
            vec![PathBuf::from("/store/prism-data.img")]
        );
    }

    #[tokio::test]
    async fn removing_a_volume_takes_the_marker_an_interrupted_grow_left_with_it() {
        let fs = FakeFs::with(&["/store/prism-data.img", "/store/prism-data.img.growing"]);
        remove_with(&fs, &reg(), &Idle, Path::new("/store"), "prism-data")
            .await
            .unwrap();
        assert!(
            !fs.exists(Path::new("/store/prism-data.img.growing")).await,
            "a volume recreated under this name would inherit a marker that excuses an unclean superblock it never made"
        );
    }

    #[tokio::test]
    async fn a_volume_that_does_not_exist_is_created_at_the_size_the_document_asked_for() {
        let fs = FakeFs::default();
        acquire_with(
            &fs,
            &reg(),
            Path::new("/store"),
            "prism-data",
            "aa01",
            40 << 30,
        )
        .await
        .unwrap();
        assert_eq!(fs.created_sizes(), vec![40 << 30]);
        assert!(
            fs.grown_to().is_empty(),
            "a fresh volume has nothing to grow"
        );
    }

    #[tokio::test]
    async fn a_volume_that_already_exists_is_offered_the_declared_size_as_a_floor() {
        let fs = FakeFs::with(&["/store/prism-data.img"]);
        acquire_with(
            &fs,
            &reg(),
            Path::new("/store"),
            "prism-data",
            "aa01",
            40 << 30,
        )
        .await
        .unwrap();
        assert!(
            fs.created_images().is_empty(),
            "an existing volume is never recreated"
        );
        assert_eq!(
            fs.grown_to(),
            vec![(PathBuf::from("/store/prism-data.img"), 40 << 30)],
            "the store hands the size down; only the writer knows whether it means growing"
        );
    }

    #[tokio::test]
    async fn a_volume_that_cannot_grow_refuses_the_run_and_names_itself() {
        let fs = FakeFs {
            fail_grow: true,
            ..FakeFs::with(&["/store/prism-data.img"])
        };
        let err = acquire_with(
            &fs,
            &reg(),
            Path::new("/store"),
            "prism-data",
            "aa01",
            40 << 30,
        )
        .await
        .unwrap_err();
        let rendered = format!("{err:#}");
        assert!(
            rendered.contains("prism-data") && rendered.contains("no room"),
            "a run that cannot get the volume it asked for must say which one: {rendered}"
        );
    }

    #[tokio::test]
    async fn one_volume_mounted_twice_gets_the_largest_size_either_mount_asked_for() {
        let fs = FakeFs::default();
        let mounts = [
            lns_ipc::VolumeMount {
                name: "cache".into(),
                target: "/a".into(),
                read_only: false,
                size_bytes: Some(10 << 30),
            },
            lns_ipc::VolumeMount {
                name: "cache".into(),
                target: "/b".into(),
                read_only: false,
                size_bytes: Some(50 << 30),
            },
        ];
        resolve_with(&fs, &reg(), Path::new("/store"), &mounts, "aa01")
            .await
            .unwrap();
        assert_eq!(
            fs.created_sizes(),
            vec![50 << 30],
            "one image cannot be two sizes, so it has to satisfy the larger mount"
        );
    }

    #[tokio::test]
    async fn a_mount_that_declares_no_size_still_asks_for_the_built_in_default() {
        let fs = FakeFs::default();
        let mounts = [lns_ipc::VolumeMount {
            name: "cache".into(),
            target: "/a".into(),
            read_only: false,
            size_bytes: None,
        }];
        resolve_with(&fs, &reg(), Path::new("/store"), &mounts, "aa01")
            .await
            .unwrap();
        assert_eq!(fs.created_sizes(), vec![VOLUME_DEFAULT_SIZE_BYTES]);
    }

    #[tokio::test]
    async fn acquiring_existing_name_does_not_recreate_the_image() {
        let fs = FakeFs::with(&["/store/prism-data.img"]);
        let got = acquire_with(
            &fs,
            &reg(),
            Path::new("/store"),
            "prism-data",
            "aa01",
            VOLUME_DEFAULT_SIZE_BYTES,
        )
        .await
        .unwrap();
        assert_eq!(got.image_path, Path::new("/store/prism-data.img"));
        assert!(
            fs.created_images().is_empty(),
            "an existing image must not be recreated"
        );
    }

    #[tokio::test]
    async fn same_volume_mounted_at_two_paths_in_one_run_shares_one_lease_and_image() {
        let registry = reg();
        let fs = FakeFs::default();
        let mounts = [
            lns_ipc::VolumeMount {
                name: "prism-data".into(),
                target: "/data".into(),
                read_only: false,
                size_bytes: None,
            },
            lns_ipc::VolumeMount {
                name: "prism-data".into(),
                target: "/srv/state".into(),
                read_only: true,
                size_bytes: None,
            },
        ];
        let (attachments, leases) =
            resolve_with(&fs, &registry, Path::new("/store"), &mounts, "aa01")
                .await
                .expect("the same volume at two paths in one run is allowed");
        assert_eq!(attachments.len(), 2, "one attachment per requested path");
        assert_eq!(attachments[0].host_image, attachments[1].host_image);
        assert_eq!(attachments[0].target, "/data");
        assert_eq!(attachments[1].target, "/srv/state");
        assert_eq!(leases.len(), 1, "one lease for the shared volume");
        assert_eq!(
            fs.created_images(),
            vec![PathBuf::from("/store/prism-data.img")],
            "the backing image is created exactly once"
        );
    }

    #[tokio::test]
    async fn second_live_acquire_is_refused_naming_the_holder() {
        let registry = reg();
        let fs = FakeFs::default();
        let _held = acquire_with(
            &fs,
            &registry,
            Path::new("/store"),
            "prism-data",
            "aa07",
            VOLUME_DEFAULT_SIZE_BYTES,
        )
        .await
        .unwrap();
        let err = acquire_with(
            &fs,
            &registry,
            Path::new("/store"),
            "prism-data",
            "aa08",
            VOLUME_DEFAULT_SIZE_BYTES,
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("in use by run aa07"), "got: {err}");
    }

    #[tokio::test]
    async fn dropping_the_lease_frees_the_volume_for_the_next_run() {
        let registry = reg();
        let fs = FakeFs::with(&["/store/prism-data.img"]);
        {
            let _held = acquire_with(
                &fs,
                &registry,
                Path::new("/store"),
                "prism-data",
                "aa07",
                VOLUME_DEFAULT_SIZE_BYTES,
            )
            .await
            .unwrap();
        }
        acquire_with(
            &fs,
            &registry,
            Path::new("/store"),
            "prism-data",
            "aa08",
            VOLUME_DEFAULT_SIZE_BYTES,
        )
        .await
        .expect("volume should be free after the prior lease dropped");
    }

    #[tokio::test]
    async fn invalid_name_is_refused_before_any_image_is_created_or_lease_taken() {
        let registry = reg();
        let fs = FakeFs::default();
        let err = acquire_with(
            &fs,
            &registry,
            Path::new("/store"),
            "../etc",
            "aa01",
            VOLUME_DEFAULT_SIZE_BYTES,
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("invalid volume name"), "got: {err}");
        assert!(
            fs.created_images().is_empty(),
            "no image for an invalid name"
        );
        registry
            .try_acquire("../etc", "aa02")
            .expect("invalid name must not have taken a lease");
    }

    #[tokio::test]
    async fn target_that_would_inject_kernel_cmdline_tokens_is_refused_before_any_image_or_lease() {
        let registry = reg();
        let fs = FakeFs::default();
        let mounts = [lns_ipc::VolumeMount {
            name: "prism-data".into(),
            target: "/data init=/bin/sh".into(),
            read_only: false,
            size_bytes: None,
        }];
        let err = resolve_with(&fs, &registry, Path::new("/store"), &mounts, "aa01")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("must not contain whitespace"), "got: {err}");
        assert!(
            fs.created_images().is_empty(),
            "no image for an unparseable target"
        );
        registry
            .try_acquire("prism-data", "aa02")
            .expect("a rejected target must not strand the lease");
    }

    #[tokio::test]
    async fn create_failure_releases_the_lease() {
        let registry = reg();
        let fs = FakeFs {
            fail_create: true,
            ..Default::default()
        };
        acquire_with(
            &fs,
            &registry,
            Path::new("/store"),
            "prism-data",
            "aa01",
            VOLUME_DEFAULT_SIZE_BYTES,
        )
        .await
        .expect_err("create should fail");
        registry
            .try_acquire("prism-data", "aa02")
            .expect("a failed create must not strand the lease");
    }

    #[test]
    fn validate_name_accepts_dotted_and_dashed_names() {
        validate_name("prism-data.v2_3").unwrap();
    }

    #[test]
    fn validate_name_rejects_empty_dots_and_separators() {
        for bad in ["", ".", "..", "a/b", "a:b", "a b"] {
            validate_name(bad).unwrap_err();
        }
    }

    #[tokio::test]
    async fn real_fs_create_ext4_image_writes_a_mountable_image() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vol.img");
        real::RealFs
            .create_ext4_image(&path, 32 * 1024 * 1024)
            .await
            .unwrap();
        assert!(real::RealFs.exists(&path).await);
        assert!(std::fs::metadata(&path).unwrap().len() > 0);
    }

    #[tokio::test]
    async fn real_fs_create_dir_all_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a/b/c");
        real::RealFs.create_dir_all(&nested).await.unwrap();
        assert!(real::RealFs.exists(&nested).await);
    }

    #[test]
    fn lease_registry_default_starts_empty() {
        LeaseRegistry::default()
            .try_acquire("x", "aa01")
            .expect("a default registry holds no leases");
    }

    #[test]
    fn global_returns_one_shared_registry() {
        assert!(Arc::ptr_eq(&global(), &global()));
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn store_root_lives_under_the_cache_root() {
        let d = tempfile::tempdir().unwrap();
        let _h = crate::test_env::EnvVarGuard::set("HOME", d.path());
        assert!(store_root().unwrap().ends_with("volumes"));
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn acquire_production_wrapper_creates_under_store_and_releases_on_drop() {
        let d = tempfile::tempdir().unwrap();
        let _h = crate::test_env::EnvVarGuard::set("HOME", d.path());
        let acq = acquire("cov-acquire", "aa01").await.unwrap();
        assert!(acq.image_path.ends_with("cov-acquire.img"));
        drop(acq);
        drop(
            acquire("cov-acquire", "aa02")
                .await
                .expect("lease freed on drop"),
        );
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn resolve_production_wrapper_maps_mounts_to_attachments() {
        let d = tempfile::tempdir().unwrap();
        let _h = crate::test_env::EnvVarGuard::set("HOME", d.path());
        let mounts = [lns_ipc::VolumeMount {
            name: "cov-resolve".to_string(),
            target: "/data".to_string(),
            read_only: true,
            size_bytes: None,
        }];
        let (attachments, leases) = resolve(&mounts, "aa03").await.unwrap();
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].target, "/data");
        assert!(attachments[0].read_only);
        drop(leases);
    }

    #[tokio::test]
    async fn list_skips_non_image_files_and_sorts_by_name() {
        let fs = FakeFs::with(&[
            "/store/zeta.img",
            "/store/alpha.img",
            "/store/notes.txt",
            "/store",
        ]);
        let got = list_with(&fs, &Idle, Path::new("/store")).await.unwrap();
        let names: Vec<&str> = got.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(names, vec!["alpha", "zeta"]);
    }

    #[tokio::test]
    async fn list_reports_size_created_and_holder() {
        let registry = reg();
        let fs = FakeFs::with(&["/store/prism-data.img"]);
        fs.set_allocated("/store/prism-data.img", 1024);
        let _held = acquire_with(
            &fs,
            &registry,
            Path::new("/store"),
            "prism-data",
            "aa07",
            VOLUME_DEFAULT_SIZE_BYTES,
        )
        .await
        .unwrap();
        let got = list_with(&fs, &HeldBy(&["reviewer"]), Path::new("/store"))
            .await
            .unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].size_bytes, VOLUME_DEFAULT_SIZE_BYTES);
        assert_eq!(got[0].disk_bytes, 1024);
        assert_eq!(got[0].created, "2025-12-06T12:00:00Z");
        assert_eq!(got[0].in_use_by, vec!["reviewer".to_string()]);
    }

    #[tokio::test]
    async fn list_of_a_missing_store_root_is_empty_not_an_error() {
        let fs = FakeFs {
            read_dir_missing: true,
            ..Default::default()
        };
        let got = list_with(&fs, &Idle, Path::new("/store")).await.unwrap();
        assert!(got.is_empty());
    }

    #[tokio::test]
    async fn list_propagates_a_read_dir_failure() {
        let fs = FakeFs {
            fail_read_dir: true,
            ..Default::default()
        };
        let err = list_with(&fs, &Idle, Path::new("/store"))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("read_dir boom"), "got: {err}");
    }

    #[tokio::test]
    async fn list_propagates_a_metadata_failure() {
        let fs = FakeFs {
            existing: Mutex::new(
                [PathBuf::from("/store/prism-data.img")]
                    .into_iter()
                    .collect(),
            ),
            fail_metadata: true,
            ..Default::default()
        };
        let err = list_with(&fs, &Idle, Path::new("/store"))
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("metadata boom"), "got: {err}");
    }

    #[tokio::test]
    async fn list_skips_a_volume_that_vanishes_before_its_metadata_read() {
        let fs = FakeFs::with(&["/store/alpha.img", "/store/zeta.img"]);
        fs.vanish("/store/zeta.img");
        let got = list_with(&fs, &Idle, Path::new("/store")).await.unwrap();
        let names: Vec<&str> = got.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["alpha"],
            "the vanished entry is skipped, not fatal"
        );
    }

    #[tokio::test]
    async fn create_reports_an_already_held_volume_as_in_use_without_recreating_it() {
        let registry = reg();
        let fs = FakeFs::default();
        let _held = acquire_with(
            &fs,
            &registry,
            Path::new("/store"),
            "prism-data",
            "aa07",
            VOLUME_DEFAULT_SIZE_BYTES,
        )
        .await
        .unwrap();
        let info = create_with(
            &fs,
            &registry,
            &HeldBy(&["reviewer"]),
            Path::new("/store"),
            "prism-data",
        )
        .await
        .unwrap();
        assert_eq!(info.in_use_by, vec!["reviewer".to_string()]);
        assert_eq!(fs.created_images().len(), 1, "no second image");
    }

    #[tokio::test]
    async fn create_of_a_new_image_yields_to_a_concurrent_holder_instead_of_racing_the_temp_file() {
        let registry = reg();
        let fs = FakeFs::default();
        let _held = take_lease(&registry, "prism-data", "aa09").unwrap();
        let err = create_with(&fs, &registry, &Idle, Path::new("/store"), "prism-data")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("in use by run aa09"), "got: {err}");
        assert!(
            fs.created_images().is_empty(),
            "create must not write the backing image while a run holds the volume"
        );
    }

    #[tokio::test]
    async fn create_rejects_an_invalid_name_without_touching_disk() {
        let fs = FakeFs::default();
        let err = create_with(&fs, &reg(), &Idle, Path::new("/store"), "../etc")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("invalid volume name"), "got: {err}");
        assert!(fs.created_images().is_empty());
    }

    #[tokio::test]
    async fn inspect_of_an_unknown_volume_names_it() {
        let fs = FakeFs::default();
        let err = inspect_with(&fs, &Idle, Path::new("/store"), "prism-data")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("no such volume: prism-data"), "got: {err}");
    }

    #[tokio::test]
    async fn inspect_rejects_an_invalid_name() {
        let fs = FakeFs::default();
        let err = inspect_with(&fs, &Idle, Path::new("/store"), "a/b")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("invalid volume name"), "got: {err}");
    }

    #[tokio::test]
    async fn remove_rejects_an_invalid_name() {
        let fs = FakeFs::default();
        let err = remove_with(&fs, &reg(), &Idle, Path::new("/store"), "a b")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("invalid volume name"), "got: {err}");
    }

    #[tokio::test]
    async fn remove_of_an_in_use_volume_is_refused_naming_the_holder() {
        let registry = reg();
        let fs = FakeFs::default();
        let _held = acquire_with(
            &fs,
            &registry,
            Path::new("/store"),
            "prism-data",
            "aa07",
            VOLUME_DEFAULT_SIZE_BYTES,
        )
        .await
        .unwrap();
        let err = remove_with(&fs, &registry, &Idle, Path::new("/store"), "prism-data")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("in use by run aa07"), "got: {err}");
        assert!(fs.exists(Path::new("/store/prism-data.img")).await);
    }

    #[tokio::test]
    async fn remove_of_an_unknown_volume_errors_and_releases_the_maintenance_lease() {
        let registry = reg();
        let fs = FakeFs::default();
        let err = remove_with(&fs, &registry, &Idle, Path::new("/store"), "prism-data")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("no such volume"), "got: {err}");
        registry
            .try_acquire("prism-data", "aa02")
            .expect("a failed remove must not strand the maintenance lease");
    }

    #[tokio::test]
    async fn remove_failure_releases_the_maintenance_lease() {
        let registry = reg();
        let fs = FakeFs {
            existing: Mutex::new(
                [PathBuf::from("/store/prism-data.img")]
                    .into_iter()
                    .collect(),
            ),
            fail_remove: true,
            ..Default::default()
        };
        remove_with(&fs, &registry, &Idle, Path::new("/store"), "prism-data")
            .await
            .expect_err("remove should fail");
        registry
            .try_acquire("prism-data", "aa02")
            .expect("a failed remove must not strand the maintenance lease");
    }

    #[tokio::test]
    async fn remove_frees_the_name_for_an_immediate_reattach() {
        let registry = reg();
        let fs = FakeFs::with(&["/store/prism-data.img"]);
        remove_with(&fs, &registry, &Idle, Path::new("/store"), "prism-data")
            .await
            .unwrap();
        assert!(!fs.exists(Path::new("/store/prism-data.img")).await);
        acquire_with(
            &fs,
            &registry,
            Path::new("/store"),
            "prism-data",
            "aa03",
            VOLUME_DEFAULT_SIZE_BYTES,
        )
        .await
        .expect("the name must be free right after remove");
    }

    #[tokio::test]
    async fn prune_keeps_a_volume_whose_image_is_open_and_says_so() {
        let registry = reg();
        let fs = FakeFs::with(&["/store/held.img", "/store/idle.img"]);
        fs.set_allocated("/store/idle.img", 2048);
        let _held = acquire_with(
            &fs,
            &registry,
            Path::new("/store"),
            "held",
            "aa07",
            VOLUME_DEFAULT_SIZE_BYTES,
        )
        .await
        .unwrap();
        let report = prune_with(&fs, &registry, &Idle, Path::new("/store"), false)
            .await
            .unwrap();
        assert_eq!(report.removed, vec!["idle".to_string()]);
        assert_eq!(report.reclaimed_bytes, 2048);
        assert_eq!(report.failed.len(), 1, "got {:?}", report.failed);
        assert_eq!(report.failed[0].name, "held");
        assert!(fs.exists(Path::new("/store/held.img")).await);
        assert!(!fs.exists(Path::new("/store/idle.img")).await);
    }

    #[tokio::test]
    async fn a_dry_run_and_the_sweep_agree_about_a_volume_whose_image_is_open() {
        let registry = reg();
        let fs = FakeFs::with(&["/store/big.img"]);
        let _open = take_lease(&registry, "big", "aa07").expect("a free volume");
        let planned = prune_with(&fs, &registry, &Idle, Path::new("/store"), true)
            .await
            .unwrap();
        let swept = prune_with(&fs, &registry, &Idle, Path::new("/store"), false)
            .await
            .unwrap();
        assert_eq!(
            (planned.removed, planned.failed.len()),
            (swept.removed.clone(), swept.failed.len()),
            "the question and the sweep must classify a volume the same way, or the user consents to one thing and gets another"
        );
        assert!(
            swept.removed.is_empty(),
            "the image is open, so nothing was removed"
        );
        assert_eq!(
            swept.failed.len(),
            1,
            "a volume the user asked to remove and that was kept must say so"
        );
        assert!(fs.exists(Path::new("/store/big.img")).await);
    }

    #[tokio::test]
    async fn prune_releases_its_maintenance_leases() {
        let registry = reg();
        let fs = FakeFs::with(&["/store/idle.img"]);
        prune_with(&fs, &registry, &Idle, Path::new("/store"), false)
            .await
            .unwrap();
        registry
            .try_acquire("idle", "aa02")
            .expect("prune must not strand maintenance leases");
    }

    #[tokio::test]
    async fn prune_records_a_remove_failure_and_keeps_reclaiming_the_rest() {
        let fs = FakeFs::with(&["/store/bad.img", "/store/good.img"]);
        fs.set_allocated("/store/good.img", 4096);
        fs.refuse_remove("/store/bad.img");
        let report = prune_with(&fs, &reg(), &Idle, Path::new("/store"), false)
            .await
            .unwrap();
        assert_eq!(report.removed, vec!["good".to_string()]);
        assert_eq!(report.reclaimed_bytes, 4096);
        assert_eq!(report.failed.len(), 1, "got {:?}", report.failed);
        assert_eq!(report.failed[0].name, "bad");
        assert!(report.failed[0].error.contains("remove boom"));
        assert!(
            fs.exists(Path::new("/store/bad.img")).await,
            "the unremovable image stays"
        );
        assert!(!fs.exists(Path::new("/store/good.img")).await);
    }

    #[test]
    fn volume_name_of_extracts_only_img_file_names() {
        assert_eq!(
            volume_name_of(Path::new("/store/prism-data.img")),
            Some("prism-data".to_string())
        );
        assert_eq!(volume_name_of(Path::new("/store/notes.txt")), None);
        assert_eq!(volume_name_of(Path::new("/")), None);
    }

    #[tokio::test]
    async fn real_fs_read_dir_metadata_and_remove_file_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vol.img");
        std::fs::write(&path, b"0123456789").unwrap();
        let listed = real::RealFs.read_dir(dir.path()).await.unwrap();
        assert_eq!(listed, vec![path.clone()]);
        let meta = real::RealFs.metadata(&path).await.unwrap();
        assert_eq!(meta.size_bytes, 10);
        assert!(meta.allocated_bytes > 0);
        assert!(meta.created_unix_secs > 0);
        real::RealFs.remove_file(&path).await.unwrap();
        assert!(!real::RealFs.exists(&path).await);
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
    #[serial_test::serial(env)]
    async fn lifecycle_production_wrappers_round_trip_under_the_cache_root() {
        let d = tempfile::tempdir().unwrap();
        let _h = crate::test_env::EnvVarGuard::set("HOME", d.path());
        let info = create("cov-lifecycle").await.unwrap();
        assert_eq!(info.name, "cov-lifecycle");
        assert!(
            list()
                .await
                .unwrap()
                .iter()
                .any(|v| v.name == "cov-lifecycle")
        );
        assert!(inspect("cov-lifecycle").await.unwrap().in_use_by.is_empty());
        remove("cov-lifecycle").await.unwrap();
        drop(create("cov-prune").await.unwrap());
        let report = prune(false).await.unwrap();
        assert!(report.removed.contains(&"cov-prune".to_string()));
    }
}
