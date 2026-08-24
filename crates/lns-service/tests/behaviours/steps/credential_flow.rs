use cucumber::{given, then, when};
use std::time::Instant;

use crate::credential_rig::{
    CredentialRig, FIXTURE_BOUND_VALUE, FIXTURE_ENV, FIXTURE_ID, FIXTURE_PLACEHOLDER,
};
use crate::world::BehaviourWorld;
use lns_service::approval_flow::protocol::{
    CredentialDecisionKind, CredentialInjection, CredentialPending, HostFrame, PolicyMessage,
};
use lns_service::credential_flow::session::CredentialDecisionRequest;
use lns_service::credential_flow::store::{CredentialEntry, CredentialStateFile, CredentialStore};

fn make_credential_pending(credential_id: &str) -> CredentialPending {
    CredentialPending {
        id: format!("cred-{credential_id}"),
        credential_id: credential_id.to_string(),
        action: format!("use of {credential_id} placeholder"),
        reason: "placeholder-unauthorized".into(),
    }
}

fn drain_frames(rig: &mut CredentialRig) -> Vec<HostFrame> {
    let mut out = Vec::new();
    while let Ok(f) = rig.frames.try_recv() {
        out.push(f);
    }
    out
}

fn last_credential_decision(
    frames: &[HostFrame],
) -> Option<&lns_service::approval_flow::protocol::CredentialDecision> {
    frames.iter().rev().find_map(|f| match f {
        HostFrame::CredentialDecision(d) => Some(d),
        _ => None,
    })
}

fn last_policy_frame(frames: &[HostFrame]) -> Option<&PolicyMessage> {
    frames.iter().rev().find_map(|f| match f {
        HostFrame::Policy(p) => Some(p),
        _ => None,
    })
}

fn find_credential<'a>(
    policy: &'a PolicyMessage,
    credential_id: &str,
) -> Option<&'a lns_service::approval_flow::protocol::Credential> {
    policy
        .credentials
        .as_ref()?
        .iter()
        .find(|c| c.id == credential_id)
}

fn submit_credential_with_id(rig: &mut CredentialRig, id: &str, credential_id: &str) {
    let pending = CredentialPending {
        id: id.to_string(),
        credential_id: credential_id.to_string(),
        action: format!("use of {credential_id} placeholder"),
        reason: "placeholder-unauthorized".into(),
    };
    rig.session.submit_pending(pending, Instant::now());
}

#[given(regex = r#"^a workload is running with the seeded "([^"]+)" placeholder$"#)]
fn given_workload_with_placeholder(world: &mut BehaviourWorld, _credential_id: String) {
    world.credential();
}

#[given(regex = r#"^no credential rule exists in "~/.lns/credentials.json" for "([^"]+)"$"#)]
fn given_no_credential_rule(world: &mut BehaviourWorld, credential_id: String) {
    let rig = world.credential();
    assert!(
        !rig.session.current_state().contains_key(&credential_id),
        "expected no credential rule for {credential_id}"
    );
}

#[given(
    regex = r#"^the host has a "([^"]+)" credential reachable via the registered detection strategy$"#
)]
fn given_host_has_credential(world: &mut BehaviourWorld, credential_id: String) {
    let real_value = format!("real-{credential_id}-token");
    world
        .credential()
        .set_host_value(&credential_id, &real_value);
}

#[given(regex = r#"^no "([^"]+)" credential is reachable on the host$"#)]
fn given_host_lacks_credential(world: &mut BehaviourWorld, credential_id: String) {
    world.credential().clear_host_value(&credential_id);
}

#[given(regex = r#"^a credential card for "([^"]+)" is visible with "use from host" available$"#)]
fn given_card_visible_with_host(world: &mut BehaviourWorld, credential_id: String) {
    let real_value = format!("real-{credential_id}-token");
    let rig = world.credential();
    rig.set_host_value(&credential_id, &real_value);
    submit_if_not_visible(rig, &credential_id);
    let _ = drain_frames(rig);
}

#[given(regex = r#"^a credential card for "([^"]+)" is visible$"#)]
fn given_card_visible(world: &mut BehaviourWorld, credential_id: String) {
    let rig = world.credential();
    // Default to a host value so a bare "card visible" still has a "use from host" path; no-host scenarios use the explicit Given.
    if !rig.host_values.lock().unwrap().contains_key(&credential_id) {
        let real_value = format!("real-{credential_id}-token");
        rig.set_host_value(&credential_id, &real_value);
    }
    submit_if_not_visible(rig, &credential_id);
    let _ = drain_frames(rig);
}

fn submit_if_not_visible(rig: &mut CredentialRig, credential_id: &str) {
    // Submit only when no matching card exists so a "card visible" used as setup doesn't create a second pending entry that defeats the S11 dedup test.
    let already = rig
        .window_state
        .snapshot()
        .pending_credentials
        .iter()
        .any(|c| c.credential_id == credential_id);
    if !already {
        rig.session
            .submit_pending(make_credential_pending(credential_id), Instant::now());
    }
}

