use std::collections::HashSet;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use lns_ipc::{RunStatus, RunSummary};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunMetadataRoots {
    pub metadata_runs: PathBuf,
    pub scratch_runs: PathBuf,
}

pub trait RunMetadataFs {
    fn create_dir_all(&self, path: &Path) -> io::Result<()>;
    fn write_secure_file(&self, path: &Path, bytes: &[u8]) -> io::Result<()>;
    fn read_to_string(&self, path: &Path) -> io::Result<String>;
    fn read_dir_paths(&self, path: &Path) -> io::Result<Vec<PathBuf>>;
    fn remove_dir_all(&self, path: &Path) -> io::Result<()>;
}

#[derive(Clone, Copy)]
pub struct RealRunMetadataFs;

impl RunMetadataFs for RealRunMetadataFs {
    fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        std::fs::create_dir_all(path)
    }

    fn write_secure_file(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
        write_secure_file(path, bytes)
    }

    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        std::fs::read_to_string(path)
    }

    fn read_dir_paths(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        std::fs::read_dir(path)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect()
    }

    fn remove_dir_all(&self, path: &Path) -> io::Result<()> {
        std::fs::remove_dir_all(path)
    }
}

#[cfg(unix)]
fn write_secure_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let tmp = path.with_extension("json.tmp");
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(&tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    std::fs::rename(tmp, path)
}

#[cfg(not(unix))]
fn write_secure_file(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(tmp, path)
}

pub fn production_roots() -> Result<RunMetadataRoots> {
    Ok(RunMetadataRoots {
        metadata_runs: lns_ipc::data_root()?.join("runs"),
        scratch_runs: lns_ipc::cache_root()?.join("runs"),
    })
}

pub fn persist(summary: &RunSummary) -> Result<()> {
    persist_with(
        &RealRunMetadataFs,
        &production_roots()?.metadata_runs,
        summary,
    )
}

pub fn list() -> Result<Vec<RunSummary>> {
    list_with(&RealRunMetadataFs, &production_roots()?.metadata_runs)
}

pub fn remove_run_dirs(run_id: u32) -> Result<()> {
    remove_run_dirs_with(&RealRunMetadataFs, &production_roots()?, run_id)
}

pub fn resolve_disk_handle(handle: &str) -> Result<Option<u32>> {
    resolve_disk_handle_from(&list()?, handle)
}

pub fn merge_live_and_disk(live: Vec<RunSummary>, disk: Vec<RunSummary>) -> Vec<RunSummary> {
    let live_ids: HashSet<u32> = live.iter().map(|run| run.id).collect();
    disk.into_iter()
        .filter(|run| !live_ids.contains(&run.id))
        .chain(live)
        .collect()
}

pub fn disk_only_ids(live: &[RunSummary], disk: &[RunSummary]) -> Vec<u32> {
    let live_ids: HashSet<u32> = live.iter().map(|run| run.id).collect();
    disk.iter()
        .filter(|run| !live_ids.contains(&run.id))
        .map(|run| run.id)
        .collect()
}

pub fn persist_with(
    fs: &impl RunMetadataFs,
    metadata_runs: &Path,
    summary: &RunSummary,
) -> Result<()> {
    let path = metadata_runs.join(summary.id.to_string()).join("run.json");
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("run metadata path has no parent: {}", path.display()))?;
    fs.create_dir_all(parent)
        .with_context(|| format!("creating {}", parent.display()))?;
    let bytes = serde_json::to_vec_pretty(summary)?;
    fs.write_secure_file(&path, &bytes)
        .with_context(|| format!("writing {}", path.display()))
}

pub fn list_with(fs: &impl RunMetadataFs, metadata_runs: &Path) -> Result<Vec<RunSummary>> {
    let entries = match fs.read_dir_paths(metadata_runs) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", metadata_runs.display())),
    };
    let mut summaries = Vec::new();
    for entry in entries {
        let path = entry.join("run.json");
        match fs.read_to_string(&path) {
            Ok(contents) => {
                let mut summary: RunSummary = serde_json::from_str(&contents)
                    .with_context(|| format!("parsing {}", path.display()))?;
                normalize_disk_only_status(&mut summary);
                summaries.push(summary);
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }
    Ok(summaries)
}

fn normalize_disk_only_status(summary: &mut RunSummary) {
    if matches!(summary.status, RunStatus::Running) {
        summary.status = RunStatus::Exited { code: 130 };
    }
}

pub fn remove_run_dirs_with(
    fs: &impl RunMetadataFs,
    roots: &RunMetadataRoots,
    run_id: u32,
) -> Result<()> {
    let metadata_file = roots
        .metadata_runs
        .join(run_id.to_string())
        .join("run.json");
    match fs.read_to_string(&metadata_file) {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).with_context(|| format!("reading {}", metadata_file.display())),
    }
    for root in [&roots.metadata_runs, &roots.scratch_runs] {
        let path = root.join(run_id.to_string());
        match fs.remove_dir_all(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).with_context(|| format!("removing {}", path.display())),
        }
    }
    Ok(())
}

