mod real;
mod traits;

pub use traits::Fs;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{Result, bail};

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
    active: Mutex<HashMap<String, u32>>,
}

impl LeaseRegistry {
    pub fn new() -> Self {
        Self {
            active: Mutex::new(HashMap::new()),
        }
    }

    fn try_acquire(&self, name: &str, run_id: u32) -> std::result::Result<(), u32> {
        let mut g = self.active.lock().expect("lease registry poisoned");
        match g.get(name) {
            Some(&holder) => Err(holder),
            None => {
                g.insert(name.to_string(), run_id);
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

pub async fn acquire(name: &str, run_id: u32) -> Result<Acquired> {
    acquire_with(&real::RealFs, &global(), &store_root()?, name, run_id).await
}

pub async fn resolve(
    mounts: &[lns_ipc::VolumeMount],
    run_id: u32,
) -> Result<(Vec<crate::vm::VolumeAttachment>, Vec<VolumeLease>)> {
    resolve_with(&real::RealFs, &global(), &store_root()?, mounts, run_id).await
}

pub async fn resolve_with<F: Fs>(
    fs: &F,
    registry: &Arc<LeaseRegistry>,
    store_root: &Path,
    mounts: &[lns_ipc::VolumeMount],
    run_id: u32,
) -> Result<(Vec<crate::vm::VolumeAttachment>, Vec<VolumeLease>)> {
    let mut attachments = Vec::with_capacity(mounts.len());
    let mut leases = Vec::new();
    let mut image_by_name: HashMap<&str, PathBuf> = HashMap::new();
    for m in mounts {
        validate_target(&m.target)?;
        let image_path = if let Some(p) = image_by_name.get(m.name.as_str()) {
            p.clone()
        } else {
            let acq = acquire_with(fs, registry, store_root, &m.name, run_id).await?;
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

pub async fn acquire_with<F: Fs>(
    fs: &F,
    registry: &Arc<LeaseRegistry>,
    store_root: &Path,
    name: &str,
    run_id: u32,
) -> Result<Acquired> {
    validate_name(name)?;

    if let Err(holder) = registry.try_acquire(name, run_id) {
        bail!("volume {name:?} in use by run #{holder}");
    }
    let lease = VolumeLease {
        registry: registry.clone(),
        name: name.to_string(),
    };

    let image_path = store_root.join(format!("{name}.img"));
    if !fs.exists(&image_path).await {
        fs.create_dir_all(store_root).await?;
        fs.create_ext4_image(&image_path, VOLUME_DEFAULT_SIZE_BYTES)
            .await?;
    }

    Ok(Acquired { image_path, lease })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::io;

    #[derive(Default)]
    struct FakeFs {
        existing: Mutex<HashSet<PathBuf>>,
        created: Mutex<Vec<PathBuf>>,
        fail_create: bool,
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
    }

    impl Fs for FakeFs {
        async fn exists(&self, p: &Path) -> bool {
            self.existing.lock().unwrap().contains(p)
        }
        async fn create_dir_all(&self, _p: &Path) -> io::Result<()> {
            Ok(())
        }
        async fn create_ext4_image(&self, p: &Path, _size: u64) -> io::Result<()> {
            if self.fail_create {
                return Err(io::Error::other("boom"));
            }
            self.existing.lock().unwrap().insert(p.to_path_buf());
            self.created.lock().unwrap().push(p.to_path_buf());
            Ok(())
        }
    }

    fn reg() -> Arc<LeaseRegistry> {
        Arc::new(LeaseRegistry::new())
    }

    #[tokio::test]
    async fn acquiring_unknown_name_creates_the_backing_image() {
        let fs = FakeFs::default();
        let got = acquire_with(&fs, &reg(), Path::new("/store"), "prism-data", 1)
            .await
            .unwrap();
        assert_eq!(got.image_path, Path::new("/store/prism-data.img"));
        assert_eq!(
            fs.created_images(),
            vec![PathBuf::from("/store/prism-data.img")]
        );
    }

    #[tokio::test]
    async fn acquiring_existing_name_does_not_recreate_the_image() {
        let fs = FakeFs::with(&["/store/prism-data.img"]);
        let got = acquire_with(&fs, &reg(), Path::new("/store"), "prism-data", 1)
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
            },
            lns_ipc::VolumeMount {
                name: "prism-data".into(),
                target: "/srv/state".into(),
                read_only: true,
            },
        ];
        let (attachments, leases) = resolve_with(&fs, &registry, Path::new("/store"), &mounts, 1)
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
        let _held = acquire_with(&fs, &registry, Path::new("/store"), "prism-data", 7)
            .await
            .unwrap();
        let err = acquire_with(&fs, &registry, Path::new("/store"), "prism-data", 8)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("in use by run #7"), "got: {err}");
    }

    #[tokio::test]
    async fn dropping_the_lease_frees_the_volume_for_the_next_run() {
        let registry = reg();
        let fs = FakeFs::with(&["/store/prism-data.img"]);
        {
            let _held = acquire_with(&fs, &registry, Path::new("/store"), "prism-data", 7)
                .await
                .unwrap();
        }
        acquire_with(&fs, &registry, Path::new("/store"), "prism-data", 8)
            .await
            .expect("volume should be free after the prior lease dropped");
    }

    #[tokio::test]
    async fn invalid_name_is_refused_before_any_image_is_created_or_lease_taken() {
        let registry = reg();
        let fs = FakeFs::default();
        let err = acquire_with(&fs, &registry, Path::new("/store"), "../etc", 1)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("invalid volume name"), "got: {err}");
        assert!(
            fs.created_images().is_empty(),
            "no image for an invalid name"
        );
        registry
            .try_acquire("../etc", 2)
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
        }];
        let err = resolve_with(&fs, &registry, Path::new("/store"), &mounts, 1)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("must not contain whitespace"), "got: {err}");
        assert!(
            fs.created_images().is_empty(),
            "no image for an unparseable target"
        );
        registry
            .try_acquire("prism-data", 2)
            .expect("a rejected target must not strand the lease");
    }

