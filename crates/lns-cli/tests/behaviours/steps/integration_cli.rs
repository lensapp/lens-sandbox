use crate::runner::CliRun;
use crate::world::BehaviourWorld;
use cucumber::{given, then, when};
use lns_cli::cli::IntegrationArgs;
use lns_cli::command::parse_args;
use lns_cli::integration::{self, IntegrationSignIn, LocalBoxFuture, SignInOutcome};
use lns_policy::Policy;
use std::io::Write;
use std::path::PathBuf;

fn cwd(world: &mut BehaviourWorld) -> PathBuf {
    if world.cwd.is_none() {
        world.cwd = Some(tempfile::TempDir::new().expect("create tempdir"));
    }
    world.cwd.as_ref().unwrap().path().to_path_buf()
}

fn policy_file(world: &mut BehaviourWorld) -> PathBuf {
    cwd(world).join("lns-policy.yaml")
}

/// Stands in for the running service: renders the verification prompt and returns the scripted outcome.
struct FakeSignIn {
    outcome: SignInOutcome,
}
impl IntegrationSignIn for FakeSignIn {
    fn sign_in<'a>(
        &'a self,
        id: &'a str,
        out: &'a mut dyn Write,
    ) -> LocalBoxFuture<'a, anyhow::Result<SignInOutcome>> {
        let outcome = self.outcome.clone();
        Box::pin(async move {
            writeln!(
                out,
                "Open https://example.com/login/device and enter code WDJB-MJHT to connect {id}"
            )?;
            Ok(outcome)
        })
    }
}

async fn run_integration(world: &mut BehaviourWorld, tail: &[&str]) {
    let dir = cwd(world);
    let catalog = dir.join(".lns-integrations.yaml");
    let signin = FakeSignIn {
        outcome: world
            .signin_outcome
            .clone()
            .unwrap_or(SignInOutcome::Completed),
    };
    let mut full = vec!["lns".to_string(), "integration".to_string()];
    full.extend(tail.iter().map(|s| s.to_string()));
    let run = match parse_args::<IntegrationArgs, _, _>(&full) {
        Ok(args) => {
            let mut buf = Vec::<u8>::new();
            match integration::run(&args.command, &dir, &catalog, &signin, &mut buf).await {
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

#[given(regex = r#"^a user catalog declares the "([^"]+)" oauth integration$"#)]
fn given_user_oauth_integration(world: &mut BehaviourWorld, id: String) {
    use lns_policy::integrations::{AuthKind, Catalog, Integration, IntegrationRoute, OauthAuth};
    use lns_policy::providers::{InjectionDef, InjectionKind};
    let dir = cwd(world);
    Catalog {
        integrations: vec![Integration {
            id,
            name: None,
            auth_kind: AuthKind::Oauth,
            routes: vec![IntegrationRoute {
                match_pattern: "api.some-oauth.example".into(),
                transport: None,
                scheme: None,
                tls_terminate: false,
                rules: Vec::new(),
            }],
            credential: None,
            oauth: Some(OauthAuth {
                client_id: "Iv1.some-oauth".into(),
                scopes: vec!["repo".into()],
                device_authorization_endpoint: "https://example.com/device/code".into(),
                token_endpoint: "https://example.com/oauth/token".into(),
                env_var: "SOME_OAUTH_TOKEN".into(),
                placeholder: "some-oauth-placeholder-0000000000000000".into(),
                injections: vec![InjectionDef {
                    kind: InjectionKind::BearerHeader,
                    domain: "api.some-oauth.example".into(),
                    header: None,
                }],
            }),
            token_fallback: None,
        }],
    }
    .save_atomic(&dir.join(".lns-integrations.yaml"))
    .unwrap();
}

#[given("the background service is available to sign in")]
fn service_available(world: &mut BehaviourWorld) {
    world.signin_outcome = Some(SignInOutcome::Completed);
}

#[given("the background service is not available")]
fn service_unavailable(world: &mut BehaviourWorld) {
    world.signin_outcome = Some(SignInOutcome::ServiceUnavailable);
}

#[when(regex = r#"^the developer runs "lns integration connect (\S+)"$"#)]
async fn run_connect(world: &mut BehaviourWorld, id: String) {
    run_integration(world, &["connect", &id]).await;
}

#[when(regex = r#"^the developer runs "lns integration list"$"#)]
async fn run_list(world: &mut BehaviourWorld) {
    run_integration(world, &["list"]).await;
}

#[then("a verification URL and user code are shown")]
fn shows_verification(world: &mut BehaviourWorld) {
    let out = &world
        .result
        .as_ref()
        .expect("a run must have happened")
        .output;
    assert!(
        out.contains("https://example.com/login/device"),
        "expected a verification URL, got: {out}"
    );
    assert!(
        out.contains("WDJB-MJHT"),
        "expected the user code, got: {out}"
    );
}

#[then(regex = r#"^"(\S+)" is recorded under integrations in lns-policy.yaml$"#)]
fn recorded(world: &mut BehaviourWorld, id: String) {
    let policy = Policy::load_or_default(&policy_file(world)).unwrap();
    assert!(
        policy.integrations.contains(&id),
        "expected {id} recorded, got: {:?}",
        policy.integrations
    );
}

#[then("lns-policy.yaml carries no token material")]
fn no_token_material(world: &mut BehaviourWorld) {
    let text = std::fs::read_to_string(policy_file(world)).unwrap_or_default();
    assert!(
        !text.contains("token") && !text.contains("access"),
        "the shareable policy must hold no token material: {text}"
    );
}

#[then("the command fails noting the service is needed to sign in")]
fn fails_needs_service(world: &mut BehaviourWorld) {
    let run = world.result.as_ref().expect("a run must have happened");
    assert_ne!(run.exit_code, 0, "expected a non-zero exit");
    assert!(
        run.output.contains("service must be running"),
        "got: {}",
        run.output
    );
}

#[then(regex = r#"^"(\S+)" is not recorded in lns-policy.yaml$"#)]
fn not_recorded(world: &mut BehaviourWorld, id: String) {
    let policy = Policy::load_or_default(&policy_file(world)).unwrap();
    assert!(
        !policy.integrations.contains(&id),
        "expected {id} absent, got: {:?}",
        policy.integrations
    );
}

#[then(regex = r#"^"(\S+)" is listed as authenticating by (oauth|credential)$"#)]
fn listed_as_kind(world: &mut BehaviourWorld, id: String, kind: String) {
    let out = &world
        .result
        .as_ref()
        .expect("a run must have happened")
        .output;
    let line = out
        .lines()
        .find(|l| l.starts_with(&id))
        .unwrap_or_else(|| panic!("no listing line for {id} in:\n{out}"));
    assert!(
        line.trim_end().ends_with(&kind),
        "expected {id} listed as {kind}, got line: {line}"
    );
}
