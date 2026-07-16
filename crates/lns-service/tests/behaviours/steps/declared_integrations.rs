use cucumber::{given, then, when};
use lns_policy::credentials::CredentialEntry;
use lns_policy::integrations::{
    AuthKind, CredentialAuth, Integration, IntegrationRoute, OauthAuth, OauthFlow,
};
use lns_policy::providers::{InjectionDef, InjectionKind};
use lns_policy::{Policy, Verdict};
use lns_service::artifact::credential_boot::{
    BootGate, ConnectChoice, SlotPlan, boot_gate, plan_declared_integrations, resolve_connect,
};
use lns_service::artifact::policy::merge_effective;
use lns_service::artifact::{plan_local_sandbox, resolved_from_sandbox};
use lns_service::credential_flow::integrations::{
    resolve_applied_integrations, unknown_integration_ids, unknown_integrations_refusal,
};
use lns_service::credential_flow::providers::Provider;

use crate::world::BehaviourWorld;

fn credential_integration(id: &str, env_var: &str, route: Option<&str>) -> Integration {
    let domain = route.unwrap_or("api.example.test");
    Integration {
        id: id.into(),
        name: None,
        auth_kind: AuthKind::Credential,
        routes: route
            .map(|host| {
                vec![IntegrationRoute {
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
        r#"{{"apiVersion":"lns.run/v1","kind":"Sandbox","metadata":{{"name":"hermes"}},"spec":{{"image":"ghcr.io/team/base:1","integrations":[{list}]}}}}"#
    )
}

fn launch(
    w: &mut BehaviourWorld,
    resolved: anyhow::Result<lns_service::artifact::assembly::ResolvedBundle>,
) {
    let rig = w.declared.get_or_insert_with(Default::default);
    let resolved = match resolved {
        Ok(resolved) => resolved,
        Err(e) => {
            rig.error = Some(format!("{e:#}"));
            return;
        }
    };
    let declared = resolved
        .policy
        .as_ref()
        .map(|p| p.integrations.clone())
        .unwrap_or_default();
    let unknown = unknown_integration_ids(&declared, &rig.catalog);
    if !unknown.is_empty() {
        rig.error = Some(unknown_integrations_refusal(&unknown));
        return;
    }
    let plans = plan_declared_integrations(&declared, &rig.catalog, &rig.store);
    if boot_gate(&plans) == BootGate::AwaitConnect {
        rig.pending = plans.into_iter().find_map(|plan| match plan {
            SlotPlan::Connect(prompt) => Some(prompt),
            SlotPlan::Armed { .. } => None,
        });
        return;
    }
    let mut policy = merge_effective(resolved.policy.as_ref(), &rig.overlay);
    let applied = resolve_applied_integrations(&policy, &rig.catalog);
    policy
        .network
        .allowed_routes
        .extend(applied.routes.iter().cloned());
    rig.providers = applied
        .providers
        .iter()
        .map(|p| {
            (
                p.id().to_string(),
                p.env_var().to_string(),
                p.placeholder().to_string(),
            )
        })
        .collect();
    rig.running_policy = Some(policy);
}

#[given(
    regex = r#"^the machine catalog has a credential integration "([^"]+)" managing "([^"]+)" with a route to "([^"]+)"$"#
)]
fn catalog_has_integration_with_route(
    w: &mut BehaviourWorld,
    id: String,
    env: String,
    host: String,
) {
    let rig = w.declared.get_or_insert_with(Default::default);
    rig.catalog
        .push(credential_integration(&id, &env, Some(&host)));
}

#[given(
    regex = r#"^the machine catalog has a credential integration "([^"]+)" managing "([^"]+)"$"#
)]
fn catalog_has_integration(w: &mut BehaviourWorld, id: String, env: String) {
    let rig = w.declared.get_or_insert_with(Default::default);
    rig.catalog.push(credential_integration(&id, &env, None));
}

#[given(regex = r#"^the machine catalog has credential integrations "([^"]+)" and "([^"]+)"$"#)]
fn catalog_has_two_integrations(w: &mut BehaviourWorld, first: String, second: String) {
    let rig = w.declared.get_or_insert_with(Default::default);
    for id in [&first, &second] {
        let env = format!("{}_TOKEN", id.to_uppercase().replace('-', "_"));
        rig.catalog.push(credential_integration(id, &env, None));
    }
}

