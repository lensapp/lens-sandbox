use std::io::{Read, Write};
use std::path::Path;

use anyhow::{Context, Result, bail};
use lns_policy::Policy;
use lns_policy::credentials::{CredentialEntry, CredentialStore, JsonFileCredentialStore};
use lns_policy::integrations::{AuthKind, Catalog, Integration, effective_integrations};
use lns_policy::providers::{ProviderDef, is_self_identifying};

use crate::cli::{
    CredentialAddArgs, CredentialClearArgs, CredentialCommand, CredentialInjectArgs,
    CredentialRemoveArgs, CredentialScopeArgs, CredentialSetArgs,
};
use crate::run::summary::policy_path;

pub fn run(
    cmd: &CredentialCommand,
    cwd: &Path,
    creds_path: &Path,
    catalog_path: &Path,
    reader: &mut impl Read,
    writer: &mut impl Write,
) -> Result<i32> {
    match cmd {
        CredentialCommand::Add(args) => add(args, cwd, creds_path, reader, writer),
        CredentialCommand::AddInjection(args) => add_injection(args, cwd, writer),
        CredentialCommand::Set(args) => set(args, cwd, creds_path, catalog_path, reader, writer),
        CredentialCommand::Clear(args) => clear(args, creds_path, writer),
        CredentialCommand::List(args) => list(args, cwd, creds_path, writer),
        CredentialCommand::Remove(args) => remove(args, cwd, creds_path, writer),
    }
}

fn is_builtin(id: &str) -> bool {
    lns_policy::providers::builtins().iter().any(|p| p.id == id)
}

/// Reads a piped secret so `--value-stdin` keeps it out of argv and shell history; one trailing newline is trimmed and an empty value is rejected.
fn read_stdin_value(reader: &mut impl Read) -> Result<String> {
    let mut buf = String::new();
    reader
        .read_to_string(&mut buf)
        .context("reading credential value from stdin")?;
    let value = buf.strip_suffix('\n').unwrap_or(&buf);
    let value = value.strip_suffix('\r').unwrap_or(value);
    if value.is_empty() {
        bail!(
            "no credential value on stdin; pipe the secret, e.g. `printf %s \"$TOKEN\" | lns credential set <id> --value-stdin`"
        );
    }
    Ok(value.to_string())
}

/// Self-identifying so the MITM can detect it without false positives; explicit `--placeholder` is for shape-sensitive providers.
fn generate_placeholder(id: &str) -> String {
    format!("lns-placeholder-{id}-0000000000000000000000")
}

fn add(
    args: &CredentialAddArgs,
    cwd: &Path,
    creds_path: &Path,
    reader: &mut impl Read,
    writer: &mut impl Write,
) -> Result<i32> {
    let path = policy_path(args.policy.as_deref(), cwd);
    let mut policy = Policy::load_or_default(&path).context("loading policy")?;
    if is_builtin(&args.id) {
        bail!(
            "{:?} is a built-in provider and cannot be redeclared",
            args.id
        );
    }
    if policy
        .credentials
        .custom_providers
        .iter()
        .any(|p| p.id == args.id)
    {
        bail!("custom provider {:?} already exists", args.id);
    }
    let placeholder = match &args.placeholder {
        Some(p) if !is_self_identifying(p) => bail!(
            "placeholder {p:?} must self-identify as fake (contain \"placeholder\" or \"lns\")"
        ),
        Some(p) => p.clone(),
        None => generate_placeholder(&args.id),
    };
    // Resolve the stored value (including any stdin read) before touching disk so an empty pipe can't leave a declared-but-valueless provider behind.
    let value = if args.value_stdin {
        Some(read_stdin_value(reader)?)
    } else {
        args.value.clone()
    };
    policy.credentials.custom_providers.push(ProviderDef {
        id: args.id.clone(),
        env_var: args.env_var.clone(),
        placeholder,
        injections: args.inject.clone(),
    });
    policy
        .save_atomic(&path)
        .with_context(|| format!("writing {}", path.display()))?;
    if let Some(value) = value {
        let (store, mut state) = load_creds(creds_path)?;
        state.insert(args.id.clone(), CredentialEntry::Stored { value });
        store
            .save(&state)
            .with_context(|| format!("writing {}", creds_path.display()))?;
    }
    writeln!(writer, "Declared custom provider {:?}", args.id)?;
    writeln!(
        writer,
        "Note: the new placeholder reaches a workload only at launch; relaunch any running sandbox to pick it up."
    )?;
    Ok(0)
}

