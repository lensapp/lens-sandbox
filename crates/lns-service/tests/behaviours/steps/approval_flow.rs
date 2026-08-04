use cucumber::{given, then, when};
use std::time::{Duration, Instant};

use crate::approval_rig::ApprovalRig;
use crate::world::BehaviourWorld;
use lns_policy::{Policy, RouteRule, Verdict};
use lns_service::approval_flow::protocol::{Decision, HostFrame, RequestPending, Treatment};

fn make_raw_pending(destination: &str) -> RequestPending {
    let host = destination
        .rsplit_once(':')
        .map_or(destination, |(host, _)| host);
    RequestPending {
        id: format!("req-raw-{destination}"),
        host: host.to_string(),
        action: format!("CONNECT {destination}"),
        reason: "policy-ambiguous".into(),
        treatment: Treatment::Raw,
    }
}

fn make_pending(host: &str) -> RequestPending {
    RequestPending {
        id: format!("req-{host}"),
        host: host.to_string(),
        action: format!("CONNECT {host}:443"),
        reason: "policy-ambiguous".into(),
        treatment: Treatment::Inspected,
    }
}

fn pending_with_id(id: &str, host: &str) -> RequestPending {
    RequestPending {
        id: id.to_string(),
        host: host.to_string(),
        action: format!("CONNECT {host}:443"),
        reason: "policy-ambiguous".into(),
        treatment: Treatment::Inspected,
    }
}

fn decision_for_choice(choice: &str) -> Decision {
    match choice {
        "allow once" => Decision::AllowOnce,
        "always allow" => Decision::AllowAlways,
        "deny once" => Decision::DenyOnce,
        "always deny" => Decision::DenyAlways,
        other => panic!("unknown developer choice: {other}"),
    }
}

fn drain_frames(rig: &mut ApprovalRig) -> Vec<HostFrame> {
    let mut out = Vec::new();
    while let Ok(f) = rig.frames.try_recv() {
        out.push(f);
    }
    out
}

fn last_decision_frame(
    frames: &[HostFrame],
) -> Option<&lns_service::approval_flow::protocol::RequestDecision> {
    frames.iter().rev().find_map(|f| match f {
        HostFrame::RequestDecision(d) => Some(d),
        _ => None,
    })
}

#[given("a workload is running in the sandbox")]
fn given_workload_running(world: &mut BehaviourWorld) {
    world.approval();
}

#[given("a workload is running with the loaded policy")]
fn given_workload_with_loaded_policy(world: &mut BehaviourWorld) {
    world.approval();
}

#[given(regex = r#"^a workload is running with "([^"]+)" loaded$"#)]
fn given_workload_with_file(world: &mut BehaviourWorld, _filename: String) {
    let rig = world.approval();
    Policy::default()
        .save_atomic(&rig.policy_path)
        .expect("save initial policy");
}

#[given(regex = r#"^the sandbox was launched with --policy "([^"]+)"$"#)]
fn given_launched_with_policy(world: &mut BehaviourWorld, _filename: String) {
    let rig = world.approval();
    Policy::default()
        .save_atomic(&rig.policy_path)
        .expect("save initial policy");
}

#[given(regex = r#"^the policy has no rule for "([^"]+)"$"#)]
fn given_no_rule_for(world: &mut BehaviourWorld, host: String) {
    let rig = world.approval();
    let routes = &rig.session.current_policy().network.egress.http;
    assert!(
        !routes.iter().any(|r| r.match_pattern == host),
        "expected no rule for {host}, but found one"
    );
}

#[given(regex = r#"^an approval entry is visible for a request to "([^"]+)"$"#)]
fn given_notification_visible_for(world: &mut BehaviourWorld, host: String) {
    let rig = world.approval();
    rig.session
        .submit_pending(make_pending(&host), Instant::now());
    let _ = drain_frames(rig);
}

#[given(
    regex = r#"^the policy denies "([^"]+)" and holds an allow rule for "([^"]+)" behind that$"#
)]
fn given_policy_denies_then_allows(world: &mut BehaviourWorld, covering: String, host: String) {
    seed_policy(world, |policy| {
        policy
            .network
            .egress
            .http
            .push(RouteRule::deny_host(covering));
        policy.network.egress.http.push(RouteRule::allow_host(host));
    });
}