#[given(regex = r#"^the sandbox definition declares integration "([^"]+)"$"#)]
fn definition_declares_one(w: &mut BehaviourWorld, id: String) {
    let rig = w.declared.get_or_insert_with(Default::default);
    rig.definition = Some(definition_declaring(&[&id]));
}

#[given(regex = r#"^the sandbox definition declares integrations "([^"]+)" and "([^"]+)"$"#)]
fn definition_declares_two(w: &mut BehaviourWorld, first: String, second: String) {
    let rig = w.declared.get_or_insert_with(Default::default);
    rig.definition = Some(definition_declaring(&[&first, &second]));
}

#[given(regex = r#"^a published sandbox artifact declares integration "([^"]+)"$"#)]
fn published_declares_one(w: &mut BehaviourWorld, id: String) {
    let rig = w.declared.get_or_insert_with(Default::default);
    rig.definition = Some(definition_declaring(&[&id]));
}

#[given("the directory's lns-policy.yaml connects no integrations")]
fn overlay_connects_nothing(w: &mut BehaviourWorld) {
    let rig = w.declared.get_or_insert_with(Default::default);
    rig.overlay = Policy::default();
}

#[given(regex = r#"^the directory's lns-policy.yaml connects "([^"]+)"$"#)]
fn overlay_connects(w: &mut BehaviourWorld, id: String) {
    let rig = w.declared.get_or_insert_with(Default::default);
    rig.overlay.integrations.push(id);
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

fn oauth_integration(id: &str) -> Integration {
    Integration {
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

#[given(regex = r#"^the machine catalog has an oauth integration "([^"]+)"$"#)]
fn catalog_has_oauth(w: &mut BehaviourWorld, id: String) {
    let rig = w.declared.get_or_insert_with(Default::default);
    rig.catalog.push(oauth_integration(&id));
}

#[given(regex = r#"^the per-machine credential store has no grant for "([^"]+)"$"#)]
fn store_has_no_grant(w: &mut BehaviourWorld, id: String) {
    let rig = w.declared.get_or_insert_with(Default::default);
    assert!(
        !rig.store.contains_key(&id),
        "the rig store unexpectedly holds a grant for {id}"
    );
}

#[given(regex = r#"^a launch blocked on the "([^"]+)" sign-in$"#)]
fn launch_blocked_on_sign_in(w: &mut BehaviourWorld, id: String) {
    {
        let rig = w.declared.get_or_insert_with(Default::default);
        rig.catalog.push(oauth_integration(&id));
        rig.definition = Some(definition_declaring(&[&id]));
    }
    relaunch(w);
    let rig = w.declared.as_ref().expect("relaunch built the rig");
    assert_eq!(
        rig.pending.as_ref().map(|p| p.integration.as_str()),
        Some(id.as_str()),
        "the launch must be blocked on the {id} sign-in"
    );
}

#[when("the sign-in completes")]
fn sign_in_completes(w: &mut BehaviourWorld) {
    {
        let rig = w.declared.as_mut().expect("a launch is blocked");
        let prompt = rig.pending.take().expect("a sign-in is pending");
        assert!(
            resolve_connect(&prompt, ConnectChoice::Connect).starts_workload(),
            "a completed sign-in must release the launch"
        );
        rig.store.insert(
            prompt.integration.clone(),
            CredentialEntry::Oauth {
                access_token: "some-access".into(),
                refresh_token: "some-refresh".into(),
                expires_at: 9999,
                scopes: vec![],
                account: None,
            },
        );
    }
    relaunch(w);
}

#[when("the developer declines the sign-in")]
fn sign_in_declined(w: &mut BehaviourWorld) {
    let rig = w.declared.as_mut().expect("a launch is blocked");
    let prompt = rig.pending.take().expect("a sign-in is pending");
    let outcome = resolve_connect(&prompt, ConnectChoice::Decline);
    if !outcome.starts_workload() {
        rig.aborted = true;
        rig.error = Some(format!(
            "sign-in for integration {} did not complete; launch aborted",
            prompt.integration
        ));
    }
}

#[then(regex = r#"^a sign-in prompt for "([^"]+)" is shown before the workload starts$"#)]
fn sign_in_prompt_shown(w: &mut BehaviourWorld, id: String) -> Result<(), String> {
    let rig = w.declared.as_ref().ok_or("no launch happened")?;
    match &rig.pending {
        Some(prompt) if prompt.integration == id => Ok(()),
        Some(prompt) => Err(format!(
            "the launch is blocked on {}, not {id}",
            prompt.integration
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

#[then(regex = r#"^the workload starts with the "([^"]+)" placeholder seeded$"#)]
fn workload_starts_with_placeholder(w: &mut BehaviourWorld, id: String) -> Result<(), String> {
    let rig = w.declared.as_ref().ok_or("no launch happened")?;
    if rig.running_policy.is_none() {
        return Err(format!(
            "the workload did not start; pending: {:?}, error: {:?}",
            rig.pending, rig.error
        ));
    }
    let armed = rig
        .providers
        .iter()
        .find(|(pid, _, _)| pid == &id)
        .ok_or_else(|| format!("no provider seeds {id}; armed: {:?}", rig.providers))?;
    if armed.2.is_empty() {
        return Err(format!("{id} was seeded without a placeholder"));
    }
    Ok(())
}

#[given(regex = r#"^a launched sandbox whose definition declares integration "([^"]+)"$"#)]
fn launched_sandbox_declaring(w: &mut BehaviourWorld, id: String) {
    {
        let rig = w.declared.get_or_insert_with(Default::default);
        rig.catalog
            .push(credential_integration(&id, "SOME_TOKEN", None));
        rig.definition = Some(definition_declaring(&[&id]));
        rig.definition_snapshot = rig.definition.clone();
    }
    relaunch(w);
    let rig = w.declared.as_ref().expect("relaunch built the rig");
    assert!(
        rig.running_policy.is_some(),
        "the declared launch must arm and start; error: {:?}",
        rig.error
    );
    w.credential();
    w.approval();
}

#[given(regex = r#"^the directory's lns-policy.yaml denies "([^"]+)"$"#)]
fn overlay_denies(w: &mut BehaviourWorld, host: String) {
    let rig = w.declared.get_or_insert_with(Default::default);
    rig.overlay
        .network
        .allowed_routes
        .push(lns_policy::RouteRule::deny_host(host));
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
    // The guest gate is first-match-wins; the merged policy must present the deny before the integration's allow.
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

#[given(regex = r#"^the machine catalog has no integration "([^"]+)"$"#)]
fn catalog_lacks_integration(w: &mut BehaviourWorld, id: String) {
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

#[then(regex = r#"^the error points at `lns integration add`$"#)]
fn error_points_at_add(w: &mut BehaviourWorld) -> Result<(), String> {
    let error = w
        .declared
        .as_ref()
        .and_then(|r| r.error.as_ref())
        .ok_or("no launch error was recorded")?;
    if error.contains("`lns integration add`") {
        Ok(())
    } else {
        Err(format!(
            "expected the error to point at `lns integration add`, got: {error}"
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

#[then(regex = r#"^the workload's environment seeds "([^"]+)" and "([^"]+)" each exactly once$"#)]
fn env_seeds_each_once(
    w: &mut BehaviourWorld,
    first: String,
    second: String,
) -> Result<(), String> {
    let rig = w.declared.as_ref().ok_or("no launch happened")?;
    for id in [&first, &second] {
        let count = rig.providers.iter().filter(|(pid, _, _)| pid == id).count();
        if count != 1 {
            return Err(format!(
                "expected {id} to be seeded exactly once, found {count}; armed: {:?}",
                rig.providers
            ));
        }
    }
    Ok(())
}

#[then("the launch is aborted")]
fn launch_aborted(w: &mut BehaviourWorld) {
    let rig = w.declared.as_ref().expect("a launch happened");
    assert!(rig.aborted, "the launch was not aborted");
}

#[then("the workload never starts")]
fn workload_never_starts(w: &mut BehaviourWorld) {
    let rig = w.declared.as_ref().expect("a launch happened");
    assert!(rig.running_policy.is_none(), "the workload started");
}
