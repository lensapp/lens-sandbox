use std::sync::{Arc, Mutex};

use lns_policy::Policy;
use lns_policy::connectors::Connector;
use lns_policy::credentials::{CredentialStateFile, CredentialStore};
use lns_service::approval_flow::window::WindowState;
use lns_service::artifact::credential_boot::ConnectPrompt;

/// Drives a sandbox definition through the launch path: catalog + definition + overlay + machine store in, armed providers (from the overlay's connected connectors and credential slots) + the ids offered for a reactive connect + running policy (or a blocked/refused launch) out.
#[derive(Default)]
pub struct DeclaredRig {
    pub catalog: Vec<Connector>,
    pub definition: Option<String>,
    pub overlay: Policy,
    pub store: CredentialStateFile,
    /// Armed providers as (connector id, env var, placeholder).
    pub providers: Vec<(String, String, String)>,
    /// Connector ids offered for a reactive connect on first use (never armed at launch).
    pub offered: Vec<String>,
    pub running_policy: Option<Policy>,
    /// The sign-in the launch is blocked on, when the gate said AwaitConnect.
    pub pending: Option<ConnectPrompt>,
    pub error: Option<String>,
    /// The definition bytes as authored, for pinning that nothing writes back to it.
    pub definition_snapshot: Option<String>,
    /// Some = the approval window is available; the launch gate cards instead of refusing.
    pub window: Option<Arc<WindowState>>,
    /// The store the in-flight value gate persists into, shared with the spawned gate task.
    pub gate_store: Option<Arc<SharedStore>>,
    pub value_gate: Option<tokio::task::JoinHandle<Result<(), String>>>,
}

impl std::fmt::Debug for DeclaredRig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeclaredRig")
            .field("definition", &self.definition)
            .field("providers", &self.providers)
            .field("offered", &self.offered)
            .field("pending", &self.pending)
            .field("error", &self.error)
            .field("window_present", &self.window.is_some())
            .field("gate_running", &self.value_gate.is_some())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Default)]
pub struct SharedStore(pub Mutex<CredentialStateFile>);

impl SharedStore {
    pub fn seeded(state: CredentialStateFile) -> Arc<Self> {
        Arc::new(Self(Mutex::new(state)))
    }

    pub fn state(&self) -> CredentialStateFile {
        self.0.lock().unwrap().clone()
    }
}

impl CredentialStore for SharedStore {
    fn load(&self) -> std::io::Result<CredentialStateFile> {
        Ok(self.state())
    }

    fn save(&self, state: &CredentialStateFile) -> std::io::Result<()> {
        *self.0.lock().unwrap() = state.clone();
        Ok(())
    }
}
