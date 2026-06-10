use crate::runner::CliRun;
use crate::world::BehaviourWorld;
use clap::Parser;
use cucumber::{given, then, when};
use lns_cli::cli::{
    Cli, Command, CredentialClearArgs, CredentialCommand, CredentialScopeArgs, CredentialSetArgs,
};
use lns_cli::credential;
use lns_policy::Policy;
use lns_policy::credentials::{
    CredentialEntry, CredentialStateFile, CredentialStore, JsonFileCredentialStore,
};
use lns_policy::providers::{InjectionDef, InjectionKind, ProviderDef};
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
    let creds = dir.join(".lns-credentials.json");
    let catalog = dir.join(".lns-integrations.yaml");
    let stdin = world.stdin.clone().unwrap_or_default();
    let mut reader = stdin.as_bytes();
    let mut buf = Vec::<u8>::new();
    let run = match credential::run(&cmd, &dir, &creds, &catalog, &mut reader, &mut buf) {
        Ok(exit_code) => CliRun {
            exit_code,
            output: String::from_utf8_lossy(&buf).into_owned(),
        },
        Err(e) => CliRun {
            exit_code: 1,
            output: format!("{e:#}"),
        },
    };
    world.result = Some(run);
}

fn run_set_via_clap(world: &mut BehaviourWorld, tail: &[&str]) {
    let dir = cwd(world);
    let creds = dir.join(".lns-credentials.json");
    let catalog = dir.join(".lns-integrations.yaml");
    let stdin = world.stdin.clone().unwrap_or_default();
    let mut reader = stdin.as_bytes();
    let mut full = vec![
        "lns".to_string(),
        "credential".to_string(),
        "set".to_string(),
    ];
    full.extend(tail.iter().map(|s| s.to_string()));
    let run = match Cli::try_parse_from(&full) {
        Ok(cli) => {
            let Command::Credential(args) = cli.command else {
                panic!("expected a credential command");
            };
            let mut buf = Vec::<u8>::new();
            match credential::run(&args.command, &dir, &creds, &catalog, &mut reader, &mut buf) {
                Ok(exit_code) => CliRun {
                    exit_code,
                    output: String::from_utf8_lossy(&buf).into_owned(),
                },
                Err(e) => CliRun {
                    exit_code: 1,
                    output: format!("{e:#}"),
                },
            }
        }
        Err(e) => CliRun {
            exit_code: e.exit_code(),
            output: e.to_string(),
        },
    };
    world.result = Some(run);
}

fn load_state(world: &mut BehaviourWorld) -> CredentialStateFile {
    JsonFileCredentialStore::new(creds_path(world))
        .load()
        .expect("load creds")
}

fn seed_entry(world: &mut BehaviourWorld, id: &str, entry: CredentialEntry) {
    let store = JsonFileCredentialStore::new(creds_path(world));
    let mut state = store.load().expect("load creds");
    state.insert(id.to_string(), entry);
    store.save(&state).expect("seed creds");
}

fn set_args(id: &str) -> CredentialSetArgs {
    CredentialSetArgs {
        id: id.to_string(),
        value: None,
        value_stdin: false,
        host: false,
        deny: false,
        policy: None,
    }
}

#[when(regex = r#"^the developer sets the "([^"]+)" credential to use the host value$"#)]
fn set_host(world: &mut BehaviourWorld, id: String) {
    let mut args = set_args(&id);
    args.host = true;
    run_credential(world, CredentialCommand::Set(args));
}

#[when(regex = r#"^the developer sets the "([^"]+)" credential to a stored value "([^"]+)"$"#)]
fn set_stored(world: &mut BehaviourWorld, id: String, value: String) {
    let mut args = set_args(&id);
    args.value = Some(value);
    run_credential(world, CredentialCommand::Set(args));
}

#[when(regex = r#"^the developer denies the "([^"]+)" credential$"#)]
fn set_deny(world: &mut BehaviourWorld, id: String) {
    let mut args = set_args(&id);
    args.deny = true;
    run_credential(world, CredentialCommand::Set(args));
}

