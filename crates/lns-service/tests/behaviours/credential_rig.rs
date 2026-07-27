use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::future::BoxFuture;
use lns_policy::grants::{GrantStore, GrantVerdict, JsonFileGrantStore, WorkloadIdentity};
use lns_service::approval_flow::protocol::{HostFrame, PolicyMessage};
use lns_service::approval_flow::window::WindowState;
use lns_service::credential_flow::notification::WindowCredentialNotifier;
use lns_service::credential_flow::providers::DefProvider;
use lns_service::credential_flow::registry::expand_credentials_with_custom;
use lns_service::credential_flow::session::CredentialSession;
use lns_service::credential_flow::store::{
    CredentialEntry, CredentialStateFile, CredentialStore, JsonFileCredentialStore,
};
use lns_service::oauth::{
    AuthCodeFlow, CallbackHandle, CallbackListener, CallbackParams, Clock, DeviceCode, DeviceFlow,
    OauthConfig, PkceChallenge, PkceConfig, PollOutcome, TokenSet,
};
use tempfile::TempDir;
use tokio::sync::mpsc;

pub const FIXTURE_ID: &str = "some-provider";
pub const FIXTURE_ENV: &str = "SOME_TOKEN";
pub const FIXTURE_PLACEHOLDER: &str = "some-placeholder-0000000000000000000000";
/// The value `lns connector connect` bound on this machine before any workload was granted it.
pub const FIXTURE_BOUND_VALUE: &str = "some-already-bound-value";
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
    pub opened: Arc<Mutex<Vec<String>>>,
    grant_store: Arc<dyn GrantStore>,
    grant_project: String,
    grant_workload: WorkloadIdentity,
    _tempdir: TempDir,
}

impl CredentialRig {
    /// The fixture models a connector this run consented to (connected in the directory), so its value decisions arm at the boundary.
    pub fn new() -> Self {
        Self::build(
            HashSet::from([FIXTURE_ID.to_string()]),
            CredentialStateFile::new(),
        )
    }

    /// A connector whose value `lns connector connect` already bound on this machine, in a workload holding no grant for it — so the value stays unarmed and its first use meets a card.
    pub fn bound_but_ungranted() -> Self {
        let mut bound = CredentialStateFile::new();
        bound.insert(
            FIXTURE_ID.to_string(),
            CredentialEntry::Stored {
                value: FIXTURE_BOUND_VALUE.to_string(),
            },
        );
        let rig = Self::build(HashSet::new(), bound.clone());
        rig.store.save(&bound).expect("seed the machine binding");
        rig
    }

