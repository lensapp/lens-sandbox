use std::path::{Path, PathBuf};

pub(super) trait RemoveDir {
    fn remove_dir_all(&self, dir: &Path) -> std::io::Result<()>;
}

pub(super) struct RealRemoveDir;

impl RemoveDir for RealRemoveDir {
    fn remove_dir_all(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::remove_dir_all(dir)
    }
}

/// Removes a run's scratch dir on drop unless `keep()` is called, so prep that fails before boot can't orphan its upper.img.
pub(super) struct RunScratchGuard<R: RemoveDir> {
    dir: PathBuf,
    armed: bool,
    remover: R,
}

impl<R: RemoveDir> RunScratchGuard<R> {
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

impl<R: RemoveDir> Drop for RunScratchGuard<R> {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        match self.remover.remove_dir_all(&self.dir) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                let dir = self.dir.display();
                crate::log::warn!(
                    "run scratch cleanup failed at {dir} ({e}); orphaned until prune"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::ErrorKind;
    use std::sync::{Arc, Mutex};

    struct FakeRemover {
        calls: Arc<Mutex<Vec<PathBuf>>>,
        err: Option<ErrorKind>,
    }

    impl FakeRemover {
        fn returning(err: Option<ErrorKind>) -> (Self, Arc<Mutex<Vec<PathBuf>>>) {
            let calls = Arc::new(Mutex::new(Vec::new()));
            (
                Self {
                    calls: Arc::clone(&calls),
                    err,
                },
                calls,
            )
        }
    }

    impl RemoveDir for FakeRemover {
        fn remove_dir_all(&self, dir: &Path) -> std::io::Result<()> {
            self.calls.lock().unwrap().push(dir.to_path_buf());
            match self.err {
                None => Ok(()),
                Some(kind) => Err(std::io::Error::new(kind, "fake remover")),
            }
        }
    }

    #[test]
    fn an_armed_guard_removes_the_run_dir_on_drop() {
        let dir = PathBuf::from("/cache/runs/7");
        let (remover, calls) = FakeRemover::returning(None);
        drop(RunScratchGuard::new(dir.clone(), remover));
        assert_eq!(
            *calls.lock().unwrap(),
            vec![dir],
            "a prep failure must reclaim the scratch dir",
        );
    }

    #[test]
    fn a_kept_guard_leaves_the_run_dir_in_place() {
        let (remover, calls) = FakeRemover::returning(None);
        let mut guard = RunScratchGuard::new(PathBuf::from("/cache/runs/7"), remover);
        guard.keep();
        drop(guard);
        assert!(
            calls.lock().unwrap().is_empty(),
            "a started run owns its dir — logs and audit must survive",
        );
    }

    #[test]
    fn an_already_gone_dir_is_a_silent_no_op() {
        let (remover, calls) = FakeRemover::returning(Some(ErrorKind::NotFound));
        drop(RunScratchGuard::new(
            PathBuf::from("/cache/runs/7"),
            remover,
        ));
        assert_eq!(
            calls.lock().unwrap().len(),
            1,
            "removal is attempted, and a missing dir is not an error",
        );
    }

    #[test]
    fn a_removal_failure_is_logged_not_panicked() {
        let (remover, calls) = FakeRemover::returning(Some(ErrorKind::PermissionDenied));
        drop(RunScratchGuard::new(
            PathBuf::from("/cache/runs/7"),
            remover,
        ));
        assert_eq!(calls.lock().unwrap().len(), 1);
    }

    #[test]
    fn the_real_remover_deletes_a_populated_run_dir() {
        let base = tempfile::tempdir().unwrap();
        let run_dir = base.path().join("runs").join("9");
        std::fs::create_dir_all(run_dir.join("sub")).unwrap();
        std::fs::write(run_dir.join("upper.img"), b"scratch").unwrap();
        RealRemoveDir.remove_dir_all(&run_dir).unwrap();
        assert!(!run_dir.exists());
    }
}