fn add_injection(args: &CredentialInjectArgs, cwd: &Path, writer: &mut impl Write) -> Result<i32> {
    let path = policy_path(args.policy.as_deref(), cwd);
    let mut policy = Policy::load_or_default(&path).context("loading policy")?;
    let Some(def) = policy
        .credentials
        .custom_providers
        .iter_mut()
        .find(|p| p.id == args.id)
    else {
        bail!(
            "no custom provider {:?}; declare it first with `lns credential add`",
            args.id
        );
    };
    def.injections.push(args.inject.clone());
    policy
        .save_atomic(&path)
        .with_context(|| format!("writing {}", path.display()))?;
    writeln!(writer, "Added injection for {:?}", args.id)?;
    writeln!(
        writer,
        "Note: the new injection reaches a workload only at launch; relaunch any running sandbox to pick it up."
    )?;
    Ok(0)
}

fn remove(
    args: &CredentialRemoveArgs,
    cwd: &Path,
    creds_path: &Path,
    writer: &mut impl Write,
) -> Result<i32> {
    if is_builtin(&args.id) {
        bail!("{:?} is a built-in provider and cannot be removed", args.id);
    }
    let path = policy_path(args.policy.as_deref(), cwd);
    let mut policy = Policy::load_or_default(&path).context("loading policy")?;
    let before = policy.credentials.custom_providers.len();
    policy
        .credentials
        .custom_providers
        .retain(|p| p.id != args.id);
    if policy.credentials.custom_providers.len() == before {
        bail!("no custom provider {:?} to remove", args.id);
    }
    policy
        .save_atomic(&path)
        .with_context(|| format!("writing {}", path.display()))?;
    let (store, mut state) = load_creds(creds_path)?;
    if state.remove(&args.id).is_some() {
        store
            .save(&state)
            .with_context(|| format!("writing {}", creds_path.display()))?;
    }
    writeln!(writer, "Removed custom provider {:?}", args.id)?;
    Ok(0)
}

fn is_known(id: &str, policy: &Policy, catalog: &[Integration]) -> bool {
    lns_policy::providers::builtins().iter().any(|p| p.id == id)
        || policy
            .credentials
            .custom_providers
            .iter()
            .any(|p| p.id == id)
        || catalog
            .iter()
            .any(|i| i.id == id && i.auth_kind == AuthKind::Credential)
}

fn load_creds(
    creds_path: &Path,
) -> Result<(
    JsonFileCredentialStore,
    lns_policy::credentials::CredentialStateFile,
)> {
    let store = JsonFileCredentialStore::new(creds_path.to_path_buf());
    let state = store
        .load()
        .with_context(|| format!("reading {}", creds_path.display()))?;
    Ok((store, state))
}

fn set(
    args: &CredentialSetArgs,
    cwd: &Path,
    creds_path: &Path,
    catalog_path: &Path,
    reader: &mut impl Read,
    writer: &mut impl Write,
) -> Result<i32> {
    let path = policy_path(args.policy.as_deref(), cwd);
    let policy = Policy::load_or_default(&path).context("loading policy")?;
    let user_catalog = Catalog::load_or_default(catalog_path).context("loading integrations")?;
    let catalog = effective_integrations(&user_catalog);
    if !is_known(&args.id, &policy, &catalog) {
        bail!(
            "unknown credential provider {:?}; declare it with `lns credential add` or connect an integration",
            args.id
        );
    }
    let (entry, word) = if args.value_stdin {
        (
            CredentialEntry::Stored {
                value: read_stdin_value(reader)?,
            },
            "stored",
        )
    } else if let Some(value) = &args.value {
        (
            CredentialEntry::Stored {
                value: value.clone(),
            },
            "stored",
        )
    } else if args.host {
        (CredentialEntry::HostDetect, "host-detect")
    } else {
        (CredentialEntry::Deny, "deny")
    };
    let (store, mut state) = load_creds(creds_path)?;
    state.insert(args.id.clone(), entry);
    store
        .save(&state)
        .with_context(|| format!("writing {}", creds_path.display()))?;
    writeln!(writer, "Set {word} credential for {:?}", args.id)?;
    Ok(0)
}