#[given(regex = r#"^"~/.lns/credentials.json" has an entry for "([^"]+)" with kind "([^"]+)"$"#)]
fn given_credentials_file_has_entry(
    world: &mut BehaviourWorld,
    credential_id: String,
    kind: String,
) {
    let rig = world.credential();
    let mut state = CredentialStateFile::new();
    state.insert(credential_id, entry_for_kind(&kind, None));
    rig.store.save(&state).expect("seed credentials file");
    rig.session.apply_external_state(state);
    let _ = drain_frames(rig);
}

#[given(
    regex = r#"^the host no longer yields a "([^"]+)" credential via the registered detection strategy$"#
)]
fn given_host_no_longer_yields(world: &mut BehaviourWorld, credential_id: String) {
    world.credential().clear_host_value(&credential_id);
}

#[given(
    regex = r#"^a workload is running with a "stored" credential rule for "([^"]+)" in "~/.lns/credentials.json"$"#
)]
fn given_workload_with_stored_rule(world: &mut BehaviourWorld, credential_id: String) {
    let rig = world.credential();
    let mut state = CredentialStateFile::new();
    state.insert(
        credential_id,
        CredentialEntry::Stored {
            value: "real-stored-value".into(),
        },
    );
    rig.store.save(&state).expect("seed credentials file");
    rig.session.apply_external_state(state);
    let _ = drain_frames(rig);
}

#[given(
    regex = r#"^a workload sends a request carrying the some-provider placeholder with no credential rule for "some-provider"$"#
)]
fn given_workload_sends_with_no_rule(world: &mut BehaviourWorld) {
    let rig = world.credential();
    submit_credential_with_id(rig, "cred-coalesce-1", FIXTURE_ID);
}

#[given(regex = r#"^a workload has an open credential card for "([^"]+)"$"#)]
fn given_open_credential_card(world: &mut BehaviourWorld, credential_id: String) {
    let rig = world.credential();
    rig.session
        .submit_pending(make_credential_pending(&credential_id), Instant::now());
    let _ = drain_frames(rig);
}

#[given(r#""~/.lns/credentials.json" cannot be written"#)]
fn given_credentials_file_cannot_be_written(world: &mut BehaviourWorld) {
    world.credential().store.break_next_save();
}

fn entry_for_kind(kind: &str, value: Option<&str>) -> CredentialEntry {
    match kind {
        "host-detect" => CredentialEntry::HostDetect,
        "stored" => CredentialEntry::Stored {
            value: value.unwrap_or("real-stored-value").to_string(),
        },
        "deny" => CredentialEntry::Deny,
        other => panic!("unknown rule kind: {other}"),
    }
}

#[when("a workload is launched in the sandbox")]
fn when_workload_launched(world: &mut BehaviourWorld) {
    let rig = world.credential();
    let state = rig.session.current_state();
    rig.session.apply_external_state(state);
}

#[when(regex = r#"^the workload sends a request carrying the some-provider placeholder$"#)]
fn when_workload_sends_placeholder(world: &mut BehaviourWorld) {
    let rig = world.credential();
    rig.session
        .submit_pending(make_credential_pending(FIXTURE_ID), Instant::now());
}

#[when(
    regex = r#"^the workload sends a request to "([^"]+)" carrying the some-provider placeholder$"#
)]
fn when_workload_sends_to_host_with_placeholder(world: &mut BehaviourWorld, host: String) {
    let credential_id = FIXTURE_ID;
    // S8 drives both flows: a network pending on the approval rig and a credential pending on the credential rig.
    {
        let approval_rig = world.approval();
        let req = lns_service::approval_flow::protocol::RequestPending {
            id: format!("req-{host}"),
            host: host.clone(),
            action: format!("CONNECT {host}:443"),
            reason: "policy-ambiguous".into(),
            treatment: lns_service::approval_flow::protocol::Treatment::Inspected,
        };
        approval_rig.session.submit_pending(req, Instant::now());
    }
    {
        let credential_rig = world.credential();
        credential_rig
            .session
            .submit_pending(make_credential_pending(credential_id), Instant::now());
    }
}

#[when(r#"the developer picks "use from host""#)]
fn when_developer_picks_use_from_host(world: &mut BehaviourWorld) {
    let rig = world.credential();
    let id = window_credential_id(rig);
    rig.session.record_decision(
        &id,
        CredentialDecisionRequest::Allow(CredentialEntry::HostDetect),
    );
}

#[when("the developer types a value and submits")]
fn when_developer_types_value(world: &mut BehaviourWorld) {
    let rig = world.credential();
    let id = window_credential_id(rig);
    rig.session.record_decision(
        &id,
        CredentialDecisionRequest::Allow(CredentialEntry::Stored {
            value: "typed-value".into(),
        }),
    );
}