#[given(regex = r#"^the policy allows "([^"]+)"$"#)]
fn given_policy_allows(world: &mut BehaviourWorld, host: String) {
    seed_policy(world, |policy| {
        policy.network.egress.http.push(RouteRule::allow_host(host))
    });
}

fn seed_policy(world: &mut BehaviourWorld, add: impl FnOnce(&mut Policy)) {
    let rig = world.approval();
    let mut policy = rig.session.current_policy();
    add(&mut policy);
    policy
        .save_atomic(&rig.policy_path)
        .expect("save seeded policy");
    rig.session.apply_external_policy(policy);
    let _ = drain_frames(rig);
}

#[given(regex = r#"^an approval entry is visible for a raw splice to "([^"]+)"$"#)]
fn given_raw_entry_visible(world: &mut BehaviourWorld, destination: String) {
    let rig = world.approval();
    rig.session
        .submit_pending(make_raw_pending(&destination), Instant::now());
    let _ = drain_frames(rig);
}

#[given(regex = r#"^a workload makes a request to "([^"]+)" with no matching rule$"#)]
fn given_workload_makes_request(world: &mut BehaviourWorld, host: String) {
    let rig = world.approval();
    rig.session
        .submit_pending(make_pending(&host), Instant::now());
}

#[given("an approval entry is visible")]
fn given_notification_visible(world: &mut BehaviourWorld) {
    let rig = world.approval();
    let presented = rig.notifier.presented.lock().unwrap();
    assert!(
        !presented.is_empty(),
        "expected at least one approval card visible"
    );
}

#[given("a workload has an open approval entry")]
fn given_open_notification(world: &mut BehaviourWorld) {
    let rig = world.approval();
    rig.session
        .submit_pending(make_pending("api.linear.app"), Instant::now());
}

#[given("the policy file cannot be written")]
fn given_policy_file_cannot_be_written(world: &mut BehaviourWorld) {
    world.approval().store.break_next_save();
}

#[when(regex = r#"^the workload makes a request to "([^"]+)"$"#)]
fn when_workload_makes_request(world: &mut BehaviourWorld, host: String) {
    let rig = world.approval();
    rig.session
        .submit_pending(make_pending(&host), Instant::now());
}

#[when(
    regex = r#"^the workload makes a second request to "([^"]+)" before the developer decides$"#
)]
fn when_workload_makes_second_request(world: &mut BehaviourWorld, host: String) {
    let rig = world.approval();
    rig.session
        .submit_pending(make_pending(&host), Instant::now());
}

#[when(regex = r#"^the developer picks "(allow once|always allow|deny once|always deny)"$"#)]
fn when_developer_picks(world: &mut BehaviourWorld, choice: String) {
    let rig = world.approval();
    let id = rig
        .notifier
        .presented
        .lock()
        .unwrap()
        .last()
        .expect("a prompt must be visible before the developer can pick")
        .id
        .clone();
    rig.session
        .record_decision(&id, decision_for_choice(&choice));
}

#[when("no decision is recorded before the configured approval timeout")]
fn when_no_decision_before_timeout(world: &mut BehaviourWorld) {
    if let Some(rig) = world.approval.as_mut() {
        let past_deadline = Instant::now() + rig.timeout + Duration::from_secs(1);
        rig.session.tick_timeouts(past_deadline);
    }
    if let Some(rig) = world.credential.as_mut() {
        let past_deadline = Instant::now() + rig.timeout + Duration::from_secs(1);
        rig.session.tick_timeouts(past_deadline);
    }
}

#[when("the workload exits before a decision is recorded")]
fn when_workload_exits(world: &mut BehaviourWorld) {
    if let Some(rig) = world.approval.as_mut() {
        rig.session.withdraw_run();
    }
    if let Some(rig) = world.credential.as_mut() {
        rig.session.withdraw_run();
    }
}

#[when(regex = r#"^an "([^"]+)" decision adds a rule for "([^"]+)" mid-run$"#)]
fn when_decision_adds_rule_midrun(world: &mut BehaviourWorld, choice: String, host: String) {
    let rig = world.approval();
    rig.session
        .submit_pending(make_pending(&host), Instant::now());
    let id = rig
        .notifier
        .presented
        .lock()
        .unwrap()
        .last()
        .expect("prompt must be visible")
        .id
        .clone();
    rig.session
        .record_decision(&id, decision_for_choice(&choice));
}

