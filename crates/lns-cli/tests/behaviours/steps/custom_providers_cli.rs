use crate::runner::CliRun;
use crate::world::BehaviourWorld;
use clap::Parser;
use cucumber::{given, then, when};
use lns_cli::cli::{
    Cli, Command, CredentialAddArgs, CredentialCommand, CredentialInjectArgs, CredentialRemoveArgs,
    CredentialScopeArgs,
};
use lns_cli::credential;
use lns_policy::Policy;
use lns_policy::providers::{InjectionDef, InjectionKind, ProviderDef, is_self_identifying};
use std::path::PathBuf;

fn cwd(world: &mut BehaviourWorld) -> PathBuf {
    if world.cwd.is_none() {
        world.cwd = Some(tempfile::TempDir::new().expect("create tempdir"));
    }
    world.cwd.as_ref().unwrap().path().to_path_buf()
}

fn creds_path(world: &mut BehaviourWorld) -> PathBuf {
    cwd(world).join(".lns-credentials.json")
}

fn run_credential(world: &mut BehaviourWorld, cmd: CredentialCommand) {
    let dir = cwd(world);
    let creds = creds_path(world);
    let catalog = dir.join(".lns-integrations.yaml");
    let stdin = world.stdin.clone().unwrap_or_default();
    let mut reader = stdin.as_bytes();
    let mut out = Vec::<u8>::new();
    let run = match credential::run(&cmd, &dir, &creds, &catalog, &mut reader, &mut out) {
        Ok(exit_code) => CliRun {
            exit_code,
            output: String::from_utf8_lossy(&out).into_owned(),
        },
        Err(e) => CliRun {
            exit_code: 1,
            output: format!("{e:#}"),
        },
    };
    world.result = Some(run);
}

fn run_add_via_clap(world: &mut BehaviourWorld, tail: &[&str]) {
    let dir = cwd(world);
    let creds = creds_path(world);
    let catalog = dir.join(".lns-integrations.yaml");
    let mut full = vec![
        "lns".to_string(),
        "credential".to_string(),
        "add".to_string(),
    ];
    full.extend(tail.iter().map(|s| s.to_string()));
    match Cli::try_parse_from(&full) {
        Ok(cli) => {
            let Command::Credential(args) = cli.command else {
                panic!("expected a credential command");
            };
            let mut reader = std::io::empty();
            let mut out = Vec::<u8>::new();
            let run =
                match credential::run(&args.command, &dir, &creds, &catalog, &mut reader, &mut out)
                {
                    Ok(exit_code) => CliRun {
                        exit_code,
                        output: String::from_utf8_lossy(&out).into_owned(),
                    },
                    Err(e) => CliRun {
                        exit_code: 1,
                        output: format!("{e:#}"),
                    },
                };
            world.result = Some(run);
        }
        Err(e) => {
            world.result = Some(CliRun {
                exit_code: e.exit_code(),
                output: e.to_string(),
            });
        }
    }
}

fn load_policy(world: &mut BehaviourWorld) -> Policy {
    let dir = cwd(world);
    Policy::load_or_default(&dir.join("lns-policy.yaml")).expect("load policy")
}

fn declared(policy: &Policy, id: &str) -> bool {
    policy
        .credentials
        .custom_providers
        .iter()
        .any(|p| p.id == id)
}

fn inject(kind: &str, domain: &str) -> InjectionDef {
    let kind = match kind {
        "bearer_header" => InjectionKind::BearerHeader,
        "uri_placeholder" => InjectionKind::UriPlaceholder,
        other => panic!("test only declares valid kinds, got {other:?}"),
    };
    InjectionDef {
        kind,
        domain: domain.into(),
        header: None,
    }
}

fn add_args(
    id: &str,
    env_var: &str,
    placeholder: Option<&str>,
    inj: InjectionDef,
) -> CredentialAddArgs {
    CredentialAddArgs {
        id: id.into(),
        env_var: env_var.into(),
        inject: vec![inj],
        placeholder: placeholder.map(str::to_string),
        value: None,
        value_stdin: false,
        policy: None,
    }
}

fn seed_custom_acme(world: &mut BehaviourWorld) {
    let dir = cwd(world);
    let mut policy = Policy::default();
    policy.credentials.custom_providers.push(ProviderDef {
        id: "acme".into(),
        env_var: "ACME_API_KEY".into(),
        placeholder: "acme_LNSPLACEHOLDER0000000000000000000000".into(),
        injections: vec![inject("bearer_header", "api.acme.corp")],
    });
    policy.save_atomic(&dir.join("lns-policy.yaml")).unwrap();
}

