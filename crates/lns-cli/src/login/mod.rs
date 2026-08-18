use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use lns_policy::registry_auth::{
    JsonFileRegistryAuthStore, RegistryAuthStore, RegistryCredential, canonical_registry,
    validate_registry_host,
};

use crate::command::{CommandSpec, subcommand};
use crate::connector::LocalBoxFuture;

mod real;
mod web;

pub use real::RealRegistryVerifier;
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
}

#[derive(clap::Args)]
pub struct LogoutArgs {
    #[arg(
        help = "Registry host to log out of; defaults to the configured `run.registry`, else hub.lns.run."
    )]
    pub registry: Option<String>,
}

/// The result of asking the running service to verify a registry credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginOutcome {
    Verified,
    Rejected(String),
    ServiceUnavailable,
}

/// Verifies a registry credential by running the pull-auth handshake through the running service.
pub trait RegistryVerifier {
    fn verify<'a>(
        &'a self,
        registry: &'a str,
        username: &'a str,
        secret: &'a str,
    ) -> LocalBoxFuture<'a, Result<LoginOutcome>>;
}

/// The terminal result of driving a browser-based device login against a registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebLoginOutcome {
    Completed { username: String, secret: String },
    Unsupported,
    Denied,
    Expired,
    Failed(String),
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
    auth_path: &Path,
    verifier: &dyn RegistryVerifier,
    web: &dyn WebLoginFlow,
    input: &mut dyn std::io::BufRead,
    out: &mut impl Write,
) -> Result<i32> {
    if args.list {
        return list(auth_path, out);
    }
    login(args, default_registry, auth_path, verifier, web, input, out).await
}

fn target_registry(positional: Option<&str>, default_registry: &str) -> Result<String> {
    let host = positional.unwrap_or(default_registry);
    validate_registry_host(host).map_err(|e| anyhow!("{e}"))?;
    Ok(canonical_registry(host))
}

async fn login(
    args: &LoginArgs,
    default_registry: &str,
    auth_path: &Path,
    verifier: &dyn RegistryVerifier,
    web: &dyn WebLoginFlow,
    input: &mut dyn std::io::BufRead,
    out: &mut impl Write,
) -> Result<i32> {
    let registry = target_registry(args.registry.as_deref(), default_registry)?;
    let (username, secret) = resolve_credentials(args, &registry, web, input, out).await?;

    match verifier.verify(&registry, &username, &secret).await? {
        LoginOutcome::ServiceUnavailable => bail!(
            "the background service must be running to verify a login; start it with `lns service start`"
        ),
        LoginOutcome::Rejected(reason) => bail!("login to {registry} was rejected: {reason}"),
        LoginOutcome::Verified => {}
    }

    let store = JsonFileRegistryAuthStore::new(auth_path.to_path_buf());
    let mut file = store
        .load()
        .with_context(|| format!("reading {}", auth_path.display()))?;
    file.insert(
        registry.clone(),
        RegistryCredential {
            username: username.clone(),
            secret,
        },
    );
    store
        .save(&file)
        .with_context(|| format!("writing {}", auth_path.display()))?;
    writeln!(out, "Logged in to {registry} as {username}.")?;
    Ok(0)
}

pub fn logout(
    args: &LogoutArgs,
    default_registry: &str,
    auth_path: &Path,
    out: &mut impl Write,
) -> Result<i32> {
    let registry = target_registry(args.registry.as_deref(), default_registry)?;
    let store = JsonFileRegistryAuthStore::new(auth_path.to_path_buf());
    let mut file = store
        .load()
        .with_context(|| format!("reading {}", auth_path.display()))?;
    if file.remove(&registry).is_none() {
        bail!("not logged in to {registry}");
    }
    store
        .save(&file)
        .with_context(|| format!("writing {}", auth_path.display()))?;
    writeln!(out, "Logged out of {registry}.")?;
    Ok(0)
}

fn list(auth_path: &Path, out: &mut impl Write) -> Result<i32> {
    let store = JsonFileRegistryAuthStore::new(auth_path.to_path_buf());
    let file = store
        .load()
        .with_context(|| format!("reading {}", auth_path.display()))?;
    if file.is_empty() {
        writeln!(out, "Not logged in to any registry.")?;
        return Ok(0);
    }
    let mut rows: Vec<(&String, &RegistryCredential)> = file.iter().collect();
    rows.sort_by(|a, b| a.0.cmp(b.0));
    for (registry, cred) in rows {
        writeln!(out, "{registry}  {}", cred.username)?;
    }
    Ok(0)
}

