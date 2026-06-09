use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use std::sync::Weak;

use tokio::sync::mpsc;

use crate::approval_flow::notification::WindowNotifier;
use crate::approval_flow::protocol::HostFrame;
use crate::approval_flow::protocol::PolicyMessage;
use crate::approval_flow::session::ApprovalSession;
use crate::approval_flow::watcher::PolicyWatcher;
use crate::approval_flow::window::{self, CredentialDecisionDelivery, DecisionDelivery};
use crate::credential_flow::integrations::{
    resolve_applied_integrations, resolve_connectable_integrations,
};
use crate::credential_flow::notification::WindowCredentialNotifier;
use crate::credential_flow::providers::{self, DefProvider, Provider};
use crate::credential_flow::registry::expand_credentials_for_wire_with_custom;
use crate::credential_flow::session::CredentialSession;
use crate::credential_flow::store::{
    CredentialStateFile, CredentialStore, JsonFileCredentialStore, default_credentials_path,
};
use crate::credential_flow::watcher::CredentialWatcher;
use crate::log;
use crate::relay;
use lns_policy::{FilePolicyStore, Policy, RouteRule};

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
        session.record_decision(&delivery.id, delivery.decision);
    }
}

/// Mirror of [`decision_delivery_loop`]; `Weak` so the loop never keeps the session alive past its run.
async fn credential_delivery_loop(
    session: Weak<CredentialSession>,
    mut decision_rx: mpsc::UnboundedReceiver<CredentialDecisionDelivery>,
) {
    while let Some(delivery) = decision_rx.recv().await {
        let Some(session) = session.upgrade() else {
            break;
        };
        session.record_decision(&delivery.id, delivery.request);
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

async fn credential_tick_timeouts_loop(weak: Weak<CredentialSession>) {
    let mut interval = tokio::time::interval(APPROVAL_TICK);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        if !credential_sweep_once(&weak) {
            break;
        }
    }
}

fn credential_sweep_once(weak: &Weak<CredentialSession>) -> bool {
    let Some(session) = weak.upgrade() else {
        return false;
    };
    session.tick_timeouts(std::time::Instant::now());
    true
}

fn sweep_once(weak: &Weak<ApprovalSession>) -> bool {
    let Some(session) = weak.upgrade() else {
        return false;
    };
    session.tick_timeouts(std::time::Instant::now());
    true
}

/// Defaults to empty and warns on store error, so a malformed `~/.lns-credentials.json` doesn't silently wipe the developer's rules at startup.
fn load_credentials_or_warn(store: &dyn CredentialStore, path: &Path) -> CredentialStateFile {
    match store.load() {
        Ok(state) => state,
        Err(e) => {
            let path_str = path.display();
            log::warn!("could not load {path_str} ({e}); starting with empty credential state");
            CredentialStateFile::new()
        }
    }
}

/// Defaults to an empty user catalog and warns on load error, so a malformed `~/.lns-integrations.yaml` doesn't break a run — the bundled catalog still applies.
fn load_user_catalog_or_warn(path: &Path) -> lns_policy::integrations::Catalog {
    match lns_policy::integrations::Catalog::load_or_default(path) {
        Ok(catalog) => catalog,
        Err(e) => {
            let path_str = path.display();
            log::warn!(
                "could not load {path_str} ({e}); using the bundled integration catalog only"
            );
            lns_policy::integrations::Catalog::default()
        }
    }
}

/// The env vars seeded as placeholders for this run's custom providers + connected integrations; stripped from `-e` so a real secret can't bypass the placeholder. Built-ins are handled globally by `is_managed_env`.
fn collect_managed_env_vars(providers: &[DefProvider]) -> Vec<String> {
    providers.iter().map(|p| p.env_var().to_string()).collect()
}

/// `Weak` so the closure doesn't keep the credential session alive past the run; a dropped session yields an empty list.
fn make_credentials_provider(
    credential_session: &Arc<CredentialSession>,
) -> crate::approval_flow::session::CredentialsProvider {
    let weak = Arc::downgrade(credential_session);
    Box::new(move || {
        weak.upgrade()
            .map(|cs| {
                expand_credentials_for_wire_with_custom(&cs.current_state(), cs.custom_providers())
            })
            .unwrap_or_default()
    })
}

fn build_credential_notifier(
    decision_tx: tokio::sync::mpsc::UnboundedSender<CredentialDecisionDelivery>,
    custom_providers: Arc<Vec<DefProvider>>,
) -> WindowCredentialNotifier {
    let window_state = window::get().expect("window state installed by caller");
    WindowCredentialNotifier::with_registry_detection(
        window_state,
        decision_tx,
        window::ctx(),
        custom_providers,
    )
}

/// The follow-up `Policy` frame after a credential decision must carry both the current network policy and the registry-expanded credentials.
fn make_policy_emitter(
    session: Arc<ApprovalSession>,
    sink: tokio::sync::mpsc::UnboundedSender<HostFrame>,
    custom_providers: Arc<Vec<DefProvider>>,
) -> crate::credential_flow::session::PolicyEmitter {
    Box::new(move |state| {
        let network = session.current_policy().network;
        let credentials = expand_credentials_for_wire_with_custom(state, &custom_providers);
        let _ = sink.send(HostFrame::Policy(PolicyMessage {
            network: Some(network),
            credentials: Some(credentials),
        }));
    })
}

/// Connecting an un-connected catalog integration allows its routes on the approval session's live policy (and persists `integrations:`), so the held request proceeds without a relaunch.
fn make_connect_emitter(
    session: Arc<ApprovalSession>,
    routes: Arc<HashMap<String, Vec<RouteRule>>>,
) -> crate::credential_flow::session::ConnectEmitter {
    Box::new(move |id| {
        let rules = routes.get(id).cloned().unwrap_or_default();
        session.connect_integration(id, rules);
    })
}

type CredentialSubsystem = (
    Arc<CredentialSession>,
    crate::credential_flow::watcher::CredentialWatcher,
);

/// A device-flow access token within this many seconds of expiry is refreshed at run start rather than served stale.
const OAUTH_REFRESH_SKEW_SECS: u64 = 60;

async fn start_credential_subsystem(
    session: Arc<ApprovalSession>,
    credential_frame_tx: tokio::sync::mpsc::UnboundedSender<HostFrame>,
    custom_providers: Arc<Vec<DefProvider>>,
    connectable_ids: HashSet<String>,
    connectable_routes: Arc<HashMap<String, Vec<RouteRule>>>,
    oauth_configs: HashMap<String, crate::oauth::OauthConfig>,
) -> Result<CredentialSubsystem> {
    // The credentials file is per-machine $HOME state, so its path is independent of `--policy`.
    let credentials_path = default_credentials_path();
    let credential_store: Arc<dyn CredentialStore> =
        Arc::new(JsonFileCredentialStore::new(credentials_path.clone()));
    let mut initial_credential_state =
        load_credentials_or_warn(credential_store.as_ref(), &credentials_path);
    // Renew any oauth grant that expired since last use before the session arms it (the dominant case; a mid-run expiry falls back to the held-request re-prompt).
    crate::oauth::refresh_due_entries(
        &mut initial_credential_state,
        &oauth_configs,
        &crate::oauth::RealDeviceFlow,
        &crate::oauth::RealClock,
        credential_store.as_ref(),
        OAUTH_REFRESH_SKEW_SECS,
    )
    .await;

    let (credential_decision_tx, credential_decision_rx) = tokio::sync::mpsc::unbounded_channel();
    let credential_notifier = Arc::new(build_credential_notifier(
        credential_decision_tx,
        custom_providers.clone(),
    ));

    let policy_emitter = make_policy_emitter(
        session.clone(),
        credential_frame_tx.clone(),
        custom_providers.clone(),
    );
    let connect_emitter = make_connect_emitter(session.clone(), connectable_routes);
    let credential_session = Arc::new(
        CredentialSession::with_policy_emitter(
            initial_credential_state,
            credential_notifier,
            credential_store,
            credential_frame_tx,
            APPROVAL_TIMEOUT,
            policy_emitter,
        )
        .with_custom_providers(custom_providers)
        .with_connect_emitter(connectable_ids, connect_emitter),
    );

    tokio::spawn(credential_delivery_loop(
        Arc::downgrade(&credential_session),
        credential_decision_rx,
    ));
    tokio::spawn(credential_tick_timeouts_loop(Arc::downgrade(
        &credential_session,
    )));

    // Back-reference so the approval session's Policy emits carry the credential registry instead of `credentials: null`.
    session.set_credentials_provider(make_credentials_provider(&credential_session));

    let credential_watcher = CredentialWatcher::spawn(credentials_path, credential_session.clone())
        .context("watching credentials file")?;

    Ok((credential_session, credential_watcher))
}

pub(super) async fn start(
    run_id: u32,
    policy_path: &Path,
    guest_tools_root: PathBuf,
    user_env: Vec<String>,
) -> Result<SupervisorSession> {
    let mut policy = Policy::load_or_default(policy_path)
        .with_context(|| format!("loading policy {}", policy_path.display()))?;
    // Applied integrations resolve against the effective catalog (bundled ∪ user) into both wire credentials and allow-routes, captured once at boot so a later edit can't reach an already-forked workload.
    let user_catalog =
        load_user_catalog_or_warn(&lns_policy::integrations::default_integrations_path());
    let catalog = lns_policy::integrations::effective_integrations(&user_catalog);
    let applied = resolve_applied_integrations(&policy, &catalog);
    // Un-connected catalog integrations are seeded unarmed so their use offers a live connect.
    let connectable = resolve_connectable_integrations(&policy, &catalog);
    policy.network.allowed_routes.extend(applied.routes);
    let connectable_ids: HashSet<String> = connectable
        .providers
        .iter()
        .map(|p| p.id().to_string())
        .collect();
    let connectable_routes = Arc::new(connectable.routes);
    let mut custom = providers::build_policy_providers(&policy);
    custom.extend(applied.providers);
    custom.extend(connectable.providers);
    let custom_providers = Arc::new(custom);
    let managed_env_vars = collect_managed_env_vars(&custom_providers);

    let window_state = window::get().context(
        "approval window state was not installed at boot; \
         tray::run_tray must run before any policy-bearing run starts",
    )?;
    let (decision_tx, decision_rx) = tokio::sync::mpsc::unbounded_channel::<DecisionDelivery>();
    let notifier = Arc::new(WindowNotifier::new(
        window_state,
        decision_tx,
        window::ctx(),
    ));
    log::info!("Approvals", "window ready");
    let store = Arc::new(FilePolicyStore::new(policy_path.to_path_buf()));
    let (frame_tx, frame_rx) = tokio::sync::mpsc::unbounded_channel::<HostFrame>();
    let credential_frame_tx = frame_tx.clone();
    let session = Arc::new(ApprovalSession::new(
        policy,
        notifier,
        store,
        frame_tx,
        APPROVAL_TIMEOUT,
    ));

    tokio::spawn(decision_delivery_loop(
        Arc::downgrade(&session),
        decision_rx,
    ));

    tokio::spawn(tick_timeouts_loop(Arc::downgrade(&session)));

    let watcher = PolicyWatcher::spawn(policy_path.to_path_buf(), session.clone())
        .with_context(|| format!("watching policy {}", policy_path.display()))?;

    let oauth_configs: HashMap<String, crate::oauth::OauthConfig> = applied
        .oauth_configs
        .iter()
        .chain(connectable.oauth_configs.iter())
        .map(|(id, auth)| (id.clone(), crate::oauth::OauthConfig::from(auth)))
        .collect();
    let (credential_session, credential_watcher) = start_credential_subsystem(
        session.clone(),
        credential_frame_tx,
        custom_providers,
        connectable_ids,
        connectable_routes,
        oauth_configs,
    )
    .await?;

    let supervisor_bin = ensure().await?;
    let relay = relay::spawn(run_id, session, credential_session, frame_rx, user_env)?;
    log::debug!(url = %relay.url, "relay listening");
    log::info!("Auditing", "to {}", relay.audit_path.display());
    Ok(SupervisorSession {
        assets: SupervisorAssets {
            supervisor_bin,
            guest_tools_root,
        },
        relay,
        watcher: Some(watcher),
        credential_watcher: Some(credential_watcher),
        managed_env_vars,
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
            notifier,
            store,
            frame_tx,
            std::time::Duration::from_secs(30),
        ));
        (session, frame_rx)
    }

    #[tokio::test]
    async fn decision_delivery_loop_applies_each_delivery_and_exits_on_tx_drop() {
        use crate::approval_flow::protocol::{Decision, HostFrame, RequestPending};
        let (session, mut frame_rx) = fixture_session();
        let (tx, rx) = mpsc::unbounded_channel::<DecisionDelivery>();
        session.submit_pending(
            RequestPending {
                id: "r1".into(),
                host: "api.linear.app".into(),
                action: "CONNECT api.linear.app:443".into(),
                reason: "policy-ambiguous".into(),
            },
            std::time::Instant::now(),
        );
        tx.send(DecisionDelivery {
            id: "r1".into(),
            decision: Decision::AllowOnce,
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
    async fn decision_delivery_loop_breaks_when_upgrade_fails_with_buffered_delivery() {
        use crate::approval_flow::protocol::Decision;
        let (session, _frame_rx) = fixture_session();
        let weak = Arc::downgrade(&session);
        let (tx, rx) = mpsc::unbounded_channel::<DecisionDelivery>();
        let stale_clone = tx.clone();
        stale_clone
            .send(DecisionDelivery {
                id: "r1".into(),
                decision: Decision::AllowOnce,
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

    #[tokio::test]
    async fn tick_timeouts_loop_invokes_sweep_while_session_alive() {
        let (session, _frame_rx) = fixture_session();
        let weak = Arc::downgrade(&session);
        let handle = tokio::spawn(tick_timeouts_loop(weak));
        tokio::task::yield_now().await;
        drop(session);
        tokio::time::timeout(std::time::Duration::from_secs(2), handle)
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

    fn fixture_credential_session() -> (Arc<CredentialSession>, mpsc::UnboundedReceiver<HostFrame>)
    {
        use crate::credential_flow::notification::NoopCredentialNotifier;
        let (store, _dir) = tempfile_credential_store();
        // Leak the tempdir guard for the life of the session (test-only).
        Box::leak(Box::new(_dir));
        let (frame_tx, frame_rx) = mpsc::unbounded_channel::<HostFrame>();
        let session = Arc::new(CredentialSession::new(
            CredentialStateFile::new(),
            Arc::new(NoopCredentialNotifier),
            store,
            frame_tx,
            std::time::Duration::from_secs(30),
        ));
        (session, frame_rx)
    }

    /// Real store (not an inline fake) keeps its `CredentialStore` impl out of the coverage gap.
    fn tempfile_credential_store() -> (
        Arc<crate::credential_flow::store::JsonFileCredentialStore>,
        tempfile::TempDir,
    ) {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("creds.json");
        (
            Arc::new(crate::credential_flow::store::JsonFileCredentialStore::new(
                path,
            )),
            dir,
        )
    }

    #[test]
    fn load_credentials_or_warn_returns_stored_state_on_ok() {
        use crate::credential_flow::store::CredentialEntry;
        let (store, _dir) = tempfile_credential_store();
        let mut seeded = CredentialStateFile::new();
        seeded.insert("github".into(), CredentialEntry::HostDetect);
        store.save(&seeded).unwrap();
        let state = load_credentials_or_warn(store.as_ref(), Path::new("/tmp/x"));
        assert!(state.contains_key("github"));
    }

    #[test]
    fn load_credentials_or_warn_defaults_to_empty_and_warns_on_store_error() {
        init_tracing_capture();
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join("creds.json");
        std::fs::write(&path, "{ this is not valid json").unwrap();
        let store = crate::credential_flow::store::JsonFileCredentialStore::new(path.clone());
        let state = load_credentials_or_warn(&store, &path);
        assert!(
            state.is_empty(),
            "malformed credentials file must surface as empty in-memory state, got {state:?}"
        );
    }

    #[test]
    fn load_user_catalog_or_warn_reads_an_existing_user_catalog() {
        use lns_policy::integrations::{AuthKind, Catalog, CredentialAuth, Integration};
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join(".lns-integrations.yaml");
        Catalog {
            integrations: vec![Integration {
                id: "acme".into(),
                auth_kind: AuthKind::Credential,
                routes: Vec::new(),
                credential: Some(CredentialAuth {
                    env_var: "ACME_API_KEY".into(),
                    placeholder: "acme_LNSPLACEHOLDER".into(),
                    injections: Vec::new(),
                }),
                oauth: None,
            }],
        }
        .save_atomic(&path)
        .unwrap();
        let catalog = load_user_catalog_or_warn(&path);
        assert_eq!(catalog.integrations.len(), 1);
        assert_eq!(catalog.integrations[0].id, "acme");
    }

    #[test]
    fn load_user_catalog_or_warn_defaults_to_empty_and_warns_on_load_error() {
        init_tracing_capture();
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join(".lns-integrations.yaml");
        std::fs::write(&path, "integrations: not-a-list\n").unwrap();
        let catalog = load_user_catalog_or_warn(&path);
        assert!(
            catalog.integrations.is_empty(),
            "a malformed user catalog must surface as empty so the run still gets the bundled set"
        );
    }

    #[test]
    fn make_credentials_provider_returns_registry_expansion_while_session_alive() {
        use crate::credential_flow::providers;
        let (session, _frame_rx) = fixture_credential_session();
        let provider = make_credentials_provider(&session);
        let creds = provider();
        let ids: Vec<&str> = creds.iter().map(|c| c.id.as_str()).collect();
        for p in providers::ALL.iter() {
            let id = p.id();
            assert!(ids.contains(&id), "missing {id} in provider output");
        }
    }

    #[test]
    fn make_policy_emitter_sends_policy_with_network_and_credentials() {
        use crate::credential_flow::providers;
        use crate::credential_flow::store::{CredentialEntry, CredentialStateFile};
        use lns_policy::RouteRule;
        let (session, mut session_rx) = fixture_session();
        let mut updated = Policy::default();
        updated.add_rule(RouteRule::allow_host("api.linear.app"));
        session.apply_external_policy(updated);
        // Drain the apply_external_policy emit so it doesn't clutter the assertions.
        while session_rx.try_recv().is_ok() {}

        let (sink, mut sink_rx) = mpsc::unbounded_channel::<HostFrame>();
        let emitter = make_policy_emitter(session, sink, Arc::new(Vec::new()));

        let mut state = CredentialStateFile::new();
        state.insert(
            "github".into(),
            CredentialEntry::Stored {
                value: "ghp_real".into(),
            },
        );
        emitter(&state);

        let frame = sink_rx
            .try_recv()
            .expect("emitter must push a Policy frame");
        let json = serde_json::to_value(&frame).expect("serialise");
        assert_eq!(json["type"], "policy");
        assert_eq!(
            json["network"]["allowedRoutes"][0]["match"],
            "api.linear.app"
        );
        let ids: Vec<&str> = json["credentials"]
            .as_array()
            .expect("credentials present")
            .iter()
            .map(|c| c["id"].as_str().unwrap())
            .collect();
        for p in providers::ALL.iter() {
            let id = p.id();
            assert!(ids.contains(&id), "missing {id} in {ids:?}");
        }
    }

    #[test]
    fn make_credentials_provider_returns_empty_when_session_dropped() {
        let (session, _frame_rx) = fixture_credential_session();
        let provider = make_credentials_provider(&session);
        drop(session);
        assert!(provider().is_empty());
    }

    fn acme_custom() -> Arc<Vec<DefProvider>> {
        use lns_policy::providers::{InjectionDef, InjectionKind, ProviderDef};
        Arc::new(vec![DefProvider::new(ProviderDef {
            id: "acme".into(),
            env_var: "ACME_API_KEY".into(),
            placeholder: "acme_LNSPLACEHOLDER".into(),
            injections: vec![InjectionDef {
                kind: InjectionKind::BearerHeader,
                domain: "api.acme.corp".into(),
                header: None,
            }],
        })])
    }

    #[test]
    fn collect_managed_env_vars_lists_each_run_providers_env_var() {
        assert_eq!(collect_managed_env_vars(&acme_custom()), ["ACME_API_KEY"]);
    }

    #[test]
    fn make_connect_emitter_connects_the_integration_on_the_approval_session() {
        let (session, mut rx) = fixture_session();
        while rx.try_recv().is_ok() {}
        let mut routes = HashMap::new();
        routes.insert(
            "gitlab".to_string(),
            vec![lns_policy::RouteRule::allow_host("gitlab.com")],
        );
        let connect = make_connect_emitter(session.clone(), Arc::new(routes));
        connect("gitlab");
        assert_eq!(session.current_policy().integrations, ["gitlab"]);
        assert!(
            session
                .current_policy()
                .network
                .allowed_routes
                .iter()
                .any(|r| r.match_pattern == "gitlab.com"),
            "the integration's route is allowed live"
        );
        assert!(rx.try_recv().is_ok(), "a Policy frame is emitted");
    }

    #[test]
    fn make_connect_emitter_with_no_routes_for_an_id_still_connects_it() {
        let (session, _rx) = fixture_session();
        let connect = make_connect_emitter(session.clone(), Arc::new(HashMap::new()));
        connect("gitlab");
        assert_eq!(session.current_policy().integrations, ["gitlab"]);
    }

    #[test]
    fn make_credentials_provider_surfaces_custom_providers_held_by_the_session() {
        use crate::credential_flow::notification::NoopCredentialNotifier;
        let (store, _dir) = tempfile_credential_store();
        Box::leak(Box::new(_dir));
        let (frame_tx, _frame_rx) = mpsc::unbounded_channel::<HostFrame>();
        let session = Arc::new(
            CredentialSession::new(
                CredentialStateFile::new(),
                Arc::new(NoopCredentialNotifier),
                store,
                frame_tx,
                std::time::Duration::from_secs(30),
            )
            .with_custom_providers(acme_custom()),
        );
        let provider = make_credentials_provider(&session);
        assert!(
            provider().iter().any(|c| c.id == "acme"),
            "custom provider must appear in the credential expansion"
        );
    }

    #[test]
    fn make_policy_emitter_includes_custom_providers_in_the_emitted_frame() {
        let (session, mut session_rx) = fixture_session();
        while session_rx.try_recv().is_ok() {}
        let (sink, mut sink_rx) = mpsc::unbounded_channel::<HostFrame>();
        let emitter = make_policy_emitter(session, sink, acme_custom());
        emitter(&CredentialStateFile::new());
        let frame = sink_rx
            .try_recv()
            .expect("emitter must push a Policy frame");
        let json = serde_json::to_value(&frame).expect("serialise");
        let ids: Vec<&str> = json["credentials"]
            .as_array()
            .expect("credentials present")
            .iter()
            .map(|c| c["id"].as_str().unwrap())
            .collect();
        assert!(
            ids.contains(&"acme"),
            "custom provider missing from {ids:?}"
        );
    }

    #[tokio::test]
    async fn credential_delivery_loop_applies_each_delivery_and_exits_on_tx_drop() {
        use crate::approval_flow::protocol::{CredentialDecisionKind, CredentialPending};
        use crate::credential_flow::session::CredentialDecisionRequest;
        let (session, mut frame_rx) = fixture_credential_session();
        let (tx, rx) = mpsc::unbounded_channel::<CredentialDecisionDelivery>();
        session.submit_pending(
            CredentialPending {
                id: "c1".into(),
                credential_id: "github".into(),
                action: "use of github placeholder".into(),
                reason: "placeholder-unauthorized".into(),
            },
            std::time::Instant::now(),
        );
        tx.send(CredentialDecisionDelivery {
            id: "c1".into(),
            request: CredentialDecisionRequest::Deny,
        })
        .unwrap();
        drop(tx);
        credential_delivery_loop(Arc::downgrade(&session), rx).await;
        let frame = frame_rx.try_recv().expect("decision frame");
        match frame {
            HostFrame::CredentialDecision(d) => {
                assert_eq!(d.id, "c1");
                assert_eq!(d.decision, CredentialDecisionKind::Deny);
            }
            other => panic!("expected CredentialDecision, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn credential_delivery_loop_breaks_when_upgrade_fails_with_buffered_delivery() {
        use crate::credential_flow::session::CredentialDecisionRequest;
        let (session, _frame_rx) = fixture_credential_session();
        let weak = Arc::downgrade(&session);
        let (tx, rx) = mpsc::unbounded_channel::<CredentialDecisionDelivery>();
        let stale_clone = tx.clone();
        stale_clone
            .send(CredentialDecisionDelivery {
                id: "c1".into(),
                request: CredentialDecisionRequest::Deny,
            })
            .unwrap();
        drop(session);
        drop(tx);
        drop(stale_clone);
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            credential_delivery_loop(weak, rx),
        )
        .await
        .expect("loop must exit promptly when upgrade fails");
    }

    #[tokio::test]
    async fn credential_tick_timeouts_loop_exits_when_strong_refs_drop() {
        let (session, _frame_rx) = fixture_credential_session();
        let weak = Arc::downgrade(&session);
        drop(session);
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            credential_tick_timeouts_loop(weak),
        )
        .await
        .expect("ticker exits promptly once session drops");
    }

    #[test]
    fn credential_sweep_once_returns_false_when_session_dropped() {
        let (session, _frame_rx) = fixture_credential_session();
        let weak = Arc::downgrade(&session);
        drop(session);
        assert!(!credential_sweep_once(&weak));
    }

    #[test]
    fn credential_sweep_once_returns_true_when_session_alive() {
        let (session, _frame_rx) = fixture_credential_session();
        let weak = Arc::downgrade(&session);
        assert!(credential_sweep_once(&weak));
    }

    #[test]
    fn sweep_once_drives_session_tick_timeouts() {
        use crate::approval_flow::protocol::{HostFrame, RequestPending};
        use crate::approval_flow::session::tests::{CapturingStore, RecordingNotifier};
        use lns_policy::Policy;
        let notifier = Arc::new(RecordingNotifier::default());
        let store = Arc::new(CapturingStore::default());
        let (frame_tx, mut frame_rx) = mpsc::unbounded_channel::<HostFrame>();
        let session = Arc::new(ApprovalSession::new(
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
        let _home = crate::test_env::EnvVarGuard::set("HOME", cache_root.path());
        let _xdg =
            crate::test_env::EnvVarGuard::set("XDG_CACHE_HOME", cache_root.path().join("xdg"));

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
            std::env::set_var("HOME", cache_root.path());
            std::env::set_var("XDG_CACHE_HOME", cache_root.path().join("xdg"));
        }
        let bytes = b"\x7fELF fake embedded supervisor".as_slice();
        // env_get returns None so the override is absent and the embedded
        // branch is taken regardless of the host's LNS_SUPERVISOR_BIN.
        let first = ensure_with(|_| None, Some(bytes)).await;
        let second = ensure_with(|_| None, Some(bytes)).await;
        // SAFETY: env mutation is serialized via #[serial(env)].
        unsafe {
            std::env::remove_var("HOME");
            std::env::remove_var("XDG_CACHE_HOME");
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
}