    #[tokio::test]
    async fn create_failure_releases_the_lease() {
        let registry = reg();
        let fs = FakeFs {
            fail_create: true,
            ..Default::default()
        };
        acquire_with(&fs, &registry, Path::new("/store"), "prism-data", 1)
            .await
            .expect_err("create should fail");
        registry
            .try_acquire("prism-data", 2)
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
            .try_acquire("x", 1)
            .expect("a default registry holds no leases");
    }

    #[test]
    fn global_returns_one_shared_registry() {
        assert!(Arc::ptr_eq(&global(), &global()));
    }

    #[tokio::test]
    #[serial_test::serial(cache_root)]
    async fn store_root_lives_under_the_cache_root() {
        let d = tempfile::tempdir().unwrap();
        let _h = crate::test_env::EnvVarGuard::set("HOME", d.path());
        let _x = crate::test_env::EnvVarGuard::set("XDG_CACHE_HOME", d.path().join("cache"));
        assert!(store_root().unwrap().ends_with("volumes"));
    }

    #[tokio::test]
    #[serial_test::serial(cache_root)]
    async fn acquire_production_wrapper_creates_under_store_and_releases_on_drop() {
        let d = tempfile::tempdir().unwrap();
        let _h = crate::test_env::EnvVarGuard::set("HOME", d.path());
        let _x = crate::test_env::EnvVarGuard::set("XDG_CACHE_HOME", d.path().join("cache"));
        let acq = acquire("cov-acquire", 1).await.unwrap();
        assert!(acq.image_path.ends_with("cov-acquire.img"));
        drop(acq);
        drop(
            acquire("cov-acquire", 2)
                .await
                .expect("lease freed on drop"),
        );
    }

    #[tokio::test]
    #[serial_test::serial(cache_root)]
    async fn resolve_production_wrapper_maps_mounts_to_attachments() {
        let d = tempfile::tempdir().unwrap();
        let _h = crate::test_env::EnvVarGuard::set("HOME", d.path());
        let _x = crate::test_env::EnvVarGuard::set("XDG_CACHE_HOME", d.path().join("cache"));
        let mounts = [lns_ipc::VolumeMount {
            name: "cov-resolve".to_string(),
            target: "/data".to_string(),
            read_only: true,
        }];
        let (attachments, leases) = resolve(&mounts, 3).await.unwrap();
        assert_eq!(attachments.len(), 1);
        assert_eq!(attachments[0].target, "/data");
        assert!(attachments[0].read_only);
        drop(leases);
    }
}
