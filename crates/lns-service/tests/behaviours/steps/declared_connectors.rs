use cucumber::{given, then, when};
use lns_policy::connectors::{
    AuthKind, Connector, ConnectorRoute, CredentialAuth, OauthAuth, OauthFlow,
};
use lns_policy::providers::{InjectionDef, InjectionKind};
use lns_policy::{Policy, Verdict};
use lns_service::artifact::credential_boot::{
    BootGate, SlotPlan, boot_gate, gate_required_slots, plan_declared_connectors, sign_in_gate_ids,
};
use lns_service::artifact::policy::merge_effective;
use lns_service::artifact::{plan_local_sandbox, resolved_from_sandbox};
use lns_service::credential_flow::connectors::{
    resolve_applied_with_slots, resolve_connectable_with_slots, unknown_connector_ids,
    unknown_connectors_refusal,
};
use lns_service::credential_flow::providers::Provider;

use crate::world::BehaviourWorld;

fn credential_connector(id: &str, env_var: &str, route: Option<&str>) -> Connector {
    let domain = route.unwrap_or("api.example.test");
    Connector {
        id: id.into(),
        name: None,
        auth_kind: AuthKind::Credential,
        routes: route
            .map(|host| {
                vec![ConnectorRoute {
                    match_pattern: host.into(),
                    transport: None,
                    scheme: None,
                    tls_terminate: false,
                    rules: Vec::new(),
                }]
            })
            .unwrap_or_default(),
        credential: Some(CredentialAuth {
            env_var: env_var.into(),
            placeholder: format!("{id}-LNSPLACEHOLDER0000"),
            injections: vec![InjectionDef {
                kind: InjectionKind::BearerHeader,
                domain: domain.into(),
                header: None,
            }],
        }),
        oauth: None,
        token_fallback: None,
    }
}

fn definition_declaring(ids: &[&str]) -> String {
    let list = ids
        .iter()
        .map(|id| format!("\"{id}\""))
        .collect::<Vec<_>>()
        .join(",");
    format!(
        r#"{{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{{"name":"hermes"}},"spec":{{"image":"ghcr.io/team/base:1","connectors":[{list}]}}}}"#
    )
}

fn launch(
    w: &mut BehaviourWorld,
    resolved: anyhow::Result<lns_service::artifact::assembly::ResolvedSandbox>,
) {
    let rig = w.declared.get_or_insert_with(Default::default);
    let resolved = match resolved {
        Ok(resolved) => resolved,
        Err(e) => {
            rig.error = Some(format!("{e:#}"));
            return;
        }
    };
    let mut declared = resolved
        .policy
        .as_ref()
        .map(|p| p.connectors.clone())
        .unwrap_or_default();
    declared.extend(resolved.credentials.iter().map(|slot| slot.name.clone()));
    let unknown = unknown_connector_ids(&declared, &rig.catalog);
    if !unknown.is_empty() {
        rig.error = Some(unknown_connectors_refusal(&unknown));
        return;
    }
    if let Err(failure) = gate_required_slots(&resolved.credentials, &rig.catalog, &rig.store) {
        rig.error = Some(failure.as_message());
        return;
    }
    let plans = plan_declared_connectors(
        &sign_in_gate_ids(&resolved.credentials),
        &rig.catalog,
        &rig.store,
    );
    if boot_gate(&plans) == BootGate::AwaitConnect {
        rig.pending = plans.into_iter().find_map(|plan| match plan {
            SlotPlan::Connect(prompt) => Some(prompt),
            SlotPlan::Armed { .. } => None,
        });
        return;
    }
    let mut policy = merge_effective(resolved.policy.as_ref(), &rig.overlay);
    let applied = resolve_applied_with_slots(&policy, &resolved.credentials, &rig.catalog);
    policy
        .network
        .allowed_routes
        .extend(applied.routes.iter().cloned());
    let declared_connectors = resolved
        .policy
        .as_ref()
        .map(|p| p.connectors.clone())
        .unwrap_or_default();
    let connectable = resolve_connectable_with_slots(
        &policy,
        &resolved.credentials,
        &declared_connectors,
        &rig.catalog,
    );
    rig.providers = applied
        .providers
        .iter()
        .chain(connectable.providers.iter().filter(|p| p.seeds_env()))
        .map(|p| {
            (
                p.id().to_string(),
                p.env_var().to_string(),
                p.placeholder().to_string(),
            )
        })
        .collect();
    rig.offered = connectable
        .providers
        .iter()
        .map(|p| p.id().to_string())
        .collect();
    rig.running_policy = Some(policy);
}

