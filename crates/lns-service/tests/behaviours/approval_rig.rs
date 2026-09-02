use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lns_policy::{FilePolicyStore, Policy, PolicyStore};
use lns_service::approval_flow::{
    protocol::HostFrame,
    session::{ApprovalSession, Notifier, PendingPrompt},
};
use lns_service::ledger::LedgerRecorder;
use tempfile::TempDir;
use tokio::sync::mpsc;

#[derive(Default)]
pub struct RigRecorder {
    pub events: Mutex<Vec<lns_ipc::LedgerEvent>>,
}

impl LedgerRecorder for RigRecorder {
    fn record(&self, event: lns_ipc::LedgerEvent) {
        self.events.lock().unwrap().push(event);
    }
}

#[derive(Default)]
pub struct TestNotifier {
    pub presented: Mutex<Vec<PendingPrompt>>,
    pub dismissed: Mutex<Vec<String>>,
    pub expired: Mutex<Vec<String>>,
    pub informed: Mutex<Vec<String>>,
    pub informs_cleared: Mutex<usize>,
}

impl Notifier for TestNotifier {
    fn present(&self, p: &PendingPrompt) {
        self.presented.lock().unwrap().push(p.clone());
    }
    fn dismiss(&self, id: &str) {
        self.dismissed.lock().unwrap().push(id.to_string());
    }
    fn expire(&self, id: &str) {
        self.expired.lock().unwrap().push(id.to_string());
    }
    fn inform(&self, m: &str) {
        self.informed.lock().unwrap().push(m.to_string());
    }
    fn clear_informs(&self) {
        *self.informs_cleared.lock().unwrap() += 1;
    }
}

pub struct FlakyStore {
    inner: FilePolicyStore,
    fail_next: Mutex<bool>,
}

impl FlakyStore {
    pub fn new(path: PathBuf) -> Self {
        Self {
            inner: FilePolicyStore::new(path),
            fail_next: Mutex::new(false),
        }
    }

    pub fn break_next_save(&self) {
        *self.fail_next.lock().unwrap() = true;
    }
}

impl PolicyStore for FlakyStore {
    fn save(&self, policy: &Policy) -> std::io::Result<()> {
        if std::mem::replace(&mut *self.fail_next.lock().unwrap(), false) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "simulated write failure",
            ));
        }
        self.inner.save(policy)
    }
}

pub struct ApprovalRig {
    pub session: Arc<ApprovalSession>,
    pub notifier: Arc<TestNotifier>,
    pub store: Arc<FlakyStore>,
    pub frames: mpsc::UnboundedReceiver<HostFrame>,
    pub policy_path: PathBuf,
    pub timeout: Duration,
    pub ledger: Arc<RigRecorder>,
    _tempdir: TempDir,
}

impl ApprovalRig {
    pub fn new() -> Self {
        let dir = TempDir::new().expect("create tempdir");
        let policy_path = dir.path().join("decisions.yaml");
        let notifier = Arc::new(TestNotifier::default());
        let store = Arc::new(FlakyStore::new(policy_path.clone()));
        let (tx, rx) = mpsc::unbounded_channel();
        let timeout = Duration::from_secs(30);
        let session = Arc::new(ApprovalSession::new(
            Policy::default(),
            Policy::default(),
            notifier.clone(),
            store.clone(),
            tx,
            timeout,
        ));
        let ledger = Arc::new(RigRecorder::default());
        session.set_ledger_recorder(ledger.clone());
        Self {
            session,
            notifier,
            store,
            frames: rx,
            policy_path,
            timeout,
            ledger,
            _tempdir: dir,
        }
    }
}

impl std::fmt::Debug for ApprovalRig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApprovalRig")
            .field("policy_path", &self.policy_path)
            .finish_non_exhaustive()
    }
}