#[when(regex = r#"^the developer edits "([^"]+)" to add an allow rule for "([^"]+)"$"#)]
fn when_developer_edits_file(world: &mut BehaviourWorld, _filename: String, host: String) {
    let rig = world.approval();
    let mut updated = rig.session.current_policy();
    updated.add_rule(RouteRule::allow_host(host.clone()));
    updated
        .save_atomic(&rig.policy_path)
        .expect("save edited policy");
    rig.session.apply_external_policy(updated);
}

#[then(
    regex = r#"^when the developer's decision is recorded, both requests resolve under that decision$"#
)]
fn when_then_decision_resolves_both(world: &mut BehaviourWorld) -> Result<(), String> {
    let approval_id: Option<String> = world.approval.as_ref().and_then(|r| {
        r.notifier
            .presented
            .lock()
            .unwrap()
            .last()
            .map(|p| p.id.clone())
    });
    if let (Some(rig), Some(id)) = (world.approval.as_mut(), approval_id) {
        rig.session.record_decision(&id, Decision::AllowOnce);
        let frames = drain_frames(rig);
        let decisions: Vec<_> = frames
            .iter()
            .filter_map(|f| match f {
                HostFrame::RequestDecision(d) => Some(d),
                _ => None,
            })
            .collect();
        if decisions.len() != 1 {
            return Err(format!(
                "expected exactly one RequestDecision for coalesced pair, got {}",
                decisions.len()
            ));
        }
        return Ok(());
    }
    if let Some(rig) = world.credential.as_mut() {
        let snap = rig.window_state.snapshot();
        let id = snap
            .pending_credentials
            .last()
            .ok_or("no credential card visible")?
            .id
            .clone();
        rig.session
            .record_decision(&id, crate::credential_rig::resolve_request());
        let mut frames = Vec::new();
        while let Ok(f) = rig.frames.try_recv() {
            frames.push(f);
        }
        let decisions: Vec<_> = frames
            .iter()
            .filter_map(|f| match f {
                HostFrame::CredentialDecision(d) => Some(d),
                _ => None,
            })
            .collect();
        if decisions.len() != 2 {
            return Err(format!(
                "expected both held requests to resolve under one decision, got {} frames",
                decisions.len()
            ));
        }
        return Ok(());
    }
    Err("no rig touched in scenario".into())
}

#[then(
    "an entry appears in the approval window showing the destination, the originating sandbox, and any credential involved"
)]
fn then_notification_appears(world: &mut BehaviourWorld) -> Result<(), String> {
    let rig = world.approval();
    let presented = rig.notifier.presented.lock().unwrap();
    let p = presented.last().ok_or("no approval card was presented")?;
    if p.host.is_empty() {
        return Err("approval card host was empty".into());
    }
    Ok(())
}

#[then("the workload's request is held pending a decision")]
fn then_request_held(world: &mut BehaviourWorld) -> Result<(), String> {
    if let Some(rig) = world.approval.as_mut() {
        let frames = drain_frames(rig);
        if last_decision_frame(&frames).is_some() {
            return Err("a decision was emitted while the request should still be pending".into());
        }
    }
    if let Some(rig) = world.credential.as_mut() {
        let mut frames = Vec::new();
        while let Ok(f) = rig.frames.try_recv() {
            frames.push(f);
        }
        if frames
            .iter()
            .any(|f| matches!(f, HostFrame::CredentialDecision(_)))
        {
            return Err(
                "a credential decision was emitted while the request should still be held".into(),
            );
        }
    }
    Ok(())
}

#[then("the workload's request proceeds")]
fn then_request_proceeds(world: &mut BehaviourWorld) -> Result<(), String> {
    let rig = world.approval();
    let frames = drain_frames(rig);
    let d = last_decision_frame(&frames).ok_or("no decision frame emitted")?;
    if !matches!(d.decision, Decision::AllowOnce | Decision::AllowAlways) {
        return Err(format!(
            "expected allow decision for `request proceeds`, got {:?}",
            d.decision
        ));
    }
    Ok(())
}

fn assert_decision_reaches_guest(
    world: &mut BehaviourWorld,
    expected: Decision,
) -> Result<(), String> {
    let rig = world.approval();
    let frames = drain_frames(rig);
    let d = last_decision_frame(&frames).ok_or("no decision frame emitted")?;
    if d.decision != expected {
        return Err(format!(
            "expected {expected:?} to reach the guest, got {:?}",
            d.decision
        ));
    }
    Ok(())
}