#[when(
    regex = r#"^the developer declares a custom provider "([^"]+)" with env var "([^"]+)", placeholder "([^"]+)", injection kind "([^"]+)", domain "([^"]+)"$"#
)]
fn declare_with_placeholder(
    world: &mut BehaviourWorld,
    id: String,
    env_var: String,
    placeholder: String,
    kind: String,
    domain: String,
) {
    let args = add_args(&id, &env_var, Some(&placeholder), inject(&kind, &domain));
    run_credential(world, CredentialCommand::Add(args));
}

#[when(
    regex = r#"^the developer declares a custom provider "([^"]+)" with env var "([^"]+)" and no placeholder, injection kind "([^"]+)", domain "([^"]+)"$"#
)]
fn declare_without_placeholder(
    world: &mut BehaviourWorld,
    id: String,
    env_var: String,
    kind: String,
    domain: String,
) {
    let args = add_args(&id, &env_var, None, inject(&kind, &domain));
    run_credential(world, CredentialCommand::Add(args));
}

#[when(
    regex = r#"^the developer declares a custom provider "([^"]+)" with env var "([^"]+)", injection kind "([^"]+)", domain "([^"]+)", and value "([^"]+)"$"#
)]
fn declare_with_value(
    world: &mut BehaviourWorld,
    id: String,
    env_var: String,
    kind: String,
    domain: String,
    value: String,
) {
    let mut args = add_args(&id, &env_var, None, inject(&kind, &domain));
    args.value = Some(value);
    run_credential(world, CredentialCommand::Add(args));
}

#[when(
    regex = r#"^the developer declares a custom provider "([^"]+)" with env var "([^"]+)", injection kind "([^"]+)", domain "([^"]+)", and a value piped on stdin as "([^"]+)"$"#
)]
fn declare_with_stdin_value(
    world: &mut BehaviourWorld,
    id: String,
    env_var: String,
    kind: String,
    domain: String,
    value: String,
) {
    world.stdin = Some(value);
    let mut args = add_args(&id, &env_var, None, inject(&kind, &domain));
    args.value_stdin = true;
    run_credential(world, CredentialCommand::Add(args));
}

#[when(
    regex = r"^the developer tries to declare a custom provider passing both --value and --value-stdin$"
)]
fn try_declare_both_value_sources(world: &mut BehaviourWorld) {
    run_add_via_clap(
        world,
        &[
            "acme",
            "--env-var",
            "ACME_API_KEY",
            "--inject",
            "bearer_header:api.acme.corp",
            "--value",
            "x",
            "--value-stdin",
        ],
    );
}

#[when(
    regex = r#"^the developer tries to declare a custom provider with injection kind "awsSigv4"$"#
)]
fn try_declare_awssigv4(world: &mut BehaviourWorld) {
    run_add_via_clap(
        world,
        &[
            "x",
            "--env-var",
            "X_TOKEN",
            "--inject",
            "awsSigv4:*.amazonaws.com",
        ],
    );
}

#[when(regex = r#"^the developer tries to declare a custom provider with id "([^"]+)"$"#)]
fn try_declare_id(world: &mut BehaviourWorld, id: String) {
    let args = add_args(
        &id,
        "X_TOKEN",
        None,
        inject("bearer_header", "api.x.example"),
    );
    run_credential(world, CredentialCommand::Add(args));
}

#[when(regex = r#"^the developer tries to declare another custom provider with id "([^"]+)"$"#)]
fn try_declare_duplicate(world: &mut BehaviourWorld, id: String) {
    let args = add_args(
        &id,
        "X_TOKEN",
        None,
        inject("bearer_header", "api.x.example"),
    );
    run_credential(world, CredentialCommand::Add(args));
}

#[when(regex = r#"^the developer tries to declare a custom provider with placeholder "([^"]+)"$"#)]
fn try_declare_bad_placeholder(world: &mut BehaviourWorld, placeholder: String) {
    let args = add_args(
        "acme",
        "ACME_API_KEY",
        Some(&placeholder),
        inject("bearer_header", "api.acme.corp"),
    );
    run_credential(world, CredentialCommand::Add(args));
}