#[when(
    regex = r#"^the developer sets the "([^"]+)" credential to a stored value piped on stdin as "([^"]+)"$"#
)]
fn set_stored_from_stdin(world: &mut BehaviourWorld, id: String, value: String) {
    world.stdin = Some(value);
    let mut args = set_args(&id);
    args.value_stdin = true;
    run_credential(world, CredentialCommand::Set(args));
}

#[when(regex = r#"^the developer sets the "([^"]+)" credential from empty stdin$"#)]
fn set_from_empty_stdin(world: &mut BehaviourWorld, id: String) {
    world.stdin = Some(String::new());
    let mut args = set_args(&id);
    args.value_stdin = true;
    run_credential(world, CredentialCommand::Set(args));
}

#[then(regex = r"^the command fails with a clear error mentioning stdin$")]
fn fails_mentioning_stdin(world: &mut BehaviourWorld) -> Result<(), String> {
    let run = world.result.as_ref().ok_or("no result")?;
    if run.exit_code != 0 && run.output.contains("stdin") {
        Ok(())
    } else {
        Err(format!("expected a stdin error, got: {}", run.output))
    }
}

#[when(regex = r#"^the developer tries to set "([^"]+)" passing both --value and --value-stdin$"#)]
fn try_set_both_value_sources(world: &mut BehaviourWorld, id: String) {
    run_set_via_clap(world, &[&id, "--value", "x", "--value-stdin"]);
}

#[then(regex = r"^the command is rejected for passing two value sources$")]
fn rejected_two_value_sources(world: &mut BehaviourWorld) -> Result<(), String> {
    let run = world.result.as_ref().ok_or("no result")?;
    if run.exit_code != 0 && run.output.contains("--value-stdin") {
        Ok(())
    } else {
        Err(format!(
            "expected a --value/--value-stdin conflict rejection, got (exit {}):\n{}",
            run.exit_code, run.output
        ))
    }
}

#[given(regex = r#"^the developer has declared the "([^"]+)" credential provider$"#)]
fn given_declared_provider(world: &mut BehaviourWorld, id: String) {
    let dir = cwd(world);
    let mut policy = Policy::default();
    policy.credentials.custom_providers.push(ProviderDef {
        id,
        env_var: "SOME_TOKEN".into(),
        placeholder: "some-placeholder-0000000000000000000000".into(),
        injections: vec![InjectionDef {
            kind: InjectionKind::BearerHeader,
            domain: "api.some-provider.example".into(),
            header: None,
        }],
    });
    policy.save_atomic(&dir.join("lns-policy.yaml")).unwrap();
}

#[given(regex = r#"^no provider with id "([^"]+)" is registered as built-in or custom$"#)]
fn no_provider_registered(world: &mut BehaviourWorld, _id: String) {
    let _ = cwd(world);
}

#[when(regex = r#"^the developer tries to set a credential for "([^"]+)"$"#)]
fn try_set_unknown(world: &mut BehaviourWorld, id: String) {
    let mut args = set_args(&id);
    args.value = Some("x".into());
    run_credential(world, CredentialCommand::Set(args));
}

#[given(
    regex = r#"^a sandbox is running with the seeded "([^"]+)" placeholder and no credential rule for "([^"]+)"$"#
)]
fn sandbox_running_no_rule(world: &mut BehaviourWorld, _seeded: String, _id: String) {
    let _ = cwd(world);
}

#[given(
    regex = r#"^"~/\.lns-credentials\.json" has entries: "anthropic" host-detect, "openai" stored, "linear" deny$"#
)]
fn seed_three_entries(world: &mut BehaviourWorld) {
    seed_entry(world, "anthropic", CredentialEntry::HostDetect);
    seed_entry(
        world,
        "openai",
        CredentialEntry::Stored {
            value: "sk-secret-token".into(),
        },
    );
    seed_entry(world, "linear", CredentialEntry::Deny);
}

#[when(regex = r"^the developer lists credential rules$")]
fn list_credentials(world: &mut BehaviourWorld) {
    let _ = cwd(world);
    run_credential(
        world,
        CredentialCommand::List(CredentialScopeArgs { policy: None }),
    );
}