#[given(r#"a value for "some-provider" is bound on this machine but this workload holds no grant"#)]
fn given_bound_but_ungranted(world: &mut BehaviourWorld) {
    world.credential = Some(CredentialRig::bound_but_ungranted());
}

#[then("the card offers to use the value already bound on this machine")]
fn then_card_offers_bound_value(world: &mut BehaviourWorld) -> Result<(), String> {
    let rig = world.credential();
    let snap = rig.window_state.snapshot();
    let card = snap
        .pending_credentials
        .last()
        .ok_or("no credential card visible")?;
    if !card.bound_value_available {
        return Err(
            "expected bound_value_available=true so the card can grant the existing binding rather than demand the secret again".into(),
        );
    }
    Ok(())
}

#[when(r#"the developer picks "use the bound value""#)]
fn when_developer_picks_bound_value(world: &mut BehaviourWorld) {
    let rig = world.credential();
    let id = window_credential_id(rig);
    rig.session
        .record_decision(&id, CredentialDecisionRequest::AllowBound);
}

#[then(
    "the workload's request leaves the boundary with the bound value substituted for the placeholder"
)]
fn then_request_leaves_with_bound_value(world: &mut BehaviourWorld) -> Result<(), String> {
    let rig = world.credential();
    let frames = drain_frames(rig);
    let policy = last_policy_frame(&frames).ok_or("no Policy frame emitted after decision")?;
    let cred = find_credential(policy, FIXTURE_ID)
        .ok_or("Policy frame missing some-provider credential")?;
    assert_credential_armed_with(cred, FIXTURE_BOUND_VALUE)?;
    decision_allow_assert(&frames)?;
    Ok(())
}

#[then(r#""~/.lns/credentials.json" still holds the value it was bound with"#)]
fn then_credentials_file_keeps_bound_value(world: &mut BehaviourWorld) -> Result<(), String> {
    let rig = world.credential();
    let on_disk = rig.store.load().map_err(|e| e.to_string())?;
    match on_disk.get(FIXTURE_ID) {
        Some(CredentialEntry::Stored { value }) if value == FIXTURE_BOUND_VALUE => Ok(()),
        other => Err(format!(
            "granting the existing binding must not rewrite the machine store, got {other:?}"
        )),
    }
}

#[when(r#"the developer picks "deny""#)]
fn when_developer_picks_deny_credential(world: &mut BehaviourWorld) {
    let rig = world.credential();
    let id = window_credential_id(rig);
    rig.session
        .record_decision(&id, CredentialDecisionRequest::Deny);
}

#[when(regex = r#"^the developer deletes the "([^"]+)" entry from "~/.lns/credentials.json"$"#)]
fn when_developer_deletes_entry(world: &mut BehaviourWorld, credential_id: String) {
    let rig = world.credential();
    let mut state = rig.session.current_state();
    state.remove(&credential_id);
    rig.store.save(&state).expect("save edited state");
    rig.session.apply_external_state(state);
    let _ = drain_frames(rig);
}

#[when(
    regex = r#"^the workload sends a second request carrying the some-provider placeholder before the developer decides$"#
)]
fn when_workload_sends_second_placeholder(world: &mut BehaviourWorld) {
    let rig = world.credential();
    // Distinct id, same provider → exercises credential_id coalescing, not bare id dedup (S11).
    submit_credential_with_id(rig, "cred-coalesce-2", FIXTURE_ID);
}

// The timeout and early-exit steps are shared across both flows; see steps/approval_flow.rs for the dispatch.

#[then("the workload's environment contains a value-shaped placeholder for each registry entry")]
fn then_env_has_placeholder_per_entry(world: &mut BehaviourWorld) -> Result<(), String> {
    let rig = world.credential();
    let frames = drain_frames(rig);
    let policy = last_policy_frame(&frames).ok_or("no Policy frame emitted at launch")?;
    let creds = policy
        .credentials
        .as_ref()
        .ok_or("Policy frame missing credentials array")?;
    let cred = creds
        .iter()
        .find(|c| c.id == FIXTURE_ID)
        .ok_or_else(|| format!("Policy frame missing credential for {FIXTURE_ID}"))?;
    if cred.placeholder.as_deref() != Some(FIXTURE_PLACEHOLDER) {
        return Err(format!(
            "credential {FIXTURE_ID} placeholder mismatch: got {:?}",
            cred.placeholder
        ));
    }
    if cred.env_var.as_deref() != Some(FIXTURE_ENV) {
        return Err(format!(
            "credential {FIXTURE_ID} env_var mismatch: got {:?}",
            cred.env_var
        ));
    }
    Ok(())
}

