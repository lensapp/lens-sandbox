use cucumber::{given, then, when};
use std::time::Instant;

use crate::credential_rig::{CredentialRig, RigPkce};
use crate::world::BehaviourWorld;
use lns_service::approval_flow::protocol::{CredentialDecisionKind, CredentialPending, HostFrame};
use lns_service::credential_flow::store::CredentialEntry;

fn prompt_id(credential_id: &str) -> String {
    format!("cred-{credential_id}")
}

fn last_decision_kind(world: &mut BehaviourWorld) -> Option<CredentialDecisionKind> {
    let mut kind = None;
    while let Ok(frame) = world.credential().frames.try_recv() {
        if let HostFrame::CredentialDecision(d) = frame {
            kind = Some(d.decision);
        }
    }
    kind
}

#[given(
    regex = r#"^an unconnected "([^"]+)" oauth connector using the pkce flow whose sign-in will complete$"#
)]
fn given_pkce_completes(world: &mut BehaviourWorld, id: String) {
    world.credential = Some(CredentialRig::pkce(&id, RigPkce::Completes));
    world.oauth_id = Some(id);
}

#[given(regex = r#"^an unconnected "([^"]+)" oauth connector using the pkce flow$"#)]
fn given_pkce_cancellable(world: &mut BehaviourWorld, id: String) {
    world.credential = Some(CredentialRig::pkce(&id, RigPkce::Cancelled));
    world.oauth_id = Some(id);
    // The sign-in blocks on the loopback callback, so the accept step must spawn it for the cancel step to fire mid-flight.
    world.spawn_connect = true;
}

#[given(regex = r#"^an unconnected "([^"]+)" oauth connector whose callback never arrives$"#)]
fn given_pkce_never_arrives(world: &mut BehaviourWorld, id: String) {
    world.credential = Some(CredentialRig::pkce(&id, RigPkce::NeverArrives));
    world.oauth_id = Some(id);
}

#[given(regex = r#"^an unconnected "([^"]+)" oauth connector whose code exchange will fail$"#)]
fn given_pkce_exchange_fails(world: &mut BehaviourWorld, id: String) {
    world.credential = Some(CredentialRig::pkce(&id, RigPkce::ExchangeFails));
    world.oauth_id = Some(id);
}

#[then("the browser is opened to the authorization page")]
fn then_browser_opened(world: &mut BehaviourWorld) -> Result<(), String> {
    let opened = world.credential().opened.lock().unwrap();
    match opened.first() {
        Some(url) if url.contains("/auth?") && url.contains("code_challenge=") => Ok(()),
        other => Err(format!(
            "expected an authorization URL to be opened, got {other:?}"
        )),
    }
}

#[then(regex = r#"^a credential is stored for "([^"]+)"$"#)]
fn then_credential_stored(world: &mut BehaviourWorld, id: String) -> Result<(), String> {
    match world.credential().session.current_state().get(&id) {
        Some(CredentialEntry::Stored { value }) if !value.is_empty() => Ok(()),
        other => Err(format!(
            "expected a durable stored credential for {id}, got {other:?}"
        )),
    }
}

#[then(regex = r#"^no credential is stored for "([^"]+)"$"#)]
fn then_no_credential_stored(world: &mut BehaviourWorld, id: String) -> Result<(), String> {
    if world.credential().session.current_state().contains_key(&id) {
        return Err(format!("expected no stored credential for {id}"));
    }
    Ok(())
}

#[when("the developer cancels the sign-in")]
async fn when_cancels_the_sign_in(world: &mut BehaviourWorld) {
    let id = world.oauth_id.clone().expect("a pkce id must be set");
    let window_state = world.credential().window_state.clone();
    // Wait for the spawned sign-in to present its card, then fire the card's Cancel.
    for _ in 0..100_000 {
        if window_state
            .snapshot()
            .sign_ins
            .iter()
            .any(|c| c.credential_id == id)
        {
            window_state.cancel_sign_in(&id);
            break;
        }
        tokio::task::yield_now().await;
    }
    if let Some(task) = world.connect_task.take() {
        let _ = task.await;
    }
}

#[given(regex = r#"^"([^"]+)" was connected and its credential stored$"#)]
fn given_pkce_connected(world: &mut BehaviourWorld, id: String) {
    world.credential = Some(CredentialRig::pkce_connected(&id));
    world.oauth_id = Some(id);
}

#[when(regex = r#"^a new run starts and a workload request carries the "([^"]+)" placeholder$"#)]
fn when_new_run_carries_placeholder(world: &mut BehaviourWorld, id: String) {
    // The pkce_connected rig injects this connector on api.some-pkce.example, so the request must target that host.
    let pending = CredentialPending {
        id: prompt_id(&id),
        credential_id: id,
        action: "GET https://api.some-pkce.example/".into(),
        reason: "placeholder-unauthorized".into(),
    };
    world
        .credential()
        .session
        .submit_pending(pending, Instant::now());
}

#[then("the request is injected without a sign-in prompt")]
fn then_injected_without_prompt(world: &mut BehaviourWorld) -> Result<(), String> {
    let snap = world.credential().window_state.snapshot();
    if !snap.sign_ins.is_empty() {
        return Err("a stored pkce key must not raise a sign-in card".into());
    }
    if !snap.pending_credentials.is_empty() {
        return Err("a stored pkce key must not raise a consent card".into());
    }
    match last_decision_kind(world) {
        Some(CredentialDecisionKind::Allow) => Ok(()),
        other => Err(format!("expected the request to be allowed, got {other:?}")),
    }
}
