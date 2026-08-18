use crate::image_store::Fs;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const CURRENT_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunRecord {
    pub version: u32,
    pub run_id: String,
    pub name: String,
    pub args: lns_ipc::RunImageArgs,
    pub descriptor_sha256: String,
    pub layer_digests: Vec<String>,
    pub image: String,
    pub command: String,
    pub created_at: String,
    pub finished_at: Option<String>,
    pub exit_code: Option<i32>,
}

pub fn record_path(cache_root: &Path, run_id: &str) -> PathBuf {
    crate::cache::run_dir(cache_root, run_id).join("record.json")
}

pub async fn save_with<F: Fs>(fs: &F, cache_root: &Path, record: &RunRecord) -> Result<()> {
    let bytes = serde_json::to_vec(record).context("serializing run record")?;
    fs.write(&record_path(cache_root, &record.run_id), &bytes)
        .await
        .with_context(|| format!("writing run record for {}", record.run_id))?;
    Ok(())
}

pub async fn mark_exited_with<F: Fs>(
    fs: &F,
    cache_root: &Path,
    run_id: &str,
    exit_code: i32,
    finished_at: String,
) -> Result<()> {
    let path = record_path(cache_root, run_id);
    let bytes = fs
        .read(&path)
        .await
        .with_context(|| format!("no run record for {run_id}"))?;
    let mut record: RunRecord = serde_json::from_slice(&bytes)
        .with_context(|| format!("parsing the run record for {run_id}"))?;
    record.exit_code = Some(exit_code);
    record.finished_at = Some(finished_at);
    save_with(fs, cache_root, &record).await
}