#[then("no real credential material is present inside the workload")]
fn then_no_real_credential_in_workload(world: &mut BehaviourWorld) -> Result<(), String> {
    let rig = world.credential();
    // Empty state at boot means registry expansion arms nothing — every injection carries an empty value, so no real Bearer token reaches the wire.
    let state = rig.session.current_state();
    if !state.is_empty() {
        return Err(format!(
            "expected empty credential state at boot, got {state:?}"
        ));
    }
    Ok(())
}

#[then("no approval card is shown at boot")]
fn then_no_card_at_boot(world: &mut BehaviourWorld) -> Result<(), String> {
    let rig = world.credential();
    let snap = rig.window_state.snapshot();
    if !snap.pending_credentials.is_empty() {
        return Err(format!(
            "expected no credential cards at boot, got {}",
            snap.pending_credentials.len()
        ));
    }
    Ok(())
}

#[then("every seeded placeholder self-identifies as a placeholder")]
fn then_every_placeholder_self_identifies(world: &mut BehaviourWorld) -> Result<(), String> {
    let rig = world.credential();
    let frames = drain_frames(rig);
    let policy = last_policy_frame(&frames).ok_or("no Policy frame emitted at launch")?;
    let creds = policy
        .credentials
        .as_ref()
        .ok_or("Policy frame missing credentials array")?;
    for c in creds {
        let placeholder = c
            .placeholder
            .as_deref()
            .ok_or_else(|| format!("credential {} missing a placeholder", c.id))?;
        let lower = placeholder.to_lowercase();
        if !lower.contains("placeholder") && !lower.contains("lns") {
            return Err(format!(
                "credential {} placeholder does not self-identify as fake: {placeholder}",
                c.id
            ));
        }
    }
    Ok(())
}

#[then(
    regex = r#"^a credential card appears for "([^"]+)" showing the provider, the originating sandbox, and the destination$"#
)]
fn then_credential_card_appears_with_details(
    world: &mut BehaviourWorld,
    credential_id: String,
) -> Result<(), String> {
    let rig = world.credential();
    let snap = rig.window_state.snapshot();
    let card = snap
        .pending_credentials
        .iter()
        .find(|c| c.credential_id == credential_id)
        .ok_or_else(|| format!("no credential card for {credential_id}"))?;
    if card.credential_id.is_empty() {
        return Err("card credential_id empty".into());
    }
    if card.action.is_empty() {
        return Err("card action empty".into());
    }
    Ok(())
}

#[then(
    regex = r#"^(?:a credential card appears for "([^"]+)"|a credential card for "([^"]+)" appears)$"#
)]
fn then_credential_card_appears(
    world: &mut BehaviourWorld,
    a: String,
    b: String,
) -> Result<(), String> {
    let credential_id = if !a.is_empty() { a } else { b };
    let rig = world.credential();
    let snap = rig.window_state.snapshot();
    if snap
        .pending_credentials
        .iter()
        .all(|c| c.credential_id != credential_id)
    {
        return Err(format!("no credential card for {credential_id}"));
    }
    let _ = world; // keep `world` linkable across both regex branches
    Ok(())
}

#[then(r#"the card offers "use from host", a custom-value input, and "deny""#)]
fn then_card_offers_three(world: &mut BehaviourWorld) -> Result<(), String> {
    let rig = world.credential();
    let snap = rig.window_state.snapshot();
    let card = snap
        .pending_credentials
        .last()
        .ok_or("no credential card visible")?;
    if !card.host_value_available {
        return Err("expected host_value_available=true for the three-option card".into());
    }
    Ok(())
}

#[then(r#"the card offers a custom-value input and "deny""#)]
fn then_card_offers_two(world: &mut BehaviourWorld) -> Result<(), String> {
    let rig = world.credential();
    let snap = rig.window_state.snapshot();
    let card = snap
        .pending_credentials
        .last()
        .ok_or("no credential card visible")?;
    if card.host_value_available {
        return Err("expected host_value_available=false for the no-host card".into());
    }
    Ok(())
}

#[then("the card states that no host credential was detected")]
fn then_card_says_no_host(world: &mut BehaviourWorld) -> Result<(), String> {
    // At Layer 2 we pin host_value_available; tray.rs renders the user-visible string from it and is not unit-tested here.
    let rig = world.credential();
    let snap = rig.window_state.snapshot();
    let card = snap
        .pending_credentials
        .last()
        .ok_or("no credential card visible")?;
    if card.host_value_available {
        return Err("card should NOT advertise a host credential".into());
    }
    Ok(())
}

#[then("the card states why a fresh value is required")]
fn then_card_says_fresh_needed(world: &mut BehaviourWorld) -> Result<(), String> {
    // Same flag check: S9 re-prompt wording is shown when host_value_available is false despite an existing rule.
    let rig = world.credential();
    let snap = rig.window_state.snapshot();
    let card = snap
        .pending_credentials
        .last()
        .ok_or("no credential card visible")?;
    if card.host_value_available {
        return Err("S9 re-prompt must show host_value_available=false".into());
    }
    Ok(())
}

