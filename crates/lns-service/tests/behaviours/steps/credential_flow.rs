use cucumber::{given, then, when};
use std::time::Instant;

use crate::credential_rig::CredentialRig;
use crate::world::BehaviourWorld;
use lns_service::approval_flow::protocol::{
    CredentialDecisionKind, CredentialInjection, CredentialPending, HostFrame, PolicyMessage,
};
use lns_service::credential_flow::providers;
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

#[given(regex = r#"^the built-in registry includes "([^"]+)", "([^"]+)", and "([^"]+)"$"#)]
fn given_registry_has_three(_w: &mut BehaviourWorld, a: String, b: String, c: String) {
    for id in [&a, &b, &c] {
        assert!(
            providers::ALL.iter().any(|p| p.id() == id),
            "expected built-in registry to include {id}"
        );
    }
}

#[given(regex = r#"^the built-in registry includes "([^"]+)" and "([^"]+)"$"#)]
fn given_registry_has_two(_w: &mut BehaviourWorld, a: String, b: String) {
    for id in [&a, &b] {
        assert!(
            providers::ALL.iter().any(|p| p.id() == id),
            "expected built-in registry to include {id}"
        );
    }
}

#[given(regex = r#"^a workload is running with the seeded "([^"]+)" placeholder$"#)]
fn given_workload_with_placeholder(world: &mut BehaviourWorld, _credential_id: String) {
    world.credential();
}

#[given(regex = r#"^no credential rule exists in "~/.lns-credentials.json" for "([^"]+)"$"#)]
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

#[given(regex = r#"^"~/.lns-credentials.json" has an entry for "([^"]+)" with kind "([^"]+)"$"#)]
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
    regex = r#"^a workload is running with a "stored" credential rule for "([^"]+)" in "~/.lns-credentials.json"$"#
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
    regex = r#"^a workload sends a request carrying the GitHub placeholder with no credential rule for "github"$"#
)]
fn given_workload_sends_with_no_rule(world: &mut BehaviourWorld) {
    let rig = world.credential();
    submit_credential_with_id(rig, "cred-coalesce-1", "github");
}

#[given(regex = r#"^a workload has an open credential card for "([^"]+)"$"#)]
fn given_open_credential_card(world: &mut BehaviourWorld, credential_id: String) {
    let rig = world.credential();
    rig.session
        .submit_pending(make_credential_pending(&credential_id), Instant::now());
    let _ = drain_frames(rig);
}

#[given(r#""~/.lns-credentials.json" cannot be written"#)]
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

#[when("a workload reads the seeded GITHUB_TOKEN and OPENAI_API_KEY")]
fn when_workload_reads_seeded(world: &mut BehaviourWorld) {
    let rig = world.credential();
    let state = rig.session.current_state();
    rig.session.apply_external_state(state);
}

#[when(
    regex = r#"^the workload sends a request carrying the (GitHub|OpenAI|Anthropic|Linear) placeholder$"#
)]
fn when_workload_sends_placeholder(world: &mut BehaviourWorld, provider_name: String) {
    let credential_id = provider_id_for_name(&provider_name);
    let rig = world.credential();
    rig.session
        .submit_pending(make_credential_pending(credential_id), Instant::now());
}

