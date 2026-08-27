use std::io::Write;

use anyhow::{Context, Result, anyhow, bail};
use lns_policy::registry_auth::{canonical_registry, validate_registry_host};

use crate::command::{CommandSpec, subcommand};
use crate::connector::LocalBoxFuture;

mod real;
mod web;

pub use real::RealRegistryAuthClient;
pub use web::{
    BrowserOpener, DeviceAuthClient, DeviceAuthorization, DeviceStart, RealDeviceAuthClient,
    TokenPoll, WebLogin,
};

#[derive(clap::Args)]
pub struct LoginArgs {
    #[arg(
        help = "Registry host to log in to (e.g. ghcr.io); defaults to the configured `run.registry`, else hub.lns.run."
    )]
    pub registry: Option<String>,
    #[arg(short = 'u', long, help = "Username for the registry.")]
    pub username: Option<String>,
    #[arg(
        short = 'p',
        long,
        help = "Password or token; prefer --password-stdin to keep secrets out of shell history."
    )]
    pub password: Option<String>,
    #[arg(
        long,
        conflicts_with = "password",
        help = "Read the password/token from stdin (e.g. `echo $TOKEN | lns login -u me --password-stdin ghcr.io`)."
    )]
    pub password_stdin: bool,
    #[arg(
        long,
        conflicts_with_all = ["username", "password", "password_stdin"],
        help = "List the registries you are logged in to (hosts and usernames, never secrets)."
    )]
    pub list: bool,
    #[command(flatten)]
    pub output: crate::output::OutputArgs,
}

#[derive(clap::Args)]
pub struct LogoutArgs {
    #[arg(
        help = "Registry host to log out of; defaults to the configured `run.registry`, else hub.lns.run."
    )]
    pub registry: Option<String>,
}

/// The result of asking the running service to verify and store a registry credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginOutcome {
    Stored,
    Rejected(String),
    ServiceUnavailable,
}

/// The result of asking the running service to remove a stored registry credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogoutOutcome {
    LoggedOut,
    Failed(String),
    ServiceUnavailable,
}

/// The result of asking the running service which registries are logged in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ListLoginsOutcome {
    Logins(Vec<lns_ipc::RegistryLoginSummary>),
    ServiceUnavailable,
}

/// The service owns the registry login store, so every credential operation goes through it.
pub trait RegistryAuthClient {
    fn available<'a>(&'a self) -> LocalBoxFuture<'a, Result<bool>>;
    fn login<'a>(
        &'a self,
        registry: &'a str,
        username: &'a str,
        secret: &'a str,
    ) -> LocalBoxFuture<'a, Result<LoginOutcome>>;
    fn logout<'a>(&'a self, registry: &'a str) -> LocalBoxFuture<'a, Result<LogoutOutcome>>;
    fn list<'a>(&'a self) -> LocalBoxFuture<'a, Result<ListLoginsOutcome>>;
}

/// The terminal result of driving a browser-based device login against a registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebLoginOutcome {
    Completed { username: String, secret: String },
    Unsupported,
    Denied,
    Expired,
}

/// Runs a browser-based device login against a registry, rendering progress to `out`.
pub trait WebLoginFlow {
    fn login<'a>(
        &'a self,
        registry: &'a str,
        out: &'a mut dyn Write,
    ) -> LocalBoxFuture<'a, Result<WebLoginOutcome>>;
}

pub fn augment_login(app: clap::Command) -> clap::Command {
    app.subcommand(subcommand::<LoginArgs>("login").about(
        "Log in to an OCI registry so `lns run` and `lns pull` can fetch its private images.",
    ))
}

pub const LOGIN_SPEC: CommandSpec = CommandSpec {
    name: "login",
    augment: augment_login,
    run: real::run_login,
    announces_update_check: true,
    owns_terminal: crate::command::never_owns_terminal,
};

pub fn augment_logout(app: clap::Command) -> clap::Command {
    app.subcommand(
        subcommand::<LogoutArgs>("logout").about("Remove stored credentials for an OCI registry."),
    )
}

pub const LOGOUT_SPEC: CommandSpec = CommandSpec {
    name: "logout",
    augment: augment_logout,
    run: real::run_logout,
    announces_update_check: true,
    owns_terminal: crate::command::never_owns_terminal,
};

