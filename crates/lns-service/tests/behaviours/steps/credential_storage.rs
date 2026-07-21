use cucumber::{given, then, when};
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use crate::credential_rig::{FIXTURE_ID, TestKeychainBlob};
use crate::world::BehaviourWorld;
use lns_policy::keychain::{BackendKind, KeychainBlob, select_credential_store};
use lns_service::credential_flow::session::CredentialDecisionRequest;
use lns_service::credential_flow::store::{
    CredentialEntry, CredentialStateFile, CredentialStore, JsonFileCredentialStore,
};

const FORCED_PATH: &str = "/tmp/lns-forced-creds.json";

#[derive(Debug)]
pub struct EnvGuard {
    key: String,
    prev: Option<std::ffi::OsString>,
}

impl EnvGuard {
    pub fn set(key: &str, value: &str) -> Self {
        let prev = std::env::var_os(key);
        // SAFETY: the credential_storage feature is @serial, so no other scenario races this env write.
        unsafe { std::env::set_var(key, value) };
        Self {
            key: key.into(),
            prev,
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: same @serial isolation as EnvGuard::set.
        unsafe {
            match &self.prev {
                Some(v) => std::env::set_var(&self.key, v),
                None => std::env::remove_var(&self.key),
            }
        }
    }
}

fn captured_warnings(f: impl FnOnce()) -> String {
    #[derive(Clone)]
    struct Buf(Arc<std::sync::Mutex<Vec<u8>>>);
    impl io::Write for Buf {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let buf = Buf(Arc::new(std::sync::Mutex::new(Vec::new())));
    let writer = buf.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::WARN)
        .finish();
    tracing::subscriber::with_default(subscriber, f);
    let bytes = buf.0.lock().unwrap().clone();
    String::from_utf8_lossy(&bytes).into_owned()
}

#[given("the OS keychain is reachable")]
fn given_keychain_reachable(world: &mut BehaviourWorld) {
    world.keychain_unreachable = false;
    world.credential_keychain();
}

#[given("no OS keychain is reachable")]
fn given_keychain_unreachable(world: &mut BehaviourWorld) {
    world.keychain_unreachable = true;
}

/// Drops any prior guard first so its uninstall cannot wipe the fresh install; the world holds the new guard until the scenario ends.
fn hold_installed_backend(
    world: &mut BehaviourWorld,
    selection: lns_policy::keychain::StoreSelection,
) {
    drop(world.backend_guard.take());
    world.backend_guard = Some(lns_service::credential_flow::backend::install(selection));
}

fn install_rig_backend(world: &mut BehaviourWorld) {
    let rig = world.credential_keychain();
    let blob = rig
        .keychain_blob
        .clone()
        .expect("keychain rig carries a blob");
    let session = rig.session.clone();
    hold_installed_backend(
        world,
        lns_policy::keychain::StoreSelection {
            store: Arc::new(lns_policy::keychain::KeychainCredentialStore::new(blob)),
            kind: BackendKind::Keychain,
            file_path: None,
            fallback_reason: None,
        },
    );
    lns_service::credential_flow::live::register(&session);
}

#[given("the keychain backend is active")]
fn given_keychain_backend_active(world: &mut BehaviourWorld) {
    install_rig_backend(world);
}

#[given("the keychain backend is active with no stored state")]
fn given_keychain_backend_active_empty(world: &mut BehaviourWorld) {
    install_rig_backend(world);
    let rig = world.credential();
    assert!(rig.session.current_state().is_empty());
}

#[given(regex = r#"^a workload is running with an unconnected "([^"]+)" integration$"#)]
fn given_unconnected_integration(world: &mut BehaviourWorld, integration_id: String) {
    let rig = world.credential();
    assert!(
        !rig.session.current_state().contains_key(&integration_id),
        "expected no value decision for {integration_id} yet"
    );
}

#[when(regex = r#"^a device sign-in for "([^"]+)" completes$"#)]
fn when_device_sign_in_completes(world: &mut BehaviourWorld, integration_id: String) {
    let _ = world.credential();
    lns_service::credential_flow::backend::persist_entry(
        &integration_id,
        CredentialEntry::Oauth {
            access_token: "some-access".into(),
            refresh_token: "some-refresh".into(),
            expires_at: 1_900_000_000,
            scopes: Vec::new(),
            account: None,
        },
    )
    .expect("persist the token set through the active backend");
}

#[then("the running session arms the some-oauth token set without a restart")]
fn then_session_armed_without_restart(world: &mut BehaviourWorld) -> Result<(), String> {
    let rig = world.credential();
    match rig.session.current_state().get("some-oauth") {
        Some(CredentialEntry::Oauth { .. }) => Ok(()),
        other => Err(format!("expected an armed oauth token set, got {other:?}")),
    }
}

#[then("the token set lands in the OS keychain")]
fn then_token_set_in_keychain(world: &mut BehaviourWorld) -> Result<(), String> {
    let rig = world.credential();
    let blob = rig
        .keychain_blob
        .as_ref()
        .ok_or("rig is not keychain-backed")?;
    let item = blob
        .data
        .lock()
        .unwrap()
        .clone()
        .ok_or("no keychain item was written")?;
    let state: CredentialStateFile =
        serde_json::from_str(&item).map_err(|e| format!("keychain item is not the map: {e}"))?;
    match state.get("some-oauth") {
        Some(CredentialEntry::Oauth { .. }) => Ok(()),
        other => Err(format!("expected the oauth token set, got {other:?}")),
    }
}

#[given(regex = r#"^LNS_CREDENTIALS_PATH points at a custom path$"#)]
fn given_credentials_path_override(world: &mut BehaviourWorld) {
    world.env_guard = Some(EnvGuard::set("LNS_CREDENTIALS_PATH", FORCED_PATH));
}

#[given(regex = r#"^a plaintext credentials file holds a "stored" entry for "([^"]+)"$"#)]
fn given_plaintext_file_with_entry(world: &mut BehaviourWorld, credential_id: String) {
    let rig = world.credential();
    let mut state = CredentialStateFile::new();
    state.insert(
        credential_id,
        CredentialEntry::Stored {
            value: "plaintext-era-value".into(),
        },
    );
    JsonFileCredentialStore::new(rig.credentials_path.clone())
        .save(&state)
        .expect("seed plaintext credentials file");
}

#[when("the service selects its credential backend")]
fn when_service_selects_backend(world: &mut BehaviourWorld) {
    let probe = Arc::new(AtomicBool::new(false));
    world.keychain_probe_invoked = Some(probe.clone());
    let unreachable = world.keychain_unreachable;
    let scratch_home = tempfile::TempDir::new()
        .expect("scratch HOME so the fallback default path never points at the real home");
    let _home = EnvGuard::set("HOME", &scratch_home.path().display().to_string());
    drop(world.backend_guard.take());
    let mut chosen: Option<(BackendKind, Option<std::path::PathBuf>)> = None;
    let mut installed = None;
    let warning = captured_warnings(|| {
        let selection = select_credential_store(move || {
            probe.store(true, Ordering::SeqCst);
            if unreachable {
                Err(io::Error::new(
                    io::ErrorKind::ConnectionRefused,
                    "no secret service reachable",
                ))
            } else {
                Ok(Arc::new(TestKeychainBlob::default()) as Arc<dyn KeychainBlob>)
            }
        });
        chosen = Some((selection.kind, selection.file_path.clone()));
        installed = Some(lns_service::credential_flow::backend::install(selection));
    });
    world.backend_guard = installed;
    let (kind, path) = chosen.expect("selection ran");
    world.selected_backend = Some(kind);
    world.selected_file_path = path;
    world.selection_warning = Some(warning);
}

#[when(regex = r#"^the developer stores a value for "([^"]+)" at the credential card$"#)]
fn when_developer_stores_value(world: &mut BehaviourWorld, credential_id: String) {
    let rig = world.credential();
    rig.session.submit_pending(
        lns_service::approval_flow::protocol::CredentialPending {
            id: format!("cred-{credential_id}"),
            credential_id: credential_id.clone(),
            action: format!("use of {credential_id} placeholder"),
            reason: "placeholder-unauthorized".into(),
        },
        Instant::now(),
    );
    let card_id = rig
        .window_state
        .snapshot()
        .pending_credentials
        .last()
        .expect("credential card visible")
        .id
        .clone();
    rig.session.record_decision(
        &card_id,
        CredentialDecisionRequest::Allow(CredentialEntry::Stored {
            value: "typed-value".into(),
        }),
    );
}

#[then("the credential state lands in the OS keychain as a single item")]
fn then_state_lands_in_keychain(world: &mut BehaviourWorld) -> Result<(), String> {
    let rig = world.credential();
    let blob = rig
        .keychain_blob
        .as_ref()
        .ok_or("rig is not keychain-backed")?;
    let item = blob
        .data
        .lock()
        .unwrap()
        .clone()
        .ok_or("no keychain item was written")?;
    let state: CredentialStateFile =
        serde_json::from_str(&item).map_err(|e| format!("keychain item is not the map: {e}"))?;
    match state.get(FIXTURE_ID) {
        Some(CredentialEntry::Stored { .. }) => Ok(()),
        other => Err(format!(
            "expected a stored entry for {FIXTURE_ID}, got {other:?}"
        )),
    }
}

#[then("no plaintext credentials file is written")]
fn then_no_plaintext_file(world: &mut BehaviourWorld) -> Result<(), String> {
    let rig = world.credential();
    if rig.credentials_path.exists() {
        return Err(format!(
            "expected no file at {}, but one exists",
            rig.credentials_path.display()
        ));
    }
    Ok(())
}

#[then("the plaintext JSON file backend is selected")]
fn then_file_backend_selected(world: &mut BehaviourWorld) -> Result<(), String> {
    match world.selected_backend {
        Some(BackendKind::File) => Ok(()),
        other => Err(format!("expected the file backend, got {other:?}")),
    }
}

#[then("the plaintext JSON file backend at that path is selected")]
fn then_file_backend_at_path_selected(world: &mut BehaviourWorld) -> Result<(), String> {
    then_file_backend_selected(world)?;
    match world.selected_file_path.as_deref() {
        Some(p) if p == std::path::Path::new(FORCED_PATH) => Ok(()),
        other => Err(format!("expected {FORCED_PATH}, got {other:?}")),
    }
}

#[then("a warning notes that credential values will be stored in plaintext")]
fn then_plaintext_warning(world: &mut BehaviourWorld) -> Result<(), String> {
    let warning = world.selection_warning.as_deref().unwrap_or("");
    if warning.contains("plaintext") {
        Ok(())
    } else {
        Err(format!("no plaintext warning captured, got: {warning:?}"))
    }
}

#[then("the OS keychain is never probed")]
fn then_keychain_never_probed(world: &mut BehaviourWorld) -> Result<(), String> {
    let probed = world
        .keychain_probe_invoked
        .as_ref()
        .ok_or("selection step never ran")?
        .load(Ordering::SeqCst);
    if probed {
        Err("the keychain factory was invoked despite LNS_CREDENTIALS_PATH".into())
    } else {
        Ok(())
    }
}

#[then("the plaintext credentials file is left untouched")]
fn then_plaintext_file_untouched(world: &mut BehaviourWorld) -> Result<(), String> {
    let rig = world.credential();
    let on_disk = JsonFileCredentialStore::new(rig.credentials_path.clone())
        .load()
        .map_err(|e| e.to_string())?;
    match on_disk.get(FIXTURE_ID) {
        Some(CredentialEntry::Stored { value }) if value == "plaintext-era-value" => Ok(()),
        other => Err(format!(
            "expected the seeded plaintext entry to survive, got {other:?}"
        )),
    }
}

#[given("the file backend is active")]
fn given_file_backend_active(world: &mut BehaviourWorld) {
    let rig = world.credential();
    let path = rig.credentials_path.clone();
    let session = rig.session.clone();
    hold_installed_backend(
        world,
        lns_policy::keychain::StoreSelection {
            store: Arc::new(JsonFileCredentialStore::new(path.clone())),
            kind: BackendKind::File,
            file_path: Some(path),
            fallback_reason: None,
        },
    );
    lns_service::credential_flow::live::register(&session);
}

#[given(regex = r#"^a workload is running with a "stored" credential rule for "([^"]+)"$"#)]
fn given_stored_rule_via_backend(world: &mut BehaviourWorld, credential_id: String) {
    let _ = world.credential();
    lns_service::credential_flow::backend::persist_entry(
        &credential_id,
        CredentialEntry::Stored {
            value: "real-stored-value".into(),
        },
    )
    .expect("persist the stored rule through the active backend");
    let rig = world.credential();
    assert!(
        rig.session.current_state().contains_key(&credential_id),
        "the stored rule must reach the running session via broadcast"
    );
}

#[when(regex = r#"^the developer revokes the "([^"]+)" credential$"#)]
async fn when_developer_revokes(world: &mut BehaviourWorld, credential_id: String) {
    let response = crate::runner::run_one_shot(
        &lns_ipc::Request::RevokeIntegration {
            id: credential_id.clone(),
        },
        world.started_at(),
    )
    .await;
    assert_eq!(
        response,
        lns_ipc::Response::IntegrationRevoked { existed: true },
        "expected the revoke to clear an existing decision"
    );
}

#[then(regex = r#"^the "([^"]+)" entry is removed from the credential state$"#)]
fn then_entry_removed(world: &mut BehaviourWorld, credential_id: String) -> Result<(), String> {
    let stored = lns_service::credential_flow::backend::store()
        .load()
        .map_err(|e| e.to_string())?;
    if stored.contains_key(&credential_id) {
        return Err(format!("{credential_id} still present in the stored state"));
    }
    let rig = world.credential();
    if rig.session.current_state().contains_key(&credential_id) {
        return Err(format!(
            "{credential_id} still armed in the running session"
        ));
    }
    Ok(())
}

#[then("a subsequent request carrying the some-provider placeholder fires a fresh credential card")]
fn then_subsequent_request_fresh_card(world: &mut BehaviourWorld) -> Result<(), String> {
    let rig = world.credential();
    rig.session.submit_pending(
        lns_service::approval_flow::protocol::CredentialPending {
            id: "cred-after-revoke".into(),
            credential_id: FIXTURE_ID.into(),
            action: format!("use of {FIXTURE_ID} placeholder"),
            reason: "placeholder-unauthorized".into(),
        },
        Instant::now(),
    );
    let snap = rig.window_state.snapshot();
    if snap
        .pending_credentials
        .iter()
        .all(|c| c.credential_id != FIXTURE_ID)
    {
        return Err(format!("no fresh credential card for {FIXTURE_ID}"));
    }
    Ok(())
}

#[when("a status request is served")]
async fn when_status_request_served(world: &mut BehaviourWorld) {
    let response = crate::runner::run_one_shot(&lns_ipc::Request::Status, world.started_at()).await;
    world.response = Some(response);
}

#[then("the response names the OS keychain as the credential backend")]
fn then_response_names_keychain(world: &mut BehaviourWorld) -> Result<(), String> {
    match world.response.as_ref() {
        Some(lns_ipc::Response::Status(info)) => match info.credential_backend {
            Some(lns_ipc::CredentialBackendKind::OsKeychain) => Ok(()),
            other => Err(format!("expected the OS keychain backend, got {other:?}")),
        },
        other => Err(format!("expected a Status response, got {other:?}")),
    }
}

#[given("the keychain backend is active holding a corrupt item")]
fn given_keychain_backend_corrupt(world: &mut BehaviourWorld) {
    install_rig_backend(world);
    let rig = world.credential();
    let blob = rig.keychain_blob.as_ref().expect("keychain rig");
    *blob.data.lock().unwrap() = Some("{ not json".into());
    assert!(
        lns_service::credential_flow::backend::store()
            .load()
            .is_err(),
        "the corrupt item must not load"
    );
}

#[when("the developer revokes all credentials")]
async fn when_developer_revokes_all(world: &mut BehaviourWorld) {
    let response =
        crate::runner::run_one_shot(&lns_ipc::Request::RevokeAllIntegrations, world.started_at())
            .await;
    assert_eq!(response, lns_ipc::Response::AllIntegrationsRevoked);
}

#[then("the credential state loads back empty")]
fn then_state_loads_back_empty(world: &mut BehaviourWorld) -> Result<(), String> {
    let stored = lns_service::credential_flow::backend::store()
        .load()
        .map_err(|e| format!("the repaired store must load: {e}"))?;
    if !stored.is_empty() {
        return Err(format!("expected empty state, got {stored:?}"));
    }
    let rig = world.credential();
    if !rig.session.current_state().is_empty() {
        return Err("the running session must be disarmed by the reset".into());
    }
    Ok(())
}