#[given(regex = r#"^"~/\.lns-credentials\.json" has a stored rule for "([^"]+)"$"#)]
fn seed_stored_rule(world: &mut BehaviourWorld, id: String) {
    seed_entry(
        world,
        &id,
        CredentialEntry::Stored {
            value: "some-existing-secret".into(),
        },
    );
}

#[when(regex = r#"^the developer clears the "([^"]+)" credential rule$"#)]
fn clear_rule(world: &mut BehaviourWorld, id: String) {
    run_credential(world, CredentialCommand::Clear(CredentialClearArgs { id }));
}

#[then(
    regex = r#"^"~/\.lns-credentials\.json" gains an entry for "([^"]+)" with kind "host-detect"$"#
)]
fn assert_host_detect(world: &mut BehaviourWorld, id: String) -> Result<(), String> {
    match load_state(world).get(&id) {
        Some(CredentialEntry::HostDetect) => Ok(()),
        other => Err(format!("expected host-detect for {id}, got {other:?}")),
    }
}

#[then(
    regex = r#"^"~/\.lns-credentials\.json" gains an entry for "([^"]+)" with kind "stored" carrying "([^"]+)"$"#
)]
fn assert_stored(world: &mut BehaviourWorld, id: String, value: String) -> Result<(), String> {
    match load_state(world).get(&id) {
        Some(CredentialEntry::Stored { value: v }) if *v == value => Ok(()),
        other => Err(format!("expected stored {value:?} for {id}, got {other:?}")),
    }
}

#[then(regex = r#"^"~/\.lns-credentials\.json" gains an entry for "([^"]+)" with kind "deny"$"#)]
fn assert_deny(world: &mut BehaviourWorld, id: String) -> Result<(), String> {
    match load_state(world).get(&id) {
        Some(CredentialEntry::Deny) => Ok(()),
        other => Err(format!("expected deny for {id}, got {other:?}")),
    }
}

#[then(regex = r"^the command fails with a clear error naming the unknown id$")]
fn fails_naming_unknown(world: &mut BehaviourWorld) -> Result<(), String> {
    let run = world.result.as_ref().ok_or("no result")?;
    if run.exit_code == 0 {
        return Err("expected a non-zero exit code".into());
    }
    if run.output.contains("made-up") {
        Ok(())
    } else {
        Err(format!(
            "error does not name the unknown id: {}",
            run.output
        ))
    }
}

#[then(regex = r#"^"~/\.lns-credentials\.json" is unchanged$"#)]
fn creds_unchanged(world: &mut BehaviourWorld) -> Result<(), String> {
    if creds_path(world).exists() {
        Err("a credentials file should not have been written".into())
    } else {
        Ok(())
    }
}

#[then(regex = r"^the output shows the three ids and their kinds$")]
fn output_shows_three(world: &mut BehaviourWorld) -> Result<(), String> {
    let out = world
        .result
        .as_ref()
        .map(|r| r.output.clone())
        .unwrap_or_default();
    for needle in [
        "anthropic",
        "host value",
        "openai",
        "stored (hidden)",
        "linear",
        "denied",
    ] {
        if !out.contains(needle) {
            return Err(format!("list output missing {needle:?}:\n{out}"));
        }
    }
    Ok(())
}

#[then(regex = r#"^the stored value for "openai" is not printed in plain text by default$"#)]
fn stored_value_masked(world: &mut BehaviourWorld) -> Result<(), String> {
    let out = world
        .result
        .as_ref()
        .map(|r| r.output.clone())
        .unwrap_or_default();
    if out.contains("sk-secret-token") {
        Err(format!("stored value leaked into list output:\n{out}"))
    } else {
        Ok(())
    }
}

#[then(regex = r#"^"~/\.lns-credentials\.json" no longer contains an entry for "([^"]+)"$"#)]
fn no_longer_contains(world: &mut BehaviourWorld, id: String) -> Result<(), String> {
    if load_state(world).contains_key(&id) {
        Err(format!("{id} entry should have been removed"))
    } else {
        Ok(())
    }
}