pub async fn load_all_with<F: Fs>(fs: &F, cache_root: &Path) -> Result<Vec<RunRecord>> {
    let entries = match fs.read_dir(&cache_root.join("runs")).await {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(anyhow::Error::from(e).context("listing the runs dir")),
    };
    let mut records = Vec::new();
    for dir in entries {
        if let Ok(bytes) = fs.read(&dir.join("record.json")).await
            && let Ok(record) = serde_json::from_slice::<RunRecord>(&bytes)
            && record.version == CURRENT_VERSION
        {
            records.push(record);
        }
    }
    records.sort_by(|a, b| a.run_id.cmp(&b.run_id));
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::io;
    use std::sync::Mutex;

    struct FakeFs {
        listing: io::Result<Vec<PathBuf>>,
        files: Mutex<HashMap<PathBuf, Vec<u8>>>,
    }

    impl FakeFs {
        fn empty() -> Self {
            Self {
                listing: Ok(Vec::new()),
                files: Mutex::new(HashMap::new()),
            }
        }

        fn with_run_dirs(root: &Path, ids: &[&str]) -> Self {
            Self {
                listing: Ok(ids
                    .iter()
                    .map(|id| crate::cache::run_dir(root, id))
                    .collect()),
                files: Mutex::new(HashMap::new()),
            }
        }

        fn stash(&self, path: PathBuf, bytes: Vec<u8>) {
            self.files.lock().unwrap().insert(path, bytes);
        }
    }

    impl Fs for FakeFs {
        async fn read_dir(&self, _dir: &Path) -> io::Result<Vec<PathBuf>> {
            match &self.listing {
                Ok(entries) => Ok(entries.clone()),
                Err(e) => Err(io::Error::new(e.kind(), e.to_string())),
            }
        }

        async fn read(&self, p: &Path) -> io::Result<Vec<u8>> {
            self.files
                .lock()
                .unwrap()
                .get(p)
                .cloned()
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
        }

        async fn write(&self, p: &Path, bytes: &[u8]) -> io::Result<()> {
            self.files
                .lock()
                .unwrap()
                .insert(p.to_path_buf(), bytes.to_vec());
            Ok(())
        }

        async fn remove_file(&self, p: &Path) -> io::Result<()> {
            self.files
                .lock()
                .unwrap()
                .remove(p)
                .map(|_| ())
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
        }
    }

    fn sample_args() -> lns_ipc::RunImageArgs {
        lns_ipc::RunImageArgs {
            image: Some("registry.example.test/some-sandbox:1".into()),
            resolved_image: None,
            mixins: Vec::new(),
            composed_mixins: Vec::new(),
            name: None,
            cpus: 2,
            mem: 512,
            cpus_explicit: false,
            mem_explicit: false,
            policy_path: None,
            sandbox_user: None,
            sandbox_uid: None,
            entrypoint: None,
            hostname: None,
            cmd: vec!["sh".into(), "-c".into(), "true".into()],
            env: vec!["A=1".into()],
            workdir: None,
            debug: false,
            tty: false,
            stdin: false,
            initial_winsize: None,
            detached: true,
            published_ports: Vec::new(),
            volumes: Vec::new(),
            binds: Vec::new(),
            auto_remove: false,
            verify_sandbox: false,
            definition: None,
            definition_dir: None,
            authored_egress: None,
            packed_filesets: Vec::new(),
        }
    }

    fn sample_record(run_id: &str) -> RunRecord {
        RunRecord {
            version: CURRENT_VERSION,
            run_id: run_id.to_string(),
            name: format!("name-{run_id}"),
            args: sample_args(),
            descriptor_sha256: "d".repeat(64),
            layer_digests: vec!["sha256:aaaa".into()],
            image: "registry.example.test/some-sandbox:1".into(),
            command: "sh -c true".into(),
            created_at: "2026-08-18T00:00:00Z".into(),
            finished_at: Some("2026-08-18T00:01:00Z".into()),
            exit_code: Some(0),
        }
    }

    #[tokio::test]
    async fn marking_a_run_exited_stamps_code_and_finish_time_in_place() {
        let root = Path::new("/cache");
        let fs = FakeFs::empty();
        let mut record = sample_record("aa01");
        record.finished_at = None;
        record.exit_code = None;
        save_with(&fs, root, &record).await.unwrap();
        mark_exited_with(&fs, root, "aa01", 7, "2026-08-18T00:02:00Z".into())
            .await
            .unwrap();
        let stored: RunRecord = serde_json::from_slice(
            &fs.files
                .lock()
                .unwrap()
                .get(&record_path(root, "aa01"))
                .cloned()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(stored.exit_code, Some(7));
        assert_eq!(stored.finished_at.as_deref(), Some("2026-08-18T00:02:00Z"));
        assert_eq!(stored.args, record.args);
    }

    #[tokio::test]
    async fn marking_a_recordless_run_exited_reports_the_missing_record() {
        let fs = FakeFs::empty();
        let err = mark_exited_with(&fs, Path::new("/cache"), "nosuch", 0, "t".into())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("nosuch"), "{err}");
    }

    #[test]
    fn a_record_survives_a_serde_round_trip_verbatim() {
        let record = sample_record("aa01");
        let bytes = serde_json::to_vec(&record).unwrap();
        let back: RunRecord = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back, record);
    }

    #[tokio::test]
    async fn save_lands_the_record_beside_the_writable_layer() {
        let root = Path::new("/cache");
        let fs = FakeFs::empty();
        let record = sample_record("aa01");
        save_with(&fs, root, &record).await.unwrap();
        let stored = fs
            .files
            .lock()
            .unwrap()
            .get(&root.join("runs").join("aa01").join("record.json"))
            .cloned()
            .expect("record.json written into the run dir");
        assert_eq!(
            serde_json::from_slice::<RunRecord>(&stored).unwrap(),
            record
        );
    }

    #[tokio::test]
    async fn load_returns_every_recorded_run_sorted_by_id() {
        let root = Path::new("/cache");
        let fs = FakeFs::with_run_dirs(root, &["bb02", "aa01"]);
        for id in ["bb02", "aa01"] {
            fs.stash(
                record_path(root, id),
                serde_json::to_vec(&sample_record(id)).unwrap(),
            );
        }
        let records = load_all_with(&fs, root).await.unwrap();
        assert_eq!(
            records
                .iter()
                .map(|r| r.run_id.as_str())
                .collect::<Vec<_>>(),
            ["aa01", "bb02"]
        );
    }

    #[tokio::test]
    async fn a_missing_runs_root_means_no_stopped_runs() {
        let fs = FakeFs {
            listing: Err(io::Error::from(io::ErrorKind::NotFound)),
            files: Mutex::new(HashMap::new()),
        };
        assert!(
            load_all_with(&fs, Path::new("/cache"))
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn any_other_listing_failure_surfaces_instead_of_hiding_runs() {
        let fs = FakeFs {
            listing: Err(io::Error::from(io::ErrorKind::PermissionDenied)),
            files: Mutex::new(HashMap::new()),
        };
        assert!(load_all_with(&fs, Path::new("/cache")).await.is_err());
    }

    #[tokio::test]
    async fn a_run_dir_without_a_record_is_an_orphan_and_is_skipped() {
        let root = Path::new("/cache");
        let fs = FakeFs::with_run_dirs(root, &["aa01", "orphan"]);
        fs.stash(
            record_path(root, "aa01"),
            serde_json::to_vec(&sample_record("aa01")).unwrap(),
        );
        let records = load_all_with(&fs, root).await.unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].run_id, "aa01");
    }

    #[tokio::test]
    async fn an_unparseable_record_is_skipped_not_fatal() {
        let root = Path::new("/cache");
        let fs = FakeFs::with_run_dirs(root, &["aa01", "bb02"]);
        fs.stash(record_path(root, "aa01"), b"not json".to_vec());
        fs.stash(
            record_path(root, "bb02"),
            serde_json::to_vec(&sample_record("bb02")).unwrap(),
        );
        let records = load_all_with(&fs, root).await.unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].run_id, "bb02");
    }

    #[tokio::test]
    async fn a_record_from_a_future_format_version_is_left_alone() {
        let root = Path::new("/cache");
        let fs = FakeFs::with_run_dirs(root, &["aa01"]);
        let mut future = sample_record("aa01");
        future.version = CURRENT_VERSION + 1;
        fs.stash(
            record_path(root, "aa01"),
            serde_json::to_vec(&future).unwrap(),
        );
        assert!(load_all_with(&fs, root).await.unwrap().is_empty());
    }
}