async fn resolve_credentials(
    args: &LoginArgs,
    registry: &str,
    web: &dyn WebLoginFlow,
    input: &mut dyn std::io::BufRead,
    out: &mut dyn Write,
) -> Result<(String, String)> {
    if args.username.is_none() && args.password.is_none() && !args.password_stdin {
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
        .with_context(|| format!("starting web-based login to {registry}; {FLAG_FALLBACK}"))?;
    match outcome {
        WebLoginOutcome::Completed { username, secret } => Ok((username, secret)),
        WebLoginOutcome::Unsupported => {
            bail!("{registry} does not offer web-based login; {FLAG_FALLBACK}")
        }
        WebLoginOutcome::Denied => bail!("login to {registry} was denied in the browser"),
        WebLoginOutcome::Expired => {
            bail!(
                "the confirmation code expired before the login was approved; run `lns login` again"
            )
        }
        WebLoginOutcome::Failed(reason) => bail!("web-based login to {registry} failed: {reason}"),
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
    use tempfile::TempDir;

    struct FakeVerifier {
        outcome: LoginOutcome,
        calls: std::sync::Mutex<Vec<(String, String, String)>>,
    }

    impl FakeVerifier {
        fn returning(outcome: LoginOutcome) -> Self {
            Self {
                outcome,
                calls: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    impl RegistryVerifier for FakeVerifier {
        fn verify<'a>(
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
            let outcome = self.outcome.clone();
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
        }
    }

    fn store_at(dir: &TempDir) -> std::path::PathBuf {
        dir.path().join(".lns-registry-auth.json")
    }

    fn loaded(path: &Path) -> lns_policy::registry_auth::RegistryAuthFile {
        JsonFileRegistryAuthStore::new(path.to_path_buf())
            .load()
            .unwrap()
    }

    #[tokio::test]
    async fn login_verifies_then_stores_the_credential_under_the_canonical_host() {
        let dir = TempDir::new().unwrap();
        let path = store_at(&dir);
        let verifier = FakeVerifier::returning(LoginOutcome::Verified);
        let mut input: &[u8] = b"";
        let mut out = Vec::new();
        let code = login(
            &login_args(Some("ghcr.io"), Some("octocat"), Some("ghp_token")),
            "docker.io",
            &path,
            &verifier,
            &NoWebLogin,
            &mut input,
            &mut out,
        )
        .await
        .unwrap();
        assert_eq!(code, 0);
        let file = loaded(&path);
        let cred = file.get("ghcr.io").expect("entry under ghcr.io");
        assert_eq!(cred.username, "octocat");
        assert_eq!(cred.secret, "ghp_token");
        assert!(
            String::from_utf8(out)
                .unwrap()
                .contains("Logged in to ghcr.io as octocat"),
        );
    }

    #[tokio::test]
    async fn login_folds_a_docker_hub_alias_onto_the_canonical_key() {
        let dir = TempDir::new().unwrap();
        let path = store_at(&dir);
        let verifier = FakeVerifier::returning(LoginOutcome::Verified);
        let mut input: &[u8] = b"";
        let mut out = Vec::new();
        login(
            &login_args(Some("index.docker.io"), Some("hubuser"), Some("hubpass")),
            "docker.io",
            &path,
            &verifier,
            &NoWebLogin,
            &mut input,
            &mut out,
        )
        .await
        .unwrap();
        assert!(loaded(&path).contains_key("docker.io"));
        // The verifier is asked about the canonical host, not the alias the user typed.
        assert_eq!(verifier.calls.lock().unwrap()[0].0, "docker.io");
    }

    #[tokio::test]
    async fn login_falls_back_to_the_default_registry_when_no_host_is_given() {
        let dir = TempDir::new().unwrap();
        let path = store_at(&dir);
        let verifier = FakeVerifier::returning(LoginOutcome::Verified);
        let mut input: &[u8] = b"";
        let mut out = Vec::new();
        login(
            &login_args(None, Some("me"), Some("tok")),
            "ghcr.io",
            &path,
            &verifier,
            &NoWebLogin,
            &mut input,
            &mut out,
        )
        .await
        .unwrap();
        assert!(loaded(&path).contains_key("ghcr.io"));
    }

    #[tokio::test]
    async fn login_reads_the_secret_from_stdin_when_password_stdin_is_set() {
        let dir = TempDir::new().unwrap();
        let path = store_at(&dir);
        let verifier = FakeVerifier::returning(LoginOutcome::Verified);
        let mut args = login_args(Some("ghcr.io"), Some("me"), None);
        args.password_stdin = true;
        let mut input: &[u8] = b"piped-secret\n";
        let mut out = Vec::new();
        login(
            &args,
            "registry.example.test",
            &path,
            &verifier,
            &NoWebLogin,
            &mut input,
            &mut out,
        )
        .await
        .unwrap();
        assert_eq!(loaded(&path).get("ghcr.io").unwrap().secret, "piped-secret");
    }

    #[tokio::test]
    async fn login_requires_a_username() {
        let dir = TempDir::new().unwrap();
        let verifier = FakeVerifier::returning(LoginOutcome::Verified);
        let mut input: &[u8] = b"";
        let mut out = Vec::new();
        let err = login(
            &login_args(Some("ghcr.io"), None, Some("tok")),
            "docker.io",
            &store_at(&dir),
            &verifier,
            &NoWebLogin,
            &mut input,
            &mut out,
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("username is required"));
    }

    #[tokio::test]
    async fn login_rejects_an_invalid_registry_before_verifying() {
        let dir = TempDir::new().unwrap();
        let verifier = FakeVerifier::returning(LoginOutcome::Verified);
        let mut input: &[u8] = b"";
        let mut out = Vec::new();
        let err = login(
            &login_args(Some("https://ghcr.io"), Some("me"), Some("tok")),
            "docker.io",
            &store_at(&dir),
            &verifier,
            &NoWebLogin,
            &mut input,
            &mut out,
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("URL"), "got: {err:#}");
        assert!(
            verifier.calls.lock().unwrap().is_empty(),
            "must not verify a bad host"
        );
    }

    #[tokio::test]
    async fn login_does_not_store_when_the_service_rejects_the_credential() {
        let dir = TempDir::new().unwrap();
        let path = store_at(&dir);
        let verifier = FakeVerifier::returning(LoginOutcome::Rejected("401 unauthorized".into()));
        let mut input: &[u8] = b"";
        let mut out = Vec::new();
        let err = login(
            &login_args(Some("ghcr.io"), Some("me"), Some("wrong")),
            "docker.io",
            &path,
            &verifier,
            &NoWebLogin,
            &mut input,
            &mut out,
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("401 unauthorized"));
        assert!(
            loaded(&path).is_empty(),
            "a rejected login must not be stored"
        );
    }

    #[tokio::test]
    async fn login_reports_when_the_service_is_unavailable() {
        let dir = TempDir::new().unwrap();
        let verifier = FakeVerifier::returning(LoginOutcome::ServiceUnavailable);
        let mut input: &[u8] = b"";
        let mut out = Vec::new();
        let err = login(
            &login_args(Some("ghcr.io"), Some("me"), Some("tok")),
            "docker.io",
            &store_at(&dir),
            &verifier,
            &NoWebLogin,
            &mut input,
            &mut out,
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("service must be running"));
    }

    #[tokio::test]
    async fn run_dispatches_to_list_and_hides_secrets() {
        let dir = TempDir::new().unwrap();
        let path = store_at(&dir);
        let verifier = FakeVerifier::returning(LoginOutcome::Verified);
        for (host, user, pass) in [
            ("ghcr.io", "octocat", "ghp_secret"),
            ("quay.io", "robot", "qpw"),
        ] {
            let mut input: &[u8] = b"";
            let mut sink = Vec::new();
            login(
                &login_args(Some(host), Some(user), Some(pass)),
                "docker.io",
                &path,
                &verifier,
                &NoWebLogin,
                &mut input,
                &mut sink,
            )
            .await
            .unwrap();
        }
        let mut args = login_args(None, None, None);
        args.list = true;
        let mut input: &[u8] = b"";
        let mut out = Vec::new();
        let code = run(
            &args,
            "registry.example.test",
            &path,
            &verifier,
            &NoWebLogin,
            &mut input,
            &mut out,
        )
        .await
        .unwrap();
        assert_eq!(code, 0);
        let text = String::from_utf8(out).unwrap();
        assert_eq!(
            text, "ghcr.io  octocat\nquay.io  robot\n",
            "sorted hosts + usernames"
        );
        assert!(
            !text.contains("ghp_secret") && !text.contains("qpw"),
            "secrets must never be listed"
        );
    }

    #[tokio::test]
    async fn run_dispatches_to_login_when_list_is_not_set() {
        let dir = TempDir::new().unwrap();
        let path = store_at(&dir);
        let verifier = FakeVerifier::returning(LoginOutcome::Verified);
        let mut input: &[u8] = b"";
        let mut out = Vec::new();
        let code = run(
            &login_args(Some("ghcr.io"), Some("me"), Some("tok")),
            "docker.io",
            &path,
            &verifier,
            &NoWebLogin,
            &mut input,
            &mut out,
        )
        .await
        .unwrap();
        assert_eq!(code, 0);
        assert!(loaded(&path).contains_key("ghcr.io"));
    }

    #[test]
    fn list_reports_when_no_registries_are_logged_in() {
        let dir = TempDir::new().unwrap();
        let mut out = Vec::new();
        let code = list(&store_at(&dir), &mut out).unwrap();
        assert_eq!(code, 0);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "Not logged in to any registry.\n"
        );
    }

    #[test]
    fn logout_removes_a_stored_credential() {
        let dir = TempDir::new().unwrap();
        let path = store_at(&dir);
        let store = JsonFileRegistryAuthStore::new(path.clone());
        let mut file = lns_policy::registry_auth::RegistryAuthFile::new();
        file.insert(
            "ghcr.io".into(),
            RegistryCredential {
                username: "me".into(),
                secret: "s".into(),
            },
        );
        store.save(&file).unwrap();
        let mut out = Vec::new();
        let code = logout(
            &LogoutArgs {
                registry: Some("ghcr.io".into()),
            },
            "docker.io",
            &path,
            &mut out,
        )
        .unwrap();
        assert_eq!(code, 0);
        assert!(loaded(&path).is_empty());
        assert!(
            String::from_utf8(out)
                .unwrap()
                .contains("Logged out of ghcr.io")
        );
    }

    #[test]
    fn logout_errors_when_not_logged_in() {
        let dir = TempDir::new().unwrap();
        let err = logout(
            &LogoutArgs {
                registry: Some("ghcr.io".into()),
            },
            "docker.io",
            &store_at(&dir),
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("not logged in to ghcr.io"));
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

    async fn flagless_login(
        web: &dyn WebLoginFlow,
        verifier: &FakeVerifier,
        path: &Path,
    ) -> (Result<i32>, String) {
        let mut input: &[u8] = b"";
        let mut out = Vec::new();
        let result = login(
            &flagless_args(Some("hub.lns.run")),
            "hub.lns.run",
            path,
            verifier,
            web,
            &mut input,
            &mut out,
        )
        .await;
        (result, String::from_utf8(out).unwrap())
    }

    #[tokio::test]
    async fn web_issued_credential_still_goes_through_the_verifier_and_a_rejection_is_not_stored() {
        let dir = TempDir::new().unwrap();
        let path = store_at(&dir);
        let verifier = FakeVerifier::returning(LoginOutcome::Rejected("401 unauthorized".into()));
        let web = FakeWebLogin {
            outcome: Ok(WebLoginOutcome::Completed {
                username: "webuser".into(),
                secret: "some-web-token".into(),
            }),
        };
        let (result, _) = flagless_login(&web, &verifier, &path).await;
        let err = result.unwrap_err();
        assert!(format!("{err:#}").contains("401 unauthorized"));
        assert_eq!(
            verifier.calls.lock().unwrap()[0],
            (
                "hub.lns.run".into(),
                "webuser".into(),
                "some-web-token".into()
            )
        );
        assert!(
            loaded(&path).is_empty(),
            "a rejected web credential must not be stored"
        );
    }

    #[tokio::test]
    async fn web_login_reports_when_the_service_is_unavailable_after_completion() {
        let dir = TempDir::new().unwrap();
        let verifier = FakeVerifier::returning(LoginOutcome::ServiceUnavailable);
        let web = FakeWebLogin {
            outcome: Ok(WebLoginOutcome::Completed {
                username: "webuser".into(),
                secret: "some-web-token".into(),
            }),
        };
        let (result, _) = flagless_login(&web, &verifier, &store_at(&dir)).await;
        let err = result.unwrap_err();
        assert!(format!("{err:#}").contains("service must be running"));
    }

    #[tokio::test]
    async fn web_login_failure_reason_is_surfaced() {
        let dir = TempDir::new().unwrap();
        let verifier = FakeVerifier::returning(LoginOutcome::Verified);
        let web = FakeWebLogin {
            outcome: Ok(WebLoginOutcome::Failed("registry answered 500".into())),
        };
        let (result, _) = flagless_login(&web, &verifier, &store_at(&dir)).await;
        let err = format!("{:#}", result.unwrap_err());
        assert!(
            err.contains("web-based login to hub.lns.run failed: registry answered 500"),
            "got: {err}"
        );
    }

    #[tokio::test]
    async fn web_login_transport_error_points_at_the_flag_fallback() {
        let dir = TempDir::new().unwrap();
        let verifier = FakeVerifier::returning(LoginOutcome::Verified);
        let web = FakeWebLogin {
            outcome: Err("connection refused".into()),
        };
        let (result, _) = flagless_login(&web, &verifier, &store_at(&dir)).await;
        let err = format!("{:#}", result.unwrap_err());
        assert!(
            err.contains("starting web-based login to hub.lns.run")
                && err.contains("--password-stdin")
                && err.contains("connection refused"),
            "got: {err}"
        );
        assert!(
            verifier.calls.lock().unwrap().is_empty(),
            "nothing to verify when the flow never started"
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
