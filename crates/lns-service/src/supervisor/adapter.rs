use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Weak;

use tokio::sync::mpsc;

use crate::approval_flow::notification::WindowNotifier;
use crate::approval_flow::protocol::HostFrame;
use crate::approval_flow::session::ApprovalSession;
use crate::approval_flow::watcher::PolicyWatcher;
use crate::approval_flow::window::{self, DecisionDelivery, RequestAction};
use crate::log;
use crate::relay;
use lns_policy::{FilePolicyStore, Policy};

use super::real::RealFs;
use super::traits::{Fs, WritableFile};
use super::{APPROVAL_TICK, APPROVAL_TIMEOUT, SupervisorAssets, SupervisorSession};

pub(super) async fn ensure() -> Result<PathBuf> {
    ensure_with(
        |k| std::env::var_os(k),
        super::resolve_embedded_supervisor(),
    )
    .await
}

pub(super) async fn ensure_with(
    env_get: impl Fn(&str) -> Option<std::ffi::OsString>,
    embedded: Option<&[u8]>,
) -> Result<PathBuf> {
    if let Some(override_path) = env_get("LNS_SUPERVISOR_BIN") {
        let p = PathBuf::from(override_path);
        if !p.is_file() {
            bail!(
                "LNS_SUPERVISOR_BIN={} is not a regular file. Set the env var to a \
                 host-readable static-musl supervisor ELF, or unset it to use the \
                 supervisor embedded into lns-service at build time.",
                p.display()
            );
        }
        let path_str = p.display();
        log::debug!("using supervisor from LNS_SUPERVISOR_BIN override: {path_str}");
        return Ok(p);
    }

    let cache = crate::cache::root()?.join("supervisor");

    let Some(bytes) = embedded else {
        bail!(
            "no embedded supervisor (lns-service was built with LNS_SUPERVISOR_BIN=skip) and no \
             LNS_SUPERVISOR_BIN override set. Rebuild without LNS_SUPERVISOR_BIN=skip to embed the \
             supervisor, or set LNS_SUPERVISOR_BIN=/path/to/static-musl/lns-supervisor."
        );
    };
    install_embedded_supervisor(&RealFs, &cache, bytes).await
}

/// Install the embedded supervisor ELF into `cache` at a content-addressed path, idempotently.
async fn install_embedded_supervisor(fs: &impl Fs, cache: &Path, bytes: &[u8]) -> Result<PathBuf> {
    let sha = format!("{:x}", Sha256::digest(bytes));
    let bin_path = cache.join(format!("supervisor-embedded-{}", &sha[..16]));
    if fs.exists(&bin_path).await {
        return Ok(bin_path);
    }
    fs.create_dir_all(cache)
        .await
        .with_context(|| format!("create_dir_all {}", cache.display()))?;
    atomic_write_executable(fs, &bin_path, bytes)
        .await
        .with_context(|| format!("installing embedded supervisor at {}", bin_path.display()))?;
    Ok(bin_path)
}

async fn atomic_write_with_mode(fs: &impl Fs, path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    let tmp = path.with_extension("tmp");
    let _ = fs.remove_file(&tmp).await;
    {
        let mut f = fs
            .create_new(&tmp)
            .await
            .with_context(|| format!("creating {}", tmp.display()))?;
        f.write_all(bytes)
            .await
            .with_context(|| format!("writing {}", tmp.display()))?;
        f.sync_all()
            .await
            .with_context(|| format!("fsync {}", tmp.display()))?;
    }
    fs.set_permissions(&tmp, mode)
        .await
        .with_context(|| format!("chmod {}", tmp.display()))?;
    fs.rename(&tmp, path)
        .await
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))?;
    Ok(())
}

async fn atomic_write_executable(fs: &impl Fs, path: &Path, bytes: &[u8]) -> Result<()> {
    atomic_write_with_mode(fs, path, bytes, 0o755).await
}

