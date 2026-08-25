use crate::runner::CliRun;
use crate::world::BehaviourWorld;
use cucumber::{given, then, when};
use lns_cli::command::parse_args;
use lns_cli::connector::LocalBoxFuture;
use lns_cli::login::{
    self, ListLoginsOutcome, LoginArgs, LoginOutcome, LogoutOutcome, RegistryAuthClient,
    WebLoginFlow, WebLoginOutcome,
};
use std::io::Write;
use std::sync::{Arc, Mutex};

/// Stands in for the running service: records what it was asked to store and accepts everything.
struct RecordingAuthClient {
    calls: Arc<Mutex<Vec<(String, String, String)>>>,
}
impl RegistryAuthClient for RecordingAuthClient {
    fn available<'a>(&'a self) -> LocalBoxFuture<'a, anyhow::Result<bool>> {
        Box::pin(async move { Ok(true) })
    }

    fn login<'a>(
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
        Box::pin(async move { Ok(LoginOutcome::Stored) })
    }

    fn logout<'a>(
        &'a self,
        _registry: &'a str,
    ) -> LocalBoxFuture<'a, anyhow::Result<LogoutOutcome>> {
        Box::pin(async move { Ok(LogoutOutcome::LoggedOut) })
    }

    fn list<'a>(&'a self) -> LocalBoxFuture<'a, anyhow::Result<ListLoginsOutcome>> {
        Box::pin(async move { Ok(ListLoginsOutcome::Logins(Vec::new())) })
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
    let client = RecordingAuthClient {
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
            match login::run(&args, "hub.lns.run", &client, &web, &mut input, &mut out).await {
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

#[then(regex = r#"^the service saw the web-issued credential for "([^"]+)"$"#)]
fn then_service_saw(world: &mut BehaviourWorld, username: String) {
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

#[then("the service stored no credential")]
fn then_service_stored_nothing(world: &mut BehaviourWorld) {
    let calls = world.web_login.verifier_calls.lock().unwrap();
    assert!(
        calls.is_empty(),
        "the service must be asked nothing: {calls:?}"
    );
}