#[given(
    regex = r#"^the machine catalog has a credential connector "([^"]+)" managing "([^"]+)" with a route to "([^"]+)"$"#
)]
fn catalog_has_connector_with_route(w: &mut BehaviourWorld, id: String, env: String, host: String) {
    let rig = w.declared.get_or_insert_with(Default::default);
    rig.catalog
        .push(credential_connector(&id, &env, Some(&host)));
}

#[given(regex = r#"^the machine catalog has a credential connector "([^"]+)" managing "([^"]+)"$"#)]
fn catalog_has_connector(w: &mut BehaviourWorld, id: String, env: String) {
    let rig = w.declared.get_or_insert_with(Default::default);
    rig.catalog.push(credential_connector(&id, &env, None));
}

#[given(regex = r#"^the sandbox definition declares connector "([^"]+)"$"#)]
fn definition_declares_one(w: &mut BehaviourWorld, id: String) {
    let rig = w.declared.get_or_insert_with(Default::default);
    rig.definition = Some(definition_declaring(&[&id]));
}

#[given(regex = r#"^a published sandbox artifact declares connector "([^"]+)"$"#)]
fn published_declares_one(w: &mut BehaviourWorld, id: String) {
    let rig = w.declared.get_or_insert_with(Default::default);
    rig.definition = Some(definition_declaring(&[&id]));
}

#[given("the directory's lns-policy.yaml connects no connectors")]
fn overlay_connects_nothing(w: &mut BehaviourWorld) {
    let rig = w.declared.get_or_insert_with(Default::default);
    rig.overlay = Policy::default();
}

#[given(regex = r#"^the directory's lns-policy.yaml connects "([^"]+)"$"#)]
fn overlay_connects(w: &mut BehaviourWorld, id: String) {
    let rig = w.declared.get_or_insert_with(Default::default);
    rig.overlay.connectors.push(id);
}

#[when("the sandbox is launched")]
fn sandbox_launched(w: &mut BehaviourWorld) {
    let definition = w
        .declared
        .get_or_insert_with(Default::default)
        .definition
        .clone()
        .expect("a Given step must declare the definition");
    launch(w, plan_local_sandbox(definition.as_bytes()));
}

#[when("the published sandbox is launched")]
fn published_sandbox_launched(w: &mut BehaviourWorld) {
    let definition = w
        .declared
        .get_or_insert_with(Default::default)
        .definition
        .clone()
        .expect("a Given step must declare the definition");
    let resolved = lns_artifact::sandbox::parse(definition.as_bytes());
    launch(w, resolved.map(|def| resolved_from_sandbox(&def)));
}

fn oauth_connector(id: &str) -> Connector {
    Connector {
        id: id.into(),
        name: None,
        auth_kind: AuthKind::Oauth,
        routes: Vec::new(),
        credential: None,
        oauth: Some(OauthAuth {
            flow: OauthFlow::Device,
            client_id: Some("some-client".into()),
            client_secret: None,
            scopes: Vec::new(),
            device_authorization_endpoint: Some("https://api.some-oauth.example/device".into()),
            authorization_endpoint: None,
            token_endpoint: "https://api.some-oauth.example/token".into(),
            userinfo_endpoint: None,
            account_field: None,
            env_var: "SOME_OAUTH_TOKEN".into(),
            placeholder: format!("{id}-LNSPLACEHOLDER0000"),
            injections: Vec::new(),
        }),
        token_fallback: None,
    }
}

