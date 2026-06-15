use crate::runner::CliRun;
use crate::world::BehaviourWorld;
use cucumber::{given, then, when};
use lns_cli::auth;
use lns_cli::cli::{LoginArgs, LogoutArgs};
use lns_policy::registry_auth::{
    JsonFileRegistryCredentialStore, RegistryCredential, RegistryCredentialStore,
};
use std::path::PathBuf;

fn cwd(world: &mut BehaviourWorld) -> PathBuf {
    if world.cwd.is_none() {
        world.cwd = Some(tempfile::TempDir::new().expect("create tempdir"));
    }
    world.cwd.as_ref().unwrap().path().to_path_buf()
}

fn store(world: &mut BehaviourWorld) -> JsonFileRegistryCredentialStore {
    JsonFileRegistryCredentialStore::new(cwd(world).join(".lns-registry-auth.json"))
}

fn capture(buf: Vec<u8>, result: anyhow::Result<i32>) -> CliRun {
    match result {
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

#[given(
    regex = r#"^a stored credential for "([^"]+)" with username "([^"]+)" and token "([^"]+)"$"#
)]
fn seed_credential(world: &mut BehaviourWorld, registry: String, username: String, token: String) {
    let store = store(world);
    let mut state = store.load().expect("load");
    state.insert(
        registry,
        RegistryCredential {
            username: Some(username),
            token,
        },
    );
    store.save(&state).expect("save");
}

#[when(
    regex = r#"^the developer logs in to "([^"]+)" with username "([^"]+)" and token "([^"]+)"$"#
)]
fn login_with_token(world: &mut BehaviourWorld, registry: String, username: String, token: String) {
    let store = store(world);
    let args = LoginArgs {
        registry,
        username: Some(username),
        password_stdin: true,
    };
    let mut buf = Vec::new();
    let res = auth::login(&args, &store, &mut token.as_bytes(), &mut buf);
    world.result = Some(capture(buf, res));
}

#[when(regex = r#"^the developer logs in to "([^"]+)" without --password-stdin$"#)]
fn login_without_stdin(world: &mut BehaviourWorld, registry: String) {
    let store = store(world);
    let args = LoginArgs {
        registry,
        username: None,
        password_stdin: false,
    };
    let mut buf = Vec::new();
    let res = auth::login(&args, &store, &mut std::io::empty(), &mut buf);
    world.result = Some(capture(buf, res));
}

#[when(regex = r"^the developer lists stored credentials$")]
fn list_credentials(world: &mut BehaviourWorld) {
    let store = store(world);
    let mut buf = Vec::new();
    let res = auth::run(&lns_cli::cli::AuthCommand::List, &store, &mut buf);
    world.result = Some(capture(buf, res));
}

#[when(regex = r#"^the developer logs out of "([^"]+)"$"#)]
fn logout(world: &mut BehaviourWorld, registry: String) {
    let store = store(world);
    let args = LogoutArgs { registry };
    let mut buf = Vec::new();
    let res = auth::logout(&args, &store, &mut buf);
    world.result = Some(capture(buf, res));
}

#[then(regex = r#"^a credential for "([^"]+)" is stored$"#)]
fn credential_is_stored(world: &mut BehaviourWorld, registry: String) -> Result<(), String> {
    let state = store(world).load().map_err(|e| e.to_string())?;
    if state.contains_key(&registry) {
        Ok(())
    } else {
        Err(format!("no stored credential for {registry}"))
    }
}

#[then(regex = r#"^no credential for "([^"]+)" is stored$"#)]
fn no_credential_stored(world: &mut BehaviourWorld, registry: String) -> Result<(), String> {
    let state = store(world).load().map_err(|e| e.to_string())?;
    if state.contains_key(&registry) {
        Err(format!("credential for {registry} was not removed"))
    } else {
        Ok(())
    }
}
