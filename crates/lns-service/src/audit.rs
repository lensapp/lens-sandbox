use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use lns_ipc::Anchor;
use serde_json::{Map, Value};

pub fn audit_path(run_id: u32) -> Result<PathBuf> {
    Ok(lns_ipc::audit_log_for_run(&run_id.to_string())?)
}

pub fn anchor_path_for(audit_path: &Path) -> PathBuf {
    audit_path.with_file_name("audit.anchor")
}

pub fn read_anchor(path: &Path) -> Option<Anchor> {
    Anchor::parse(&std::fs::read_to_string(path).ok()?).ok()
}

pub async fn read_anchor_async(path: &Path) -> Option<Anchor> {
    Anchor::parse(&tokio::fs::read_to_string(path).await.ok()?).ok()
}

#[cfg(unix)]
fn open_audit_log(path: &Path) -> Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
        .context("opening audit log")
}

#[cfg(not(unix))]
fn open_audit_log(path: &Path) -> Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .context("opening audit log")
}

#[cfg(unix)]
pub async fn open_audit_log_async(path: &Path) -> Result<tokio::fs::File> {
    crate::paths::ensure_parent_dir(path).context("creating audit log dir")?;
    tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
        .await
        .with_context(|| format!("opening {}", path.display()))
}

#[cfg(not(unix))]
pub async fn open_audit_log_async(path: &Path) -> Result<tokio::fs::File> {
    crate::paths::ensure_parent_dir(path).context("creating audit log dir")?;
    tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .await
        .with_context(|| format!("opening {}", path.display()))
}

pub trait AnchorSink {
    fn write(&mut self, anchor: &Anchor) -> Result<()>;
}

pub trait AuditLog {
    fn append_synced(&mut self, bytes: &[u8]) -> impl std::future::Future<Output = Result<()>>;
}

impl AuditLog for tokio::fs::File {
    async fn append_synced(&mut self, bytes: &[u8]) -> Result<()> {
        use tokio::io::AsyncWriteExt;
        self.write_all(bytes)
            .await
            .context("appending audit event")?;
        self.sync_all().await.context("fsync audit event")?;
        Ok(())
    }
}

pub struct FileAnchorSink {
    path: PathBuf,
}

impl FileAnchorSink {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn for_audit_log(audit_path: &Path) -> Self {
        Self::new(anchor_path_for(audit_path))
    }
}

fn anchor_tmp_path(path: &Path) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    path.with_extension(format!("anchor.tmp.{}.{}", std::process::id(), nanos))
}

#[cfg(unix)]
fn open_anchor_tmp(path: &Path) -> Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .context("opening audit anchor")
}

#[cfg(not(unix))]
fn open_anchor_tmp(path: &Path) -> Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .context("opening audit anchor")
}

#[cfg(unix)]
fn fsync_parent_dir(path: &Path) -> Result<()> {
    let Some(parent) = path.parent() else {
        return Ok(());
    };
    std::fs::File::open(parent)
        .and_then(|d| d.sync_all())
        .context("fsync audit anchor parent dir")
}

#[cfg(not(unix))]
fn fsync_parent_dir(_path: &Path) -> Result<()> {
    Ok(())
}

fn write_anchor_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = anchor_tmp_path(path);
    let write_result = (|| -> Result<()> {
        let mut f = open_anchor_tmp(&tmp)?;
        f.write_all(bytes).context("writing audit anchor")?;
        f.sync_all().context("fsync audit anchor")?;
        Ok(())
    })();
    if let Err(e) = write_result {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&tmp, path).context("renaming audit anchor into place") {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    fsync_parent_dir(path)
}

impl AnchorSink for FileAnchorSink {
    fn write(&mut self, anchor: &Anchor) -> Result<()> {
        write_anchor_file(&self.path, &anchor.to_line())
    }
}

pub fn append_event_at(path: &Path, obj: Map<String, Value>) -> Result<()> {
    let anchor = read_anchor(&anchor_path_for(path));
    let mut sink = FileAnchorSink::for_audit_log(path);
    append_event_with_sink(path, anchor, obj, &mut sink)
}

pub fn append_event_with_sink<S: AnchorSink>(
    path: &Path,
    anchor: Option<Anchor>,
    obj: Map<String, Value>,
    sink: &mut S,
) -> Result<()> {
    crate::paths::ensure_parent_dir(path).context("creating audit log dir")?;
    let mut chain = lns_ipc::AuditChain::resuming_from_anchor(anchor.as_ref());
    let mut line = chain.augment_obj(obj);
    line.push(b'\n');
    let mut f = open_audit_log(path)?;
    f.write_all(&line).context("appending audit event")?;
    f.sync_all().context("fsync audit event")?;
    if let Some(updated) = chain.anchor() {
        sink.write(&updated)?;
    }
    Ok(())
}