fn relaunch(w: &mut BehaviourWorld) {
    let definition = w
        .declared
        .get_or_insert_with(Default::default)
        .definition
        .clone()
        .expect("the blocked launch kept its definition");
    launch(w, plan_local_sandbox(definition.as_bytes()));
}

#[given(regex = r#"^the machine catalog has an oauth connector "([^"]+)"$"#)]
fn catalog_has_oauth(w: &mut BehaviourWorld, id: String) {
    let rig = w.declared.get_or_insert_with(Default::default);
    rig.catalog.push(oauth_connector(&id));
}

#[given(regex = r#"^the per-machine credential store has no grant for "([^"]+)"$"#)]
fn store_has_no_grant(w: &mut BehaviourWorld, id: String) {
    let rig = w.declared.get_or_insert_with(Default::default);
    assert!(
        !rig.store.contains_key(&id),
        "the rig store unexpectedly holds a grant for {id}"
    );
}

#[then(regex = r#"^a sign-in prompt for "([^"]+)" is shown before the workload starts$"#)]
fn sign_in_prompt_shown(w: &mut BehaviourWorld, id: String) -> Result<(), String> {
    let rig = w.declared.as_ref().ok_or("no launch happened")?;
    match &rig.pending {
        Some(prompt) if prompt.connector == id => Ok(()),
        Some(prompt) => Err(format!(
            "the launch is blocked on {}, not {id}",
            prompt.connector
        )),
        None => Err("the launch was not blocked on a sign-in".to_string()),
    }
}

#[then("the workload does not start until the sign-in is decided")]
fn workload_waits_for_sign_in(w: &mut BehaviourWorld) -> Result<(), String> {
    let rig = w.declared.as_ref().ok_or("no launch happened")?;
    if rig.running_policy.is_some() {
        return Err("the workload started while the sign-in was undecided".to_string());
    }
    if rig.error.is_some() {
        return Err("the launch errored instead of waiting".to_string());
    }
    Ok(())
}

#[then("the workload starts")]
fn workload_starts(w: &mut BehaviourWorld) -> Result<(), String> {
    let rig = w.declared.as_ref().ok_or("no launch happened")?;
    if rig.running_policy.is_none() {
        return Err(format!(
            "the workload did not start; pending: {:?}, error: {:?}",
            rig.pending, rig.error
        ));
    }
    Ok(())
}

#[given(regex = r#"^a launched sandbox whose definition declares connector "([^"]+)"$"#)]
fn launched_sandbox_declaring(w: &mut BehaviourWorld, id: String) {
    {
        let rig = w.declared.get_or_insert_with(Default::default);
        rig.catalog
            .push(credential_connector(&id, "SOME_TOKEN", None));
        rig.definition = Some(definition_declaring(&[&id]));
        rig.definition_snapshot = rig.definition.clone();
    }
    relaunch(w);
    let rig = w.declared.as_ref().expect("relaunch built the rig");
    assert!(
        rig.running_policy.is_some(),
        "the declared launch must start (a declared connector is offered, not armed); error: {:?}",
        rig.error
    );
    w.credential();
    w.approval();
}

#[given("the directory's lns-policy.yaml denies all by default")]
fn overlay_denies_by_default(w: &mut BehaviourWorld) {
    let rig = w.declared.get_or_insert_with(Default::default);
    rig.overlay.network.default_verdict = Verdict::Deny;
}

#[when(regex = r#"^the developer approves a new destination "([^"]+)" with "always allow"$"#)]
fn developer_approves_destination(w: &mut BehaviourWorld, host: String) {
    use lns_service::approval_flow::protocol::{Decision, RequestPending};
    let rig = w.approval();
    rig.session.submit_pending(
        RequestPending {
            id: format!("req-{host}"),
            host: host.clone(),
            action: format!("CONNECT {host}:443"),
            reason: "policy-ambiguous".into(),
        },
        std::time::Instant::now(),
    );
    let id = rig
        .notifier
        .presented
        .lock()
        .unwrap()
        .last()
        .expect("the approval card must be visible")
        .id
        .clone();
    rig.session.record_decision(&id, Decision::AllowAlways);
}