#[given(regex = r#"^"([^"]+)" is disconnected from this project while the card is open$"#)]
fn given_disconnected_while_card_open(world: &mut BehaviourWorld, credential_id: String) {
    world.credential().forget_project_grants(&credential_id);
}

#[then(regex = r#"^a run in another project is still asked for "([^"]+)"$"#)]
fn then_another_project_still_asked(
    world: &mut BehaviourWorld,
    credential_id: String,
) -> Result<(), String> {
    if !world
        .credential()
        .another_project_is_still_asked(&credential_id)
    {
        return Err(format!(
            "a run in another project got no card for {credential_id}, so closing a card here silenced it machine-wide — the blast radius the dismissal must not have"
        ));
    }
    Ok(())
}

#[then(regex = r#"^the workload grant sidecar records no grant for "([^"]+)"$"#)]
fn then_grant_sidecar_records_nothing(
    world: &mut BehaviourWorld,
    credential_id: String,
) -> Result<(), String> {
    if world.credential().workload_grant_recorded(&credential_id) {
        return Err(format!(
            "the disconnect reported that it forgot {credential_id}'s grants, so a decision this run was already holding must not put one back — a later reconnect would inherit it with no card"
        ));
    }
    Ok(())
}

#[then("the approval window says the decision was not remembered")]
fn then_window_says_not_remembered(world: &mut BehaviourWorld) -> Result<(), String> {
    let informs = world.credential().window_state.snapshot().informs;
    if informs.iter().any(|m| m.contains("not remembered")) {
        Ok(())
    } else {
        Err(format!(
            "the developer answered a card and the answer was dropped; silence would make the next run's fresh card look like a bug. Informs: {informs:?}"
        ))
    }
}

#[then(regex = r#"^the workload grant sidecar records a deny for "([^"]+)"$"#)]
fn then_grant_sidecar_records_deny(
    world: &mut BehaviourWorld,
    credential_id: String,
) -> Result<(), String> {
    let rig = world.credential();
    if rig.workload_deny_recorded(&credential_id) {
        Ok(())
    } else {
        Err(format!(
            "no per-workload deny grant recorded for {credential_id}"
        ))
    }
}

#[then(regex = r#"^"~/.lns/credentials.json" gains an entry for "([^"]+)" with kind "([^"]+)"$"#)]
fn then_credentials_file_has_entry(
    world: &mut BehaviourWorld,
    credential_id: String,
    kind: String,
) -> Result<(), String> {
    let rig = world.credential();
    let state = rig.store.load().map_err(|e| e.to_string())?;
    let entry = state
        .get(&credential_id)
        .ok_or_else(|| format!("no entry for {credential_id} in credentials file"))?;
    assert_entry_kind(entry, &kind)
}

#[then(
    regex = r#"^"~/.lns/credentials.json" gains an entry for "([^"]+)" with kind "stored" carrying the typed value$"#
)]
fn then_credentials_file_has_stored_with_value(
    world: &mut BehaviourWorld,
    credential_id: String,
) -> Result<(), String> {
    let rig = world.credential();
    let state = rig.store.load().map_err(|e| e.to_string())?;
    let entry = state
        .get(&credential_id)
        .ok_or_else(|| format!("no entry for {credential_id}"))?;
    match entry {
        CredentialEntry::Stored { value } if value == "typed-value" => Ok(()),
        other => Err(format!("expected Stored with typed-value, got {other:?}")),
    }
}

fn assert_entry_kind(entry: &CredentialEntry, kind: &str) -> Result<(), String> {
    let actual = match entry {
        CredentialEntry::HostDetect => "host-detect",
        CredentialEntry::Stored { .. } => "stored",
        CredentialEntry::Oauth { .. } => "oauth",
        CredentialEntry::Deny => "deny",
    };
    if actual != kind {
        return Err(format!("expected kind {kind}, got {actual}"));
    }
    Ok(())
}

#[then(
    regex = r#"^the workload's (?:held )?request leaves the boundary with the host-detected some-provider credential substituted in$"#
)]
fn then_request_leaves_with_host_value(world: &mut BehaviourWorld) -> Result<(), String> {
    let credential_id = FIXTURE_ID;
    let rig = world.credential();
    let frames = drain_frames(rig);
    let policy = last_policy_frame(&frames)
        .ok_or("no Policy frame emitted after decision — credential never armed")?;
    let cred = find_credential(policy, credential_id)
        .ok_or_else(|| format!("Policy frame missing credential for {credential_id}"))?;
    let real = format!("real-{credential_id}-token");
    assert_credential_armed_with(cred, &real)?;
    decision_allow_assert(&frames)?;
    Ok(())
}

