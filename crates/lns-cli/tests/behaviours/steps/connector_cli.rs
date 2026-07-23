use crate::runner::CliRun;
use crate::world::BehaviourWorld;
use cucumber::{given, then, when};
use lns_cli::command::parse_args;
use lns_cli::connector::ConnectorArgs;
use lns_cli::connector::{
    self, BindOutcome, ConnectorPublisher, ConnectorPuller, ConnectorSignIn, LocalBoxFuture,
    PullReport, SignInOutcome,
};
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

/// Stands in for the running service: renders the sign-in prompt the CLI would show and returns the scripted outcome.
struct FakeSignIn {
    outcome: SignInOutcome,
    pkce: bool,
}
impl ConnectorSignIn for FakeSignIn {
    fn sign_in<'a>(
        &'a self,
        id: &'a str,
        out: &'a mut dyn Write,
    ) -> LocalBoxFuture<'a, anyhow::Result<SignInOutcome>> {
        let outcome = self.outcome.clone();
        let pkce = self.pkce;
        Box::pin(async move {
            if pkce {
                writeln!(
                    out,
                    "Opening your browser to authorize {id}… (if it didn't open, visit https://openrouter.ai/auth?code_challenge=abc)"
                )?;
            } else {
                writeln!(
                    out,
                    "Open https://example.com/login/device and enter code WDJB-MJHT to connect {id}"
                )?;
            }
            Ok(outcome)
        })
    }

    fn bind_credential<'a>(
        &'a self,
        id: &'a str,
        out: &'a mut dyn Write,
    ) -> LocalBoxFuture<'a, anyhow::Result<BindOutcome>> {
        let bind = match &self.outcome {
            SignInOutcome::Completed => {
                BindOutcome::Completed(lns_ipc::CredentialBindDecision::Stored)
            }
            SignInOutcome::Failed(reason) => BindOutcome::Failed(reason.clone()),
            SignInOutcome::ServiceUnavailable => BindOutcome::ServiceUnavailable,
        };
        Box::pin(async move {
            writeln!(out, "Decide how \"{id}\" binds in the approval window…")?;
            Ok(bind)
        })
    }
}

/// Stands in for the registry: the publish scenarios assert on the command's output, so the fake just accepts the upload.
struct FakePublisher;
impl ConnectorPublisher for FakePublisher {
    fn push<'a>(
        &'a self,
        _built: &'a lns_artifact::build::BuiltArtifact,
        _reference: &'a str,
    ) -> LocalBoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move { Ok(()) })
    }
}

/// Stands in for the background service's pull: the pull scenarios assert on the command's output, so the fake reports a fresh pull and accepts removals.
struct FakePuller;
impl ConnectorPuller for FakePuller {
    fn pull<'a>(
        &'a self,
        _reference: &'a str,
        _confirm_replace: bool,
    ) -> LocalBoxFuture<'a, anyhow::Result<PullReport>> {
        Box::pin(async move {
            Ok(PullReport::Pulled {
                id: "some-provider".into(),
                config_digest: format!("sha256:{}", "a".repeat(64)),
                replaced: false,
            })
        })
    }

    fn remove<'a>(&'a self, _id: &'a str) -> LocalBoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move { Ok(()) })
    }
}