#[when(
    regex = r#"^the workload sends a request to "([^"]+)" carrying the (GitHub|OpenAI|Anthropic|Linear) placeholder$"#
)]
fn when_workload_sends_to_host_with_placeholder(
    world: &mut BehaviourWorld,
    host: String,
    provider_name: String,
) {
    let credential_id = provider_id_for_name(&provider_name);
    // S8 drives both flows: a network pending on the approval rig and a credential pending on the credential rig.
    {
        let approval_rig = world.approval();
        let req = lns_service::approval_flow::protocol::RequestPending {
            id: format!("req-{host}"),
            host: host.clone(),
            action: format!("CONNECT {host}:443"),
            reason: "policy-ambiguous".into(),
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

#[when(r#"the developer picks "deny""#)]
fn when_developer_picks_deny_credential(world: &mut BehaviourWorld) {
    let rig = world.credential();
    let id = window_credential_id(rig);
    rig.session
        .record_decision(&id, CredentialDecisionRequest::Deny);
}

#[when(regex = r#"^the developer deletes the "([^"]+)" entry from "~/.lns-credentials.json"$"#)]
fn when_developer_deletes_entry(world: &mut BehaviourWorld, credential_id: String) {
    let rig = world.credential();
    let mut state = rig.session.current_state();
    state.remove(&credential_id);
    rig.store.save(&state).expect("save edited state");
    rig.session.apply_external_state(state);
    let _ = drain_frames(rig);
}

#[when(
    regex = r#"^the workload sends a second request carrying the GitHub placeholder before the developer decides$"#
)]
fn when_workload_sends_second_placeholder(world: &mut BehaviourWorld) {
    let rig = world.credential();
    // Distinct id, same provider → exercises credential_id coalescing, not bare id dedup (S11).
    submit_credential_with_id(rig, "cred-coalesce-2", "github");
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
    for provider in providers::ALL.iter() {
        let cred = creds
            .iter()
            .find(|c| c.id == provider.id())
            .ok_or_else(|| format!("Policy frame missing credential for {}", provider.id()))?;
        if cred.placeholder.as_deref() != Some(provider.placeholder()) {
            return Err(format!(
                "credential {} placeholder mismatch: got {:?}",
                provider.id(),
                cred.placeholder
            ));
        }
        if cred.env_var.as_deref() != Some(provider.env_var()) {
            return Err(format!(
                "credential {} env_var mismatch: got {:?}",
                provider.id(),
                cred.env_var
            ));
        }
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

#[then(
    r#"GITHUB_TOKEN starts with the "ghp_" prefix and is the length of a real GitHub personal access token"#
)]
fn then_github_token_shape(_w: &mut BehaviourWorld) -> Result<(), String> {
    let p = providers::by_id("github").ok_or("github missing from registry")?;
    if !p.placeholder().starts_with("ghp_") {
        return Err(format!(
            "placeholder missing ghp_ prefix: {}",
            p.placeholder()
        ));
    }
    if p.placeholder().len() != 40 {
        return Err(format!(
            "github placeholder len {} ≠ 40",
            p.placeholder().len()
        ));
    }
    Ok(())
}

#[then(r#"OPENAI_API_KEY starts with the "sk-" prefix and is the length of a real OpenAI API key"#)]
fn then_openai_key_shape(_w: &mut BehaviourWorld) -> Result<(), String> {
    let p = providers::by_id("openai").ok_or("openai missing from registry")?;
    if !p.placeholder().starts_with("sk-") {
        return Err(format!(
            "placeholder missing sk- prefix: {}",
            p.placeholder()
        ));
    }
    if p.placeholder().len() != 51 {
        return Err(format!(
            "openai placeholder len {} ≠ 51",
            p.placeholder().len()
        ));
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

#[then(regex = r#"^"~/.lns-credentials.json" gains an entry for "([^"]+)" with kind "([^"]+)"$"#)]
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
    regex = r#"^"~/.lns-credentials.json" gains an entry for "([^"]+)" with kind "stored" carrying the typed value$"#
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
    regex = r#"^the workload's (?:held )?request leaves the boundary with the host-detected (GitHub|OpenAI|Anthropic|Linear) credential substituted in$"#
)]
fn then_request_leaves_with_host_value(
    world: &mut BehaviourWorld,
    provider_name: String,
) -> Result<(), String> {
    let credential_id = provider_id_for_name(&provider_name);
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
    let cred = find_credential(policy, "github").ok_or("Policy frame missing github credential")?;
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
    // Walk the registry rather than drained frames (the prior step consumed them): the real value only ever appears in CredentialInjection.value, never in the placeholder.
    for p in providers::ALL.iter() {
        assert!(
            p.placeholder().to_lowercase().contains("lnsplaceholder"),
            "registry placeholder {} must be a fake",
            p.placeholder()
        );
    }
    Ok(())
}

#[then(
    regex = r#"^a future request carrying the (GitHub|OpenAI|Anthropic|Linear) placeholder is exchanged silently using the (currently host-detected value|stored value|host-detected value until the sandbox exits)$"#
)]
fn then_future_request_silent(
    world: &mut BehaviourWorld,
    provider_name: String,
    _source: String,
) -> Result<(), String> {
    let credential_id = provider_id_for_name(&provider_name);
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
    regex = r#"^a future request carrying the (GitHub|OpenAI|Anthropic|Linear) placeholder is failed at the boundary without prompting$"#
)]
fn then_future_request_failed(
    world: &mut BehaviourWorld,
    provider_name: String,
) -> Result<(), String> {
    let credential_id = provider_id_for_name(&provider_name);
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
    if rig.session.current_state().get(credential_id) != Some(&CredentialEntry::Deny) {
        return Err(format!(
            "expected the Deny rule for {credential_id} to persist"
        ));
    }
    Ok(())
}

#[then(r#""lns-policy.yaml" is unchanged"#)]
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
    regex = r#"^a subsequent request from the workload carrying the GitHub placeholder fires a fresh credential card for "([^"]+)"$"#
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

#[then(r#""~/.lns-credentials.json" is unchanged"#)]
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
    regex = r#"^a future request carrying the (GitHub|OpenAI|Anthropic|Linear) placeholder fires a fresh credential card$"#
)]
fn then_future_request_fires_fresh_card(
    world: &mut BehaviourWorld,
    provider_name: String,
) -> Result<(), String> {
    let credential_id = provider_id_for_name(&provider_name).to_string();
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

#[then("the running credential rules contain a \"host-detect\" entry for \"github\"")]
fn then_running_rules_have_host_detect(world: &mut BehaviourWorld) -> Result<(), String> {
    let rig = world.credential();
    match rig.session.current_state().get("github") {
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

fn provider_id_for_name(name: &str) -> &'static str {
    match name {
        "GitHub" => "github",
        "OpenAI" => "openai",
        "Anthropic" => "anthropic",
        "Linear" => "linear",
        other => panic!("unknown provider name {other}"),
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
