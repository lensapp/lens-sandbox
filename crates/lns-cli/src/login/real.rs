use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::FromArgMatches;
use lns_ipc::{Request, Response, decode_frame, encode_frame, read_frame_bytes_async};
use lns_policy::registry_auth::{RegistryAuthStore, RegistryCredential};
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;

use super::{
    ListLoginsOutcome, LoginArgs, LoginOutcome, LogoutArgs, LogoutOutcome, RegistryAuthClient,
    WebLoginFlow, WebLoginOutcome,
};
use crate::command::{RunCtx, RunFuture};
use crate::local_future::LocalBoxFuture;

pub fn run_login<'a>(matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = LoginArgs::from_arg_matches(matches)?;
        let default_registry = configured_default_registry()?;
        let socket = crate::service::socket_path()?;
        let socket_exists = socket.exists();
        let service = RealRegistryAuthClient::new(socket);
        let store = lns_policy::registry_auth::JsonFileRegistryAuthStore::new(
            lns_ipc::registry_auth_path()?,
        );
        let file_login_allowed = args.username.is_some() && args.password_stdin;
        let client = RegistryAuthRouter::new(service, store, socket_exists, file_login_allowed);
        let input = ctx.input;
        let mut out = ctx.out;
        super::run(
            &args,
            &default_registry,
            &client,
            &RealWebLoginFlow,
            input,
            &mut out,
        )
        .await
    })
}

pub fn run_logout<'a>(matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = LogoutArgs::from_arg_matches(matches)?;
        let default_registry = configured_default_registry()?;
        let socket = crate::service::socket_path()?;
        let socket_exists = socket.exists();
        let service = RealRegistryAuthClient::new(socket);
        let store = lns_policy::registry_auth::JsonFileRegistryAuthStore::new(
            lns_ipc::registry_auth_path()?,
        );
        let client = RegistryAuthRouter::new(service, store, socket_exists, false);
        let mut out = ctx.out;
        super::logout(&args, &default_registry, &client, &mut out).await
    })
}

fn configured_default_registry() -> Result<String> {
    let path = crate::config::default_config_path()?;
    let defaults = crate::config::load_run_defaults(&path)?;
    Ok(defaults.registry_or_default().to_string())
}

/// Opens `url` via the platform opener (`open` on macOS, `xdg-open` elsewhere), reporting whether it spawned.
struct RealBrowserOpener;

impl super::BrowserOpener for RealBrowserOpener {
    fn open(&self, url: &str) -> bool {
        let opener = if cfg!(target_os = "macos") {
            "open"
        } else {
            "xdg-open"
        };
        std::process::Command::new(opener).arg(url).spawn().is_ok()
    }
}

/// Builds the reqwest device-auth client for the registry and drives the browser device flow with it.
struct RealWebLoginFlow;

impl WebLoginFlow for RealWebLoginFlow {
    fn login<'a>(
        &'a self,
        registry: &'a str,
        out: &'a mut dyn std::io::Write,
    ) -> LocalBoxFuture<'a, Result<WebLoginOutcome>> {
        Box::pin(async move {
            let client = super::RealDeviceAuthClient::for_registry(
                registry,
                env!("CARGO_PKG_VERSION"),
                &crate::platform::detect(),
            )?;
            let flow = super::WebLogin::new(client, RealBrowserOpener);
            flow.login(registry, out).await
        })
    }
}

struct RegistryAuthRouter<C, S> {
    service: C,
    store: S,
    socket_exists: bool,
    file_login_allowed: bool,
}

impl<C, S> RegistryAuthRouter<C, S> {
    fn new(service: C, store: S, socket_exists: bool, file_login_allowed: bool) -> Self {
        Self {
            service,
            store,
            socket_exists,
            file_login_allowed,
        }
    }
}