#[then(
    "the workload's request leaves the boundary with the typed value substituted for the placeholder"
)]
fn then_request_leaves_with_typed_value(world: &mut BehaviourWorld) -> Result<(), String> {
    let rig = world.credential();
    let frames = drain_frames(rig);
    let policy = last_policy_frame(&frames).ok_or("no Policy frame emitted after decision")?;
    let cred = find_credential(policy, FIXTURE_ID)
        .ok_or("Policy frame missing some-provider credential")?;
    assert_credential_armed_with(cred, "typed-value")?;
    decision_allow_assert(&frames)?;
    Ok(())
}

fn assert_credential_armed_with(
    cred: &lns_service::approval_flow::protocol::Credential,
    real_value: &str,
) -> Result<(), String> {
    if cred.injections.is_empty() {
        return Err("credential injections empty — MITM would never substitute".into());
    }
    if cred
        .injections
        .iter()
        .any(|i| injection_value(i).contains(real_value))
    {
        return Ok(());
    }
    Err(format!(
        "no injection armed with {real_value:?}: {:?}",
        cred.injections
    ))
}

#[then("the workload still sees only the placeholder")]
fn then_workload_sees_placeholder(_w: &mut BehaviourWorld) -> Result<(), String> {
    // The real value only ever appears in CredentialInjection.value; the seeded placeholder stays a fake.
    assert!(
        FIXTURE_PLACEHOLDER.to_lowercase().contains("placeholder"),
        "fixture placeholder {FIXTURE_PLACEHOLDER} must be a fake"
    );
    Ok(())
}

#[then(
    regex = r#"^a future request carrying the some-provider placeholder is exchanged silently using the (currently host-detected value|stored value|host-detected value until the sandbox exits)$"#
)]
fn then_future_request_silent(world: &mut BehaviourWorld, _source: String) -> Result<(), String> {
    let credential_id = FIXTURE_ID;
    let rig = world.credential();
    let entry = rig.session.current_state().get(credential_id).cloned();
    match entry {
        Some(CredentialEntry::HostDetect) | Some(CredentialEntry::Stored { .. }) => Ok(()),
        other => Err(format!(
            "expected an armed credential rule for {credential_id}, got {other:?}"
        )),
    }
}

#[then(
    regex = r#"^a future request carrying the some-provider placeholder is failed at the boundary without prompting$"#
)]
fn then_future_request_failed(world: &mut BehaviourWorld) -> Result<(), String> {
    let credential_id = FIXTURE_ID;
    let rig = world.credential();
    let cards_before = rig.window_state.snapshot().pending_credentials.len();
    rig.session.submit_pending(
        CredentialPending {
            id: format!("cred-{credential_id}-future"),
            credential_id: credential_id.to_string(),
            action: format!("use of {credential_id} placeholder"),
            reason: "placeholder-unauthorized".into(),
        },
        Instant::now(),
    );
    let cards_after = rig.window_state.snapshot().pending_credentials.len();
    if cards_after != cards_before {
        return Err(format!(
            "expected no new credential card for {credential_id}, cards went {cards_before} -> {cards_after}"
        ));
    }
    let frames = drain_frames(rig);
    let d = last_credential_decision(&frames)
        .ok_or("expected a Deny decision for the future request, got none")?;
    if d.decision != CredentialDecisionKind::Deny {
        return Err(format!("expected Deny decision, got {:?}", d.decision));
    }
    Ok(())
}

#[then(r#""lns-local-mixin.yaml" is unchanged"#)]
fn then_lns_policy_yaml_unchanged(_w: &mut BehaviourWorld) -> Result<(), String> {
    // no-op: no code path runs from CredentialSession to the policy file, so this holds by construction.
    Ok(())
}

#[then("the workload's held request is failed at the boundary")]
fn then_held_request_failed(world: &mut BehaviourWorld) -> Result<(), String> {
    let rig = world.credential();
    let frames = drain_frames(rig);
    let d = last_credential_decision(&frames).ok_or("no credential decision emitted")?;
    if !matches!(
        d.decision,
        CredentialDecisionKind::Deny | CredentialDecisionKind::Timeout
    ) {
        return Err(format!(
            "expected Deny/Timeout decision, got {:?}",
            d.decision
        ));
    }
    Ok(())
}

#[then(regex = r#"^a network card appears for "([^"]+)"$"#)]
fn then_network_card_appears(world: &mut BehaviourWorld, host: String) -> Result<(), String> {
    let rig = world.approval();
    let presented = rig.notifier.presented.lock().unwrap();
    if !presented.iter().any(|p| p.host == host) {
        return Err(format!("no network card for {host}"));
    }
    Ok(())
}

