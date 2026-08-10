use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use lns_ipc::Anchor;
use serde_json::{Map, Value};

use crate::oauth::Clock;

pub fn audit_path(run_id: &str) -> Result<PathBuf> {
    Ok(lns_ipc::audit_log_for_run(run_id)?)
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

pub struct LazyAuditLog {
    path: PathBuf,
    file: Option<tokio::fs::File>,
}

impl LazyAuditLog {
    pub fn new(path: PathBuf) -> Self {
        Self { path, file: None }
    }

    pub async fn flush(&mut self) -> Result<()> {
        if let Some(file) = self.file.as_mut() {
            use tokio::io::AsyncWriteExt;
            file.flush().await.context("flushing audit log")?;
        }
        Ok(())
    }
}

impl AuditLog for LazyAuditLog {
    async fn append_synced(&mut self, bytes: &[u8]) -> Result<()> {
        if self.file.is_none() {
            self.file = Some(open_audit_log_async(&self.path).await?);
        }
        self.file
            .as_mut()
            .expect("file was opened on the preceding line")
            .append_synced(bytes)
            .await
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

fn append_ocsf_at(path: &Path, obj: Map<String, Value>) -> Result<()> {
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
    crate::dashboard::live::note_write();
    Ok(())
}

fn run_ctx(run: &str, microvm: &str, clock: &dyn Clock) -> crate::ocsf_audit::OcsfCtx {
    crate::ocsf_audit::OcsfCtx::at_unix(run.to_string(), microvm.to_string(), clock.now_unix())
}

pub fn record_run_launched_at(
    path: &Path,
    cx: &crate::ocsf_audit::OcsfCtx,
    image: &str,
) -> Result<()> {
    append_ocsf_at(path, crate::ocsf_audit::workload_launch_event(cx, image))
}

pub fn record_run_launched(
    run_id: &str,
    microvm: &str,
    image: &str,
    clock: &dyn Clock,
) -> Result<()> {
    record_run_launched_at(
        &audit_path(run_id)?,
        &run_ctx(run_id, microvm, clock),
        image,
    )
}

pub fn record_sandbox_run_at(
    path: &Path,
    cx: &crate::ocsf_audit::OcsfCtx,
    reference: &str,
    digest: &str,
    connectors: &[String],
    policy_hash: &str,
) -> Result<()> {
    append_ocsf_at(
        path,
        crate::ocsf_audit::sandbox_run_event(cx, reference, digest, connectors, policy_hash),
    )
}

pub fn record_sandbox_run(
    run_id: &str,
    microvm: &str,
    reference: &str,
    digest: &str,
    connectors: &[String],
    policy_hash: &str,
    clock: &dyn Clock,
) -> Result<()> {
    record_sandbox_run_at(
        &audit_path(run_id)?,
        &run_ctx(run_id, microvm, clock),
        reference,
        digest,
        connectors,
        policy_hash,
    )
}

pub fn record_tool_provisioned_at(
    path: &Path,
    cx: &crate::ocsf_audit::OcsfCtx,
    outcome: &crate::tools::ProvisionOutcome,
) -> Result<()> {
    append_ocsf_at(
        path,
        crate::ocsf_audit::tool_event(
            cx,
            &outcome.tool,
            &outcome.requested,
            &outcome.resolved,
            outcome.source_host.as_deref(),
            &outcome.backend,
        ),
    )
}

pub fn record_tool_provisioned(
    run_id: &str,
    microvm: &str,
    outcome: &crate::tools::ProvisionOutcome,
    clock: &dyn Clock,
) -> Result<()> {
    record_tool_provisioned_at(
        &audit_path(run_id)?,
        &run_ctx(run_id, microvm, clock),
        outcome,
    )
}

pub fn record_volume_attached_at(
    path: &Path,
    cx: &crate::ocsf_audit::OcsfCtx,
    name: &str,
    target: &str,
) -> Result<()> {
    append_ocsf_at(path, crate::ocsf_audit::volume_event(cx, name, target))
}

pub fn record_volume_attached(
    run_id: &str,
    microvm: &str,
    name: &str,
    target: &str,
    clock: &dyn Clock,
) -> Result<()> {
    record_volume_attached_at(
        &audit_path(run_id)?,
        &run_ctx(run_id, microvm, clock),
        name,
        target,
    )
}

pub fn record_bind_attached_at(
    path: &Path,
    cx: &crate::ocsf_audit::OcsfCtx,
    source: &str,
    target: &str,
    exposed_secrets: &[String],
    dropped_secrets: &[String],
) -> Result<()> {
    append_ocsf_at(
        path,
        crate::ocsf_audit::bind_event(cx, source, target, exposed_secrets, dropped_secrets),
    )
}

pub fn record_bind_attached(
    run_id: &str,
    microvm: &str,
    source: &str,
    target: &str,
    exposed_secrets: &[String],
    dropped_secrets: &[String],
    clock: &dyn Clock,
) -> Result<()> {
    record_bind_attached_at(
        &audit_path(run_id)?,
        &run_ctx(run_id, microvm, clock),
        source,
        target,
        exposed_secrets,
        dropped_secrets,
    )
}

pub fn record_host_access_attached_at(
    path: &Path,
    cx: &crate::ocsf_audit::OcsfCtx,
    id: &str,
    host_source: &str,
    guest_target: &str,
) -> Result<()> {
    append_ocsf_at(
        path,
        crate::ocsf_audit::host_access_event(cx, id, host_source, guest_target),
    )
}

pub fn record_host_access_attached(
    run_id: &str,
    microvm: &str,
    id: &str,
    host_source: &str,
    guest_target: &str,
    clock: &dyn Clock,
) -> Result<()> {
    record_host_access_attached_at(
        &audit_path(run_id)?,
        &run_ctx(run_id, microvm, clock),
        id,
        host_source,
        guest_target,
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_forwarded_host_socket_is_recorded_with_its_guest_path() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("audit.jsonl");
        record_host_access_attached_at(
            &path,
            &cx(),
            "git-signing",
            "/run/user/501/gnupg/S.gpg-agent.extra",
            "~/.gnupg/S.gpg-agent",
        )
        .unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(
            body.contains("/run/user/501/gnupg/S.gpg-agent.extra"),
            "the chain must name the host socket that was attached: {body}"
        );
        assert!(
            body.contains("~/.gnupg/S.gpg-agent"),
            "and where the guest reaches it: {body}"
        );
        assert!(
            body.contains("git-signing"),
            "the grant's id is its policy-facing identity: {body}"
        );
    }

    use super::*;

    struct FixedClock(u64);
    impl Clock for FixedClock {
        fn now_unix(&self) -> u64 {
            self.0
        }
    }
    static CLOCK: FixedClock = FixedClock(1_700_000_000);

    fn cx() -> crate::ocsf_audit::OcsfCtx {
        crate::ocsf_audit::OcsfCtx::at_unix("aa01".into(), "calm-finch".into(), 1_700_000_000)
    }

    fn volume_obj(name: &str, target: &str) -> Map<String, Value> {
        crate::ocsf_audit::volume_event(&cx(), name, target)
    }

    #[test]
    fn a_recorded_volume_event_is_marked_host_authored() {
        assert_eq!(
            volume_obj("prism-data", "/data")["unmapped"]["lns_origin"],
            "host",
            "host-authored events must be distinguishable from guest-proxied ones"
        );
    }

    #[test]
    fn record_run_launched_writes_a_host_authored_launch_line_with_the_image() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("audit.jsonl");
        record_run_launched_at(&path, &cx(), "alpine:latest").unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(
            content.contains("\"class_uid\":1007"),
            "OCSF launch: {content}"
        );
        assert!(content.contains("\"lns_kind\":\"launch\""), "{content}");
        assert!(
            content.contains("\"lns_image\":\"alpine:latest\""),
            "{content}"
        );
        assert!(content.contains("\"lns_origin\":\"host\""), "{content}");
        assert!(
            content.contains(&format!("\"prev_hash\":\"{}\"", lns_ipc::GENESIS_PREV_HASH)),
            "the launch line is the run's genesis: {content}"
        );
    }

    #[test]
    fn record_sandbox_run_writes_the_reference_digest_connectors_and_policy_hash() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("audit.jsonl");
        record_sandbox_run_at(
            &path,
            &cx(),
            "some-registry.example/some-agent:research",
            "sha256:beef",
            &["some-connector".to_string()],
            "sha256:po1icy",
        )
        .unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(
            content.contains("\"lns_kind\":\"sandbox_run\""),
            "{content}"
        );
        assert!(
            content.contains("\"lns_sandbox\":\"some-registry.example/some-agent:research\""),
            "{content}"
        );
        assert!(
            content.contains("\"lns_sandbox_digest\":\"sha256:beef\""),
            "the audit must pin the resolved digest, not just the tag: {content}"
        );
        assert!(
            content.contains("\"lns_policy_hash\":\"sha256:po1icy\""),
            "{content}"
        );
        assert!(
            content.contains("\"lns_connectors\":[\"some-connector\"]"),
            "{content}"
        );
        assert!(content.contains("\"lns_origin\":\"host\""), "{content}");
    }

    #[test]
    #[serial_test::serial(env)]
    fn record_sandbox_run_writes_under_the_runs_audit_log() {
        let d = tempfile::tempdir().unwrap();
        let _h = crate::test_env::EnvVarGuard::set("HOME", d.path());
        let _x = crate::test_env::EnvVarGuard::set("XDG_CACHE_HOME", d.path().join("cache"));
        record_sandbox_run(
            "aa125",
            "calm-finch",
            "reg/some-agent:1",
            "sha256:beef",
            &[],
            "sha256:po1icy",
            &CLOCK,
        )
        .unwrap();
        let content = std::fs::read_to_string(audit_path("aa125").unwrap()).unwrap();
        assert!(
            content.contains("\"lns_sandbox\":\"reg/some-agent:1\""),
            "{content}"
        );
        assert!(
            content.contains("\"lns_sandbox_digest\":\"sha256:beef\""),
            "{content}"
        );
        assert!(
            content.contains("\"lns_policy_hash\":\"sha256:po1icy\""),
            "{content}"
        );
    }

    #[test]
    fn record_volume_attached_writes_an_ocsf_genesis_line() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("audit.jsonl");
        record_volume_attached_at(&path, &cx(), "prism-data", "/data").unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("\"class_uid\":1001"), "OCSF: {content}");
        assert!(content.contains("\"lns_name\":\"prism-data\""), "{content}");
        assert!(content.contains("\"lns_target\":\"/data\""), "{content}");
        assert!(
            content.contains(&format!(
                "\"lns_ts\":\"{}\"",
                crate::time_fmt::rfc3339_from_unix(1_700_000_000)
            )),
            "the host stamps the event time: {content}"
        );
        assert!(
            content.contains(&format!("\"prev_hash\":\"{}\"", lns_ipc::GENESIS_PREV_HASH)),
            "first line must be genesis: {content}"
        );
    }

    #[test]
    fn record_bind_attached_writes_host_source_and_target() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("audit.jsonl");
        record_bind_attached_at(&path, &cx(), "/Users/me/proj", "/work", &[], &[]).unwrap();

        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("\"class_uid\":1001"), "OCSF: {content}");
        assert!(content.contains("\"lns_origin\":\"host\""), "{content}");
        assert!(
            content.contains("\"lns_source\":\"/Users/me/proj\""),
            "{content}"
        );
        assert!(content.contains("\"lns_target\":\"/work\""), "{content}");
    }

    #[test]
    fn record_bind_attached_records_which_secrets_were_exposed_versus_masked() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("audit.jsonl");
        record_bind_attached_at(
            &path,
            &cx(),
            "/Users/me/proj",
            "/work",
            &[".env".to_string()],
            &[".npmrc".to_string(), ".ssh".to_string()],
        )
        .unwrap();

        let line: serde_json::Value =
            serde_json::from_str(std::fs::read_to_string(&path).unwrap().trim()).unwrap();
        assert_eq!(
            line["unmapped"]["lns_exposed_secrets"],
            serde_json::json!([".env"]),
            "the tamper-evident log must name the real secret the run exposed"
        );
        assert_eq!(
            line["unmapped"]["lns_dropped_secrets"],
            serde_json::json!([".npmrc", ".ssh"]),
            "and the secrets it masked"
        );
    }

    #[tokio::test]
    async fn lazy_audit_log_creates_no_file_or_dir_until_the_first_event() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("runs").join("zzz").join("audit.jsonl");
        let mut log = LazyAuditLog::new(path.clone());
        log.flush().await.unwrap();
        assert!(!path.exists(), "a zero-event run must leave no audit log");
        assert!(
            !path.parent().unwrap().exists(),
            "a zero-event run must leave no run dir under data_root"
        );

        log.append_synced(b"first\n").await.unwrap();
        assert!(path.exists(), "the first event opens (creates) the log");
        log.append_synced(b"second\n").await.unwrap();
        log.flush().await.unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "first\nsecond\n",
            "the reused handle appends after the first event"
        );
    }

    #[test]
    #[serial_test::serial(env)]
    fn record_host_access_attached_writes_under_the_runs_audit_log() {
        let d = tempfile::tempdir().unwrap();
        let _h = crate::test_env::EnvVarGuard::set("HOME", d.path());
        let _x = crate::test_env::EnvVarGuard::set("XDG_CACHE_HOME", d.path().join("cache"));
        record_host_access_attached(
            "aa123",
            "calm-finch",
            "git-signing",
            "/run/user/501/gnupg/S.gpg-agent.extra",
            "~/.gnupg/S.gpg-agent",
            &CLOCK,
        )
        .unwrap();
        let content = std::fs::read_to_string(audit_path("aa123").unwrap()).unwrap();
        assert!(
            content.contains("\"lns_source\":\"/run/user/501/gnupg/S.gpg-agent.extra\""),
            "{content}"
        );
        assert!(
            content.contains("\"lns_target\":\"~/.gnupg/S.gpg-agent\""),
            "{content}"
        );
    }

    #[test]
    #[serial_test::serial(env)]
    fn record_bind_attached_writes_under_the_runs_audit_log() {
        let d = tempfile::tempdir().unwrap();
        let _h = crate::test_env::EnvVarGuard::set("HOME", d.path());
        let _x = crate::test_env::EnvVarGuard::set("XDG_CACHE_HOME", d.path().join("cache"));
        record_bind_attached(
            "aa123",
            "calm-finch",
            "/Users/me/proj",
            "/work",
            &[],
            &[],
            &CLOCK,
        )
        .unwrap();
        let content = std::fs::read_to_string(audit_path("aa123").unwrap()).unwrap();
        assert!(
            content.contains("\"lns_source\":\"/Users/me/proj\""),
            "{content}"
        );
        assert!(content.contains("\"lns_target\":\"/work\""), "{content}");
    }

    #[test]
    #[serial_test::serial(env)]
    fn record_tool_provisioned_writes_under_the_runs_audit_log() {
        let d = tempfile::tempdir().unwrap();
        let _h = crate::test_env::EnvVarGuard::set("HOME", d.path());
        let _x = crate::test_env::EnvVarGuard::set("XDG_CACHE_HOME", d.path().join("cache"));
        let outcome = crate::tools::ProvisionOutcome {
            tool: "some-tool".into(),
            requested: "some-tool@1".into(),
            resolved: "1.2.3".into(),
            backend: "core:some-tool".into(),
            source_host: Some("upstream.example.test".into()),
        };
        record_tool_provisioned("aa125", "calm-finch", &outcome, &CLOCK).unwrap();
        let content = std::fs::read_to_string(audit_path("aa125").unwrap()).unwrap();
        assert!(content.contains("\"lns_resolved\":\"1.2.3\""), "{content}");
        assert!(
            content.contains("\"lns_source\":\"upstream.example.test\""),
            "{content}"
        );
    }

    #[test]
    fn successive_events_form_a_valid_hash_chain() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join("audit.jsonl");
        record_volume_attached_at(&path, &cx(), "a", "/a").unwrap();
        record_volume_attached_at(&path, &cx(), "b", "/b").unwrap();

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
    #[serial_test::serial(env)]
    fn audit_path_lands_under_the_run_directory() {
        let p = audit_path("aa99").unwrap();
        assert!(p.ends_with("runs/aa99/audit.jsonl"), "got {}", p.display());
    }

    #[test]
    #[serial_test::serial(env)]
    fn record_volume_attached_writes_under_the_runs_audit_log() {
        let d = tempfile::tempdir().unwrap();
        let _h = crate::test_env::EnvVarGuard::set("HOME", d.path());
        let _x = crate::test_env::EnvVarGuard::set("XDG_CACHE_HOME", d.path().join("cache"));
        record_volume_attached("aa123", "calm-finch", "prism-data", "/data", &CLOCK).unwrap();
        let content = std::fs::read_to_string(audit_path("aa123").unwrap()).unwrap();
        assert!(content.contains("\"lns_name\":\"prism-data\""), "{content}");
        assert!(content.contains("\"lns_target\":\"/data\""), "{content}");
    }

    #[test]
    #[serial_test::serial(env)]
    fn record_run_launched_writes_under_the_runs_audit_log() {
        let d = tempfile::tempdir().unwrap();
        let _h = crate::test_env::EnvVarGuard::set("HOME", d.path());
        let _x = crate::test_env::EnvVarGuard::set("XDG_CACHE_HOME", d.path().join("cache"));
        record_run_launched("aa124", "calm-finch", "alpine:latest", &CLOCK).unwrap();
        let content = std::fs::read_to_string(audit_path("aa124").unwrap()).unwrap();
        assert!(
            content.contains("\"lns_image\":\"alpine:latest\""),
            "{content}"
        );
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

        append_event_with_sink(&path, None, volume_obj("a", "/a"), &mut sink).unwrap();
        append_event_with_sink(
            &path,
            sink.anchors.last().cloned(),
            volume_obj("b", "/b"),
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
        record_volume_attached_at(&path, &cx(), "a", "/a").unwrap();
        record_volume_attached_at(&path, &cx(), "b", "/b").unwrap();

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
        record_volume_attached_at(&path, &cx(), "a", "/a").unwrap();
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
        record_volume_attached_at(&path, &cx(), "a", "/a").unwrap();

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
        record_volume_attached_at(&path, &cx(), "a", "/a").unwrap();
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
        append_event_with_sink(&path, Some(resume), volume_obj("a", "/a"), &mut sink).unwrap();
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