fn clear(args: &CredentialClearArgs, creds_path: &Path, writer: &mut impl Write) -> Result<i32> {
    let (store, mut state) = load_creds(creds_path)?;
    state.remove(&args.id);
    store
        .save(&state)
        .with_context(|| format!("writing {}", creds_path.display()))?;
    writeln!(writer, "Cleared credential decision for {:?}", args.id)?;
    Ok(0)
}

fn list(
    args: &CredentialScopeArgs,
    cwd: &Path,
    creds_path: &Path,
    writer: &mut impl Write,
) -> Result<i32> {
    let path = policy_path(args.policy.as_deref(), cwd);
    let policy = Policy::load_or_default(&path).context("loading policy")?;
    let (_store, state) = load_creds(creds_path)?;
    for def in lns_policy::providers::builtins() {
        writeln!(
            writer,
            "{}  (built-in)  {}",
            def.id,
            describe(state.get(&def.id))
        )?;
    }
    for def in &policy.credentials.custom_providers {
        // A custom id colliding with a built-in is dropped at run start, so don't show it as a live provider.
        if is_builtin(&def.id) {
            continue;
        }
        writeln!(
            writer,
            "{}  (custom)  {}",
            def.id,
            describe(state.get(&def.id))
        )?;
    }
    Ok(0)
}