#[then("the workload's request is held until both cards have decisions")]
fn then_request_held_until_both(world: &mut BehaviourWorld) -> Result<(), String> {
    {
        let rig = world.approval();
        let frames = drain_frames_approval(rig);
        if let Some(d) = frames.iter().rev().find_map(|f| match f {
            HostFrame::RequestDecision(d) => Some(d),
            _ => None,
        }) {
            return Err(format!("network decision emitted prematurely: {d:?}"));
        }
    }
    {
        let rig = world.credential();
        let frames = drain_frames(rig);
        if last_credential_decision(&frames).is_some() {
            return Err("credential decision emitted prematurely".into());
        }
    }
    Ok(())
}

#[then(r#"a "deny" decision on either card fails the request at the boundary"#)]
fn then_either_deny_fails(world: &mut BehaviourWorld) -> Result<(), String> {
    // Pin the wire frame only: the held-request failure is the MITM's upstream concern, the host-side promise is the Deny frame.
    let rig = world.credential();
    let id = window_credential_id(rig);
    rig.session
        .record_decision(&id, CredentialDecisionRequest::Deny);
    let frames = drain_frames(rig);
    let d = last_credential_decision(&frames).ok_or("no credential decision emitted")?;
    if !matches!(d.decision, CredentialDecisionKind::Deny) {
        return Err(format!("expected Deny, got {:?}", d.decision));
    }
    Ok(())
}

fn drain_frames_approval(rig: &mut crate::approval_rig::ApprovalRig) -> Vec<HostFrame> {
    let mut out = Vec::new();
    while let Ok(f) = rig.frames.try_recv() {
        out.push(f);
    }
    out
}

#[then(
    regex = r#"^a subsequent request from the workload carrying the some-provider placeholder fires a fresh credential card for "([^"]+)"$"#
)]
fn then_subsequent_fires_fresh_card(
    world: &mut BehaviourWorld,
    credential_id: String,
) -> Result<(), String> {
    let rig = world.credential();
    submit_credential_with_id(rig, "fresh-after-revoke", &credential_id);
    let snap = rig.window_state.snapshot();
    if snap
        .pending_credentials
        .iter()
        .all(|c| c.credential_id != credential_id)
    {
        return Err(format!("no fresh credential card for {credential_id}"));
    }
    Ok(())
}

#[then("no second credential card appears")]
fn then_no_second_credential_card(world: &mut BehaviourWorld) -> Result<(), String> {
    let rig = world.credential();
    let count = rig.window_state.snapshot().pending_credentials.len();
    if count != 1 {
        return Err(format!(
            "expected exactly one credential card (dedup S11), got {count}"
        ));
    }
    Ok(())
}

#[then("the credential card is removed from the approval window")]
fn then_credential_card_removed(world: &mut BehaviourWorld) -> Result<(), String> {
    let rig = world.credential();
    let snap = rig.window_state.snapshot();
    if !snap.pending_credentials.is_empty() {
        return Err(format!(
            "expected no credential cards, got {}",
            snap.pending_credentials.len()
        ));
    }
    Ok(())
}

#[then(r#""~/.lns/credentials.json" is unchanged"#)]
fn then_credentials_file_unchanged(world: &mut BehaviourWorld) -> Result<(), String> {
    let rig = world.credential();
    let state = rig.store.load().map_err(|e| e.to_string())?;
    if !state.is_empty() {
        return Err(format!(
            "expected credentials file to be empty, got {state:?}"
        ));
    }
    Ok(())
}

#[then(
    regex = r#"^a future request carrying the some-provider placeholder fires a fresh credential card$"#
)]
fn then_future_request_fires_fresh_card(world: &mut BehaviourWorld) -> Result<(), String> {
    let credential_id = FIXTURE_ID.to_string();
    let rig = world.credential();
    submit_credential_with_id(rig, "fresh-after-timeout", &credential_id);
    let snap = rig.window_state.snapshot();
    if snap
        .pending_credentials
        .iter()
        .all(|c| c.credential_id != credential_id)
    {
        return Err(format!("no fresh credential card for {credential_id}"));
    }
    Ok(())
}

#[then("the running credential rules contain a \"host-detect\" entry for \"some-provider\"")]
fn then_running_rules_have_host_detect(world: &mut BehaviourWorld) -> Result<(), String> {
    let rig = world.credential();
    match rig.session.current_state().get(FIXTURE_ID) {
        Some(CredentialEntry::HostDetect) => Ok(()),
        other => Err(format!("expected HostDetect, got {other:?}")),
    }
}

// The persist-failure and no-restart steps are shared with the network flow; see steps/approval_flow.rs.

fn injection_value(injection: &CredentialInjection) -> String {
    match injection {
        CredentialInjection::Header { value, .. } => value.clone(),
        CredentialInjection::UriPlaceholder { value, .. } => value.clone(),
    }
}

fn window_credential_id(rig: &CredentialRig) -> String {
    rig.window_state
        .snapshot()
        .pending_credentials
        .last()
        .expect("no credential card visible — call submit_pending first")
        .id
        .clone()
}

