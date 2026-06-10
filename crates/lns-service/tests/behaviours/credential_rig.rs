use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::future::BoxFuture;
use lns_service::approval_flow::protocol::{HostFrame, PolicyMessage};
use lns_service::approval_flow::window::WindowState;
use lns_service::credential_flow::notification::WindowCredentialNotifier;
use lns_service::credential_flow::providers::DefProvider;
use lns_service::credential_flow::registry::{
    expand_credentials_with, expand_credentials_with_custom,
};
use lns_service::credential_flow::session::CredentialSession;
use lns_service::credential_flow::store::{
    CredentialStateFile, CredentialStore, JsonFileCredentialStore,
};
use lns_service::oauth::{Clock, DeviceCode, DeviceFlow, OauthConfig, PollOutcome, TokenSet};
use tempfile::TempDir;
use tokio::sync::mpsc;

pub const FIXTURE_ID: &str = "some-provider";
pub const FIXTURE_ENV: &str = "SOME_TOKEN";
pub const FIXTURE_PLACEHOLDER: &str = "some-placeholder-0000000000000000000000";
pub const FIXTURE_DOMAIN: &str = "api.some-provider.example";

/// An illustrative provider so the credential-flow scenarios pin the mechanism without coupling to any shipped service.
fn fixture_providers() -> Vec<DefProvider> {
    use lns_policy::providers::{InjectionDef, InjectionKind, ProviderDef};
    vec![DefProvider::new(ProviderDef {
        id: FIXTURE_ID.into(),
        env_var: FIXTURE_ENV.into(),
        placeholder: FIXTURE_PLACEHOLDER.into(),
        injections: vec![InjectionDef {
            kind: InjectionKind::BearerHeader,
            domain: FIXTURE_DOMAIN.into(),
            header: None,
        }],
    })]
}

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
    pub connected: Arc<Mutex<Vec<String>>>,
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
                // Production registry expansion (built-ins ∪ the illustrative fixture) with the host-detect source pointed at the in-memory map instead of process env.
                let credentials =
                    expand_credentials_with_custom(state, &fixture_providers(), &|id: &str| {
                        values.get(id).cloned()
                    });
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
            connected: Arc::new(Mutex::new(Vec::new())),
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

#[derive(Clone, Copy)]
pub enum RigSignIn {
    Completes,
    Expires,
    Denied,
}

struct RigDeviceFlow {
    poll: Mutex<Option<PollOutcome>>,
}

impl DeviceFlow for RigDeviceFlow {
    fn request_device_code<'a>(
        &'a self,
        _cfg: &'a OauthConfig,
    ) -> BoxFuture<'a, anyhow::Result<DeviceCode>> {
        Box::pin(async move {
            Ok(DeviceCode {
                device_code: "dc".into(),
                user_code: "WXYZ-1234".into(),
                verification_uri: "https://example.com/device".into(),
                interval: Duration::ZERO,
                expires_in: Duration::from_secs(900),
            })
        })
    }
    fn poll_token<'a>(
        &'a self,
        _cfg: &'a OauthConfig,
        _device_code: &'a str,
    ) -> BoxFuture<'a, anyhow::Result<PollOutcome>> {
        Box::pin(async move {
            Ok(self
                .poll
                .lock()
                .unwrap()
                .take()
                .expect("poll scripted once"))
        })
    }
    fn refresh<'a>(
        &'a self,
        _cfg: &'a OauthConfig,
        _refresh_token: &'a str,
    ) -> BoxFuture<'a, anyhow::Result<TokenSet>> {
        Box::pin(async move { anyhow::bail!("refresh is not exercised by connect scenarios") })
    }
}

struct FixedClock;
impl Clock for FixedClock {
    fn now_unix(&self) -> u64 {
        1000
    }
}

impl CredentialRig {
    /// A rig whose session is wired with one connectable oauth integration `id` and a device flow scripted to `outcome`.
    pub fn oauth(id: &str, outcome: RigSignIn) -> Self {
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
        let connected = Arc::new(Mutex::new(Vec::new()));
        let connected_cb = connected.clone();
        let poll = match outcome {
            RigSignIn::Completes => PollOutcome::Token(TokenSet {
                access_token: "gho_access".into(),
                refresh_token: "ghr_refresh".into(),
                expires_in: Duration::from_secs(3600),
            }),
            RigSignIn::Expires => PollOutcome::Expired,
            RigSignIn::Denied => PollOutcome::Denied,
        };
        let configs = HashMap::from([(
            id.to_string(),
            OauthConfig {
                client_id: format!("Iv1.{id}"),
                scopes: vec!["repo".into()],
                device_authorization_endpoint: format!("https://example.com/{id}/device/code"),
                token_endpoint: format!("https://example.com/{id}/oauth/token"),
            },
        )]);
        let session = Arc::new(
            CredentialSession::with_policy_emitter(
                CredentialStateFile::new(),
                notifier,
                store.clone(),
                frame_tx,
                timeout,
                Box::new(move |state| {
                    let values = host_values_for_emitter.lock().unwrap().clone();
                    let credentials =
                        expand_credentials_with(state, &|id: &str| values.get(id).cloned());
                    let _ = frame_tx_for_emitter.send(HostFrame::Policy(PolicyMessage {
                        network: None,
                        credentials: Some(credentials),
                    }));
                }),
            )
            .with_connect_emitter(
                HashSet::from([id.to_string()]),
                Box::new(move |connected_id| {
                    connected_cb.lock().unwrap().push(connected_id.to_string())
                }),
            )
            .with_oauth(
                configs,
                Arc::new(RigDeviceFlow {
                    poll: Mutex::new(Some(poll)),
                }),
                Arc::new(FixedClock),
            ),
        );
        Self {
            session,
            window_state,
            frames: frame_rx,
            host_values,
            store,
            credentials_path,
            timeout,
            connected,
            _tempdir: dir,
        }
    }
}