pub fn resolve_disk_handle_from(summaries: &[RunSummary], handle: &str) -> Result<Option<u32>> {
    if let Ok(id) = handle.parse::<u32>() {
        return Ok(summaries.iter().any(|run| run.id == id).then_some(id));
    }
    let matches: Vec<u32> = summaries
        .iter()
        .filter(|run| run.name == handle)
        .map(|run| run.id)
        .collect();
    match matches.as_slice() {
        [] => Ok(None),
        [id] => Ok(Some(*id)),
        _ => anyhow::bail!("run name {handle:?} is ambiguous in history; use the run id"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    #[derive(Default)]
    struct FakeFs {
        files: RefCell<HashMap<PathBuf, String>>,
        dirs: RefCell<HashSet<PathBuf>>,
        removed: RefCell<Vec<PathBuf>>,
        remove_fail: RefCell<Option<PathBuf>>,
    }

    impl FakeFs {
        fn put_summary(&self, root: &Path, summary: &RunSummary) {
            let dir = root.join(summary.id.to_string());
            self.dirs.borrow_mut().insert(dir.clone());
            self.files.borrow_mut().insert(
                dir.join("run.json"),
                serde_json::to_string(summary).expect("serialize summary"),
            );
        }
    }

    impl RunMetadataFs for FakeFs {
        fn create_dir_all(&self, path: &Path) -> io::Result<()> {
            self.dirs.borrow_mut().insert(path.to_path_buf());
            Ok(())
        }

        fn write_secure_file(&self, path: &Path, bytes: &[u8]) -> io::Result<()> {
            self.files.borrow_mut().insert(
                path.to_path_buf(),
                String::from_utf8(bytes.to_vec()).expect("metadata is utf8"),
            );
            Ok(())
        }

        fn read_to_string(&self, path: &Path) -> io::Result<String> {
            self.files
                .borrow()
                .get(path)
                .cloned()
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
        }

        fn read_dir_paths(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
            let children = self
                .dirs
                .borrow()
                .iter()
                .filter(|dir| dir.parent() == Some(path))
                .cloned()
                .collect();
            Ok(children)
        }

        fn remove_dir_all(&self, path: &Path) -> io::Result<()> {
            if self.remove_fail.borrow().as_ref() == Some(&path.to_path_buf()) {
                return Err(io::Error::from(io::ErrorKind::PermissionDenied));
            }
            self.removed.borrow_mut().push(path.to_path_buf());
            Ok(())
        }
    }

    fn summary(id: u32, name: &str, status: RunStatus) -> RunSummary {
        RunSummary {
            id,
            name: name.into(),
            image: "some-image".into(),
            command: "some-command".into(),
            status,
            started: format!("2026-01-01T00:00:0{id}Z"),
        }
    }

    #[test]
    fn persist_writes_run_json_under_the_data_runs_root() {
        let fs = FakeFs::default();
        let root = Path::new("/data/runs");
        let run = summary(7, "reviewer", RunStatus::Running);

        persist_with(&fs, root, &run).unwrap();

        let path = root.join("7/run.json");
        let written = fs.files.borrow().get(&path).cloned().unwrap();
        assert!(written.contains("\"id\": 7"), "got: {written}");
        assert!(fs.dirs.borrow().contains(&root.join("7")));
    }

    #[test]
    fn list_reads_metadata_and_normalizes_stale_running_runs_to_exited() {
        let fs = FakeFs::default();
        let root = Path::new("/data/runs");
        fs.put_summary(root, &summary(7, "reviewer", RunStatus::Running));
        fs.put_summary(root, &summary(8, "auditor", RunStatus::Exited { code: 0 }));

        let mut listed = list_with(&fs, root).unwrap();
        listed.sort_by_key(|run| run.id);

        assert_eq!(listed[0].status, RunStatus::Exited { code: 130 });
        assert_eq!(listed[1].status, RunStatus::Exited { code: 0 });
    }

    #[test]
    fn list_missing_root_returns_empty() {
        struct MissingRootFs;
        impl RunMetadataFs for MissingRootFs {
            fn create_dir_all(&self, _path: &Path) -> io::Result<()> {
                Ok(())
            }
            fn write_secure_file(&self, _path: &Path, _bytes: &[u8]) -> io::Result<()> {
                Ok(())
            }
            fn read_to_string(&self, _path: &Path) -> io::Result<String> {
                unreachable!("no entries are read when the root is missing")
            }
            fn read_dir_paths(&self, _path: &Path) -> io::Result<Vec<PathBuf>> {
                Err(io::Error::from(io::ErrorKind::NotFound))
            }
            fn remove_dir_all(&self, _path: &Path) -> io::Result<()> {
                Ok(())
            }
        }

        assert!(
            list_with(&MissingRootFs, Path::new("/missing"))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn merge_live_and_disk_keeps_live_entries_authoritative() {
        let live = vec![summary(7, "live", RunStatus::Running)];
        let disk = vec![
            summary(7, "stale", RunStatus::Exited { code: 130 }),
            summary(8, "old", RunStatus::Exited { code: 0 }),
        ];

        let mut merged = merge_live_and_disk(live, disk);
        merged.sort_by_key(|run| run.id);

        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0].name, "live");
        assert_eq!(merged[0].status, RunStatus::Running);
        assert_eq!(merged[1].id, 8);
    }

    #[test]
    fn disk_only_ids_excludes_live_runs() {
        let live = vec![summary(7, "live", RunStatus::Running)];
        let disk = vec![
            summary(7, "stale", RunStatus::Exited { code: 130 }),
            summary(8, "old", RunStatus::Exited { code: 0 }),
        ];

        assert_eq!(disk_only_ids(&live, &disk), vec![8]);
    }

    #[test]
    fn remove_run_dirs_deletes_metadata_and_scratch_dirs() {
        let fs = FakeFs::default();
        let roots = RunMetadataRoots {
            metadata_runs: "/data/runs".into(),
            scratch_runs: "/cache/runs".into(),
        };
        fs.put_summary(
            &roots.metadata_runs,
            &summary(9, "reviewer", RunStatus::Exited { code: 0 }),
        );

        remove_run_dirs_with(&fs, &roots, 9).unwrap();

        assert_eq!(
            *fs.removed.borrow(),
            vec![
                PathBuf::from("/data/runs/9"),
                PathBuf::from("/cache/runs/9")
            ]
        );
    }

    #[test]
    fn remove_run_dirs_surfaces_non_not_found_failures() {
        let fs = FakeFs::default();
        let roots = RunMetadataRoots {
            metadata_runs: "/data/runs".into(),
            scratch_runs: "/cache/runs".into(),
        };
        fs.put_summary(
            &roots.metadata_runs,
            &summary(9, "reviewer", RunStatus::Exited { code: 0 }),
        );
        *fs.remove_fail.borrow_mut() = Some(PathBuf::from("/cache/runs/9"));

        let err = remove_run_dirs_with(&fs, &roots, 9).unwrap_err();
        assert!(format!("{err:#}").contains("/cache/runs/9"));
    }

    #[test]
    fn remove_run_dirs_skips_paths_without_a_metadata_record() {
        let fs = FakeFs::default();
        let roots = RunMetadataRoots {
            metadata_runs: "/data/runs".into(),
            scratch_runs: "/cache/runs".into(),
        };

        remove_run_dirs_with(&fs, &roots, 9).unwrap();

        assert!(fs.removed.borrow().is_empty());
    }

    #[test]
    fn resolve_disk_handle_accepts_id_or_unique_name() {
        let summaries = vec![
            summary(7, "reviewer", RunStatus::Exited { code: 0 }),
            summary(8, "auditor", RunStatus::Exited { code: 1 }),
        ];

        assert_eq!(resolve_disk_handle_from(&summaries, "7").unwrap(), Some(7));
        assert_eq!(
            resolve_disk_handle_from(&summaries, "auditor").unwrap(),
            Some(8)
        );
        assert_eq!(resolve_disk_handle_from(&summaries, "ghost").unwrap(), None);
        assert_eq!(resolve_disk_handle_from(&summaries, "99").unwrap(), None);
    }

    #[test]
    fn resolve_disk_handle_rejects_ambiguous_names() {
        let summaries = vec![
            summary(7, "reviewer", RunStatus::Exited { code: 0 }),
            summary(8, "reviewer", RunStatus::Exited { code: 1 }),
        ];

        let err = resolve_disk_handle_from(&summaries, "reviewer").unwrap_err();
        assert!(format!("{err:#}").contains("ambiguous"));
    }

    #[test]
    fn real_fs_writes_and_removes_metadata_in_a_temp_root() {
        let dir = tempfile::tempdir().unwrap();
        let metadata_runs = dir.path().join("data/runs");
        let scratch_runs = dir.path().join("cache/runs");
        let run = summary(7, "reviewer", RunStatus::Exited { code: 0 });

        persist_with(&RealRunMetadataFs, &metadata_runs, &run).unwrap();
        std::fs::create_dir_all(scratch_runs.join("7")).unwrap();

        let listed = list_with(&RealRunMetadataFs, &metadata_runs).unwrap();
        assert_eq!(listed, vec![run]);

        remove_run_dirs_with(
            &RealRunMetadataFs,
            &RunMetadataRoots {
                metadata_runs,
                scratch_runs,
            },
            7,
        )
        .unwrap();
        assert!(!dir.path().join("data/runs/7").exists());
        assert!(!dir.path().join("cache/runs/7").exists());
    }
}