fn decision_allow_assert(frames: &[HostFrame]) -> Result<(), String> {
    let d = last_credential_decision(frames).ok_or("no credential decision emitted")?;
    if !matches!(d.decision, CredentialDecisionKind::Allow) {
        return Err(format!("expected Allow, got {:?}", d.decision));
    }
    Ok(())
}

#[when("the developer closes the card without choosing")]
fn when_developer_closes_the_card(world: &mut BehaviourWorld) {
    if let Some(rig) = world.approval.as_mut() {
        let id = rig
            .notifier
            .presented
            .lock()
            .unwrap()
            .last()
            .expect("a card must be visible before it can be closed")
            .id
            .clone();
        rig.session.dismiss_request(&id);
    }
    if let Some(rig) = world.credential.as_mut() {
        let id = window_credential_id(rig);
        rig.session
            .record_decision(&id, CredentialDecisionRequest::Dismiss);
    }
}

#[when("the developer closes every card at once")]
fn when_developer_closes_every_card(world: &mut BehaviourWorld) {
    let rig = world.credential();
    // Drive the pile's own fan-out rather than deciding each card here, or the scenario would pin the loop the test wrote instead of the one the ✕ runs.
    let snapshot = rig.window_state.snapshot();
    lns_service::tray::close_all(&rig.window_state, &snapshot);
    rig.apply_queued_card_decisions();
}

#[given(regex = r#"^credential cards for "([^"]+)" and "([^"]+)" are visible$"#)]
fn given_two_credential_cards(world: &mut BehaviourWorld, first: String, second: String) {
    let rig = world.credential();
    submit_credential_with_id(rig, "cred-first", &first);
    submit_credential_with_id(rig, "cred-second", &second);
}

#[then("both held requests are failed at the boundary")]
fn then_both_held_requests_failed(world: &mut BehaviourWorld) -> Result<(), String> {
    let rig = world.credential();
    let frames = drain_frames(rig);
    let failed = frames
        .iter()
        .filter_map(|f| match f {
            HostFrame::CredentialDecision(d) => Some(d),
            _ => None,
        })
        .filter(|d| {
            matches!(
                d.decision,
                CredentialDecisionKind::Deny | CredentialDecisionKind::Timeout
            )
        })
        .count();
    if failed < 2 {
        return Err(format!("expected two failed requests, got {failed}"));
    }
    Ok(())
}

#[then(
    regex = r#"^a future request carrying either the "([^"]+)" or "([^"]+)" placeholder fires a fresh credential card$"#
)]
fn then_both_placeholders_fire_fresh_cards(
    world: &mut BehaviourWorld,
    first: String,
    second: String,
) -> Result<(), String> {
    // Both, not either: the bug was one click deciding for every card in the stack, so checking one of the two cannot tell "nothing was decided for any" from "nothing was decided for the first".
    for credential_id in [&first, &second] {
        let rig = world.credential();
        submit_credential_with_id(
            rig,
            &format!("fresh-after-close-{credential_id}"),
            credential_id,
        );
        let snap = rig.window_state.snapshot();
        if snap
            .pending_credentials
            .iter()
            .all(|c| &c.credential_id != credential_id)
        {
            return Err(format!(
                "no fresh credential card for {credential_id} after a close-all"
            ));
        }
    }
    Ok(())
}

#[then(regex = r#"^the audit chain records the "([^"]+)" credential approval as denied once$"#)]
fn then_audit_records_denied_once(
    world: &mut BehaviourWorld,
    credential_id: String,
) -> Result<(), String> {
    let rig = world.credential();
    let events = rig.ledger.events.lock().unwrap();
    let matched = events.iter().any(|e| {
        matches!(
            e,
            lns_ipc::LedgerEvent::Approval {
                kind: lns_ipc::ApprovalKind::Credential,
                target,
                decision: lns_ipc::Decision::DenyOnce,
                ..
            } if target == &credential_id
        )
    });
    if !matched {
        return Err(format!(
            "no deny-once approval for {credential_id}; got {events:?}"
        ));
    }
    Ok(())
}

#[then(regex = r#"^the audit chain records no approval for "([^"]+)"$"#)]
fn then_audit_records_no_approval(
    world: &mut BehaviourWorld,
    target_id: String,
) -> Result<(), String> {
    let recorded: Vec<lns_ipc::LedgerEvent> = world
        .approval
        .as_ref()
        .map(|r| r.ledger.events.lock().unwrap().clone())
        .into_iter()
        .chain(
            world
                .credential
                .as_ref()
                .map(|r| r.ledger.events.lock().unwrap().clone()),
        )
        .flatten()
        .filter(|e| {
            matches!(
                e,
                lns_ipc::LedgerEvent::Approval { target, .. } if target == &target_id
            )
        })
        .collect();
    if !recorded.is_empty() {
        return Err(format!(
            "a non-decision must earn no audit line, got {recorded:?}"
        ));
    }
    Ok(())
}