    fn build(armed: HashSet<String>, initial: CredentialStateFile) -> Self {
        let dir = TempDir::new().expect("create tempdir");
        let credentials_path = dir.path().join("lns-credentials.json");
        let (grant_store, grant_project, grant_workload) = rig_grant_context(&dir);
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
        let session = Arc::new(
            CredentialSession::with_policy_emitter(
                initial,
                notifier,
                store.clone(),
                frame_tx,
                timeout,
                Box::new(move |state, armed| {
                    let values = host_values_for_emitter.lock().unwrap().clone();
                    // Production registry expansion (built-ins ∪ the illustrative fixture) with the host-detect source pointed at the in-memory map instead of process env.
                    let credentials = expand_credentials_with_custom(
                        state,
                        &fixture_providers(),
                        armed,
                        &|id: &str| values.get(id).cloned(),
                    );
                    let _ = frame_tx_for_emitter.send(HostFrame::Policy(PolicyMessage {
                        network: None,
                        credentials: Some(credentials),
                    }));
                }),
            )
            .with_armed_ids(armed)
            .with_grants(
                grant_project.clone(),
                grant_workload.clone(),
                grant_store.clone(),
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
            connected: Arc::new(Mutex::new(Vec::new())),
            opened: Arc::new(Mutex::new(Vec::new())),
            grant_store,
            grant_project,
            grant_workload,
            _tempdir: dir,
        }
    }

    /// True when the run's grant sidecar records a per-workload deny for the connector — the per-workload replacement for the machine-global credential Deny.
    pub fn workload_deny_recorded(&self, credential_id: &str) -> bool {
        self.grant_store
            .load()
            .map(|f| {
                f.lookup(&self.grant_project, &self.grant_workload, credential_id)
                    .is_some_and(|g| g.verdict == GrantVerdict::Deny)
            })
            .unwrap_or(false)
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
    /// A rig whose session is wired with one connectable oauth connector `id` and a device flow scripted to `outcome`.
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
        let (grant_store, grant_project, grant_workload) = rig_grant_context(&dir);
        let poll = match outcome {
            RigSignIn::Completes => PollOutcome::Token(TokenSet {
                scopes: Vec::new(),
                account: None,
                access_token: "some-access".into(),
                refresh_token: "some-refresh".into(),
                expires_in: Duration::from_secs(3600),
            }),
            RigSignIn::Expires => PollOutcome::Expired,
            RigSignIn::Denied => PollOutcome::Denied,
        };
        let configs = HashMap::from([(
            id.to_string(),
            OauthConfig {
                userinfo_endpoint: None,
                account_field: None,
                client_id: format!("Iv1.{id}"),
                client_secret: String::new(),
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
                Box::new(move |state, armed| {
                    let values = host_values_for_emitter.lock().unwrap().clone();
                    let credentials = expand_credentials_with_custom(state, &[], armed, &|id| {
                        values.get(id).cloned()
                    });
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
            )
            .with_grants(
                grant_project.clone(),
                grant_workload.clone(),
                grant_store.clone(),
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
            opened: Arc::new(Mutex::new(Vec::new())),
            grant_store,
            grant_project,
            grant_workload,
            _tempdir: dir,
        }
    }
}

#[derive(Clone, Copy)]
pub enum RigPkce {
    Completes,
    ExchangeFails,
    NeverArrives,
    Cancelled,
}

pub const RIG_PKCE_STATE: &str = "rig-state";

struct RigAuthCodeFlow {
    result: Mutex<Option<anyhow::Result<String>>>,
}
impl AuthCodeFlow for RigAuthCodeFlow {
    fn exchange_code<'a>(
        &'a self,
        _cfg: &'a PkceConfig,
        _code: &'a str,
        _verifier: &'a str,
    ) -> BoxFuture<'a, anyhow::Result<String>> {
        Box::pin(async move {
            self.result
                .lock()
                .unwrap()
                .take()
                .expect("exchange scripted once")
        })
    }
}

enum RigCallback {
    Returns(String),
    Pends,
}
struct RigCallbackListener {
    plan: Mutex<Option<RigCallback>>,
}
impl CallbackListener for RigCallbackListener {
    fn bind(&self) -> BoxFuture<'_, anyhow::Result<Box<dyn CallbackHandle>>> {
        let plan = self.plan.lock().unwrap().take().expect("bind once");
        Box::pin(async move { Ok(Box::new(RigCallbackHandle { plan }) as Box<dyn CallbackHandle>) })
    }
}
struct RigCallbackHandle {
    plan: RigCallback,
}
impl CallbackHandle for RigCallbackHandle {
    fn redirect_url(&self) -> &str {
        "http://localhost:0/callback"
    }
    fn wait(self: Box<Self>) -> BoxFuture<'static, anyhow::Result<CallbackParams>> {
        Box::pin(async move {
            match self.plan {
                RigCallback::Returns(state) => Ok(CallbackParams {
                    code: "rig-code".into(),
                    state,
                }),
                RigCallback::Pends => std::future::pending().await,
            }
        })
    }
}

struct RigShared {
    window_state: Arc<WindowState>,
    host_values: Arc<Mutex<HashMap<String, String>>>,
    store: Arc<FlakyCredentialStore>,
    frames: mpsc::UnboundedReceiver<HostFrame>,
    credentials_path: PathBuf,
    grant_store: Arc<dyn GrantStore>,
    grant_project: String,
    grant_workload: WorkloadIdentity,
    dir: TempDir,
}

fn rig_grant_context(dir: &TempDir) -> (Arc<dyn GrantStore>, String, WorkloadIdentity) {
    let store: Arc<dyn GrantStore> = Arc::new(JsonFileGrantStore::new(
        dir.path().join("workload-grants.json"),
    ));
    (
        store,
        "rig-project".to_string(),
        WorkloadIdentity::Definition {
            dir: "/rig".to_string(),
        },
    )
}

/// The notifier/store/window/emitter scaffolding shared by the pkce rigs (mirrors `new`/`oauth`).
fn scaffold(state: CredentialStateFile, timeout: Duration) -> (CredentialSession, RigShared) {
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
    let (grant_store, grant_project, grant_workload) = rig_grant_context(&dir);
    let session = CredentialSession::with_policy_emitter(
        state,
        notifier,
        store.clone(),
        frame_tx,
        timeout,
        Box::new(move |state, armed| {
            let values = host_values_for_emitter.lock().unwrap().clone();
            let credentials =
                expand_credentials_with_custom(state, &[], armed, &|id| values.get(id).cloned());
            let _ = frame_tx_for_emitter.send(HostFrame::Policy(PolicyMessage {
                network: None,
                credentials: Some(credentials),
            }));
        }),
    )
    .with_grants(
        grant_project.clone(),
        grant_workload.clone(),
        grant_store.clone(),
    );
    (
        session,
        RigShared {
            window_state,
            host_values,
            store,
            frames: frame_rx,
            credentials_path,
            grant_store,
            grant_project,
            grant_workload,
            dir,
        },
    )
}

impl CredentialRig {
    /// A rig whose session is wired with one connectable pkce connector `id` and a fake authorization-code flow + loopback listener scripted to `outcome`.
    pub fn pkce(id: &str, outcome: RigPkce) -> Self {
        let (exchange, callback, pkce_timeout): (anyhow::Result<String>, RigCallback, Duration) =
            match outcome {
                RigPkce::Completes => (
                    Ok("openrouter-key".into()),
                    RigCallback::Returns(RIG_PKCE_STATE.into()),
                    Duration::from_secs(30),
                ),
                RigPkce::ExchangeFails => (
                    Err(anyhow::anyhow!("the authorization code was rejected")),
                    RigCallback::Returns(RIG_PKCE_STATE.into()),
                    Duration::from_secs(30),
                ),
                RigPkce::NeverArrives => (
                    Err(anyhow::anyhow!("must not exchange")),
                    RigCallback::Pends,
                    Duration::from_millis(50),
                ),
                RigPkce::Cancelled => (
                    Err(anyhow::anyhow!("must not exchange")),
                    RigCallback::Pends,
                    Duration::from_secs(30),
                ),
            };
        let (session, shared) = scaffold(CredentialStateFile::new(), Duration::from_secs(30));
        let connected = Arc::new(Mutex::new(Vec::new()));
        let connected_cb = connected.clone();
        let opened = Arc::new(Mutex::new(Vec::new()));
        let opened_cb = opened.clone();
        let configs = HashMap::from([(
            id.to_string(),
            PkceConfig {
                authorization_endpoint: format!("https://{id}.example/auth"),
                token_endpoint: format!("https://{id}.example/api/v1/auth/keys"),
                scopes: Vec::new(),
            },
        )]);
        let session = Arc::new(
            session
                .with_connect_emitter(
                    HashSet::from([id.to_string()]),
                    Box::new(move |cid| connected_cb.lock().unwrap().push(cid.to_string())),
                )
                .with_pkce(
                    configs,
                    Arc::new(RigAuthCodeFlow {
                        result: Mutex::new(Some(exchange)),
                    }),
                    Arc::new(RigCallbackListener {
                        plan: Mutex::new(Some(callback)),
                    }),
                    Box::new(move |url: &str| opened_cb.lock().unwrap().push(url.to_string())),
                    Box::new(|| PkceChallenge {
                        verifier: "rig-verifier".into(),
                        challenge: "rig-challenge".into(),
                        state: RIG_PKCE_STATE.into(),
                    }),
                    pkce_timeout,
                )
                .with_oauth_display_names(HashMap::from([(
                    id.to_string(),
                    "OpenRouter".to_string(),
                )])),
        );
        Self {
            session,
            window_state: shared.window_state,
            frames: shared.frames,
            host_values: shared.host_values,
            store: shared.store,
            credentials_path: shared.credentials_path,
            timeout: Duration::from_secs(30),
            connected,
            opened,
            grant_store: shared.grant_store,
            grant_project: shared.grant_project,
            grant_workload: shared.grant_workload,
            _tempdir: shared.dir,
        }
    }

    /// A rig with `id` already connected and its pkce key stored, plus the custom provider that injects it, so a fresh run's request to its host is armed without any sign-in.
    pub fn pkce_connected(id: &str) -> Self {
        use lns_policy::providers::{InjectionDef, InjectionKind, ProviderDef};
        let mut state = CredentialStateFile::new();
        state.insert(
            id.to_string(),
            CredentialEntry::Stored {
                value: "openrouter-key".into(),
            },
        );
        let (session, shared) = scaffold(state, Duration::from_secs(30));
        let provider = DefProvider::new(ProviderDef {
            id: id.into(),
            env_var: "OPENROUTER_API_KEY".into(),
            placeholder: "sk-or-LNSPLACEHOLDER0000000000000000".into(),
            injections: vec![InjectionDef {
                kind: InjectionKind::BearerHeader,
                domain: "api.some-pkce.example".into(),
                header: None,
            }],
        });
        let session = Arc::new(
            session
                .with_custom_providers(Arc::new(vec![provider]))
                .with_armed_ids(HashSet::from([id.to_string()])),
        );
        Self {
            session,
            window_state: shared.window_state,
            frames: shared.frames,
            host_values: shared.host_values,
            store: shared.store,
            credentials_path: shared.credentials_path,
            timeout: Duration::from_secs(30),
            connected: Arc::new(Mutex::new(Vec::new())),
            opened: Arc::new(Mutex::new(Vec::new())),
            grant_store: shared.grant_store,
            grant_project: shared.grant_project,
            grant_workload: shared.grant_workload,
            _tempdir: shared.dir,
        }
    }
}
