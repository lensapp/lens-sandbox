use cucumber::{given, then, when};
use std::time::Instant;

use crate::world::BehaviourWorld;
use lns_policy::providers::{InjectionDef, InjectionKind, ProviderDef};
use lns_service::approval_flow::protocol::{Credential, CredentialInjection, CredentialPending};
use lns_service::credential_flow::providers;
use lns_service::credential_flow::registry::expand_credentials_for_wire_with_custom;
use lns_service::credential_flow::store::CredentialStateFile;

const ACME_PLACEHOLDER: &str = "acme_LNSPLACEHOLDER0000000000000000000000";

fn acme_def(domains: &[&str]) -> ProviderDef {
    ProviderDef {
        id: "acme".into(),
        env_var: "ACME_API_KEY".into(),
        placeholder: ACME_PLACEHOLDER.into(),
        injections: domains
            .iter()
            .map(|d| InjectionDef {
                kind: InjectionKind::BearerHeader,
                domain: (*d).into(),
                header: None,
            })
            .collect(),
    }
}

fn policy_with(custom: Vec<ProviderDef>) -> lns_policy::Policy {
    let mut p = lns_policy::Policy::default();
    p.credentials.custom_providers = custom;
    p
}

/// What the supervisor sends in the initial Policy frame — the placeholders the guest seeds as workload env vars at boot.
fn seed(policy: &lns_policy::Policy) -> Vec<Credential> {
    let custom = providers::build_policy_providers(policy);
    expand_credentials_for_wire_with_custom(&CredentialStateFile::new(), &custom)
}

fn find<'a>(creds: &'a [Credential], id: &str) -> Option<&'a Credential> {
    creds.iter().find(|c| c.id == id)
}

fn injection_domain(inj: &CredentialInjection) -> &str {
    match inj {
        CredentialInjection::Header { domain, .. } => domain,
        CredentialInjection::UriPlaceholder { domain, .. } => domain,
    }
}

#[given(
    regex = r#"^"lns-policy\.yaml" declares the "acme" custom provider with env var "ACME_API_KEY"$"#
)]
fn given_declares_acme(world: &mut BehaviourWorld) {
    world.custom_policy = Some(policy_with(vec![acme_def(&["api.acme.corp"])]));
}

#[when(regex = r"^a sandbox is launched against that policy file$")]
fn when_launched(world: &mut BehaviourWorld) {
    let policy = world.custom_policy.clone().expect("policy declared");
    world.launched_seed = Some(seed(&policy));
}

#[then(
    regex = r#"^the workload's environment contains "ACME_API_KEY" set to the declared acme placeholder$"#
)]
fn then_env_has_acme(world: &mut BehaviourWorld) -> Result<(), String> {
    let creds = world.launched_seed.as_ref().ok_or("no launched seed")?;
    let acme = find(creds, "acme").ok_or("acme not seeded")?;
    if acme.env_var.as_deref() == Some("ACME_API_KEY")
        && acme.placeholder.as_deref() == Some(ACME_PLACEHOLDER)
    {
        Ok(())
    } else {
        Err(format!("acme env/placeholder wrong: {acme:?}"))
    }
}

#[then(
    regex = r"^the built-in placeholders for github, openai, anthropic, linear, and telegram are still present$"
)]
fn then_builtins_present(world: &mut BehaviourWorld) -> Result<(), String> {
    let creds = world.launched_seed.as_ref().ok_or("no launched seed")?;
    for id in ["github", "openai", "anthropic", "linear", "telegram"] {
        if find(creds, id).is_none() {
            return Err(format!("missing built-in {id}"));
        }
    }
    Ok(())
}

#[given(regex = r#"^a sandbox is running with the "acme" custom provider declared$"#)]
fn given_running_acme_declared(world: &mut BehaviourWorld) {
    world.custom_policy = Some(policy_with(vec![acme_def(&["api.acme.corp"])]));
    let _ = world.credential();
}