pub async fn run(
    args: &LoginArgs,
    default_registry: &str,
    client: &dyn RegistryAuthClient,
    web: &dyn WebLoginFlow,
    input: &mut dyn std::io::BufRead,
    out: &mut impl Write,
) -> Result<i32> {
    if args.list {
        return list(client, args.output.format, out).await;
    }
    login(args, default_registry, client, web, input, out).await
}

fn target_registry(positional: Option<&str>, default_registry: &str) -> Result<String> {
    let host = positional.unwrap_or(default_registry);
    validate_registry_host(host).map_err(|e| anyhow!("{e}"))?;
    Ok(canonical_registry(host))
}

async fn login(
    args: &LoginArgs,
    default_registry: &str,
    client: &dyn RegistryAuthClient,
    web: &dyn WebLoginFlow,
    input: &mut dyn std::io::BufRead,
    out: &mut impl Write,
) -> Result<i32> {
    let registry = target_registry(args.registry.as_deref(), default_registry)?;
    if args.wants_web_login() && !client.available().await? {
        bail!("{SERVICE_REQUIRED}");
    }
    let (username, secret) = resolve_credentials(args, &registry, web, input, out).await?;

    match client.login(&registry, &username, &secret).await? {
        LoginOutcome::ServiceUnavailable => bail!("{SERVICE_REQUIRED}"),
        LoginOutcome::Rejected(reason) => bail!("login to {registry} was rejected: {reason}"),
        LoginOutcome::Stored => {}
    }

    writeln!(out, "You are now logged in to {registry} as {username}.")?;
    Ok(0)
}

pub async fn logout(
    args: &LogoutArgs,
    default_registry: &str,
    client: &dyn RegistryAuthClient,
    out: &mut impl Write,
) -> Result<i32> {
    let registry = target_registry(args.registry.as_deref(), default_registry)?;
    match client.logout(&registry).await? {
        LogoutOutcome::ServiceUnavailable => bail!("{SERVICE_REQUIRED}"),
        LogoutOutcome::Failed(reason) => bail!("{reason}"),
        LogoutOutcome::LoggedOut => {}
    }
    writeln!(out, "Logged out of {registry}.")?;
    Ok(0)
}

#[derive(serde::Serialize)]
struct LoginRow {
    registry: String,
    username: String,
}

impl crate::output::TableRow for LoginRow {
    const HEADERS: &'static [&'static str] = &["REGISTRY", "USERNAME"];

    fn cells(&self) -> Vec<String> {
        vec![self.registry.clone(), self.username.clone()]
    }
}

async fn list(
    client: &dyn RegistryAuthClient,
    format: crate::output::Format,
    out: &mut impl Write,
) -> Result<i32> {
    let logins = match client.list().await? {
        ListLoginsOutcome::ServiceUnavailable => bail!("{SERVICE_REQUIRED}"),
        ListLoginsOutcome::Logins(logins) => logins,
    };
    let rows: Vec<LoginRow> = logins
        .into_iter()
        .map(|login| LoginRow {
            registry: login.registry,
            username: login.username,
        })
        .collect();
    crate::output::emit(format, &rows, "Not logged in to any registry.", out)?;
    Ok(0)
}

impl LoginArgs {
    fn wants_web_login(&self) -> bool {
        self.username.is_none() && self.password.is_none() && !self.password_stdin
    }
}

const SERVICE_REQUIRED: &str = "the background service must be running to manage registry logins; start it with `lns service start`";

async fn resolve_credentials(
    args: &LoginArgs,
    registry: &str,
    web: &dyn WebLoginFlow,
    input: &mut dyn std::io::BufRead,
    out: &mut dyn Write,
) -> Result<(String, String)> {
    if args.wants_web_login() {
        return web_credentials(registry, web, out).await;
    }
    let username = args
        .username
        .clone()
        .ok_or_else(|| anyhow!("a username is required; pass --username/-u"))?;
    let secret = resolve_secret(args, input)?;
    Ok((username, secret))
}

const FLAG_FALLBACK: &str = "pass --username/-u and --password-stdin (recommended) or --password";