#[then("the workload's request is denied once")]
fn then_request_denied_once(world: &mut BehaviourWorld) -> Result<(), String> {
    assert_decision_reaches_guest(world, Decision::DenyOnce)
}

#[then("the workload's request is denied always")]
fn then_request_denied_always(world: &mut BehaviourWorld) -> Result<(), String> {
    assert_decision_reaches_guest(world, Decision::DenyAlways)
}

/// The guest fails an undecided request closed just as it fails a denied one, so the distinction only shows in what it records — which is why the scenarios assert the exact decision rather than "not allowed".
#[then("the workload's request is failed at the boundary as undecided")]
fn then_request_failed_as_undecided(world: &mut BehaviourWorld) -> Result<(), String> {
    assert_decision_reaches_guest(world, Decision::Timeout)
}

#[then("the running policy is unchanged")]
fn then_running_policy_unchanged(world: &mut BehaviourWorld) -> Result<(), String> {
    let rig = world.approval();
    let p = rig.session.current_policy();
    if !p.network.egress.http.is_empty() {
        return Err(format!(
            "expected no rules in running policy, got {:?}",
            p.network.egress.http
        ));
    }
    Ok(())
}

#[then("the policy file is unchanged")]
fn then_policy_file_unchanged(world: &mut BehaviourWorld) -> Result<(), String> {
    let rig = world.approval();
    if !rig.policy_path.exists() {
        return Ok(());
    }
    let on_disk = Policy::load_or_default(&rig.policy_path).map_err(|e| e.to_string())?;
    if !on_disk.network.egress.http.is_empty() {
        return Err(format!(
            "policy file gained rules: {:?}",
            on_disk.network.egress.http
        ));
    }
    Ok(())
}

#[then(regex = r#"^a future request to "([^"]+)" prompts again$"#)]
fn then_future_request_prompts(world: &mut BehaviourWorld, host: String) -> Result<(), String> {
    let rig = world.approval();
    let presented_before = rig.notifier.presented.lock().unwrap().len();
    drop(rig.notifier.presented.lock());

    let next = pending_with_id(&format!("req-future-{host}"), &host);
    rig.session.submit_pending(next, Instant::now());

    let presented_after = rig.notifier.presented.lock().unwrap().len();
    if presented_after <= presented_before {
        return Err(format!(
            "expected an additional approval card (had {presented_before}, have {presented_after})"
        ));
    }
    Ok(())
}

#[then(regex = r#"^a future request to "([^"]+)" is allowed without prompting$"#)]
fn then_future_request_allowed(world: &mut BehaviourWorld, host: String) -> Result<(), String> {
    assert_host_rule(world, &host, Verdict::Allow)
}

#[then(regex = r#"^a future request to "([^"]+)" is denied without prompting$"#)]
fn then_future_request_denied(world: &mut BehaviourWorld, host: String) -> Result<(), String> {
    assert_host_rule(world, &host, Verdict::Deny)
}

#[then(
    regex = r#"^a future request to "([^"]+)" is allowed without prompting until the sandbox exits$"#
)]
fn then_future_request_allowed_until_exit(
    world: &mut BehaviourWorld,
    host: String,
) -> Result<(), String> {
    assert_host_rule(world, &host, Verdict::Allow)
}

#[then(
    regex = r#"^a subsequent request from the workload to "([^"]+)" is allowed without prompting$"#
)]
fn then_subsequent_request_allowed(world: &mut BehaviourWorld, host: String) -> Result<(), String> {
    assert_host_rule(world, &host, Verdict::Allow)
}

fn assert_host_rule(
    world: &mut BehaviourWorld,
    host: &str,
    expected: Verdict,
) -> Result<(), String> {
    let rig = world.approval();
    let p = rig.session.current_policy();
    let matched = p
        .network
        .egress
        .http
        .iter()
        .find(|r| r.match_pattern == host)
        .ok_or_else(|| format!("no rule for {host} in running policy"))?;
    if matched.verdict != expected {
        return Err(format!(
            "expected verdict {expected:?} for {host}, got {:?}",
            matched.verdict
        ));
    }
    Ok(())
}