async fn run_connector(world: &mut BehaviourWorld, tail: &[&str]) {
    let dir = cwd(world);
    let catalog = dir.join(".lns-connectors.yaml");
    let pulled = dir.join(".lns-pulled-connectors.yaml");
    let signin = FakeSignIn {
        outcome: world
            .signin_outcome
            .clone()
            .unwrap_or(SignInOutcome::Completed),
        pkce: world.signin_is_pkce,
    };
    let publisher = FakePublisher;
    let puller = FakePuller;
    let mut full = vec!["lns".to_string(), "connector".to_string()];
    full.extend(tail.iter().map(|s| s.to_string()));
    let run = match parse_args::<ConnectorArgs, _, _>(&full) {
        Ok(args) => {
            let mut buf = Vec::<u8>::new();
            match connector::run(
                &args.command,
                &dir,
                &catalog,
                &pulled,
                &signin,
                &publisher,
                &puller,
                &mut buf,
            )
            .await
            {
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

#[given(regex = r#"^a user catalog declares the "([^"]+)" oauth connector$"#)]
fn given_user_oauth_connector(world: &mut BehaviourWorld, id: String) {
    use lns_policy::connectors::{
        AuthKind, Catalog, Connector, ConnectorRoute, OauthAuth, OauthFlow,
    };
    use lns_policy::providers::{InjectionDef, InjectionKind};
    let dir = cwd(world);
    Catalog {
        connectors: vec![Connector {
            id,
            name: None,
            auth_kind: AuthKind::Oauth,
            routes: vec![ConnectorRoute {
                match_pattern: "api.some-oauth.example".into(),
                transport: None,
                scheme: None,
                tls_terminate: false,
                rules: Vec::new(),
            }],
            credential: None,
            oauth: Some(OauthAuth {
                flow: OauthFlow::Device,
                client_id: Some("Iv1.some-oauth".into()),
                client_secret: None,
                scopes: vec!["repo".into()],
                device_authorization_endpoint: Some("https://example.com/device/code".into()),
                authorization_endpoint: None,
                token_endpoint: "https://example.com/oauth/token".into(),
                userinfo_endpoint: None,
                account_field: None,
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
    .save_atomic(&dir.join(".lns-connectors.yaml"))
    .unwrap();
}

#[given(regex = r#"^a user catalog declares the "([^"]+)" pkce connector$"#)]
fn given_user_pkce_connector(world: &mut BehaviourWorld, id: String) {
    use lns_policy::connectors::{
        AuthKind, Catalog, Connector, ConnectorRoute, OauthAuth, OauthFlow,
    };
    use lns_policy::providers::{InjectionDef, InjectionKind};
    world.signin_is_pkce = true;
    let dir = cwd(world);
    Catalog {
        connectors: vec![Connector {
            id,
            name: None,
            auth_kind: AuthKind::Oauth,
            routes: vec![ConnectorRoute {
                match_pattern: "api.some-pkce.example".into(),
                transport: None,
                scheme: None,
                tls_terminate: false,
                rules: Vec::new(),
            }],
            credential: None,
            oauth: Some(OauthAuth {
                flow: OauthFlow::Pkce,
                client_id: None,
                client_secret: None,
                scopes: Vec::new(),
                device_authorization_endpoint: None,
                authorization_endpoint: Some("https://api.some-pkce.example/auth".into()),
                token_endpoint: "https://api.some-pkce.example/api/v1/auth/keys".into(),
                userinfo_endpoint: None,
                account_field: None,
                env_var: "SOME_PKCE_TOKEN".into(),
                placeholder: "some-pkce-LNSPLACEHOLDER0000000000000000".into(),
                injections: vec![InjectionDef {
                    kind: InjectionKind::BearerHeader,
                    domain: "api.some-pkce.example".into(),
                    header: None,
                }],
            }),
            token_fallback: None,
        }],
    }
    .save_atomic(&dir.join(".lns-connectors.yaml"))
    .unwrap();
}

fn write_credential_catalog(world: &mut BehaviourWorld, id: String) {
    use lns_policy::connectors::{AuthKind, Catalog, Connector, ConnectorRoute, CredentialAuth};
    use lns_policy::providers::{InjectionDef, InjectionKind};
    let dir = cwd(world);
    Catalog {
        connectors: vec![Connector {
            id: id.clone(),
            name: None,
            auth_kind: AuthKind::Credential,
            routes: vec![ConnectorRoute {
                match_pattern: format!("api.{id}.example"),
                transport: None,
                scheme: None,
                tls_terminate: false,
                rules: Vec::new(),
            }],
            credential: Some(CredentialAuth {
                env_var: "SOME_TOKEN".into(),
                placeholder: format!("{id}-LNSPLACEHOLDER0000000000"),
                injections: vec![InjectionDef {
                    kind: InjectionKind::BearerHeader,
                    domain: format!("api.{id}.example"),
                    header: None,
                }],
            }),
            oauth: None,
            token_fallback: None,
        }],
    }
    .save_atomic(&dir.join(".lns-connectors.yaml"))
    .unwrap();
}

#[given(regex = r#"^a user catalog declares the "([^"]+)" credential connector$"#)]
fn given_user_credential_connector(world: &mut BehaviourWorld, id: String) {
    write_credential_catalog(world, id);
}

#[given(regex = r#"^the connector "([^"]+)" is in the catalog$"#)]
fn given_connector_in_catalog(world: &mut BehaviourWorld, id: String) {
    write_credential_catalog(world, id);
}

#[given(regex = r#"^a connector definition file "([^"]+)" declares a credential connector$"#)]
fn given_credential_connector_file(world: &mut BehaviourWorld, file: String) {
    let dir = cwd(world);
    std::fs::write(
        dir.join(file),
        "\
id: some-provider
authKind: credential
routes:
  - match: api.some-provider.example
credential:
  envVar: SOME_TOKEN
  placeholder: some-provider-LNSPLACEHOLDER0000
  injections:
    - kind: bearer_header
      domain: api.some-provider.example
",
    )
    .unwrap();
}

#[given(
    regex = r#"^a connector definition file "([^"]+)" declares an oauth connector carrying a client secret$"#
)]
fn given_oauth_connector_file_with_secret(world: &mut BehaviourWorld, file: String) {
    let dir = cwd(world);
    std::fs::write(
        dir.join(file),
        "\
id: some-oauth
authKind: oauth
oauth:
  clientId: some-client
  clientSecret: some-secret
  deviceAuthorizationEndpoint: https://api.some-oauth.example/device
  tokenEndpoint: https://api.some-oauth.example/token
  envVar: SOME_OAUTH_TOKEN
  placeholder: some-oauth-LNSPLACEHOLDER0000
",
    )
    .unwrap();
}

#[when(regex = r#"^the user runs connector command "([^"]+)"$"#)]
async fn run_connector_command(world: &mut BehaviourWorld, command: String) {
    let parts: Vec<&str> = command.split_whitespace().collect();
    run_connector(world, &parts).await;
}

#[then("the output describes binding a credential value")]
fn output_describes_binding(world: &mut BehaviourWorld) {
    let out = &world
        .result
        .as_ref()
        .expect("a run must have happened")
        .output;
    assert!(
        out.contains("Bound a value") && out.contains("on this machine"),
        "expected a machine-scoped bind description, got: {out}"
    );
}

#[then("the output does not claim to add the connector to a sandbox")]
fn output_claims_no_sandbox_effect(world: &mut BehaviourWorld) {
    let out = &world
        .result
        .as_ref()
        .expect("a run must have happened")
        .output;
    assert!(
        !out.contains("relaunch any running sandbox") && !out.contains("Connected"),
        "a bind must not claim a sandbox effect: {out}"
    );
}

#[then("the command fails noting the service is needed to bind")]
fn fails_needs_service_to_bind(world: &mut BehaviourWorld) {
    let run = world.result.as_ref().expect("a run must have happened");
    assert_ne!(run.exit_code, 0, "expected a non-zero exit");
    assert!(
        run.output.contains("service must be running to bind"),
        "got: {}",
        run.output
    );
}

#[given("the background service is available to sign in")]
fn service_available(world: &mut BehaviourWorld) {
    world.signin_outcome = Some(SignInOutcome::Completed);
}

#[given("the background service is not available")]
fn service_unavailable(world: &mut BehaviourWorld) {
    world.signin_outcome = Some(SignInOutcome::ServiceUnavailable);
}

#[when(regex = r#"^the developer runs "lns connector connect (\S+)"$"#)]
async fn run_connect(world: &mut BehaviourWorld, id: String) {
    run_connector(world, &["connect", &id]).await;
}

#[when(regex = r#"^the developer runs "lns connector list"$"#)]
async fn run_list(world: &mut BehaviourWorld) {
    run_connector(world, &["list"]).await;
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

#[then(regex = r#"^"(\S+)" is recorded under connectors in lns-policy.yaml$"#)]
fn recorded(world: &mut BehaviourWorld, id: String) {
    let policy = Policy::load_or_default(&policy_file(world)).unwrap();
    assert!(
        policy.connectors.contains(&id),
        "expected {id} recorded, got: {:?}",
        policy.connectors
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

#[then("the browser is opened to the authorization page")]
fn shows_browser_opened(world: &mut BehaviourWorld) {
    let out = &world
        .result
        .as_ref()
        .expect("a run must have happened")
        .output;
    assert!(
        out.contains("Opening your browser") && out.contains("/auth?"),
        "expected a browser-opening prompt to the authorization page, got: {out}"
    );
}

#[then("no user code is shown")]
fn shows_no_user_code(world: &mut BehaviourWorld) {
    let out = &world
        .result
        .as_ref()
        .expect("a run must have happened")
        .output;
    assert!(
        !out.contains("enter code"),
        "a pkce sign-in has no user code to type, got: {out}"
    );
}

#[then("lns-policy.yaml carries no credential material")]
fn no_credential_material(world: &mut BehaviourWorld) {
    let text = std::fs::read_to_string(policy_file(world)).unwrap_or_default();
    assert!(
        !text.contains("key") && !text.contains("token") && !text.contains("secret"),
        "the shareable policy must hold no credential material: {text}"
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
        !policy.connectors.contains(&id),
        "expected {id} absent, got: {:?}",
        policy.connectors
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
