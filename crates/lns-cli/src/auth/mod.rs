use std::io::{Read, Write};

use anyhow::{Context, Result, bail};
use lns_policy::registry_auth::{RegistryCredential, RegistryCredentialStore};

use crate::cli::{AuthCommand, LoginArgs, LogoutArgs};

pub fn run(
    cmd: &AuthCommand,
    store: &dyn RegistryCredentialStore,
    writer: &mut impl Write,
) -> Result<i32> {
    match cmd {
        AuthCommand::List => list(store, writer),
    }
}

pub fn login(
    args: &LoginArgs,
    store: &dyn RegistryCredentialStore,
    stdin: &mut dyn Read,
    writer: &mut impl Write,
) -> Result<i32> {
    if !args.password_stdin {
        bail!("pass --password-stdin and pipe the token in; tokens are never accepted as a flag");
    }
    let mut token = String::new();
    stdin
        .read_to_string(&mut token)
        .context("reading token from stdin")?;
    let token = token.trim().to_string();
    if token.is_empty() {
        bail!("no token received on stdin");
    }
    let mut state = store.load().context("loading registry credentials")?;
    state.insert(
        args.registry.clone(),
        RegistryCredential {
            username: args.username.clone(),
            token,
        },
    );
    store.save(&state).context("saving registry credentials")?;
    writeln!(writer, "Stored credential for {}", args.registry)?;
    Ok(0)
}

pub fn logout(
    args: &LogoutArgs,
    store: &dyn RegistryCredentialStore,
    writer: &mut impl Write,
) -> Result<i32> {
    let mut state = store.load().context("loading registry credentials")?;
    if state.remove(&args.registry).is_none() {
        bail!("no stored credential for {}", args.registry);
    }
    store.save(&state).context("saving registry credentials")?;
    writeln!(writer, "Removed credential for {}", args.registry)?;
    Ok(0)
}

fn list(store: &dyn RegistryCredentialStore, writer: &mut impl Write) -> Result<i32> {
    let state = store.load().context("loading registry credentials")?;
    if state.is_empty() {
        writeln!(writer, "No stored registry credentials")?;
        return Ok(0);
    }
    let mut registries: Vec<&String> = state.keys().collect();
    registries.sort();
    for registry in registries {
        let username = state[registry].username.as_deref().unwrap_or("any");
        writeln!(writer, "{registry}  {username}  (token stored)")?;
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lns_policy::registry_auth::RegistryAuthFile;
    use std::io;
    use std::sync::Mutex;

    #[derive(Default)]
    struct MemStore(Mutex<RegistryAuthFile>);
    impl RegistryCredentialStore for MemStore {
        fn load(&self) -> io::Result<RegistryAuthFile> {
            Ok(self.0.lock().unwrap().clone())
        }
        fn save(&self, state: &RegistryAuthFile) -> io::Result<()> {
            *self.0.lock().unwrap() = state.clone();
            Ok(())
        }
    }

    fn login_args(registry: &str, username: Option<&str>, password_stdin: bool) -> LoginArgs {
        LoginArgs {
            registry: registry.into(),
            username: username.map(str::to_string),
            password_stdin,
        }
    }

    #[test]
    fn login_stores_the_token_read_from_stdin() {
        let store = MemStore::default();
        let mut out = Vec::new();
        let code = login(
            &login_args("registry.example.test", Some("any"), true),
            &store,
            &mut &b"lns_secret_token\n"[..],
            &mut out,
        )
        .unwrap();
        assert_eq!(code, 0);
        let state = store.load().unwrap();
        let cred = &state["registry.example.test"];
        assert_eq!(cred.token, "lns_secret_token");
        assert_eq!(cred.username.as_deref(), Some("any"));
        assert!(
            String::from_utf8(out)
                .unwrap()
                .contains("Stored credential")
        );
    }

    #[test]
    fn login_requires_password_stdin() {
        let err = login(
            &login_args("reg", None, false),
            &MemStore::default(),
            &mut &b"tok"[..],
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("--password-stdin"),
            "got: {err:#}"
        );
    }

    #[test]
    fn login_rejects_an_empty_token() {
        let err = login(
            &login_args("reg", None, true),
            &MemStore::default(),
            &mut &b"   \n"[..],
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("no token received"),
            "got: {err:#}"
        );
    }

    #[test]
    fn login_overwrites_an_existing_credential() {
        let store = MemStore::default();
        login(
            &login_args("reg", None, true),
            &store,
            &mut &b"first"[..],
            &mut Vec::new(),
        )
        .unwrap();
        login(
            &login_args("reg", None, true),
            &store,
            &mut &b"second"[..],
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(store.load().unwrap()["reg"].token, "second");
    }

    #[test]
    fn logout_removes_a_stored_credential() {
        let store = MemStore::default();
        login(
            &login_args("reg", None, true),
            &store,
            &mut &b"tok"[..],
            &mut Vec::new(),
        )
        .unwrap();
        let mut out = Vec::new();
        logout(
            &LogoutArgs {
                registry: "reg".into(),
            },
            &store,
            &mut out,
        )
        .unwrap();
        assert!(store.load().unwrap().is_empty());
        assert!(
            String::from_utf8(out)
                .unwrap()
                .contains("Removed credential")
        );
    }

    #[test]
    fn logout_errors_when_no_credential_is_stored() {
        let err = logout(
            &LogoutArgs {
                registry: "ghost".into(),
            },
            &MemStore::default(),
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("no stored credential"),
            "got: {err:#}"
        );
    }

    #[test]
    fn list_reports_registries_and_usernames_without_the_token() {
        let store = MemStore::default();
        login(
            &login_args("b.example", Some("ci"), true),
            &store,
            &mut &b"super_secret"[..],
            &mut Vec::new(),
        )
        .unwrap();
        login(
            &login_args("a.example", None, true),
            &store,
            &mut &b"another_secret"[..],
            &mut Vec::new(),
        )
        .unwrap();
        let mut out = Vec::new();
        run(&AuthCommand::List, &store, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(
            !text.contains("super_secret"),
            "token must never be printed: {text}"
        );
        assert!(
            !text.contains("another_secret"),
            "token must never be printed: {text}"
        );
        assert!(
            text.contains("a.example  any  (token stored)"),
            "got: {text}"
        );
        assert!(
            text.contains("b.example  ci  (token stored)"),
            "got: {text}"
        );
        // sorted: a before b
        assert!(text.find("a.example").unwrap() < text.find("b.example").unwrap());
    }

    #[test]
    fn list_reports_when_no_credentials_are_stored() {
        let mut out = Vec::new();
        run(&AuthCommand::List, &MemStore::default(), &mut out).unwrap();
        assert!(
            String::from_utf8(out)
                .unwrap()
                .contains("No stored registry credentials")
        );
    }
}
