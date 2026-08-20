use std::io::ErrorKind;
use std::path::{Path, PathBuf};

pub trait ScratchFs {
    fn remove_dir_all(&self, dir: &Path) -> std::io::Result<()>;
    fn remove_file(&self, path: &Path) -> std::io::Result<()>;
    fn list_dirs(&self, dir: &Path) -> std::io::Result<Vec<PathBuf>>;
    fn allocated_bytes(&self, dir: &Path) -> u64;
}

pub struct RealScratchFs;

impl ScratchFs for RealScratchFs {
    fn remove_dir_all(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::remove_dir_all(dir)
    }

    fn remove_file(&self, path: &Path) -> std::io::Result<()> {
        std::fs::remove_file(path)
    }

    fn list_dirs(&self, dir: &Path) -> std::io::Result<Vec<PathBuf>> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                out.push(entry.path());
            }
        }
        Ok(out)
    }

    fn allocated_bytes(&self, dir: &Path) -> u64 {
        allocated_under(dir)
    }
}

fn allocated_under(path: &Path) -> u64 {
    use std::os::unix::fs::MetadataExt;
    let Ok(meta) = std::fs::symlink_metadata(path) else { return 0 };
    if !meta.is_dir() {
        return meta.blocks() * 512;
    }
    let Ok(entries) = std::fs::read_dir(path) else { return 0 };
    entries
        .flatten()
        .map(|entry| allocated_under(&entry.path()))
        .sum()
}

fn run_dir(root: &Path, run_id: &str) -> PathBuf {
    root.join("runs").join(run_id)
}

pub fn reclaim_upper_with<F: ScratchFs>(fs: &F, root: &Path, run_id: &str) {
    let upper = run_dir(root, run_id).join("upper.img");
    match fs.remove_file(&upper) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => {
            let upper = upper.display();
            crate::log::warn!("run upper reclaim failed at {upper} ({e}); orphaned until removal");
        }
    }
}

pub fn remove_dir_with<F: ScratchFs>(fs: &F, root: &Path, run_id: &str) -> u64 {
    let dir = run_dir(root, run_id);
    let bytes = fs.allocated_bytes(&dir);
    match fs.remove_dir_all(&dir) {
        Ok(()) => bytes,
        Err(e) if e.kind() == ErrorKind::NotFound => 0,
        Err(e) => {
            let dir = dir.display();
            crate::log::warn!("run scratch reclaim failed at {dir} ({e}); orphaned until sweep");
            0
        }
    }
}

pub fn prune_with<F: ScratchFs>(fs: &F, root: &Path, run_ids: &[String]) -> u64 {
    run_ids
        .iter()
        .map(|id| remove_dir_with(fs, root, id))
        .sum()
}

pub fn sweep_orphans_with<F: ScratchFs>(fs: &F, root: &Path, live: &[String]) {
    let runs = root.join("runs");
    let dirs = match fs.list_dirs(&runs) {
        Ok(dirs) => dirs,
        Err(e) if e.kind() == ErrorKind::NotFound => return,
        Err(e) => {
            let runs = runs.display();
            crate::log::warn!("run scratch sweep could not list {runs} ({e})");
            return;
        }
    };
    for dir in dirs {
        let Some(name) = dir.file_name().and_then(|n| n.to_str()) else { continue };
        if live.iter().any(|id| id == name) {
            continue;
        }
        match fs.remove_dir_all(&dir) {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => {
                let dir = dir.display();
                crate::log::warn!("run scratch sweep failed at {dir} ({e})");
            }
        }
    }
}

pub fn reclaim_upper(run_id: &str) {
    let Ok(root) = crate::cache::root() else { return };
    reclaim_upper_with(&RealScratchFs, &root, run_id);
}

pub fn remove_dir(run_id: &str) -> u64 {
    let Ok(root) = crate::cache::root() else { return 0 };
    remove_dir_with(&RealScratchFs, &root, run_id)
}

pub fn prune(run_ids: &[String]) -> u64 {
    let Ok(root) = crate::cache::root() else { return 0 };
    prune_with(&RealScratchFs, &root, run_ids)
}