fn describe(entry: Option<&CredentialEntry>) -> &'static str {
    match entry {
        None => "no decision yet",
        Some(CredentialEntry::HostDetect) => "host value",
        Some(CredentialEntry::Stored { .. }) => "stored (hidden)",
        Some(CredentialEntry::Deny) => "denied",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lns_policy::providers::{InjectionDef, InjectionKind, ProviderDef};
    use tempfile::TempDir;

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

    fn no_stdin() -> std::io::Empty {
        std::io::empty()
    }

    fn no_catalog() -> std::path::PathBuf {
        std::path::PathBuf::from("/nonexistent-lns-test/.lns-integrations.yaml")
    }

    fn acme_policy(dir: &Path) {
        let mut policy = Policy::default();
        policy.credentials.custom_providers.push(ProviderDef {
            id: "acme".into(),
            env_var: "ACME_API_KEY".into(),
            placeholder: "acme_LNSPLACEHOLDER".into(),
            injections: vec![InjectionDef {
                kind: InjectionKind::BearerHeader,
                domain: "api.acme.corp".into(),
                header: None,
            }],
        });
        policy.save_atomic(&dir.join("lns-policy.yaml")).unwrap();
    }

    fn load_state(creds: &Path) -> lns_policy::credentials::CredentialStateFile {
        JsonFileCredentialStore::new(creds.to_path_buf())
            .load()
            .unwrap()
    }

    #[test]
    fn set_stored_writes_a_stored_entry() {
        let dir = TempDir::new().unwrap();
        let creds = dir.path().join("creds.json");
        let mut args = set_args("github");
        args.value = Some("ghp_real".into());
        let mut out = Vec::new();
        run(
            &CredentialCommand::Set(args),
            dir.path(),
            &creds,
            &no_catalog(),
            &mut no_stdin(),
            &mut out,
        )
        .unwrap();
        assert_eq!(
            load_state(&creds).get("github"),
            Some(&CredentialEntry::Stored {
                value: "ghp_real".into()
            })
        );
    }

    #[test]
    fn set_host_writes_a_host_detect_entry() {
        let dir = TempDir::new().unwrap();
        let creds = dir.path().join("creds.json");
        let mut args = set_args("github");
        args.host = true;
        let mut out = Vec::new();
        set(
            &args,
            dir.path(),
            &creds,
            &no_catalog(),
            &mut no_stdin(),
            &mut out,
        )
        .unwrap();
        assert_eq!(
            load_state(&creds).get("github"),
            Some(&CredentialEntry::HostDetect)
        );
    }

    #[test]
    fn set_deny_writes_a_deny_entry() {
        let dir = TempDir::new().unwrap();
        let creds = dir.path().join("creds.json");
        let mut args = set_args("github");
        args.deny = true;
        let mut out = Vec::new();
        set(
            &args,
            dir.path(),
            &creds,
            &no_catalog(),
            &mut no_stdin(),
            &mut out,
        )
        .unwrap();
        assert_eq!(
            load_state(&creds).get("github"),
            Some(&CredentialEntry::Deny)
        );
    }

    #[test]
    fn set_accepts_a_declared_custom_provider() {
        let dir = TempDir::new().unwrap();
        let creds = dir.path().join("creds.json");
        acme_policy(dir.path());
        let mut args = set_args("acme");
        args.value = Some("acme_real".into());
        let mut out = Vec::new();
        set(
            &args,
            dir.path(),
            &creds,
            &no_catalog(),
            &mut no_stdin(),
            &mut out,
        )
        .unwrap();
        assert!(load_state(&creds).contains_key("acme"));
    }

    #[test]
    fn set_rejects_an_unknown_provider_and_leaves_the_file_untouched() {
        let dir = TempDir::new().unwrap();
        let creds = dir.path().join("creds.json");
        let mut args = set_args("made-up");
        args.value = Some("x".into());
        let mut out = Vec::new();
        let err = set(
            &args,
            dir.path(),
            &creds,
            &no_catalog(),
            &mut no_stdin(),
            &mut out,
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("made-up"));
        assert!(
            !creds.exists(),
            "no creds file should be written for an unknown id"
        );
    }

    #[test]
    fn clear_removes_an_existing_entry() {
        let dir = TempDir::new().unwrap();
        let creds = dir.path().join("creds.json");
        let mut args = set_args("github");
        args.host = true;
        set(
            &args,
            dir.path(),
            &creds,
            &no_catalog(),
            &mut no_stdin(),
            &mut Vec::new(),
        )
        .unwrap();

        let mut out = Vec::new();
        clear(
            &CredentialClearArgs {
                id: "github".into(),
            },
            &creds,
            &mut out,
        )
        .unwrap();
        assert!(!load_state(&creds).contains_key("github"));
    }

    #[test]
    fn list_shows_builtin_decisions_and_masks_stored_values() {
        let dir = TempDir::new().unwrap();
        let creds = dir.path().join("creds.json");
        for (id, mut a) in [
            ("github", set_args("github")),
            ("openai", set_args("openai")),
            ("linear", set_args("linear")),
        ] {
            match id {
                "github" => a.host = true,
                "openai" => a.value = Some("sk-real-token".into()),
                _ => a.deny = true,
            }
            set(
                &a,
                dir.path(),
                &creds,
                &no_catalog(),
                &mut no_stdin(),
                &mut Vec::new(),
            )
            .unwrap();
        }
        let mut out = Vec::new();
        list(
            &CredentialScopeArgs { policy: None },
            dir.path(),
            &creds,
            &mut out,
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("github  (built-in)  host value"), "{text}");
        assert!(
            text.contains("openai  (built-in)  stored (hidden)"),
            "{text}"
        );
        assert!(text.contains("linear  (built-in)  denied"), "{text}");
        assert!(
            text.contains("anthropic  (built-in)  no decision yet"),
            "{text}"
        );
        assert!(
            !text.contains("sk-real-token"),
            "stored value must be masked: {text}"
        );
    }

    #[test]
    fn list_labels_custom_providers_from_the_policy_file() {
        let dir = TempDir::new().unwrap();
        let creds = dir.path().join("creds.json");
        acme_policy(dir.path());
        let mut out = Vec::new();
        list(
            &CredentialScopeArgs { policy: None },
            dir.path(),
            &creds,
            &mut out,
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("acme  (custom)  no decision yet"), "{text}");
    }

    fn bearer(domain: &str) -> InjectionDef {
        InjectionDef {
            kind: InjectionKind::BearerHeader,
            domain: domain.into(),
            header: None,
        }
    }

    #[test]
    fn add_injection_to_an_unknown_provider_errors() {
        let dir = TempDir::new().unwrap();
        let err = add_injection(
            &CredentialInjectArgs {
                id: "ghost".into(),
                inject: bearer("api.ghost.example"),
                policy: None,
            },
            dir.path(),
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("ghost"));
    }

    #[test]
    fn remove_a_nonexistent_custom_provider_errors() {
        let dir = TempDir::new().unwrap();
        let creds = dir.path().join("creds.json");
        let err = remove(
            &CredentialRemoveArgs {
                id: "ghost".into(),
                policy: None,
            },
            dir.path(),
            &creds,
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("ghost"));
    }

    #[test]
    fn remove_also_clears_a_stored_value_for_the_custom_provider() {
        let dir = TempDir::new().unwrap();
        let creds = dir.path().join("creds.json");
        acme_policy(dir.path());
        let mut set_value = set_args("acme");
        set_value.value = Some("acme_real".into());
        set(
            &set_value,
            dir.path(),
            &creds,
            &no_catalog(),
            &mut no_stdin(),
            &mut Vec::new(),
        )
        .unwrap();
        assert!(load_state(&creds).contains_key("acme"));

        remove(
            &CredentialRemoveArgs {
                id: "acme".into(),
                policy: None,
            },
            dir.path(),
            &creds,
            &mut Vec::new(),
        )
        .unwrap();

        let policy = Policy::load_or_default(&dir.path().join("lns-policy.yaml")).unwrap();
        assert!(policy.credentials.custom_providers.is_empty());
        assert!(
            !load_state(&creds).contains_key("acme"),
            "remove must also clear the stored value"
        );
    }

    #[test]
    fn list_skips_a_custom_provider_that_shadows_a_builtin() {
        let dir = TempDir::new().unwrap();
        let creds = dir.path().join("creds.json");
        let mut policy = Policy::default();
        policy.credentials.custom_providers.push(ProviderDef {
            id: "github".into(),
            env_var: "GITHUB_TOKEN".into(),
            placeholder: "lns-shadow".into(),
            injections: vec![bearer("api.github.com")],
        });
        policy
            .save_atomic(&dir.path().join("lns-policy.yaml"))
            .unwrap();
        let mut out = Vec::new();
        list(
            &CredentialScopeArgs { policy: None },
            dir.path(),
            &creds,
            &mut out,
        )
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("github  (built-in)"), "{text}");
        assert!(
            !text.contains("github  (custom)"),
            "a custom id shadowing a built-in is inert at run start and must not be listed: {text}"
        );
    }

    #[test]
    fn add_with_value_keeps_the_secret_out_of_the_shareable_policy_file() {
        let dir = TempDir::new().unwrap();
        let creds = dir.path().join("creds.json");
        let args = CredentialAddArgs {
            id: "acme".into(),
            env_var: "ACME_API_KEY".into(),
            inject: vec![bearer("api.acme.corp")],
            placeholder: None,
            value: Some("acme_secret_value".into()),
            value_stdin: false,
            policy: None,
        };
        add(&args, dir.path(), &creds, &mut no_stdin(), &mut Vec::new()).unwrap();
        let policy_text = std::fs::read_to_string(dir.path().join("lns-policy.yaml")).unwrap();
        assert!(
            !policy_text.contains("acme_secret_value"),
            "the real value must never land in the shareable policy file:\n{policy_text}"
        );
        assert_eq!(
            load_state(&creds).get("acme"),
            Some(&CredentialEntry::Stored {
                value: "acme_secret_value".into()
            }),
            "the value belongs only in the per-machine credentials file"
        );
    }

    #[test]
    fn set_value_stdin_stores_the_piped_secret_trimming_the_trailing_newline() {
        let dir = TempDir::new().unwrap();
        let creds = dir.path().join("creds.json");
        let mut args = set_args("github");
        args.value_stdin = true;
        set(
            &args,
            dir.path(),
            &creds,
            &no_catalog(),
            &mut &b"ghp_real\n"[..],
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(
            load_state(&creds).get("github"),
            Some(&CredentialEntry::Stored {
                value: "ghp_real".into()
            })
        );
    }

    #[test]
    fn set_value_stdin_trims_a_crlf_line_ending() {
        let dir = TempDir::new().unwrap();
        let creds = dir.path().join("creds.json");
        let mut args = set_args("github");
        args.value_stdin = true;
        set(
            &args,
            dir.path(),
            &creds,
            &no_catalog(),
            &mut &b"ghp_real\r\n"[..],
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(
            load_state(&creds).get("github"),
            Some(&CredentialEntry::Stored {
                value: "ghp_real".into()
            })
        );
    }

    #[test]
    fn set_value_stdin_accepts_a_value_with_no_trailing_newline() {
        let dir = TempDir::new().unwrap();
        let creds = dir.path().join("creds.json");
        let mut args = set_args("github");
        args.value_stdin = true;
        set(
            &args,
            dir.path(),
            &creds,
            &no_catalog(),
            &mut &b"ghp_real"[..],
            &mut Vec::new(),
        )
        .unwrap();
        assert_eq!(
            load_state(&creds).get("github"),
            Some(&CredentialEntry::Stored {
                value: "ghp_real".into()
            })
        );
    }

    #[test]
    fn set_value_stdin_with_empty_input_errors_and_leaves_the_file_untouched() {
        let dir = TempDir::new().unwrap();
        let creds = dir.path().join("creds.json");
        let mut args = set_args("github");
        args.value_stdin = true;
        let err = set(
            &args,
            dir.path(),
            &creds,
            &no_catalog(),
            &mut &b"\n"[..],
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("stdin"), "got: {err:#}");
        assert!(
            !creds.exists(),
            "an empty piped value must not write the credentials file"
        );
    }

    #[test]
    fn set_value_stdin_surfaces_non_utf8_input_as_an_error() {
        let dir = TempDir::new().unwrap();
        let creds = dir.path().join("creds.json");
        let mut args = set_args("github");
        args.value_stdin = true;
        let err = set(
            &args,
            dir.path(),
            &creds,
            &no_catalog(),
            &mut &[0xff_u8, 0xfe][..],
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("stdin"), "got: {err:#}");
    }

    #[test]
    fn add_value_stdin_stores_the_secret_only_in_the_credentials_file() {
        let dir = TempDir::new().unwrap();
        let creds = dir.path().join("creds.json");
        let args = CredentialAddArgs {
            id: "acme".into(),
            env_var: "ACME_API_KEY".into(),
            inject: vec![bearer("api.acme.corp")],
            placeholder: None,
            value: None,
            value_stdin: true,
            policy: None,
        };
        add(
            &args,
            dir.path(),
            &creds,
            &mut &b"acme_secret_value\n"[..],
            &mut Vec::new(),
        )
        .unwrap();
        let policy_text = std::fs::read_to_string(dir.path().join("lns-policy.yaml")).unwrap();
        assert!(
            !policy_text.contains("acme_secret_value"),
            "the piped value must never land in the shareable policy file:\n{policy_text}"
        );
        assert_eq!(
            load_state(&creds).get("acme"),
            Some(&CredentialEntry::Stored {
                value: "acme_secret_value".into()
            })
        );
    }

    #[test]
    fn add_value_stdin_with_empty_input_declares_nothing() {
        let dir = TempDir::new().unwrap();
        let creds = dir.path().join("creds.json");
        let args = CredentialAddArgs {
            id: "acme".into(),
            env_var: "ACME_API_KEY".into(),
            inject: vec![bearer("api.acme.corp")],
            placeholder: None,
            value: None,
            value_stdin: true,
            policy: None,
        };
        let err = add(&args, dir.path(), &creds, &mut &b""[..], &mut Vec::new()).unwrap_err();
        assert!(format!("{err:#}").contains("stdin"), "got: {err:#}");
        assert!(
            !dir.path().join("lns-policy.yaml").exists(),
            "an empty piped value must not leave a half-declared provider behind"
        );
    }
}