async fn web_credentials(
    registry: &str,
    web: &dyn WebLoginFlow,
    out: &mut dyn Write,
) -> Result<(String, String)> {
    let outcome = web
        .login(registry, out)
        .await
        .with_context(|| format!("starting browser login to {registry}; {FLAG_FALLBACK}"))?;
    match outcome {
        WebLoginOutcome::Completed { username, secret } => Ok((username, secret)),
        WebLoginOutcome::Unsupported => {
            bail!("{registry} does not support browser login; {FLAG_FALLBACK}")
        }
        WebLoginOutcome::Denied => bail!("login to {registry} was denied in the browser"),
        WebLoginOutcome::Expired => {
            bail!(
                "the confirmation code expired before the login was approved; run `lns login` again"
            )
        }
    }
}

fn resolve_secret(args: &LoginArgs, input: &mut dyn std::io::BufRead) -> Result<String> {
    if args.password_stdin {
        let mut buf = String::new();
        input
            .read_to_string(&mut buf)
            .context("reading password from stdin")?;
        let secret = buf.trim_end_matches(['\n', '\r']).to_string();
        if secret.is_empty() {
            bail!("no password received on stdin");
        }
        return Ok(secret);
    }
    match args.password.as_deref() {
        Some("") => bail!("password must not be empty"),
        Some(p) => Ok(p.to_string()),
        None => bail!("a password is required; pass --password-stdin (recommended) or --password"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeClient {
        login_outcome: LoginOutcome,
        logout_outcome: LogoutOutcome,
        list_outcome: ListLoginsOutcome,
        calls: std::sync::Mutex<Vec<(String, String, String)>>,
        logout_calls: std::sync::Mutex<Vec<String>>,
    }

    impl FakeClient {
        fn returning(login_outcome: LoginOutcome) -> Self {
            Self {
                login_outcome,
                logout_outcome: LogoutOutcome::LoggedOut,
                list_outcome: ListLoginsOutcome::Logins(Vec::new()),
                calls: std::sync::Mutex::new(Vec::new()),
                logout_calls: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn for_logout(logout_outcome: LogoutOutcome) -> Self {
            let mut client = Self::returning(LoginOutcome::Stored);
            client.logout_outcome = logout_outcome;
            client
        }

        fn listing(list_outcome: ListLoginsOutcome) -> Self {
            let mut client = Self::returning(LoginOutcome::Stored);
            client.list_outcome = list_outcome;
            client
        }
    }

    impl RegistryAuthClient for FakeClient {
        fn available<'a>(&'a self) -> LocalBoxFuture<'a, Result<bool>> {
            let up = !matches!(self.login_outcome, LoginOutcome::ServiceUnavailable);
            Box::pin(async move { Ok(up) })
        }

        fn login<'a>(
            &'a self,
            registry: &'a str,
            username: &'a str,
            secret: &'a str,
        ) -> LocalBoxFuture<'a, Result<LoginOutcome>> {
            self.calls.lock().unwrap().push((
                registry.to_string(),
                username.to_string(),
                secret.to_string(),
            ));
            let outcome = self.login_outcome.clone();
            Box::pin(async move { Ok(outcome) })
        }

        fn logout<'a>(&'a self, registry: &'a str) -> LocalBoxFuture<'a, Result<LogoutOutcome>> {
            self.logout_calls.lock().unwrap().push(registry.to_string());
            let outcome = self.logout_outcome.clone();
            Box::pin(async move { Ok(outcome) })
        }

        fn list<'a>(&'a self) -> LocalBoxFuture<'a, Result<ListLoginsOutcome>> {
            let outcome = self.list_outcome.clone();
            Box::pin(async move { Ok(outcome) })
        }
    }

    struct NoWebLogin;
    impl WebLoginFlow for NoWebLogin {
        fn login<'a>(
            &'a self,
            _registry: &'a str,
            _out: &'a mut dyn Write,
        ) -> LocalBoxFuture<'a, Result<WebLoginOutcome>> {
            panic!("a flag-driven login must never consult the web flow");
        }
    }

    #[test]
    #[should_panic(expected = "must never consult the web flow")]
    fn the_no_web_login_guard_panics_when_consulted() {
        let mut out = Vec::new();
        drop(NoWebLogin.login("hub.lns.run", &mut out));
    }

    struct FakeWebLogin {
        outcome: Result<WebLoginOutcome, String>,
    }
    impl WebLoginFlow for FakeWebLogin {
        fn login<'a>(
            &'a self,
            _registry: &'a str,
            _out: &'a mut dyn Write,
        ) -> LocalBoxFuture<'a, Result<WebLoginOutcome>> {
            let outcome = self.outcome.clone();
            Box::pin(async move { outcome.map_err(|e| anyhow!(e)) })
        }
    }

    fn flagless_args(registry: Option<&str>) -> LoginArgs {
        login_args(registry, None, None)
    }

    fn login_args(
        registry: Option<&str>,
        username: Option<&str>,
        password: Option<&str>,
    ) -> LoginArgs {
        LoginArgs {
            registry: registry.map(str::to_string),
            username: username.map(str::to_string),
            password: password.map(str::to_string),
            password_stdin: false,
            list: false,
            output: crate::output::OutputArgs {
                format: crate::output::Format::Table,
            },
        }
    }

    #[tokio::test]
    async fn login_sends_the_credential_to_the_service_which_verifies_and_stores_it() {
        let client = FakeClient::returning(LoginOutcome::Stored);
        let mut input: &[u8] = b"";
        let mut out = Vec::new();
        let code = login(
            &login_args(Some("ghcr.io"), Some("octocat"), Some("ghp_token")),
            "docker.io",
            &client,
            &NoWebLogin,
            &mut input,
            &mut out,
        )
        .await
        .unwrap();
        assert_eq!(code, 0);
        assert_eq!(
            client.calls.lock().unwrap()[0],
            ("ghcr.io".into(), "octocat".into(), "ghp_token".into())
        );
        assert!(
            String::from_utf8(out)
                .unwrap()
                .contains("You are now logged in to ghcr.io as octocat"),
        );
    }

    #[tokio::test]
    async fn login_folds_a_docker_hub_alias_onto_the_canonical_key() {
        let client = FakeClient::returning(LoginOutcome::Stored);
        let mut input: &[u8] = b"";
        let mut out = Vec::new();
        login(
            &login_args(Some("index.docker.io"), Some("hubuser"), Some("hubpass")),
            "docker.io",
            &client,
            &NoWebLogin,
            &mut input,
            &mut out,
        )
        .await
        .unwrap();
        // The service is asked about the canonical host, not the alias the user typed.
        assert_eq!(client.calls.lock().unwrap()[0].0, "docker.io");
    }

    #[tokio::test]
    async fn login_falls_back_to_the_default_registry_when_no_host_is_given() {
        let client = FakeClient::returning(LoginOutcome::Stored);
        let mut input: &[u8] = b"";
        let mut out = Vec::new();
        login(
            &login_args(None, Some("me"), Some("tok")),
            "ghcr.io",
            &client,
            &NoWebLogin,
            &mut input,
            &mut out,
        )
        .await
        .unwrap();
        assert_eq!(client.calls.lock().unwrap()[0].0, "ghcr.io");
    }

    #[tokio::test]
    async fn login_reads_the_secret_from_stdin_when_password_stdin_is_set() {
        let client = FakeClient::returning(LoginOutcome::Stored);
        let mut args = login_args(Some("ghcr.io"), Some("me"), None);
        args.password_stdin = true;
        let mut input: &[u8] = b"piped-secret\n";
        let mut out = Vec::new();
        login(
            &args,
            "registry.example.test",
            &client,
            &NoWebLogin,
            &mut input,
            &mut out,
        )
        .await
        .unwrap();
        assert_eq!(client.calls.lock().unwrap()[0].2, "piped-secret");
    }

    #[tokio::test]
    async fn login_requires_a_username() {
        let client = FakeClient::returning(LoginOutcome::Stored);
        let mut input: &[u8] = b"";
        let mut out = Vec::new();
        let err = login(
            &login_args(Some("ghcr.io"), None, Some("tok")),
            "docker.io",
            &client,
            &NoWebLogin,
            &mut input,
            &mut out,
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("username is required"));
    }

    #[tokio::test]
    async fn login_rejects_an_invalid_registry_before_asking_the_service() {
        let client = FakeClient::returning(LoginOutcome::Stored);
        let mut input: &[u8] = b"";
        let mut out = Vec::new();
        let err = login(
            &login_args(Some("https://ghcr.io"), Some("me"), Some("tok")),
            "docker.io",
            &client,
            &NoWebLogin,
            &mut input,
            &mut out,
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("URL"), "got: {err:#}");
        assert!(
            client.calls.lock().unwrap().is_empty(),
            "must not send a bad host to the service"
        );
    }

    #[tokio::test]
    async fn login_fails_when_the_service_rejects_the_credential() {
        let client = FakeClient::returning(LoginOutcome::Rejected("401 unauthorized".into()));
        let mut input: &[u8] = b"";
        let mut out = Vec::new();
        let err = login(
            &login_args(Some("ghcr.io"), Some("me"), Some("wrong")),
            "docker.io",
            &client,
            &NoWebLogin,
            &mut input,
            &mut out,
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("401 unauthorized"));
        assert!(
            !String::from_utf8(out).unwrap().contains("logged in"),
            "a rejected login must not claim success"
        );
    }

    #[tokio::test]
    async fn login_reports_when_the_service_is_unavailable() {
        let client = FakeClient::returning(LoginOutcome::ServiceUnavailable);
        let mut input: &[u8] = b"";
        let mut out = Vec::new();
        let err = login(
            &login_args(Some("ghcr.io"), Some("me"), Some("tok")),
            "docker.io",
            &client,
            &NoWebLogin,
            &mut input,
            &mut out,
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("service must be running"));
    }

    #[tokio::test]
    async fn run_dispatches_to_list_and_renders_a_table_of_hosts_and_usernames() {
        let client = FakeClient::listing(ListLoginsOutcome::Logins(vec![
            lns_ipc::RegistryLoginSummary {
                registry: "ghcr.io".into(),
                username: "octocat".into(),
            },
            lns_ipc::RegistryLoginSummary {
                registry: "quay.io".into(),
                username: "robot".into(),
            },
        ]));
        let mut args = login_args(None, None, None);
        args.list = true;
        let mut input: &[u8] = b"";
        let mut out = Vec::new();
        let code = run(
            &args,
            "registry.example.test",
            &client,
            &NoWebLogin,
            &mut input,
            &mut out,
        )
        .await
        .unwrap();
        assert_eq!(code, 0);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "REGISTRY  USERNAME\nghcr.io   octocat\nquay.io   robot\n",
            "an uppercase header row over the hosts and usernames the service reported"
        );
        assert!(
            client.calls.lock().unwrap().is_empty(),
            "--list only reports, never logs in"
        );
    }

    #[tokio::test]
    async fn run_dispatches_to_login_when_list_is_not_set() {
        let client = FakeClient::returning(LoginOutcome::Stored);
        let mut input: &[u8] = b"";
        let mut out = Vec::new();
        let code = run(
            &login_args(Some("ghcr.io"), Some("me"), Some("tok")),
            "docker.io",
            &client,
            &NoWebLogin,
            &mut input,
            &mut out,
        )
        .await
        .unwrap();
        assert_eq!(code, 0);
        assert_eq!(client.calls.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn list_reports_when_no_registries_are_logged_in() {
        let client = FakeClient::listing(ListLoginsOutcome::Logins(Vec::new()));
        let mut out = Vec::new();
        let code = list(&client, crate::output::Format::Table, &mut out)
            .await
            .unwrap();
        assert_eq!(code, 0);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "Not logged in to any registry.\n"
        );
    }

    #[tokio::test]
    async fn list_requires_the_service_that_keeps_the_logins() {
        let client = FakeClient::listing(ListLoginsOutcome::ServiceUnavailable);
        let mut out = Vec::new();
        let err = list(&client, crate::output::Format::Table, &mut out)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("service must be running"));
    }

    #[tokio::test]
    async fn logout_asks_the_service_to_remove_the_stored_credential() {
        let client = FakeClient::for_logout(LogoutOutcome::LoggedOut);
        let mut out = Vec::new();
        let code = logout(
            &LogoutArgs {
                registry: Some("GHCR.IO".into()),
            },
            "docker.io",
            &client,
            &mut out,
        )
        .await
        .unwrap();
        assert_eq!(code, 0);
        assert_eq!(
            client.logout_calls.lock().unwrap().as_slice(),
            ["ghcr.io"],
            "the service is asked about the canonical host"
        );
        assert!(
            String::from_utf8(out)
                .unwrap()
                .contains("Logged out of ghcr.io")
        );
    }

    #[tokio::test]
    async fn logout_surfaces_the_services_not_logged_in_answer() {
        let client =
            FakeClient::for_logout(LogoutOutcome::Failed("not logged in to ghcr.io".into()));
        let err = logout(
            &LogoutArgs {
                registry: Some("ghcr.io".into()),
            },
            "docker.io",
            &client,
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("not logged in to ghcr.io"));
    }

    #[tokio::test]
    async fn logout_requires_the_service_that_keeps_the_logins() {
        let client = FakeClient::for_logout(LogoutOutcome::ServiceUnavailable);
        let err = logout(
            &LogoutArgs {
                registry: Some("ghcr.io".into()),
            },
            "docker.io",
            &client,
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("service must be running"));
    }

    #[test]
    fn resolve_secret_prefers_stdin_and_trims_a_trailing_newline() {
        let mut args = login_args(Some("ghcr.io"), Some("me"), None);
        args.password_stdin = true;
        let mut input: &[u8] = b"tok\r\n";
        assert_eq!(resolve_secret(&args, &mut input).unwrap(), "tok");
    }

    #[test]
    fn resolve_secret_rejects_empty_stdin() {
        let mut args = login_args(Some("ghcr.io"), Some("me"), None);
        args.password_stdin = true;
        let mut input: &[u8] = b"\n";
        assert!(
            resolve_secret(&args, &mut input)
                .unwrap_err()
                .to_string()
                .contains("no password received")
        );
    }

    #[test]
    fn resolve_secret_takes_the_password_flag_when_stdin_is_not_used() {
        let args = login_args(Some("ghcr.io"), Some("me"), Some("flagpass"));
        let mut input: &[u8] = b"";
        assert_eq!(resolve_secret(&args, &mut input).unwrap(), "flagpass");
    }

    #[test]
    fn resolve_secret_rejects_an_empty_password_flag() {
        let args = login_args(Some("ghcr.io"), Some("me"), Some(""));
        let mut input: &[u8] = b"";
        assert!(
            resolve_secret(&args, &mut input)
                .unwrap_err()
                .to_string()
                .contains("must not be empty")
        );
    }

    async fn flagless_login(web: &dyn WebLoginFlow, client: &FakeClient) -> (Result<i32>, String) {
        let mut input: &[u8] = b"";
        let mut out = Vec::new();
        let result = login(
            &flagless_args(Some("hub.lns.run")),
            "hub.lns.run",
            client,
            web,
            &mut input,
            &mut out,
        )
        .await;
        (result, String::from_utf8(out).unwrap())
    }

    #[tokio::test]
    async fn web_issued_credential_still_goes_through_the_service_and_a_rejection_fails() {
        let client = FakeClient::returning(LoginOutcome::Rejected("401 unauthorized".into()));
        let web = FakeWebLogin {
            outcome: Ok(WebLoginOutcome::Completed {
                username: "webuser".into(),
                secret: "some-web-token".into(),
            }),
        };
        let (result, _) = flagless_login(&web, &client).await;
        let err = result.unwrap_err();
        assert!(format!("{err:#}").contains("401 unauthorized"));
        assert_eq!(
            client.calls.lock().unwrap()[0],
            (
                "hub.lns.run".into(),
                "webuser".into(),
                "some-web-token".into()
            )
        );
    }

    #[tokio::test]
    async fn a_down_service_stops_a_web_login_before_the_browser_flow_starts() {
        let client = FakeClient::returning(LoginOutcome::ServiceUnavailable);
        let (result, _) = flagless_login(&NoWebLogin, &client).await;
        let err = result.unwrap_err();
        assert!(format!("{err:#}").contains("service must be running"));
    }

    #[tokio::test]
    async fn web_login_transport_error_points_at_the_flag_fallback() {
        let client = FakeClient::returning(LoginOutcome::Stored);
        let web = FakeWebLogin {
            outcome: Err("connection refused".into()),
        };
        let (result, _) = flagless_login(&web, &client).await;
        let err = format!("{:#}", result.unwrap_err());
        assert!(
            err.contains("starting browser login to hub.lns.run")
                && err.contains("--password-stdin")
                && err.contains("connection refused"),
            "got: {err}"
        );
        assert!(
            client.calls.lock().unwrap().is_empty(),
            "nothing to send when the flow never started"
        );
    }

    #[test]
    fn resolve_secret_requires_a_password_source() {
        let args = login_args(Some("ghcr.io"), Some("me"), None);
        let mut input: &[u8] = b"";
        assert!(
            resolve_secret(&args, &mut input)
                .unwrap_err()
                .to_string()
                .contains("password is required")
        );
    }
}