#[given(regex = r#"^no credential rule exists for "acme"$"#)]
fn given_no_rule_for_acme(_world: &mut BehaviourWorld) {}

#[when(regex = r"^the workload sends a request carrying the acme placeholder$")]
fn when_sends_acme(world: &mut BehaviourWorld) {
    // The card must be a consequence of acme genuinely being in the run's provider union:
    // if build_policy_providers dropped it, its placeholder would never reach the wire.
    let policy = world
        .custom_policy
        .clone()
        .expect("a prior step must declare the acme custom provider");
    assert!(
        seed(&policy).iter().any(|c| c.id == "acme"),
        "acme must be in the run's seeded credentials for a placeholder request to be possible"
    );
    let rig = world.credential();
    rig.session.submit_pending(
        CredentialPending {
            id: "cred-acme".into(),
            credential_id: "acme".into(),
            action: "use of acme placeholder".into(),
            reason: "placeholder-unauthorized".into(),
        },
        Instant::now(),
    );
}

#[given(regex = r#"^a sandbox is running with the "acme" custom provider not declared$"#)]
fn given_running_acme_not_declared(world: &mut BehaviourWorld) {
    let policy = lns_policy::Policy::default();
    world.running_seed = Some(seed(&policy));
    world.custom_policy = Some(policy);
}

#[when(regex = r#"^the developer declares the "acme" custom provider in the loaded policy file$"#)]
fn when_declares_mid_run(world: &mut BehaviourWorld) {
    let mut policy = world.custom_policy.clone().unwrap_or_default();
    policy
        .credentials
        .custom_providers
        .push(acme_def(&["api.acme.corp"]));
    world.custom_policy = Some(policy);
}

#[then(regex = r#"^the running workload's environment does not contain "ACME_API_KEY"$"#)]
fn then_running_env_no_acme(world: &mut BehaviourWorld) -> Result<(), String> {
    let creds = world.running_seed.as_ref().ok_or("no running seed")?;
    if find(creds, "acme").is_some() {
        Err("running workload must not gain the placeholder mid-run".into())
    } else {
        Ok(())
    }
}

#[then(
    regex = r#"^a new sandbox launched against the same policy file contains "ACME_API_KEY" set to the acme placeholder$"#
)]
fn then_new_sandbox_has_acme(world: &mut BehaviourWorld) -> Result<(), String> {
    let policy = world.custom_policy.clone().expect("policy");
    let creds = seed(&policy);
    let acme = find(&creds, "acme").ok_or("acme not seeded for a new sandbox")?;
    if acme.placeholder.as_deref() == Some(ACME_PLACEHOLDER) {
        Ok(())
    } else {
        Err("new sandbox acme placeholder wrong".into())
    }
}

#[given(
    regex = r#"^a sandbox is running with the "acme" custom provider declaring injections for "api\.acme\.corp" and "api-eu\.acme\.corp"$"#
)]
fn given_multi_domain(world: &mut BehaviourWorld) {
    let policy = policy_with(vec![acme_def(&["api.acme.corp", "api-eu.acme.corp"])]);
    world.launched_seed = Some(seed(&policy));
    world.custom_policy = Some(policy);
}

#[then(
    regex = r#"^the seeded credentials declare a boundary injection for each of "api\.acme\.corp" and "api-eu\.acme\.corp"$"#
)]
fn then_injection_per_domain(world: &mut BehaviourWorld) -> Result<(), String> {
    let creds = world.launched_seed.as_ref().ok_or("no launched seed")?;
    let acme = find(creds, "acme").ok_or("no acme")?;
    let domains: Vec<&str> = acme.injections.iter().map(injection_domain).collect();
    for d in ["api.acme.corp", "api-eu.acme.corp"] {
        if !domains.contains(&d) {
            return Err(format!("no boundary injection for {d}; got {domains:?}"));
        }
    }
    Ok(())
}
