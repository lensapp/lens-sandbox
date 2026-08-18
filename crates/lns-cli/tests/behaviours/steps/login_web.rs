use crate::runner::CliRun;
use crate::world::BehaviourWorld;
use cucumber::{given, then, when};
use lns_cli::command::parse_args;
use lns_cli::connector::LocalBoxFuture;
use lns_cli::login::{
    self, LoginArgs, LoginOutcome, RegistryVerifier, WebLoginFlow, WebLoginOutcome,
};
use lns_policy::registry_auth::RegistryAuthStore;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

fn auth_path(world: &mut BehaviourWorld) -> PathBuf {
    if world.cwd.is_none() {
        world.cwd = Some(tempfile::TempDir::new().expect("create tempdir"));
    }
    world
        .cwd
        .as_ref()
        .unwrap()
        .path()
        .join(".lns-registry-auth.json")
}

/// Stands in for the running service's pull-auth handshake: records what it was asked and verifies everything.
struct RecordingVerifier {
    calls: Arc<Mutex<Vec<(String, String, String)>>>,
}
impl RegistryVerifier for RecordingVerifier {
    fn verify<'a>(
        &'a self,
        registry: &'a str,
        username: &'a str,
        secret: &'a str,
    ) -> LocalBoxFuture<'a, anyhow::Result<LoginOutcome>> {
        self.calls.lock().unwrap().push((
            registry.to_string(),
            username.to_string(),
            secret.to_string(),
        ));
        Box::pin(async move { Ok(LoginOutcome::Verified) })
    }
}

/// Stands in for the browser device flow: renders the code prompt a real flow would and returns the scripted outcome; panics if a flag-driven path consults it.
struct FakeWebLoginFlow {
    outcome: Option<WebLoginOutcome>,
}
impl WebLoginFlow for FakeWebLoginFlow {
    fn login<'a>(
        &'a self,
        _registry: &'a str,
        out: &'a mut dyn Write,
    ) -> LocalBoxFuture<'a, anyhow::Result<WebLoginOutcome>> {
        let outcome = self
            .outcome
            .clone()
            .expect("the web flow must not be consulted on a flag-driven path");
        Box::pin(async move {
            if !matches!(outcome, WebLoginOutcome::Unsupported) {
                writeln!(out, "Your one-time confirmation code is: SOME-CODE")?;
                writeln!(out, "Waiting for you to approve the login in the browser…")?;
            }
            Ok(outcome)
        })
    }
}

#[given(regex = r#"^the web flow will issue a token for "([^"]+)"$"#)]
fn given_web_issues(world: &mut BehaviourWorld, username: String) {
    world.web_login.outcome = Some(WebLoginOutcome::Completed {
        username,
        secret: "some-web-token".into(),
    });
}

#[given("the web flow would panic if it were consulted")]
fn given_web_panics(world: &mut BehaviourWorld) {
    world.web_login.outcome = None;
}

#[given("the web flow reports the registry does not support it")]
fn given_web_unsupported(world: &mut BehaviourWorld) {
    world.web_login.outcome = Some(WebLoginOutcome::Unsupported);
}

#[given("the web flow reports the browser denied the login")]
fn given_web_denied(world: &mut BehaviourWorld) {
    world.web_login.outcome = Some(WebLoginOutcome::Denied);
}

#[given("the web flow reports the confirmation code expired")]
fn given_web_expired(world: &mut BehaviourWorld) {
    world.web_login.outcome = Some(WebLoginOutcome::Expired);
}

#[when(regex = r#"^I log in with "([^"]+)"$"#)]
async fn when_log_in(world: &mut BehaviourWorld, command: String) {
    let path = auth_path(world);
    let verifier = RecordingVerifier {
        calls: world.web_login.verifier_calls.clone(),
    };
    let web = FakeWebLoginFlow {
        outcome: world.web_login.outcome.clone(),
    };
    let argv: Vec<String> = command.split_whitespace().map(str::to_string).collect();
    let run = match parse_args::<LoginArgs, _, _>(&argv) {
        Ok(args) => {
            let mut input: &[u8] = b"";
            let mut out = Vec::<u8>::new();
            match login::run(
                &args,
                "hub.lns.run",
                &path,
                &verifier,
                &web,
                &mut input,
                &mut out,
            )
            .await
            {
                Ok(exit_code) => CliRun {
                    exit_code,
                    output: String::from_utf8_lossy(&out).into_owned(),
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

#[then(regex = r#"^the verifier saw the web-issued credential for "([^"]+)"$"#)]
fn then_verifier_saw(world: &mut BehaviourWorld, username: String) {
    let calls = world.web_login.verifier_calls.lock().unwrap();
    assert_eq!(
        calls.as_slice(),
        [(
            "hub.lns.run".to_string(),
            username,
            "some-web-token".to_string()
        )]
    );
}

#[then(regex = r#"^the credential store holds "([^"]+)" for "([^"]+)"$"#)]
fn then_store_holds(world: &mut BehaviourWorld, registry: String, username: String) {
    let path = auth_path(world);
    let file = lns_policy::registry_auth::JsonFileRegistryAuthStore::new(path)
        .load()
        .expect("read the auth store");
    let cred = file.get(&registry).expect("entry for the registry");
    assert_eq!(cred.username, username);
}

#[then("the credential store is empty")]
fn then_store_empty(world: &mut BehaviourWorld) {
    let path = auth_path(world);
    let file = lns_policy::registry_auth::JsonFileRegistryAuthStore::new(path)
        .load()
        .expect("read the auth store");
    assert!(file.is_empty(), "the store must hold nothing: {file:?}");
}