pub fn sweep_orphans() {
    let Ok(root) = crate::cache::root() else { return };
    let live: Vec<String> = crate::run_registry::snapshot()
        .into_iter()
        .map(|run| run.id)
        .collect();
    sweep_orphans_with(&RealScratchFs, &root, &live);
}

/// Removes a run's scratch dir on drop unless `keep()` is called, so prep that fails before boot can't orphan its upper.img.
pub(super) struct RunScratchGuard<R: ScratchFs> {
    dir: PathBuf,
    armed: bool,
    remover: R,
}

impl<R: ScratchFs> RunScratchGuard<R> {
    pub(super) fn new(dir: PathBuf, remover: R) -> Self {
        Self {
            dir,
            armed: true,
            remover,
        }
    }

    pub(super) fn keep(&mut self) {
        self.armed = false;
    }
}

impl<R: ScratchFs> Drop for RunScratchGuard<R> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        match self.remover.remove_dir_all(&self.dir) {
            Ok(()) => {}
            Err(e) if e.kind() == ErrorKind::NotFound => {}
            Err(e) => {
                let dir = self.dir.display();
                crate::log::warn!(
                    "run scratch cleanup failed at {dir} ({e}); reclaimed by the startup sweep"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct FakeFsState {
        removed_dirs: Vec<PathBuf>,
        removed_files: Vec<PathBuf>,
        remove_dir_err: Option<ErrorKind>,
        remove_file_err: Option<ErrorKind>,
        list_err: Option<ErrorKind>,
        dirs: Vec<PathBuf>,
        usage: u64,
    }

    #[derive(Clone, Default)]
    struct FakeFs(Arc<Mutex<FakeFsState>>);

    impl FakeFs {
        fn with(f: impl FnOnce(&mut FakeFsState)) -> Self {
            let fs = Self::default();
            f(&mut fs.0.lock().unwrap());
            fs
        }

        fn removed_dirs(&self) -> Vec<PathBuf> {
            self.0.lock().unwrap().removed_dirs.clone()
        }

        fn removed_files(&self) -> Vec<PathBuf> {
            self.0.lock().unwrap().removed_files.clone()
        }
    }

    impl ScratchFs for FakeFs {
        fn remove_dir_all(&self, dir: &Path) -> std::io::Result<()> {
            let mut s = self.0.lock().unwrap();
            s.removed_dirs.push(dir.to_path_buf());
            match s.remove_dir_err {
                None => Ok(()),
                Some(kind) => Err(std::io::Error::new(kind, "fake remove_dir_all")),
            }
        }

        fn remove_file(&self, path: &Path) -> std::io::Result<()> {
            let mut s = self.0.lock().unwrap();
            s.removed_files.push(path.to_path_buf());
            match s.remove_file_err {
                None => Ok(()),
                Some(kind) => Err(std::io::Error::new(kind, "fake remove_file")),
            }
        }

        fn list_dirs(&self, _dir: &Path) -> std::io::Result<Vec<PathBuf>> {
            let s = self.0.lock().unwrap();
            match s.list_err {
                None => Ok(s.dirs.clone()),
                Some(kind) => Err(std::io::Error::new(kind, "fake list_dirs")),
            }
        }

        fn allocated_bytes(&self, _dir: &Path) -> u64 {
            self.0.lock().unwrap().usage
        }
    }

    const ROOT: &str = "/cache";

    #[test]
    fn reclaim_upper_removes_only_the_upper_disk() {
        let fs = FakeFs::default();
        reclaim_upper_with(&fs, Path::new(ROOT), "7");
        assert_eq!(
            fs.removed_files(),
            vec![PathBuf::from("/cache/runs/7/upper.img")],
            "exit reclaim must target upper.img and nothing else",
        );
        assert!(
            fs.removed_dirs().is_empty(),
            "console.log stays for post-mortem until removal",
        );
    }

    #[test]
    fn reclaim_upper_treats_a_missing_upper_as_already_reclaimed() {
        let fs = FakeFs::with(|s| s.remove_file_err = Some(ErrorKind::NotFound));
        reclaim_upper_with(&fs, Path::new(ROOT), "7");
        assert_eq!(fs.removed_files().len(), 1);
    }

    #[test]
    fn reclaim_upper_logs_a_failure_and_never_panics() {
        let fs = FakeFs::with(|s| s.remove_file_err = Some(ErrorKind::PermissionDenied));
        reclaim_upper_with(&fs, Path::new(ROOT), "7");
        assert_eq!(fs.removed_files().len(), 1);
    }

    #[test]
    fn remove_dir_reports_the_bytes_the_dir_held() {
        let fs = FakeFs::with(|s| s.usage = 4096);
        let bytes = remove_dir_with(&fs, Path::new(ROOT), "7");
        assert_eq!(bytes, 4096);
        assert_eq!(fs.removed_dirs(), vec![PathBuf::from("/cache/runs/7")]);
    }

    #[test]
    fn remove_dir_of_an_already_gone_dir_reclaims_nothing() {
        let fs = FakeFs::with(|s| {
            s.usage = 4096;
            s.remove_dir_err = Some(ErrorKind::NotFound);
        });
        assert_eq!(remove_dir_with(&fs, Path::new(ROOT), "7"), 0);
    }

    #[test]
    fn remove_dir_failure_is_logged_and_reports_nothing_reclaimed() {
        let fs = FakeFs::with(|s| {
            s.usage = 4096;
            s.remove_dir_err = Some(ErrorKind::PermissionDenied);
        });
        assert_eq!(remove_dir_with(&fs, Path::new(ROOT), "7"), 0);
    }

    #[test]
    fn prune_sums_the_bytes_of_every_removed_dir() {
        let fs = FakeFs::with(|s| s.usage = 1024);
        let bytes = prune_with(&fs, Path::new(ROOT), &["a".into(), "b".into()]);
        assert_eq!(bytes, 2048);
        assert_eq!(fs.removed_dirs().len(), 2);
    }

    #[test]
    fn sweep_removes_only_dirs_with_no_live_run() {
        let fs = FakeFs::with(|s| {
            s.dirs = vec![
                PathBuf::from("/cache/runs/live1"),
                PathBuf::from("/cache/runs/orphan"),
            ];
        });
        sweep_orphans_with(&fs, Path::new(ROOT), &["live1".into()]);
        assert_eq!(
            fs.removed_dirs(),
            vec![PathBuf::from("/cache/runs/orphan")],
            "a live run's scratch dir must survive the sweep",
        );
    }

    #[test]
    fn sweep_of_a_missing_runs_dir_is_a_no_op() {
        let fs = FakeFs::with(|s| s.list_err = Some(ErrorKind::NotFound));
        sweep_orphans_with(&fs, Path::new(ROOT), &[]);
        assert!(fs.removed_dirs().is_empty());
    }

    #[test]
    fn sweep_logs_an_unlistable_runs_dir_and_removes_nothing() {
        let fs = FakeFs::with(|s| s.list_err = Some(ErrorKind::PermissionDenied));
        sweep_orphans_with(&fs, Path::new(ROOT), &[]);
        assert!(fs.removed_dirs().is_empty());
    }

    #[test]
    fn sweep_tolerates_a_dir_that_vanishes_or_resists_mid_sweep() {
        let fs = FakeFs::with(|s| {
            s.dirs = vec![PathBuf::from("/cache/runs/gone")];
            s.remove_dir_err = Some(ErrorKind::NotFound);
        });
        sweep_orphans_with(&fs, Path::new(ROOT), &[]);
        let fs = FakeFs::with(|s| {
            s.dirs = vec![PathBuf::from("/cache/runs/stuck")];
            s.remove_dir_err = Some(ErrorKind::PermissionDenied);
        });
        sweep_orphans_with(&fs, Path::new(ROOT), &[]);
        assert_eq!(fs.removed_dirs().len(), 1, "removal is attempted once");
    }

    #[test]
    fn the_real_fs_deletes_a_populated_run_dir() {
        let base = tempfile::tempdir().unwrap();
        let run_dir = base.path().join("runs").join("9");
        std::fs::create_dir_all(run_dir.join("sub")).unwrap();
        std::fs::write(run_dir.join("upper.img"), b"scratch").unwrap();
        RealScratchFs.remove_dir_all(&run_dir).unwrap();
        assert!(!run_dir.exists());
    }

    #[test]
    fn the_real_fs_removes_a_single_file_and_lists_only_directories() {
        let base = tempfile::tempdir().unwrap();
        let runs = base.path().join("runs");
        std::fs::create_dir_all(runs.join("a")).unwrap();
        std::fs::write(runs.join("stray-file"), b"x").unwrap();
        std::fs::write(runs.join("a").join("upper.img"), b"scratch").unwrap();
        RealScratchFs
            .remove_file(&runs.join("a").join("upper.img"))
            .unwrap();
        assert!(!runs.join("a").join("upper.img").exists());
        assert_eq!(RealScratchFs.list_dirs(&runs).unwrap(), vec![runs.join("a")]);
    }

    #[test]
    fn the_real_fs_reports_actual_usage_not_apparent_size_and_zero_when_gone() {
        let base = tempfile::tempdir().unwrap();
        let dir = base.path().join("runs").join("9");
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("upper.img"), vec![7u8; 8192]).unwrap();
        std::fs::write(dir.join("sub").join("console.log"), b"boot").unwrap();
        let bytes = RealScratchFs.allocated_bytes(&dir);
        assert!(bytes >= 8192, "written data must be accounted, got {bytes}");
        assert_eq!(RealScratchFs.allocated_bytes(&base.path().join("missing")), 0);
    }

    #[test]
    fn an_armed_guard_removes_the_run_dir_on_drop() {
        let dir = PathBuf::from("/cache/runs/7");
        let fs = FakeFs::default();
        drop(RunScratchGuard::new(dir.clone(), fs.clone()));
        assert_eq!(
            fs.removed_dirs(),
            vec![dir],
            "a prep failure must reclaim the scratch dir",
        );
    }

    #[test]
    fn a_kept_guard_leaves_the_run_dir_in_place() {
        let fs = FakeFs::default();
        let mut guard = RunScratchGuard::new(PathBuf::from("/cache/runs/7"), fs.clone());
        guard.keep();
        drop(guard);
        assert!(
            fs.removed_dirs().is_empty(),
            "a started run owns its dir — logs and audit must survive",
        );
    }

    #[test]
    fn an_already_gone_dir_is_a_silent_no_op() {
        let fs = FakeFs::with(|s| s.remove_dir_err = Some(ErrorKind::NotFound));
        drop(RunScratchGuard::new(PathBuf::from("/cache/runs/7"), fs.clone()));
        assert_eq!(
            fs.removed_dirs().len(),
            1,
            "removal is attempted, and a missing dir is not an error",
        );
    }

    #[test]
    fn a_removal_failure_is_logged_not_panicked() {
        let fs = FakeFs::with(|s| s.remove_dir_err = Some(ErrorKind::PermissionDenied));
        drop(RunScratchGuard::new(PathBuf::from("/cache/runs/7"), fs.clone()));
        assert_eq!(fs.removed_dirs().len(), 1);
    }

    #[test]
    #[serial_test::serial(env)]
    fn the_production_wrappers_reclaim_under_the_real_cache_root() {
        let d = tempfile::tempdir().unwrap();
        let _h = crate::test_env::EnvVarGuard::set("HOME", d.path());
        let _x = crate::test_env::EnvVarGuard::set("XDG_CACHE_HOME", d.path().join("cache"));
        let root = crate::cache::root().unwrap();
        let exited = root.join("runs").join("exited1");
        let orphan = root.join("runs").join("orphan1");
        std::fs::create_dir_all(&exited).unwrap();
        std::fs::create_dir_all(&orphan).unwrap();
        std::fs::write(exited.join("upper.img"), vec![7u8; 4096]).unwrap();
        std::fs::write(exited.join("console.log"), b"boot").unwrap();

        reclaim_upper("exited1");
        assert!(!exited.join("upper.img").exists());
        assert!(exited.join("console.log").exists());

        assert!(remove_dir("exited1") > 0, "removal reports the bytes it freed");
        assert!(!exited.exists());

        std::fs::create_dir_all(&exited).unwrap();
        std::fs::write(exited.join("upper.img"), vec![7u8; 4096]).unwrap();
        let reclaimed = prune(&["exited1".into()]);
        assert!(reclaimed > 0, "prune sums what its removals freed");
        assert!(!exited.exists());

        sweep_orphans();
        assert!(!orphan.exists(), "an orphaned dir is swept at startup");
    }
}