impl<C: RegistryAuthClient, S: RegistryAuthStore> RegistryAuthClient for RegistryAuthRouter<C, S> {
    fn available<'a>(&'a self) -> LocalBoxFuture<'a, Result<bool>> {
        self.service.available()
    }

    fn login<'a>(
        &'a self,
        registry: &'a str,
        username: &'a str,
        secret: &'a str,
    ) -> LocalBoxFuture<'a, Result<LoginOutcome>> {
        if self.socket_exists || !self.file_login_allowed {
            return self.service.login(registry, username, secret);
        }
        Box::pin(async move {
            let mut state = self
                .store
                .load()
                .context("reading the registry login store")?;
            state.insert(
                registry.to_string(),
                RegistryCredential {
                    username: username.to_string(),
                    secret: secret.to_string(),
                },
            );
            self.store
                .save(&state)
                .context("writing the registry login store")?;
            crate::log::warn!(
                "The background service was not running; the registry login was written to ~/.lns/registry-auth.json."
            );
            Ok(LoginOutcome::Stored)
        })
    }

    fn logout<'a>(&'a self, registry: &'a str) -> LocalBoxFuture<'a, Result<LogoutOutcome>> {
        if self.socket_exists {
            return self.service.logout(registry);
        }
        Box::pin(async move {
            let mut state = self
                .store
                .load()
                .context("reading the registry login store")?;
            if state.remove(registry).is_some() {
                self.store
                    .save(&state)
                    .context("writing the registry login store")?;
            }
            Ok(LogoutOutcome::LoggedOutFromFile)
        })
    }

    fn list<'a>(&'a self) -> LocalBoxFuture<'a, Result<ListLoginsOutcome>> {
        self.service.list()
    }
}

pub struct RealRegistryAuthClient {
    socket: PathBuf,
}

impl RealRegistryAuthClient {
    pub fn new(socket: PathBuf) -> Self {
        Self { socket }
    }

    async fn roundtrip(&self, request: &Request, what: &str) -> Result<Option<Response>> {
        let Ok(mut stream) = UnixStream::connect(&self.socket).await else {
            return Ok(None);
        };
        let frame = encode_frame(request).with_context(|| format!("encoding {what} request"))?;
        stream
            .write_all(&frame)
            .await
            .with_context(|| format!("writing {what} request"))?;
        let bytes = read_frame_bytes_async(&mut stream)
            .await
            .with_context(|| format!("reading {what} response"))?;
        let response = decode_frame::<Response, _>(&mut &bytes[..])
            .with_context(|| format!("decoding {what} response"))?;
        Ok(Some(response))
    }
}