#[given(
    regex = r#"^the developer has declared a custom provider "acme" with a bearer_header injection for "api\.acme\.corp"$"#
)]
fn given_acme_one_injection(world: &mut BehaviourWorld) {
    seed_custom_acme(world);
}

#[when(
    regex = r#"^the developer adds a second bearer_header injection for "([^"]+)" to "([^"]+)"$"#
)]
fn add_second_injection(world: &mut BehaviourWorld, domain: String, id: String) {
    run_credential(
        world,
        CredentialCommand::AddInjection(CredentialInjectArgs {
            id,
            inject: inject("bearer_header", &domain),
            policy: None,
        }),
    );
}

#[given(regex = r#"^"lns-policy\.yaml" (?:already )?declares a custom provider "acme"$"#)]
fn given_declares_acme(world: &mut BehaviourWorld) {
    seed_custom_acme(world);
}

#[when(regex = r"^the developer lists providers$")]
fn list_providers(world: &mut BehaviourWorld) {
    let _ = cwd(world);
    run_credential(
        world,
        CredentialCommand::List(CredentialScopeArgs { policy: None }),
    );
}

#[when(regex = r#"^the developer removes the "([^"]+)" custom provider$"#)]
fn remove_custom(world: &mut BehaviourWorld, id: String) {
    run_credential(
        world,
        CredentialCommand::Remove(CredentialRemoveArgs { id, policy: None }),
    );
}

#[when(regex = r#"^the developer tries to remove the built-in "([^"]+)" provider$"#)]
fn try_remove_builtin(world: &mut BehaviourWorld, id: String) {
    run_credential(
        world,
        CredentialCommand::Remove(CredentialRemoveArgs { id, policy: None }),
    );
}

#[given(regex = r"^a sandbox is running$")]
fn given_sandbox_running(world: &mut BehaviourWorld) {
    let _ = cwd(world);
}

#[when(regex = r#"^the developer declares a new custom provider "([^"]+)"$"#)]
fn declare_new(world: &mut BehaviourWorld, id: String) {
    let args = add_args(
        &id,
        "ACME_API_KEY",
        None,
        inject("bearer_header", "api.acme.corp"),
    );
    run_credential(world, CredentialCommand::Add(args));
}

#[then(regex = r#"^"lns-policy\.yaml" contains the "([^"]+)" custom provider declaration$"#)]
fn policy_contains_provider(world: &mut BehaviourWorld, id: String) -> Result<(), String> {
    let policy = load_policy(world);
    if declared(&policy, &id) {
        Ok(())
    } else {
        Err(format!("policy has no custom provider {id}"))
    }
}

#[then(regex = r"^the declaration carries no resolved credential value$")]
fn declaration_has_no_value(world: &mut BehaviourWorld) -> Result<(), String> {
    let dir = cwd(world);
    let text = std::fs::read_to_string(dir.join("lns-policy.yaml")).map_err(|e| e.to_string())?;
    // The value-bearing scenario passes "acme_real"; it must land only in the per-machine
    // credentials file, never in the shareable policy declaration.
    if text.contains("acme_real") {
        Err(format!(
            "a real credential value leaked into the shareable policy file:\n{text}"
        ))
    } else {
        Ok(())
    }
}

#[then(regex = r#"^the "([^"]+)" placeholder self-identifies as a placeholder$"#)]
fn placeholder_self_identifies(world: &mut BehaviourWorld, id: String) -> Result<(), String> {
    let policy = load_policy(world);
    let def = policy
        .credentials
        .custom_providers
        .iter()
        .find(|p| p.id == id)
        .ok_or_else(|| format!("no custom provider {id}"))?;
    if is_self_identifying(&def.placeholder) {
        Ok(())
    } else {
        Err(format!(
            "generated placeholder is not self-identifying: {}",
            def.placeholder
        ))
    }
}

#[then(
    regex = r"^the command fails with a clear error explaining that awsSigv4 is not declarable from the CLI$"
)]
fn fails_awssigv4(world: &mut BehaviourWorld) -> Result<(), String> {
    let run = world.result.as_ref().ok_or("no result")?;
    if run.exit_code != 0 && run.output.contains("awsSigv4") {
        Ok(())
    } else {
        Err(format!(
            "expected an awsSigv4 rejection, got: {}",
            run.output
        ))
    }
}

