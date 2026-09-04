use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lns_ipc::ConnectorView;
use lns_policy::{FilePolicyStore, Policy, PolicyStore};
use lns_service::approval_flow::{
    entries::{Entry, EntryStore, FileEntryStore},
    protocol::{GrantedPayload, HostFrame, RequestPending, Treatment},
    session::{ApprovalSession, ConnectionChoice, ConnectorPort, Notifier, PendingPrompt},
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

/// A connector store that says yes and opens nothing, so a grant can be taken without a real connector on the machine.
struct GrantingPort;

impl ConnectorPort for GrantingPort {
    fn connect(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: lns_ipc::SecretValues,
    ) -> Result<Vec<String>, String> {
        Ok(Vec::new())
    }

    fn grant(&self, _: &str, _: &str, _: &str, _: Option<&str>) -> Result<GrantedPayload, String> {
        Ok(GrantedPayload {
            egress: Policy::default(),
            credentials: Vec::new(),
            env: Default::default(),
            files: Vec::new(),
        })
    }

    fn decline(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
}

pub struct ApprovalRig {
    pub session: Arc<ApprovalSession>,
    pub notifier: Arc<TestNotifier>,
    pub store: Arc<FlakyStore>,
    pub frames: mpsc::UnboundedReceiver<HostFrame>,
    pub policy_path: PathBuf,
    pub entries_path: PathBuf,
    pub timeout: Duration,
    pub ledger: Arc<RigRecorder>,
    _tempdir: TempDir,
}

impl ApprovalRig {
    /// What the run keeps, read the way a restarted service reads it — from the file, never from the session that wrote it.
    pub fn entries(&self) -> Vec<Entry> {
        FileEntryStore::new(self.entries_path.clone()).list()
    }

    pub fn entry_for(&self, subject: &str) -> Option<Entry> {
        self.entries()
            .into_iter()
            .find(|entry| entry.subject() == subject)
    }

    pub fn new() -> Self {
        Self::for_run(None)
    }

    /// A second session over the same run directory: what a restarted lns-service reads.
    pub fn restart(&mut self) {
        let notifier = Arc::new(TestNotifier::default());
        let (tx, rx) = mpsc::unbounded_channel();
        let session = Arc::new(ApprovalSession::new(
            Policy::default(),
            Policy::default(),
            notifier.clone(),
            self.store.clone(),
            tx,
            self.timeout,
        ));
        session.set_entry_store(Arc::new(FileEntryStore::new(self.entries_path.clone())));
        self.session = session;
        self.notifier = notifier;
        self.frames = rx;
    }

    /// Grants a connector the way a card does: the run holds an offer, and the developer takes it.
    pub fn grant_connector(&self, name: &str) {
        let host = "api.linear.app";
        self.session.set_connector_port(Arc::new(GrantingPort));
        self.session.hold_for_offers(vec![ConnectorView {
            name: name.to_string(),
            digest: "sha256:test".into(),
            serves: vec![host.to_string()],
            methods: Vec::new(),
            connections: Vec::new(),
        }]);
        self.session.submit_pending(
            RequestPending {
                id: format!("req-{host}"),
                host: host.to_string(),
                action: format!("CONNECT {host}:443"),
                reason: "policy-ambiguous".into(),
                treatment: Treatment::Inspected,
            },
            std::time::Instant::now(),
        );
        self.session
            .grant_offer(&format!("req-{host}"), "token", ConnectionChoice::None);
    }

    pub fn for_run(run: Option<String>) -> Self {
        let dir = TempDir::new().expect("create tempdir");
        let policy_path = dir.path().join("decisions.yaml");
        let entries_path = dir.path().join("approvals.json");
        let notifier = Arc::new(TestNotifier::default());
        let store = Arc::new(FlakyStore::new(policy_path.clone()));
        let (tx, rx) = mpsc::unbounded_channel();
        let timeout = Duration::from_secs(30);
        let built = ApprovalSession::new(
            Policy::default(),
            Policy::default(),
            notifier.clone(),
            store.clone(),
            tx,
            timeout,
        );
        let session = Arc::new(match run {
            Some(run) => built.for_run(run),
            None => built,
        });
        let ledger = Arc::new(RigRecorder::default());
        session.set_ledger_recorder(ledger.clone());
        session.set_entry_store(Arc::new(FileEntryStore::new(entries_path.clone())));
        Self {
            session,
            notifier,
            store,
            frames: rx,
            policy_path,
            entries_path,
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