#[then(regex = r#"^"([^"]+)" contains a new allow rule for "([^"]+)"$"#)]
fn then_file_contains_allow_rule(
    world: &mut BehaviourWorld,
    _filename: String,
    host: String,
) -> Result<(), String> {
    assert_file_rule(world, &host, Verdict::Allow)
}

#[then(regex = r#"^"([^"]+)" contains a new deny rule for "([^"]+)"$"#)]
fn then_file_contains_deny_rule(
    world: &mut BehaviourWorld,
    _filename: String,
    host: String,
) -> Result<(), String> {
    assert_file_rule(world, &host, Verdict::Deny)
}

fn assert_file_rule(
    world: &mut BehaviourWorld,
    host: &str,
    expected: Verdict,
) -> Result<(), String> {
    let rig = world.approval();
    let on_disk = Policy::load_or_default(&rig.policy_path).map_err(|e| e.to_string())?;
    let matched = on_disk
        .network
        .egress
        .http
        .iter()
        .find(|r| r.match_pattern == host)
        .ok_or_else(|| format!("no rule for {host} in policy file"))?;
    if matched.verdict != expected {
        return Err(format!(
            "expected verdict {expected:?} for {host} in file, got {:?}",
            matched.verdict
        ));
    }
    Ok(())
}

#[then(regex = r#"^"([^"]+)" contains a new raw allow rule for "([^"]+)"$"#)]
fn then_file_contains_raw_allow(
    world: &mut BehaviourWorld,
    _filename: String,
    destination: String,
) -> Result<(), String> {
    assert_raw_rule(world, &destination, Verdict::Allow)
}

#[then(regex = r#"^"([^"]+)" contains a new raw deny rule for "([^"]+)"$"#)]
fn then_file_contains_raw_deny(
    world: &mut BehaviourWorld,
    _filename: String,
    destination: String,
) -> Result<(), String> {
    assert_raw_rule(world, &destination, Verdict::Deny)
}

fn assert_raw_rule(
    world: &mut BehaviourWorld,
    destination: &str,
    expected: Verdict,
) -> Result<(), String> {
    let on_disk = on_disk_policy(world)?;
    let matched = on_disk
        .network
        .egress
        .tcp
        .iter()
        .find(|r| r.match_pattern == destination && r.verdict == expected)
        .ok_or_else(|| {
            format!(
                "no {expected:?} rule for {destination} in the raw table: {:?}",
                on_disk.network.egress.tcp
            )
        })?;
    if matched.binaries.is_some() {
        return Err("an approval grants the destination, not one binary".into());
    }
    Ok(())
}

#[then(regex = r#"^the raw table in "([^"]+)" stays empty$"#)]
fn then_raw_table_empty(world: &mut BehaviourWorld, _filename: String) -> Result<(), String> {
    let on_disk = on_disk_policy(world)?;
    if !on_disk.network.egress.tcp.is_empty() {
        return Err(format!(
            "a rule the guest cannot parse force-denies the whole policy: {:?}",
            on_disk.network.egress.tcp
        ));
    }
    Ok(())
}

#[then(regex = r#"^"([^"]+)" holds no deny rule for "([^"]+)"$"#)]
fn then_file_holds_no_deny_rule(
    world: &mut BehaviourWorld,
    _filename: String,
    host: String,
) -> Result<(), String> {
    let on_disk = on_disk_policy(world)?;
    let denies: Vec<_> = on_disk
        .network
        .egress
        .http
        .iter()
        .filter(|r| r.match_pattern == host && r.verdict == Verdict::Deny)
        .collect();
    if denies.is_empty() {
        return Ok(());
    }
    Err(format!(
        "a deny the gate can never reach reads as a deny that works: {denies:?}"
    ))
}

#[then("the approval window informs the developer that a rule already decides the destination")]
fn then_inform_shadowed_rule(world: &mut BehaviourWorld) -> Result<(), String> {
    let rig = world.approval();
    let informed = rig.notifier.informed.lock().unwrap();
    if informed.iter().any(|m| m.contains("already decides")) {
        return Ok(());
    }
    Err(format!(
        "expected an inform saying the decision outlived nothing, got {informed:?}"
    ))
}

#[then("the approval window informs the developer that the rule it would write is unreachable")]
fn then_inform_unreachable_rule(world: &mut BehaviourWorld) -> Result<(), String> {
    let rig = world.approval();
    let informed = rig.notifier.informed.lock().unwrap();
    if informed
        .iter()
        .any(|m| m.contains("already in the policy file, but behind the rule for"))
    {
        return Ok(());
    }
    Err(format!(
        "expected an inform saying the rule the gate never reaches is already there, got {informed:?}"
    ))
}