#[then(regex = r"^the command fails with a clear error$")]
fn command_fails(world: &mut BehaviourWorld) -> Result<(), String> {
    match world.result.as_ref().map(|r| r.exit_code) {
        Some(0) | None => Err("expected a non-zero exit code".into()),
        Some(_) => Ok(()),
    }
}

#[then(regex = r#"^"lns-policy\.yaml" is unchanged$"#)]
fn policy_unchanged(world: &mut BehaviourWorld) -> Result<(), String> {
    let policy = load_policy(world);
    if policy.credentials.custom_providers.is_empty() {
        Ok(())
    } else {
        Err("a rejected declaration must not be written to the policy file".into())
    }
}

#[then(regex = r#"^the existing "([^"]+)" declaration is unchanged$"#)]
fn existing_declaration_unchanged(world: &mut BehaviourWorld, id: String) -> Result<(), String> {
    let policy = load_policy(world);
    let count = policy
        .credentials
        .custom_providers
        .iter()
        .filter(|p| p.id == id)
        .count();
    if count == 1 {
        Ok(())
    } else {
        Err(format!(
            "expected exactly one {id} declaration, found {count}"
        ))
    }
}

#[then(regex = r"^the output shows the five built-in providers labelled as built-in$")]
fn output_shows_builtins(world: &mut BehaviourWorld) -> Result<(), String> {
    let out = world
        .result
        .as_ref()
        .map(|r| r.output.clone())
        .unwrap_or_default();
    for id in ["github", "openai", "anthropic", "linear", "telegram"] {
        if !out.contains(&format!("{id}  (built-in)")) {
            return Err(format!("missing built-in {id} in:\n{out}"));
        }
    }
    Ok(())
}

#[then(regex = r#"^the "([^"]+)" provider labelled as custom from the policy file$"#)]
fn output_labels_custom(world: &mut BehaviourWorld, id: String) -> Result<(), String> {
    let out = world
        .result
        .as_ref()
        .map(|r| r.output.clone())
        .unwrap_or_default();
    if out.contains(&format!("{id}  (custom)")) {
        Ok(())
    } else {
        Err(format!("{id} not labelled custom in:\n{out}"))
    }
}

#[then(regex = r#"^"lns-policy\.yaml" shows the "([^"]+)" provider with two injection entries$"#)]
fn provider_has_two_injections(world: &mut BehaviourWorld, id: String) -> Result<(), String> {
    let policy = load_policy(world);
    let def = policy
        .credentials
        .custom_providers
        .iter()
        .find(|p| p.id == id)
        .ok_or_else(|| format!("no custom provider {id}"))?;
    if def.injections.len() == 2 {
        Ok(())
    } else {
        Err(format!(
            "expected two injections, found {}",
            def.injections.len()
        ))
    }
}

#[then(
    regex = r"^the command output states that the new injection will not reach a running workload until a new sandbox is launched$"
)]
fn output_states_injection_relaunch(world: &mut BehaviourWorld) -> Result<(), String> {
    let out = world
        .result
        .as_ref()
        .map(|r| r.output.clone())
        .unwrap_or_default();
    if out.contains("relaunch") && out.contains("launch") {
        Ok(())
    } else {
        Err(format!(
            "relaunch notice missing from add-injection output:\n{out}"
        ))
    }
}

#[then(regex = r#"^"lns-policy\.yaml" no longer contains the "([^"]+)" declaration$"#)]
fn policy_no_longer_contains(world: &mut BehaviourWorld, id: String) -> Result<(), String> {
    let policy = load_policy(world);
    if declared(&policy, &id) {
        Err(format!("{id} declaration should have been removed"))
    } else {
        Ok(())
    }
}

#[then(regex = r"^the command succeeds$")]
fn command_succeeds(world: &mut BehaviourWorld) -> Result<(), String> {
    match world.result.as_ref().map(|r| r.exit_code) {
        Some(0) => Ok(()),
        other => Err(format!("expected exit 0, got {other:?}")),
    }
}

#[then(
    regex = r"^the command output states that the new placeholder will not appear in the running workload's environment until a new sandbox is launched$"
)]
fn output_states_relaunch(world: &mut BehaviourWorld) -> Result<(), String> {
    let out = world
        .result
        .as_ref()
        .map(|r| r.output.clone())
        .unwrap_or_default();
    if out.contains("relaunch") && out.contains("launch") {
        Ok(())
    } else {
        Err(format!("relaunch notice missing from output:\n{out}"))
    }
}