#[then(regex = r#"^a workload request to "([^"]+)" is denied by policy$"#)]
fn request_denied_by_policy(w: &mut BehaviourWorld, host: String) -> Result<(), String> {
    let policy = w
        .declared
        .as_ref()
        .and_then(|r| r.running_policy.as_ref())
        .ok_or("no running policy was produced")?;
    // The guest gate is first-match-wins; the merged policy must present the deny before the connector's allow.
    let verdict = policy
        .network
        .allowed_routes
        .iter()
        .find(|r| r.match_pattern == host)
        .map(|r| r.verdict)
        .unwrap_or(policy.network.default_verdict);
    if verdict == Verdict::Deny {
        Ok(())
    } else {
        Err(format!(
            "expected {host} to be denied, first match gave {verdict:?}; routes: {:?}",
            policy.network.allowed_routes
        ))
    }
}

#[then("the allow rule is written to the directory's lns-policy.yaml")]
fn allow_rule_written_to_policy_file(w: &mut BehaviourWorld) -> Result<(), String> {
    let rig = w.approval();
    let on_disk = Policy::load_or_default(&rig.policy_path).map_err(|e| e.to_string())?;
    let has_allow = on_disk
        .network
        .allowed_routes
        .iter()
        .any(|r| r.verdict == Verdict::Allow);
    if has_allow {
        Ok(())
    } else {
        Err(format!(
            "no allow rule landed in {}: {:?}",
            rig.policy_path.display(),
            on_disk.network.allowed_routes
        ))
    }
}

#[then("the sandbox definition is not modified")]
fn definition_not_modified(w: &mut BehaviourWorld) -> Result<(), String> {
    let rig = w.declared.as_ref().ok_or("no declared launch happened")?;
    match (&rig.definition, &rig.definition_snapshot) {
        (Some(now), Some(then)) if now == then => Ok(()),
        (now, then) => Err(format!("the definition changed: was {then:?}, now {now:?}")),
    }
}

#[given(regex = r#"^the machine catalog has no connector "([^"]+)"$"#)]
fn catalog_lacks_connector(w: &mut BehaviourWorld, id: String) {
    let rig = w.declared.get_or_insert_with(Default::default);
    assert!(
        rig.catalog.iter().all(|i| i.id != id),
        "the rig catalog unexpectedly knows {id}"
    );
}

#[then("the launch is refused")]
fn launch_refused(w: &mut BehaviourWorld) -> Result<(), String> {
    match w.declared.as_ref().and_then(|r| r.error.as_ref()) {
        Some(_) => Ok(()),
        None => Err("the launch was not refused".to_string()),
    }
}

#[then(regex = r#"^the error names "([^"]+)"$"#)]
fn error_names(w: &mut BehaviourWorld, id: String) -> Result<(), String> {
    let error = w
        .declared
        .as_ref()
        .and_then(|r| r.error.as_ref())
        .ok_or("no launch error was recorded")?;
    if error.contains(&id) {
        Ok(())
    } else {
        Err(format!("expected the error to name {id}, got: {error}"))
    }
}

#[then(regex = r#"^the error points at `lns connector add`$"#)]
fn error_points_at_add(w: &mut BehaviourWorld) -> Result<(), String> {
    let error = w
        .declared
        .as_ref()
        .and_then(|r| r.error.as_ref())
        .ok_or("no launch error was recorded")?;
    if error.contains("`lns connector add`") {
        Ok(())
    } else {
        Err(format!(
            "expected the error to point at `lns connector add`, got: {error}"
        ))
    }
}

