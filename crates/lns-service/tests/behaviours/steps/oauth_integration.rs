use cucumber::{given, then, when};
use std::time::Instant;

use crate::credential_rig::{CredentialRig, RigSignIn};
use crate::world::BehaviourWorld;
use lns_service::approval_flow::protocol::{CredentialDecisionKind, CredentialPending, HostFrame};
use lns_service::credential_flow::store::CredentialEntry;

fn prompt_id(credential_id: &str) -> String {
    format!("cred-{credential_id}")
}

fn make_pending(credential_id: &str) -> CredentialPending {
    CredentialPending {
        id: prompt_id(credential_id),
        credential_id: credential_id.to_string(),
        action: format!("use of {credential_id} placeholder"),
        reason: "placeholder-unauthorized".into(),
    }
}

fn last_decision_kind(rig: &mut CredentialRig) -> Option<CredentialDecisionKind> {
    let mut kind = None;
    while let Ok(frame) = rig.frames.try_recv() {
        if let HostFrame::CredentialDecision(d) = frame {
            kind = Some(d.decision);
        }
    }
    kind
}

#[given(
    regex = r#"^an unconnected "([^"]+)" oauth integration whose sign-in will (complete|expire|be denied)$"#
)]
fn given_unconnected_oauth(world: &mut BehaviourWorld, id: String, outcome: String) {
    let outcome = match outcome.as_str() {
        "complete" => RigSignIn::Completes,
        "expire" => RigSignIn::Expires,
        _ => RigSignIn::Denied,
    };
    world.credential = Some(CredentialRig::oauth(&id, outcome));
    world.oauth_id = Some(id);
}

#[when(regex = r#"^a workload request carries the "([^"]+)" placeholder$"#)]
fn when_request_carries_placeholder(world: &mut BehaviourWorld, id: String) {
    world
        .credential()
        .session
        .submit_pending(make_pending(&id), Instant::now());
}

#[then(regex = r#"^the request is held and a "connect to ([^"]+)" prompt is presented$"#)]
fn then_held_with_connect_prompt(world: &mut BehaviourWorld, id: String) -> Result<(), String> {
    let rig = world.credential();
    let snap = rig.window_state.snapshot();
    let card = snap
        .pending_credentials
        .iter()
        .find(|c| c.credential_id == id)
        .ok_or_else(|| format!("no held prompt for {id}"))?;
    if card.action != format!("connect to {id}") {
        return Err(format!(
            "expected action 'connect to {id}', got {:?}",
            card.action
        ));
    }
    Ok(())
}

#[when("the developer accepts the prompt")]
async fn when_accepts_the_prompt(world: &mut BehaviourWorld) {
    let id = world.oauth_id.clone().expect("an oauth id must be set");
    let session = world.credential().session.clone();
    let pid = prompt_id(&id);
    if world.spawn_connect {
        world.connect_task = Some(tokio::spawn(async move {
            let _ = session.connect_oauth(&pid).await;
        }));
    } else {
        session.connect_oauth(&pid).await;
    }
}

#[then(regex = r#"^the "([^"]+)" integration is connected live$"#)]
fn then_connected_live(world: &mut BehaviourWorld, id: String) -> Result<(), String> {
    if !world.credential().connected.lock().unwrap().contains(&id) {
        return Err(format!("{id} was not connected live"));
    }
    Ok(())
}

#[then(regex = r#"^the "([^"]+)" integration is not connected$"#)]
fn then_not_connected(world: &mut BehaviourWorld, id: String) -> Result<(), String> {
    if world.credential().connected.lock().unwrap().contains(&id) {
        return Err(format!("{id} must not be connected after a failed sign-in"));
    }
    Ok(())
}

#[then(regex = r#"^a token set is stored for "([^"]+)"$"#)]
fn then_token_stored(world: &mut BehaviourWorld, id: String) -> Result<(), String> {
    match world.credential().session.current_state().get(&id) {
        Some(CredentialEntry::Oauth { .. }) => Ok(()),
        other => Err(format!(
            "expected an oauth token set for {id}, got {other:?}"
        )),
    }
}

#[then(regex = r#"^no token set is stored for "([^"]+)"$"#)]
fn then_no_token_stored(world: &mut BehaviourWorld, id: String) -> Result<(), String> {
    if world.credential().session.current_state().contains_key(&id) {
        return Err(format!("expected no stored entry for {id}"));
    }
    Ok(())
}

#[then("the held request is released for injection")]
fn then_released(world: &mut BehaviourWorld) -> Result<(), String> {
    match last_decision_kind(world.credential()) {
        Some(CredentialDecisionKind::Allow) => Ok(()),
        other => Err(format!("expected an Allow decision, got {other:?}")),
    }
}

#[then("the held request is failed at the boundary")]
fn then_failed(world: &mut BehaviourWorld) -> Result<(), String> {
    match last_decision_kind(world.credential()) {
        Some(CredentialDecisionKind::Deny) => Ok(()),
        other => Err(format!("expected a Deny decision, got {other:?}")),
    }
}