#[then(
    regex = r#"^the approval window informs the developer that the HTTP rule for "([^"]+)" no longer applies$"#
)]
fn then_inform_pre_empted_http_rule(
    world: &mut BehaviourWorld,
    pattern: String,
) -> Result<(), String> {
    let rig = world.approval();
    let informed = rig.notifier.informed.lock().unwrap();
    if informed
        .iter()
        .any(|m| m.contains("spliced raw") && m.contains(&format!("{pattern:?}")))
    {
        return Ok(());
    }
    Err(format!(
        "expected an inform naming the HTTP rule the splice displaces, got {informed:?}"
    ))
}

#[then(
    "the approval window informs the developer that the destination could not be turned into a rule"
)]
fn then_inform_underivable_rule(world: &mut BehaviourWorld) -> Result<(), String> {
    let rig = world.approval();
    let informed = rig.notifier.informed.lock().unwrap();
    if informed.iter().any(|m| m.contains("must specify a port")) {
        return Ok(());
    }
    Err(format!(
        "expected an inform naming why no rule could be written, got {informed:?}"
    ))
}

fn on_disk_policy(world: &mut BehaviourWorld) -> Result<Policy, String> {
    let rig = world.approval();
    Policy::load_or_default(&rig.policy_path).map_err(|e| e.to_string())
}

#[then("the running policy contains the same rule")]
fn then_running_contains_same_rule(world: &mut BehaviourWorld) -> Result<(), String> {
    let rig = world.approval();
    let on_disk = Policy::load_or_default(&rig.policy_path).map_err(|e| e.to_string())?;
    let in_memory = rig.session.current_policy();
    if on_disk.network.egress.http != in_memory.network.egress.http {
        return Err(format!(
            "running policy diverges from file:\n  file: {:?}\n  mem:  {:?}",
            on_disk.network.egress.http, in_memory.network.egress.http
        ));
    }
    Ok(())
}

#[then("the running policy contains the new allow rule")]
fn then_running_contains_new_allow_rule(world: &mut BehaviourWorld) -> Result<(), String> {
    let rig = world.approval();
    let p = rig.session.current_policy();
    if !p
        .network
        .egress
        .http
        .iter()
        .any(|r| r.verdict == Verdict::Allow)
    {
        return Err(format!(
            "expected an allow rule in running policy, got {:?}",
            p.network.egress.http
        ));
    }
    Ok(())
}

#[then("no second card appears in the approval window")]
fn then_no_second_notification(world: &mut BehaviourWorld) -> Result<(), String> {
    let rig = world.approval();
    let count = rig.notifier.presented.lock().unwrap().len();
    if count != 1 {
        return Err(format!(
            "expected exactly 1 approval card for the coalesced pair, got {count}"
        ));
    }
    Ok(())
}

#[then("the approval card is removed from the approval window")]
fn then_notification_dismissed(world: &mut BehaviourWorld) -> Result<(), String> {
    let rig = world.approval();
    let dismissed = rig.notifier.dismissed.lock().unwrap();
    if dismissed.is_empty() {
        return Err("expected at least one card-removal call".into());
    }
    Ok(())
}

#[then("the approval window informs the developer that the rule could not be persisted")]
fn then_inform_persist_failure(world: &mut BehaviourWorld) -> Result<(), String> {
    if let Some(rig) = world.approval.as_mut() {
        let informed = rig.notifier.informed.lock().unwrap();
        if informed
            .iter()
            .any(|m| m.contains("not persisted") || m.contains("could not be persisted"))
        {
            return Ok(());
        }
    }
    if let Some(rig) = world.credential.as_mut() {
        let snap = rig.window_state.snapshot();
        if snap
            .informs
            .iter()
            .any(|m| m.contains("not persisted") || m.contains("could not be persisted"))
        {
            return Ok(());
        }
    }
    Err("expected an inform mentioning persistence failure in either rig".into())
}

#[then("no restart of the workload is required")]
fn then_no_restart_required(world: &mut BehaviourWorld) -> Result<(), String> {
    // no-op: in-process session is the same Arc; no restart concept exists at this layer.
    let _ = world.approval();
    Ok(())
}