#[then(regex = r#"^the workload's environment contains the "([^"]+)" placeholder$"#)]
fn env_contains_placeholder(w: &mut BehaviourWorld, env: String) -> Result<(), String> {
    let rig = w.declared.as_ref().ok_or("no launch happened")?;
    if let Some(err) = &rig.error {
        return Err(format!("the launch failed: {err}"));
    }
    let armed = rig
        .providers
        .iter()
        .find(|(_, var, _)| var == &env)
        .ok_or_else(|| format!("no provider seeds {env}; armed: {:?}", rig.providers))?;
    if armed.2.is_empty() {
        return Err(format!("{env} was seeded without a placeholder"));
    }
    Ok(())
}

#[then(regex = r#"^the running policy allows the "([^"]+)" route$"#)]
fn running_policy_allows(w: &mut BehaviourWorld, host: String) -> Result<(), String> {
    let policy = w
        .declared
        .as_ref()
        .and_then(|r| r.running_policy.as_ref())
        .ok_or("no running policy was produced")?;
    let allowed = policy
        .network
        .allowed_routes
        .iter()
        .any(|r| r.match_pattern == host && r.verdict == Verdict::Allow);
    if allowed {
        Ok(())
    } else {
        Err(format!(
            "expected an allow route for {host}, got: {:?}",
            policy.network.allowed_routes
        ))
    }
}

#[then(regex = r#"^the workload's environment does not seed the "([^"]+)" placeholder$"#)]
fn env_does_not_seed_placeholder(w: &mut BehaviourWorld, env: String) -> Result<(), String> {
    let rig = w.declared.as_ref().ok_or("no launch happened")?;
    if let Some(err) = &rig.error {
        return Err(format!("the launch failed: {err}"));
    }
    match rig.providers.iter().find(|(_, var, _)| var == &env) {
        Some(seeded) => Err(format!(
            "{env} was seeded at launch, but only a declared or connected connector may seed: {seeded:?}"
        )),
        None => Ok(()),
    }
}

#[then(regex = r#"^the running policy does not allow the "([^"]+)" route$"#)]
fn running_policy_does_not_allow(w: &mut BehaviourWorld, host: String) -> Result<(), String> {
    let policy = w
        .declared
        .as_ref()
        .and_then(|r| r.running_policy.as_ref())
        .ok_or("no running policy was produced")?;
    match policy
        .network
        .allowed_routes
        .iter()
        .find(|r| r.match_pattern == host && r.verdict == Verdict::Allow)
    {
        Some(rule) => Err(format!(
            "an allow route for {host} was opened on the sandbox's behalf: {rule:?}"
        )),
        None => Ok(()),
    }
}

#[then(regex = r#"^"([^"]+)" is offered for a reactive connect$"#)]
fn connector_is_offered(w: &mut BehaviourWorld, id: String) -> Result<(), String> {
    let rig = w.declared.as_ref().ok_or("no launch happened")?;
    if rig.offered.contains(&id) {
        Ok(())
    } else {
        Err(format!(
            "expected {id} to be offered for a reactive connect, offered: {:?}",
            rig.offered
        ))
    }
}

fn definition_with_credential_slot(id: &str, env: &str) -> String {
    format!(
        r#"{{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{{"name":"hermes"}},"spec":{{"image":"ghcr.io/team/base:1","credentials":[{{"name":"{id}","env":"{env}"}}]}}}}"#
    )
}

#[given(
    regex = r#"^the sandbox definition declares an optional credential slot "([^"]+)" injected as "([^"]+)"$"#
)]
fn definition_declares_credential_slot(w: &mut BehaviourWorld, id: String, env: String) {
    let rig = w.declared.get_or_insert_with(Default::default);
    rig.definition = Some(definition_with_credential_slot(&id, &env));
}

#[then(regex = r#"^"([^"]+)" is not offered for a reactive connect$"#)]
fn connector_is_not_offered(w: &mut BehaviourWorld, id: String) -> Result<(), String> {
    let rig = w.declared.as_ref().ok_or("no launch happened")?;
    if rig.offered.contains(&id) {
        Err(format!(
            "{id} shares a declared connector's domain and must be suppressed; instead it was offered, so its machine-stored value would arm and inject over the declared credential: {:?}",
            rig.offered
        ))
    } else {
        Ok(())
    }
}