async fn decision_delivery_loop(
    session: Weak<ApprovalSession>,
    mut decision_rx: mpsc::UnboundedReceiver<DecisionDelivery>,
) {
    while let Some(delivery) = decision_rx.recv().await {
        let Some(session) = session.upgrade() else {
            break;
        };
        match delivery.action {
            RequestAction::Decide(decision) => {
                session.record_decision(&delivery.id, decision);
            }
            RequestAction::Dismiss => {
                session.dismiss_request(&delivery.id);
            }
        }
    }
}

async fn tick_timeouts_loop(weak: Weak<ApprovalSession>) {
    let mut interval = tokio::time::interval(APPROVAL_TICK);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        if !sweep_once(&weak) {
            break;
        }
    }
}

fn sweep_once(weak: &Weak<ApprovalSession>) -> bool {
    let Some(session) = weak.upgrade() else {
        return false;
    };
    session.tick_timeouts(std::time::Instant::now());
    true
}

const WINDOW_NOT_INSTALLED: &str = "approval window state was not installed at boot; tray::run_tray must run before any policy-bearing run starts";

/// The effective policy the guest enforces and, second, the developer's own file — the only one an approval writes back.
fn running_policies(
    policy_path: &Path,
    sandbox_policy: Option<&Policy>,
) -> Result<(Policy, Policy)> {
    let own = Policy::load_or_default(policy_path)
        .with_context(|| format!("loading policy {}", policy_path.display()))?;
    let effective = match sandbox_policy {
        Some(baseline) => crate::artifact::policy::merge_effective(Some(baseline), &own),
        None => own.clone(),
    };
    Ok((effective, own))
}

/// The decisions file sits in the project directory, which is the directory a grant is keyed by.
fn held_patterns_for_project(policy_path: &Path) -> Vec<String> {
    // An empty parent names no directory, so it would match no grant and ask about every served destination with no way to answer.
    match policy_path
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
    {
        Some(project) => crate::connector::real::held_patterns_for(project),
        None => Vec::new(),
    }
}