impl RegistryAuthClient for RealRegistryAuthClient {
    fn available<'a>(&'a self) -> LocalBoxFuture<'a, Result<bool>> {
        Box::pin(async move {
            match self.roundtrip(&Request::Ping, "ping").await? {
                Some(response) => Ok(matches!(response, Response::Pong)),
                None => Ok(false),
            }
        })
    }

    fn login<'a>(
        &'a self,
        registry: &'a str,
        username: &'a str,
        secret: &'a str,
    ) -> LocalBoxFuture<'a, Result<LoginOutcome>> {
        Box::pin(async move {
            let request = Request::RegistryLogin {
                registry: registry.to_string(),
                username: username.to_string(),
                secret: secret.to_string(),
            };
            match self.roundtrip(&request, "login").await? {
                None => Ok(LoginOutcome::ServiceUnavailable),
                Some(Response::RegistryLoginStored) => Ok(LoginOutcome::Stored),
                Some(Response::Error { message }) => Ok(LoginOutcome::Rejected(message)),
                Some(other) => bail!("unexpected response during login: {other:?}"),
            }
        })
    }

    fn logout<'a>(&'a self, registry: &'a str) -> LocalBoxFuture<'a, Result<LogoutOutcome>> {
        Box::pin(async move {
            let request = Request::RegistryLogout {
                registry: registry.to_string(),
            };
            match self.roundtrip(&request, "logout").await? {
                None => Ok(LogoutOutcome::ServiceUnavailable),
                Some(Response::RegistryLoggedOut) => Ok(LogoutOutcome::LoggedOut),
                Some(Response::Error { message }) => Ok(LogoutOutcome::Failed(message)),
                Some(other) => bail!("unexpected response during logout: {other:?}"),
            }
        })
    }

    fn list<'a>(&'a self) -> LocalBoxFuture<'a, Result<ListLoginsOutcome>> {
        Box::pin(async move {
            match self
                .roundtrip(&Request::ListRegistryLogins, "login list")
                .await?
            {
                None => Ok(ListLoginsOutcome::ServiceUnavailable),
                Some(Response::RegistryLogins { logins }) => Ok(ListLoginsOutcome::Logins(logins)),
                Some(Response::Error { message }) => bail!("listing registry logins: {message}"),
                Some(other) => bail!("unexpected response during login list: {other:?}"),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::io;
    use std::sync::Mutex;

    use lns_policy::registry_auth::{RegistryAuthFile, RegistryCredential};

    use super::*;

    struct FakeService {
        login_calls: Mutex<usize>,
        logout_calls: Mutex<usize>,
    }

    impl FakeService {
        fn new() -> Self {
            Self {
                login_calls: Mutex::new(0),
                logout_calls: Mutex::new(0),
            }
        }
    }

    impl RegistryAuthClient for FakeService {
        fn available<'a>(&'a self) -> LocalBoxFuture<'a, Result<bool>> {
            Box::pin(async { Ok(true) })
        }

        fn login<'a>(
            &'a self,
            _registry: &'a str,
            _username: &'a str,
            _secret: &'a str,
        ) -> LocalBoxFuture<'a, Result<LoginOutcome>> {
            *self.login_calls.lock().expect("login calls lock") += 1;
            Box::pin(async { Ok(LoginOutcome::Stored) })
        }

        fn logout<'a>(&'a self, _registry: &'a str) -> LocalBoxFuture<'a, Result<LogoutOutcome>> {
            *self.logout_calls.lock().expect("logout calls lock") += 1;
            Box::pin(async { Ok(LogoutOutcome::LoggedOut) })
        }

        fn list<'a>(&'a self) -> LocalBoxFuture<'a, Result<ListLoginsOutcome>> {
            Box::pin(async { Ok(ListLoginsOutcome::Logins(Vec::new())) })
        }
    }

    struct MemoryStore {
        state: Mutex<RegistryAuthFile>,
        saves: Mutex<usize>,
    }

    impl MemoryStore {
        fn with_entry(registry: &str) -> Self {
            Self {
                state: Mutex::new(HashMap::from([(
                    registry.to_string(),
                    RegistryCredential {
                        username: "old-user".into(),
                        secret: "old-secret".into(),
                    },
                )])),
                saves: Mutex::new(0),
            }
        }
    }

    impl RegistryAuthStore for MemoryStore {
        fn load(&self) -> io::Result<RegistryAuthFile> {
            Ok(self.state.lock().expect("state lock").clone())
        }

        fn save(&self, state: &RegistryAuthFile) -> io::Result<()> {
            *self.state.lock().expect("state lock") = state.clone();
            *self.saves.lock().expect("saves lock") += 1;
            Ok(())
        }
    }

    #[tokio::test]
    async fn headless_login_writes_the_file_when_the_service_socket_is_missing() {
        let router = RegistryAuthRouter::new(
            FakeService::new(),
            MemoryStore::with_entry("quay.io"),
            false,
            true,
        );

        assert_eq!(
            router.login("ghcr.io", "octocat", "token").await.unwrap(),
            LoginOutcome::Stored
        );
        let state = router.store.load().unwrap();
        assert_eq!(state["ghcr.io"].username, "octocat");
        assert!(state.contains_key("quay.io"), "other hosts stay untouched");
        assert_eq!(*router.service.login_calls.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn login_uses_the_service_without_writing_the_file_when_the_socket_exists() {
        let router = RegistryAuthRouter::new(
            FakeService::new(),
            MemoryStore::with_entry("quay.io"),
            true,
            true,
        );

        router.login("ghcr.io", "octocat", "token").await.unwrap();

        assert_eq!(*router.service.login_calls.lock().unwrap(), 1);
        assert_eq!(*router.store.saves.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn logout_removes_only_the_file_entry_when_the_service_socket_is_missing() {
        let router = RegistryAuthRouter::new(
            FakeService::new(),
            MemoryStore::with_entry("ghcr.io"),
            false,
            false,
        );

        assert_eq!(
            router.logout("ghcr.io").await.unwrap(),
            LogoutOutcome::LoggedOutFromFile
        );
        assert!(!router.store.load().unwrap().contains_key("ghcr.io"));
        assert_eq!(*router.service.logout_calls.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn logout_succeeds_without_rewriting_a_file_that_has_no_entry() {
        let router = RegistryAuthRouter::new(
            FakeService::new(),
            MemoryStore::with_entry("quay.io"),
            false,
            false,
        );

        assert_eq!(
            router.logout("ghcr.io").await.unwrap(),
            LogoutOutcome::LoggedOutFromFile
        );
        assert_eq!(*router.store.saves.lock().unwrap(), 0);
        assert!(router.store.load().unwrap().contains_key("quay.io"));
    }

    #[tokio::test]
    async fn logout_uses_the_service_without_writing_the_file_when_the_socket_exists() {
        let router = RegistryAuthRouter::new(
            FakeService::new(),
            MemoryStore::with_entry("ghcr.io"),
            true,
            false,
        );

        router.logout("ghcr.io").await.unwrap();

        assert_eq!(*router.service.logout_calls.lock().unwrap(), 1);
        assert_eq!(*router.store.saves.lock().unwrap(), 0);
    }
}