fn volume_attached_obj(name: &str, target: &str) -> Map<String, Value> {
    let mut obj = Map::new();
    obj.insert(
        "type".to_string(),
        Value::String("volume_attached".to_string()),
    );
    obj.insert("origin".to_string(), Value::String("host".to_string()));
    obj.insert("name".to_string(), Value::String(name.to_string()));
    obj.insert("target".to_string(), Value::String(target.to_string()));
    obj
}

pub fn record_volume_attached_at(path: &Path, name: &str, target: &str) -> Result<()> {
    append_event_at(path, volume_attached_obj(name, target))
}

pub fn record_volume_attached(run_id: u32, name: &str, target: &str) -> Result<()> {
    record_volume_attached_at(&audit_path(run_id)?, name, target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_attached_obj_stamps_host_origin() {
        let obj = volume_attached_obj("prism-data", "/data");
        assert_eq!(
            obj.get("origin").unwrap(),
            "host",
            "host-authored volume_attached events must be distinguishable from guest-proxied events"
        );
    }

    #[test]
    fn record_volume_attached_writes_name_and_target_as_genesis_line() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("audit.jsonl");
        record_volume_attached_at(&path, "prism-data", "/data").unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(
            content.contains("\"type\":\"volume_attached\""),
            "{content}"
        );
        assert!(content.contains("\"name\":\"prism-data\""), "{content}");
        assert!(content.contains("\"target\":\"/data\""), "{content}");
        assert!(
            content.contains(&format!("\"prev_hash\":\"{}\"", lns_ipc::GENESIS_PREV_HASH)),
            "first line must be genesis: {content}"
        );
    }

    #[test]
    fn successive_events_form_a_valid_hash_chain() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("audit.jsonl");
        record_volume_attached_at(&path, "a", "/a").unwrap();
        record_volume_attached_at(&path, "b", "/b").unwrap();

        let content = std::fs::read(&path).unwrap();
        let lines: Vec<&[u8]> = content
            .split(|&b| b == b'\n')
            .filter(|l| !l.is_empty())
            .collect();
        assert_eq!(lines.len(), 2);
        let first_hash = lns_ipc::line_hash(lines[0]);
        let second = std::str::from_utf8(lines[1]).unwrap();
        assert!(
            second.contains(&format!("\"prev_hash\":\"{first_hash}\"")),
            "second line must chain from the first: {second}"
        );
    }

    #[test]
    #[serial_test::serial(cache_root)]
    fn audit_path_lands_under_the_run_directory() {
        let p = audit_path(99).unwrap();
        assert!(p.ends_with("runs/99/audit.jsonl"), "got {}", p.display());
    }

    #[test]
    #[serial_test::serial(cache_root)]
    fn record_volume_attached_writes_under_the_runs_audit_log() {
        let d = tempfile::tempdir().unwrap();
        let _h = crate::test_env::EnvVarGuard::set("HOME", d.path());
        let _x = crate::test_env::EnvVarGuard::set("XDG_CACHE_HOME", d.path().join("cache"));
        record_volume_attached(123, "prism-data", "/data").unwrap();
        let content = std::fs::read_to_string(audit_path(123).unwrap()).unwrap();
        assert!(content.contains("\"name\":\"prism-data\""), "{content}");
        assert!(content.contains("\"target\":\"/data\""), "{content}");
    }

    #[derive(Default)]
    struct RecordingSink {
        anchors: Vec<Anchor>,
    }

    impl AnchorSink for RecordingSink {
        fn write(&mut self, anchor: &Anchor) -> Result<()> {
            self.anchors.push(anchor.clone());
            Ok(())
        }
    }

    #[test]
    fn append_updates_the_anchor_head_and_line_count_each_call() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("audit.jsonl");
        let mut sink = RecordingSink::default();

        append_event_with_sink(&path, None, volume_attached_obj("a", "/a"), &mut sink).unwrap();
        append_event_with_sink(
            &path,
            sink.anchors.last().cloned(),
            volume_attached_obj("b", "/b"),
            &mut sink,
        )
        .unwrap();

        assert_eq!(sink.anchors.len(), 2);
        assert_eq!(sink.anchors[0].line_count, 1);
        assert_eq!(sink.anchors[1].line_count, 2);

        let content = std::fs::read(&path).unwrap();
        let last = content
            .split(|&b| b == b'\n')
            .rfind(|l| !l.is_empty())
            .unwrap();
        assert_eq!(
            sink.anchors[1].head_hash,
            lns_ipc::line_hash(last),
            "anchor head must equal the hash of the last written line"
        );
    }

    #[test]
    fn anchor_persisted_by_append_event_at_round_trips_via_read_anchor() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("audit.jsonl");
        record_volume_attached_at(&path, "a", "/a").unwrap();
        record_volume_attached_at(&path, "b", "/b").unwrap();

        let anchor = read_anchor(&anchor_path_for(&path)).expect("anchor file present");
        assert_eq!(anchor.line_count, 2);

        let content = std::fs::read(&path).unwrap();
        let last = content
            .split(|&b| b == b'\n')
            .rfind(|l| !l.is_empty())
            .unwrap();
        assert_eq!(anchor.head_hash, lns_ipc::line_hash(last));
    }

    #[tokio::test]
    async fn read_anchor_async_matches_the_blocking_reader() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("audit.jsonl");
        record_volume_attached_at(&path, "a", "/a").unwrap();
        let anchor_path = anchor_path_for(&path);
        assert_eq!(
            read_anchor_async(&anchor_path).await,
            read_anchor(&anchor_path)
        );
        assert!(read_anchor_async(&anchor_path).await.is_some());
    }

    #[tokio::test]
    async fn read_anchor_async_is_none_for_missing_file() {
        let d = tempfile::tempdir().unwrap();
        assert!(
            read_anchor_async(&d.path().join("nope.anchor"))
                .await
                .is_none()
        );
    }

    #[test]
    fn read_anchor_is_none_for_missing_or_garbage_file() {
        let d = tempfile::tempdir().unwrap();
        assert!(read_anchor(&d.path().join("nope.anchor")).is_none());
        let garbage = d.path().join("garbage.anchor");
        std::fs::write(&garbage, "not-json").unwrap();
        assert!(read_anchor(&garbage).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn audit_log_and_anchor_are_created_mode_0600() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("audit.jsonl");
        record_volume_attached_at(&path, "a", "/a").unwrap();

        let log_mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(log_mode, 0o600, "audit log must be private to the owner");
        let anchor_mode = std::fs::metadata(anchor_path_for(&path))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(anchor_mode, 0o600, "anchor must be private to the owner");
    }

    #[cfg(unix)]
    #[test]
    fn run_dir_is_created_mode_0700() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let run_dir = d.path().join("runs").join("7");
        let path = run_dir.join("audit.jsonl");
        record_volume_attached_at(&path, "a", "/a").unwrap();
        let mode = std::fs::metadata(&run_dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "run dir must be private to the owner");
    }

    #[test]
    fn append_with_sink_resumes_line_count_from_a_supplied_anchor() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("audit.jsonl");
        let resume = Anchor {
            head_hash: "abc".to_string(),
            line_count: 9,
        };
        let mut sink = RecordingSink::default();
        append_event_with_sink(
            &path,
            Some(resume),
            volume_attached_obj("a", "/a"),
            &mut sink,
        )
        .unwrap();
        assert_eq!(sink.anchors[0].line_count, 10);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn open_audit_log_async_creates_a_private_log_and_append_synced_persists_bytes() {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("runs").join("9").join("audit.jsonl");

        let mut f = open_audit_log_async(&path).await.unwrap();
        f.append_synced(b"first\n").await.unwrap();
        f.append_synced(b"second\n").await.unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), "first\nsecond\n");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "async-opened audit log must be private to the owner"
        );
    }

    #[test]
    fn write_anchor_file_round_trips_and_overwrites_atomically() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("audit.anchor");
        let first = Anchor {
            head_hash: "a".repeat(64),
            line_count: 1,
        };
        write_anchor_file(&path, &first.to_line()).unwrap();
        assert_eq!(read_anchor(&path), Some(first));

        let second = Anchor {
            head_hash: "b".repeat(64),
            line_count: 2,
        };
        write_anchor_file(&path, &second.to_line()).unwrap();
        assert_eq!(read_anchor(&path), Some(second));

        let leftover: Vec<_> = std::fs::read_dir(d.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .filter(|n| n.to_string_lossy().contains("anchor.tmp"))
            .collect();
        assert!(
            leftover.is_empty(),
            "no temp file may survive: {leftover:?}"
        );
    }

    #[test]
    fn write_anchor_file_surfaces_an_error_when_the_temp_cannot_be_created() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("absent").join("audit.anchor");
        let err = write_anchor_file(&path, b"x").unwrap_err();
        assert!(
            format!("{err:#}").contains("audit anchor"),
            "error must name the anchor open: {err:#}"
        );
    }

    #[test]
    fn write_anchor_file_surfaces_an_error_and_cleans_up_when_rename_fails() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("audit.anchor");
        std::fs::create_dir(&path).unwrap();
        let err = write_anchor_file(&path, b"x").unwrap_err();
        assert!(
            format!("{err:#}").contains("renaming audit anchor"),
            "rename-onto-directory must surface the rename context: {err:#}"
        );
        let leftover: Vec<_> = std::fs::read_dir(d.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .filter(|n| n.to_string_lossy().contains("anchor.tmp"))
            .collect();
        assert!(
            leftover.is_empty(),
            "temp must be removed on rename failure"
        );
    }

    #[cfg(unix)]
    #[test]
    fn fsync_parent_dir_is_a_noop_when_the_path_has_no_parent() {
        assert!(fsync_parent_dir(Path::new("/")).is_ok());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn open_audit_log_async_surfaces_an_error_when_the_target_is_a_directory() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("audit.jsonl");
        std::fs::create_dir(&path).unwrap();
        let err = open_audit_log_async(&path).await.unwrap_err();
        assert!(
            format!("{err:#}").contains("opening"),
            "error must name the failing open: {err:#}"
        );
    }
}
