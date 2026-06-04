use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lns_service::approval_flow::protocol::{HostFrame, PolicyMessage};
use lns_service::approval_flow::window::WindowState;
use lns_service::credential_flow::notification::WindowCredentialNotifier;
use lns_service::credential_flow::registry::expand_credentials_with;
use lns_service::credential_flow::session::CredentialSession;
use lns_service::credential_flow::store::{
    CredentialStateFile, CredentialStore, JsonFileCredentialStore,
};
use tempfile::TempDir;
use tokio::sync::mpsc;

pub struct FlakyCredentialStore {
    inner: JsonFileCredentialStore,
    fail_next: Mutex<bool>,
}

impl FlakyCredentialStore {
    pub fn new(path: PathBuf) -> Self {
        Self {
            inner: JsonFileCredentialStore::new(path),
            fail_next: Mutex::new(false),
        }
    }

    pub fn break_next_save(&self) {
        *self.fail_next.lock().unwrap() = true;
    }
}

impl CredentialStore for FlakyCredentialStore {
    fn load(&self) -> std::io::Result<CredentialStateFile> {
        self.inner.load()
    }

    fn save(&self, state: &CredentialStateFile) -> std::io::Result<()> {
        if std::mem::replace(&mut *self.fail_next.lock().unwrap(), false) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "simulated write failure",
            ));
        }
        self.inner.save(state)
    }
}

pub struct CredentialRig {
    pub session: Arc<CredentialSession>,
    pub window_state: Arc<WindowState>,
    pub frames: mpsc::UnboundedReceiver<HostFrame>,
    pub host_values: Arc<Mutex<HashMap<String, String>>>,
    pub store: Arc<FlakyCredentialStore>,
    pub credentials_path: PathBuf,
    pub timeout: Duration,
    _tempdir: TempDir,
}

impl CredentialRig {
    pub fn new() -> Self {
        let dir = TempDir::new().expect("create tempdir");
        let credentials_path = dir.path().join("lns-credentials.json");
        let window_state = WindowState::new();
        let host_values = Arc::new(Mutex::new(HashMap::<String, String>::new()));
        let detector_values = host_values.clone();
        let (decision_tx, _decision_rx) = mpsc::unbounded_channel();
        let notifier = Arc::new(WindowCredentialNotifier::new(
            window_state.clone(),
            decision_tx,
            None,
            Arc::new(move |id: &str| detector_values.lock().unwrap().contains_key(id)),
        ));
        let store = Arc::new(FlakyCredentialStore::new(credentials_path.clone()));
        let (frame_tx, frame_rx) = mpsc::unbounded_channel();
        let frame_tx_for_emitter = frame_tx.clone();
        let host_values_for_emitter = host_values.clone();
        let timeout = Duration::from_secs(30);
        let session = Arc::new(CredentialSession::with_policy_emitter(
            CredentialStateFile::new(),
            notifier,
            store.clone(),
            frame_tx,
            timeout,
            Box::new(move |state| {
                let values = host_values_for_emitter.lock().unwrap().clone();
                // Production registry expansion with the host-detect source pointed at the in-memory map instead of process env.
                let credentials =
                    expand_credentials_with(state, &|id: &str| values.get(id).cloned());
                let _ = frame_tx_for_emitter.send(HostFrame::Policy(PolicyMessage {
                    network: None,
                    credentials: Some(credentials),
                }));
            }),
        ));
        Self {
            session,
            window_state,
            frames: frame_rx,
            host_values,
            store,
            credentials_path,
            timeout,
            _tempdir: dir,
        }
    }

    pub fn set_host_value(&self, credential_id: &str, value: &str) {
        self.host_values
            .lock()
            .unwrap()
            .insert(credential_id.to_string(), value.to_string());
    }

    pub fn clear_host_value(&self, credential_id: &str) {
        self.host_values.lock().unwrap().remove(credential_id);
    }
}

// A free function so the approval_flow step module can reach this without importing the request enum.
pub fn resolve_request() -> lns_service::credential_flow::session::CredentialDecisionRequest {
    lns_service::credential_flow::session::CredentialDecisionRequest::Deny
}

impl std::fmt::Debug for CredentialRig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CredentialRig")
            .field("credentials_path", &self.credentials_path)
            .finish_non_exhaustive()
    }
}