pub(super) async fn start(
    run_id: String,
    microvm_name: String,
    policy_path: &Path,
    sandbox_policy: Option<&Policy>,
    guest_tools_root: PathBuf,
    user_env: Vec<String>,
) -> Result<SupervisorSession> {
    let (policy, own_policy) = running_policies(policy_path, sandbox_policy)?;

    let window_state = window::get().context(WINDOW_NOT_INSTALLED)?;
    let (decision_tx, decision_rx) = tokio::sync::mpsc::unbounded_channel::<DecisionDelivery>();
    let notifier = Arc::new(WindowNotifier::new(
        window_state,
        decision_tx,
        window::ctx(),
    ));
    log::info!("Approvals", "window ready");

    let store = Arc::new(FilePolicyStore::new(policy_path.to_path_buf()));
    let (frame_tx, frame_rx) = tokio::sync::mpsc::unbounded_channel::<HostFrame>();
    let session = Arc::new(
        ApprovalSession::new(
            policy,
            own_policy,
            notifier,
            store,
            frame_tx,
            APPROVAL_TIMEOUT,
        )
        .for_run(microvm_name.clone()),
    );

    if let Some(baseline) = sandbox_policy {
        session.set_shipped_policy(baseline.clone());
    }

    session.hold_for_offers(held_patterns_for_project(policy_path));

    tokio::spawn(decision_delivery_loop(
        Arc::downgrade(&session),
        decision_rx,
    ));

    tokio::spawn(tick_timeouts_loop(Arc::downgrade(&session)));

    session.set_ledger_recorder(Arc::new(crate::ledger::RunLedgerRecorder::new(
        run_id.clone(),
        microvm_name.clone(),
        Arc::new(crate::clock::RealClock),
    )));

    let watcher = PolicyWatcher::spawn(policy_path.to_path_buf(), session.clone())
        .with_context(|| format!("watching policy {}", policy_path.display()))?;
    session.apply_external_policy(
        Policy::load_or_default(policy_path)
            .with_context(|| format!("reloading policy {}", policy_path.display()))?,
    );

    let supervisor_bin = ensure().await?;
    let relay = relay::spawn(&run_id, &microvm_name, session, frame_rx, user_env)?;
    log::debug!(url = %relay.url, "relay listening");
    log::info!("Auditing", "to {}", relay.audit_path.display());
    Ok(SupervisorSession {
        assets: SupervisorAssets {
            supervisor_bin,
            guest_tools_root,
        },
        relay,
        watcher: Some(watcher),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    fn fixture_session() -> (Arc<ApprovalSession>, mpsc::UnboundedReceiver<HostFrame>) {
        use crate::approval_flow::session::tests::{CapturingStore, RecordingNotifier};
        use lns_policy::Policy;
        let notifier = Arc::new(RecordingNotifier::default());
        let store = Arc::new(CapturingStore::default());
        let (frame_tx, frame_rx) = mpsc::unbounded_channel::<HostFrame>();
        let session = Arc::new(ApprovalSession::new(
            Policy::default(),
            Policy::default(),
            notifier,
            store,
            frame_tx,
            std::time::Duration::from_secs(30),
        ));
        (session, frame_rx)
    }

    #[tokio::test]
    async fn decision_delivery_loop_applies_each_delivery_and_exits_on_tx_drop() {
        use crate::approval_flow::protocol::{Decision, HostFrame, RequestPending, Treatment};
        let (session, mut frame_rx) = fixture_session();
        let (tx, rx) = mpsc::unbounded_channel::<DecisionDelivery>();
        session.submit_pending(
            RequestPending {
                id: "r1".into(),
                host: "api.linear.app".into(),
                action: "CONNECT api.linear.app:443".into(),
                reason: "policy-ambiguous".into(),
                treatment: Treatment::Inspected,
            },
            std::time::Instant::now(),
        );
        tx.send(DecisionDelivery {
            id: "r1".into(),
            action: RequestAction::Decide(Decision::AllowOnce),
        })
        .unwrap();
        drop(tx);
        decision_delivery_loop(Arc::downgrade(&session), rx).await;
        let frame = frame_rx.try_recv().expect("decision frame");
        match frame {
            HostFrame::RequestDecision(d) => {
                assert_eq!(d.id, "r1");
                assert_eq!(d.decision, Decision::AllowOnce);
            }
            other => panic!("expected RequestDecision, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn decision_delivery_loop_dismisses_a_closed_card_without_recording_a_decision() {
        use crate::approval_flow::protocol::{Decision, HostFrame, RequestPending};
        let (session, mut frame_rx) = fixture_session();
        let (tx, rx) = mpsc::unbounded_channel::<DecisionDelivery>();
        session.submit_pending(
            RequestPending {
                id: "r1".into(),
                host: "api.linear.app".into(),
                action: "CONNECT api.linear.app:443".into(),
                treatment: Default::default(),
                reason: "policy-ambiguous".into(),
            },
            std::time::Instant::now(),
        );
        tx.send(DecisionDelivery {
            id: "r1".into(),
            action: RequestAction::Dismiss,
        })
        .unwrap();
        drop(tx);

        decision_delivery_loop(Arc::downgrade(&session), rx).await;

        match frame_rx.try_recv().expect("decision frame") {
            HostFrame::RequestDecision(d) => {
                assert_eq!(d.id, "r1");
                assert_eq!(
                    d.decision,
                    Decision::Timeout,
                    "the held request still fails closed, as an undecided card rather than a deny"
                );
            }
            other => panic!("expected RequestDecision, got {other:?}"),
        }
        assert_eq!(session.current_policy(), Policy::default());
    }

    #[tokio::test]
    async fn decision_delivery_loop_breaks_when_upgrade_fails_with_buffered_delivery() {
        use crate::approval_flow::protocol::Decision;
        let (session, _frame_rx) = fixture_session();
        let weak = Arc::downgrade(&session);
        let (tx, rx) = mpsc::unbounded_channel::<DecisionDelivery>();
        let stale_clone = tx.clone();
        stale_clone
            .send(DecisionDelivery {
                id: "r1".into(),
                action: RequestAction::Decide(Decision::AllowOnce),
            })
            .unwrap();
        drop(session);
        drop(tx);
        drop(stale_clone);

        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            decision_delivery_loop(weak, rx),
        )
        .await
        .expect("loop must exit promptly when upgrade fails");
    }

    #[tokio::test]
    async fn decision_delivery_loop_exits_when_session_strong_refs_drop() {
        use crate::approval_flow::notification::WindowNotifier;
        use crate::approval_flow::session::ApprovalSession;
        use crate::approval_flow::window::WindowState;
        use lns_policy::Policy;
        use std::sync::Arc;

        let window_state = WindowState::new();
        let (decision_tx, decision_rx) = mpsc::unbounded_channel::<DecisionDelivery>();
        let notifier = Arc::new(WindowNotifier::new(window_state, decision_tx, None));
        use crate::approval_flow::session::tests::CapturingStore;
        let store = Arc::new(CapturingStore::default());
        let (frame_tx, _frame_rx) = mpsc::unbounded_channel::<HostFrame>();
        let session = Arc::new(ApprovalSession::new(
            Policy::default(),
            Policy::default(),
            notifier,
            store,
            frame_tx,
            std::time::Duration::from_secs(30),
        ));

        let weak = Arc::downgrade(&session);
        let handle = tokio::spawn(decision_delivery_loop(weak, decision_rx));
        tokio::task::yield_now().await;
        drop(session);
        tokio::time::timeout(std::time::Duration::from_secs(2), handle)
            .await
            .expect("decision_delivery_loop must exit once strong refs drop")
            .expect("decision_delivery_loop task panicked");
    }

    #[tokio::test]
    async fn tick_timeouts_loop_exits_when_strong_refs_drop() {
        let (session, _frame_rx) = fixture_session();
        let weak = Arc::downgrade(&session);
        drop(session);
        tokio::time::timeout(std::time::Duration::from_secs(2), tick_timeouts_loop(weak))
            .await
            .expect("ticker exits promptly once session drops");
    }

    #[tokio::test(start_paused = true)]
    async fn tick_timeouts_loop_keeps_sweeping_across_ticks_while_session_alive() {
        let (session, _frame_rx) = fixture_session();
        let handle = tokio::spawn(tick_timeouts_loop(Arc::downgrade(&session)));
        tokio::time::sleep(APPROVAL_TICK * 3).await;
        assert!(
            !handle.is_finished(),
            "a live session must keep the sweeper alive past its first tick, or a card only ever times out on the tick that created it"
        );
        drop(session);
        tokio::time::timeout(APPROVAL_TICK * 2, handle)
            .await
            .expect("ticker exits after session drop")
            .expect("ticker task panicked");
    }

    #[test]
    fn sweep_once_returns_false_when_session_dropped() {
        let (session, _frame_rx) = fixture_session();
        let weak = Arc::downgrade(&session);
        drop(session);
        assert!(!sweep_once(&weak), "no strong refs → loop should exit");
    }

    #[test]
    fn sweep_once_drives_session_tick_timeouts() {
        use crate::approval_flow::protocol::{HostFrame, RequestPending, Treatment};
        use crate::approval_flow::session::tests::{CapturingStore, RecordingNotifier};
        use lns_policy::Policy;
        let notifier = Arc::new(RecordingNotifier::default());
        let store = Arc::new(CapturingStore::default());
        let (frame_tx, mut frame_rx) = mpsc::unbounded_channel::<HostFrame>();
        let session = Arc::new(ApprovalSession::new(
            Policy::default(),
            Policy::default(),
            notifier,
            store,
            frame_tx,
            std::time::Duration::from_millis(0),
        ));
        session.submit_pending(
            RequestPending {
                id: "r1".into(),
                host: "api.linear.app".into(),
                action: "CONNECT api.linear.app:443".into(),
                reason: "policy-ambiguous".into(),
                treatment: Treatment::Inspected,
            },
            std::time::Instant::now() - std::time::Duration::from_secs(1),
        );
        let weak = Arc::downgrade(&session);
        assert!(sweep_once(&weak), "live session → loop continues");
        let frame = frame_rx.try_recv().expect("timeout frame");
        let v = serde_json::to_value(&frame).expect("serialise");
        assert_eq!(v["type"], "request_decision");
        assert_eq!(v["id"], "r1");
        assert_eq!(v["decision"], "timeout");
    }

    fn init_tracing_capture() {
        use std::sync::Once;
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            let subscriber = tracing_subscriber::fmt()
                .with_writer(std::io::sink)
                .with_ansi(false)
                .with_max_level(tracing::Level::TRACE)
                .finish();
            tracing::subscriber::set_global_default(subscriber).ok();
        });
    }

    fn supervisor_elf() -> Vec<u8> {
        b"\x7fELF stub bytes for the supervisor".to_vec()
    }

    fn fake_cache_root() -> PathBuf {
        PathBuf::from("/fake/cache/supervisor")
    }

    fn embedded_bin_path(cache: &Path, bytes: &[u8]) -> PathBuf {
        let sha = format!("{:x}", Sha256::digest(bytes));
        cache.join(format!("supervisor-embedded-{}", &sha[..16]))
    }

    #[derive(Default)]
    struct FakeState {
        files: std::collections::HashMap<PathBuf, Vec<u8>>,
        perms: std::collections::HashMap<PathBuf, u32>,
        fail_remove_file: Option<io::Error>,
        fail_create_new: Option<io::Error>,
        fail_write_all: Option<io::Error>,
        fail_sync_all: Option<io::Error>,
        fail_set_permissions: Option<io::Error>,
        fail_rename: Option<io::Error>,
    }

    #[derive(Clone)]
    struct FakeFs {
        state: std::sync::Arc<std::sync::Mutex<FakeState>>,
    }

    impl FakeFs {
        fn new() -> Self {
            Self {
                state: std::sync::Arc::new(std::sync::Mutex::new(FakeState::default())),
            }
        }

        fn perm_of(&self, p: &Path) -> Option<u32> {
            self.state.lock().unwrap().perms.get(p).copied()
        }

        fn fail_next_remove_file(&self, e: io::Error) {
            self.state.lock().unwrap().fail_remove_file = Some(e);
        }
        fn fail_next_create_new(&self, e: io::Error) {
            self.state.lock().unwrap().fail_create_new = Some(e);
        }
        fn fail_next_write_all(&self, e: io::Error) {
            self.state.lock().unwrap().fail_write_all = Some(e);
        }
        fn fail_next_sync_all(&self, e: io::Error) {
            self.state.lock().unwrap().fail_sync_all = Some(e);
        }
        fn fail_next_set_permissions(&self, e: io::Error) {
            self.state.lock().unwrap().fail_set_permissions = Some(e);
        }
        fn fail_next_rename(&self, e: io::Error) {
            self.state.lock().unwrap().fail_rename = Some(e);
        }
    }

    impl Fs for FakeFs {
        type WritableFile = FakeWritableFile;

        async fn create_dir_all(&self, _p: &Path) -> io::Result<()> {
            Ok(())
        }

        async fn exists(&self, p: &Path) -> bool {
            self.state.lock().unwrap().files.contains_key(p)
        }

        async fn remove_file(&self, p: &Path) -> io::Result<()> {
            let mut s = self.state.lock().unwrap();
            if let Some(e) = s.fail_remove_file.take() {
                return Err(e);
            }
            s.files.remove(p);
            s.perms.remove(p);
            Ok(())
        }

        async fn create_new(&self, p: &Path) -> io::Result<FakeWritableFile> {
            let mut s = self.state.lock().unwrap();
            if let Some(e) = s.fail_create_new.take() {
                return Err(e);
            }
            let path_str = p.display();
            assert!(
                !s.files.contains_key(p),
                "FakeFs::create_new: {path_str} already exists"
            );
            s.files.insert(p.to_path_buf(), Vec::new());
            Ok(FakeWritableFile {
                path: p.to_path_buf(),
                state: std::sync::Arc::clone(&self.state),
            })
        }

        async fn set_permissions(&self, p: &Path, mode: u32) -> io::Result<()> {
            let mut s = self.state.lock().unwrap();
            if let Some(e) = s.fail_set_permissions.take() {
                return Err(e);
            }
            s.perms.insert(p.to_path_buf(), mode);
            Ok(())
        }

        async fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
            let mut s = self.state.lock().unwrap();
            if let Some(e) = s.fail_rename.take() {
                return Err(e);
            }
            let bytes = s
                .files
                .remove(from)
                .unwrap_or_else(|| panic!("FakeFs::rename: no source at {}", from.display()));
            s.files.insert(to.to_path_buf(), bytes);
            if let Some(mode) = s.perms.remove(from) {
                s.perms.insert(to.to_path_buf(), mode);
            }
            Ok(())
        }
    }

    struct FakeWritableFile {
        path: PathBuf,
        state: std::sync::Arc<std::sync::Mutex<FakeState>>,
    }

    impl WritableFile for FakeWritableFile {
        async fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
            let mut s = self.state.lock().unwrap();
            if let Some(e) = s.fail_write_all.take() {
                return Err(e);
            }
            s.files
                .get_mut(&self.path)
                .expect("fake fs: write_all on a path that was never create_new'd")
                .extend_from_slice(bytes);
            Ok(())
        }

        async fn sync_all(&mut self) -> io::Result<()> {
            if let Some(e) = self.state.lock().unwrap().fail_sync_all.take() {
                return Err(e);
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn atomic_install_pre_existing_tmp_remove_failure_is_silent() {
        init_tracing_capture();
        let fs = FakeFs::new();
        let cache = fake_cache_root();
        fs.fail_next_remove_file(io::Error::new(io::ErrorKind::PermissionDenied, "stale"));

        let bytes = supervisor_elf();
        let resolved = install_embedded_supervisor(&fs, &cache, &bytes)
            .await
            .expect("remove_file failure must be discarded silently");

        assert_eq!(resolved, embedded_bin_path(&cache, &bytes));
    }

    #[tokio::test]
    async fn atomic_install_create_new_failure_surfaces_with_path() {
        init_tracing_capture();
        let fs = FakeFs::new();
        let cache = fake_cache_root();
        fs.fail_next_create_new(io::Error::new(io::ErrorKind::PermissionDenied, "denied"));

        let bytes = supervisor_elf();
        let err = install_embedded_supervisor(&fs, &cache, &bytes)
            .await
            .expect_err("create_new failure must propagate");

        let msg = format!("{err:#}");
        let tmp = embedded_bin_path(&cache, &bytes).with_extension("tmp");
        assert!(msg.contains("creating"), "inner context: {msg}");
        assert!(msg.contains(tmp.to_str().unwrap()), "tmp path: {msg}");
    }

    #[tokio::test]
    async fn atomic_install_write_all_failure_surfaces_with_tmp_path() {
        init_tracing_capture();
        let fs = FakeFs::new();
        let cache = fake_cache_root();
        fs.fail_next_write_all(io::Error::other("disk full"));

        let bytes = supervisor_elf();
        let err = install_embedded_supervisor(&fs, &cache, &bytes)
            .await
            .expect_err("write_all failure must propagate");

        let msg = format!("{err:#}");
        let tmp = embedded_bin_path(&cache, &bytes).with_extension("tmp");
        assert!(msg.contains("writing"), "inner context: {msg}");
        assert!(msg.contains(tmp.to_str().unwrap()), "tmp path: {msg}");
    }

    #[tokio::test]
    async fn atomic_install_sync_all_failure_surfaces_with_tmp_path() {
        init_tracing_capture();
        let fs = FakeFs::new();
        let cache = fake_cache_root();
        fs.fail_next_sync_all(io::Error::other("fsync failed"));

        let bytes = supervisor_elf();
        let err = install_embedded_supervisor(&fs, &cache, &bytes)
            .await
            .expect_err("sync_all failure must propagate");

        let msg = format!("{err:#}");
        let tmp = embedded_bin_path(&cache, &bytes).with_extension("tmp");
        assert!(msg.contains("fsync"), "fsync context: {msg}");
        assert!(msg.contains(tmp.to_str().unwrap()), "tmp path: {msg}");
    }

    #[tokio::test]
    async fn atomic_install_set_permissions_failure_surfaces_with_tmp_path() {
        init_tracing_capture();
        let fs = FakeFs::new();
        let cache = fake_cache_root();
        fs.fail_next_set_permissions(io::Error::other("chmod denied"));

        let bytes = supervisor_elf();
        let err = install_embedded_supervisor(&fs, &cache, &bytes)
            .await
            .expect_err("set_permissions failure must propagate");

        let msg = format!("{err:#}");
        let tmp = embedded_bin_path(&cache, &bytes).with_extension("tmp");
        assert!(msg.contains("chmod"), "chmod context: {msg}");
        assert!(msg.contains(tmp.to_str().unwrap()), "tmp path: {msg}");
    }

    #[tokio::test]
    async fn atomic_install_rename_failure_surfaces_with_both_paths() {
        init_tracing_capture();
        let fs = FakeFs::new();
        let cache = fake_cache_root();
        fs.fail_next_rename(io::Error::other("rename failed"));

        let bytes = supervisor_elf();
        let err = install_embedded_supervisor(&fs, &cache, &bytes)
            .await
            .expect_err("rename failure must propagate");

        let msg = format!("{err:#}");
        let bin_path = embedded_bin_path(&cache, &bytes);
        let tmp = bin_path.with_extension("tmp");
        assert!(msg.contains("rename"), "context: {msg}");
        assert!(msg.contains(tmp.to_str().unwrap()), "tmp in msg: {msg}");
        assert!(
            msg.contains(bin_path.to_str().unwrap()),
            "destination in msg: {msg}"
        );
    }

    #[tokio::test]
    async fn atomic_install_sets_executable_mode_on_installed_elf() {
        init_tracing_capture();
        let fs = FakeFs::new();
        let cache = fake_cache_root();
        let bytes = supervisor_elf();
        let resolved = install_embedded_supervisor(&fs, &cache, &bytes)
            .await
            .expect("happy path through FakeFs");

        assert_eq!(resolved, embedded_bin_path(&cache, &bytes));
        assert_eq!(fs.perm_of(&resolved), Some(0o755));
    }

    #[tokio::test]
    async fn install_create_dir_all_failure_is_surfaced_with_context() {
        init_tracing_capture();
        let d = tempfile::TempDir::new().unwrap();
        let cache = d.path().join("supervisor-as-file");
        std::fs::write(&cache, b"i am a file, not a dir").unwrap();

        let err = install_embedded_supervisor(&RealFs, &cache, &supervisor_elf())
            .await
            .expect_err("create_dir_all on a regular file must fail");

        let msg = format!("{err:#}");
        assert!(msg.contains("create_dir_all"), "context: {msg}");
        assert!(
            msg.contains(cache.to_str().unwrap()),
            "cache path in message: {msg}"
        );
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn ensure_without_embed_or_override_bails() {
        let cache_root = tempfile::TempDir::new().unwrap();
        let _home = crate::test_env::EnvVarGuard::set("LNS_HOME", cache_root.path());

        let err = ensure_with(|_| None, None)
            .await
            .expect_err("no embed + no override must bail regardless of build-time embed");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("no embedded supervisor"),
            "actionable error: {msg}"
        );
        assert!(
            msg.contains("LNS_SUPERVISOR_BIN"),
            "names the override knob: {msg}"
        );
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn embedded_supervisor_installs_content_addressed_and_is_idempotent() {
        use std::os::unix::fs::PermissionsExt;

        let cache_root = tempfile::TempDir::new().unwrap();
        // SAFETY: env mutation is serialized via #[serial(env)].
        unsafe {
            std::env::set_var("LNS_HOME", cache_root.path());
        }
        let bytes = b"\x7fELF fake embedded supervisor".as_slice();
        // env_get returns None so the override is absent and the embedded
        // branch is taken regardless of the host's LNS_SUPERVISOR_BIN.
        let first = ensure_with(|_| None, Some(bytes)).await;
        let second = ensure_with(|_| None, Some(bytes)).await;
        // SAFETY: env mutation is serialized via #[serial(env)].
        unsafe {
            std::env::remove_var("LNS_HOME");
        }

        let first = first.expect("embedded install succeeds");
        let second = second.expect("second resolve is a cache hit");
        assert_eq!(first, second, "content-addressed path is stable");
        let name = first
            .file_name()
            .and_then(|n| n.to_str())
            .expect("install path has a utf-8 file name");
        assert!(name.starts_with("supervisor-embedded-"), "name: {name}");
        let mode = std::fs::metadata(&first).unwrap().permissions().mode();
        assert_eq!(mode & 0o111, 0o111, "installed binary must be executable");
    }

    #[test]
    #[serial_test::serial(env)]
    fn a_run_holds_what_the_connectors_beside_its_decisions_file_serve() {
        // The project is the decisions file's own directory, which is what a grant is keyed by; reading any other directory would offer a connector the project already answered for.
        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = crate::test_env::EnvVarGuard::set("LNS_HOME", home.path());
        install_some_provider(home.path());

        let decided = tempfile::tempdir().expect("tempdir");
        let undecided = tempfile::tempdir().expect("tempdir");
        decline_in(home.path(), decided.path());

        assert!(
            held_patterns_for_project(&decided.path().join("lns-local-mixin.yaml")).is_empty(),
            "the decisions file's own directory is what the decline was keyed by"
        );
        assert_eq!(
            held_patterns_for_project(&undecided.path().join("lns-local-mixin.yaml")),
            ["api.some-provider.example"],
            "the control: the empty answer above must come from the decline, not from state this test failed to write"
        );
    }

    fn install_some_provider(home: &Path) {
        let connector = home.join("connectors").join("some-provider");
        std::fs::create_dir_all(&connector).unwrap();
        std::fs::write(
            connector.join("document.json"),
            r#"{"apiVersion":"lns.run/v1","kind":"connector","name":"some-provider","spec":{"serves":["api.some-provider.example"],"methods":[{"name":"token","auth":{"kind":"token"}}]}}"#,
        )
        .unwrap();
        std::fs::write(connector.join("digest"), "sha256:abc").unwrap();
    }

    /// Keyed by the project directory itself, the way `lns connector grant` writes one.
    fn decline_in(home: &Path, project: &Path) {
        use lns_policy::decision_store::JsonDecisionStore;
        let installed = crate::connector::dir::ConnectorDir::new(home.join("connectors"));
        let values: JsonDecisionStore<crate::connector::store::Profile> =
            JsonDecisionStore::new(home.join("connector-values.json"));
        let grants: JsonDecisionStore<crate::connector::store::ProjectDecision> =
            JsonDecisionStore::new(home.join("connector-grants.json"));
        crate::connector::store::ConnectorStore::new(&installed, &values, &grants)
            .decide(
                lns_artifact::sandbox::fold_path(project)
                    .to_str()
                    .expect("a utf-8 tempdir"),
                "some-provider",
                crate::connector::store::ProjectDecision::Declined,
            )
            .expect("record a decline");
    }

    #[test]
    #[serial_test::serial(env)]
    fn a_relative_decisions_file_holds_nothing_rather_than_asking_about_everything() {
        let home = tempfile::tempdir().expect("tempdir");
        let _home_guard = crate::test_env::EnvVarGuard::set("LNS_HOME", home.path());
        install_some_provider(home.path());
        // Its parent is "", which matches no grant, so every served destination would ask and no grant could ever silence it.
        assert!(held_patterns_for_project(Path::new("lns-local-mixin.yaml")).is_empty());
        assert!(held_patterns_for_project(Path::new("")).is_empty());
    }
}
