use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use lns_policy::Policy;
use lns_policy::connectors::{
    AuthKind, Catalog, Connector, ConnectorRoute, CredentialAuth, bundled_connectors,
    effective_connectors,
};
use lns_policy::providers::is_self_identifying;
use lns_policy::pulled;

use crate::command::{CommandSpec, subcommand};
use crate::run::summary::policy_path;

mod real;
mod sign_in;

pub use real::RealConnectorSignIn;
pub use sign_in::{BindOutcome, ConnectorSignIn, LocalBoxFuture, SignInOutcome};

#[derive(clap::Args)]
pub struct ConnectorArgs {
    #[command(subcommand)]
    pub command: ConnectorCommand,
}

#[derive(clap::Subcommand)]
pub enum ConnectorCommand {
    #[command(about = "Declare a credential connector in your machine-global catalog.")]
    Add(ConnectorAddArgs),
    #[command(about = "List the bundled and user-declared connectors.")]
    List,
    #[command(about = "Remove a user-declared connector; bundled ones cannot be removed.")]
    Remove(ConnectorRemoveArgs),
    #[command(
        about = "Bind a connector's per-machine value decision (oauth connectors sign in); records the id in this directory's policy."
    )]
    Connect(ConnectArgs),
    #[command(about = "Disconnect a connector from this directory's policy.")]
    Disconnect(DisconnectArgs),
    #[command(
        about = "Build a connector definition file and upload it to a registry as a connector artifact."
    )]
    Publish(ConnectorPublishArgs),
}

#[derive(clap::Args)]
pub struct ConnectorAddArgs {
    #[arg(help = "New connector id; must not collide with a bundled or existing user connector.")]
    pub id: String,
    #[arg(long, help = "Environment variable the placeholder is seeded into.")]
    pub env_var: String,
    #[arg(
        long = "inject",
        required = true,
        value_parser = parse_injection,
        help = "Per-domain injection as KIND:DOMAIN (api_key_header needs KIND:DOMAIN:HEADER). Repeatable."
    )]
    pub inject: Vec<lns_policy::providers::InjectionDef>,
    #[arg(
        long = "route",
        help = "A host pattern the connector needs reachable. Repeatable."
    )]
    pub route: Vec<String>,
    #[arg(
        long,
        help = "Placeholder value; auto-generated (self-identifying) when omitted."
    )]
    pub placeholder: Option<String>,
}

#[derive(clap::Args)]
pub struct ConnectorRemoveArgs {
    #[arg(help = "User-declared connector id to remove.")]
    pub id: String,
}

#[derive(clap::Args)]
pub struct ConnectArgs {
    #[arg(help = "Connector id to connect (from `lns connector list`).")]
    pub id: String,
    #[arg(
        long,
        help = "Policy file path; defaults to `lns-policy.yaml` in the current directory."
    )]
    pub policy: Option<PathBuf>,
}

#[derive(clap::Args)]
pub struct DisconnectArgs {
    #[arg(help = "Connector id to disconnect.")]
    pub id: String,
    #[arg(
        long,
        help = "Policy file path; defaults to `lns-policy.yaml` in the current directory."
    )]
    pub policy: Option<PathBuf>,
}

#[derive(clap::Args)]
pub struct ConnectorPublishArgs {
    #[arg(
        help = "Registry reference to publish to, e.g. registry.lns.run/connectors/some-provider:0.1.0."
    )]
    pub reference: String,
    #[arg(
        short = 'f',
        long = "file",
        help = "Connector definition file (YAML) to publish."
    )]
    pub file: PathBuf,
    #[arg(
        long,
        help = "Build and validate without uploading; prints the digest that would publish."
    )]
    pub dry_run: bool,
}

/// Uploads a built connector artifact to a registry; the real impl reuses the `lns login` credential, a fake drives the publish scenarios offline.
pub trait ConnectorPublisher {
    fn push<'a>(
        &'a self,
        built: &'a lns_artifact::build::BuiltArtifact,
        reference: &'a str,
    ) -> LocalBoxFuture<'a, Result<()>>;
}

fn parse_injection(s: &str) -> Result<lns_policy::providers::InjectionDef, String> {
    use lns_policy::providers::{InjectionDef, InjectionKind};
    let mut parts = s.splitn(3, ':');
    let kind_str = parts
        .next()
        .ok_or_else(|| format!("expected KIND:DOMAIN, got {s:?}"))?;
    let domain = parts
        .next()
        .ok_or_else(|| format!("expected KIND:DOMAIN, got {s:?}"))?;
    if domain.is_empty() {
        return Err(format!("injection {s:?} is missing a domain"));
    }
    let header_segment = parts.next();
    let kind = match kind_str {
        "bearer_header" => InjectionKind::BearerHeader,
        "uri_placeholder" => InjectionKind::UriPlaceholder,
        "token_header" => InjectionKind::TokenHeader,
        "basic_x_access_token" => InjectionKind::BasicXAccessToken,
        "api_key_header" => InjectionKind::ApiKeyHeader,
        "awsSigv4" | "aws_sigv4" => {
            return Err(
                "awsSigv4 carries real STS material and is not declarable from the CLI".to_string(),
            );
        }
        other => {
            return Err(format!(
                "unknown injection kind {other:?}; use bearer_header, uri_placeholder, token_header, basic_x_access_token, or api_key_header"
            ));
        }
    };
    let header = match (kind, header_segment) {
        (InjectionKind::ApiKeyHeader, Some(h)) if !h.is_empty() => Some(h.to_string()),
        (InjectionKind::ApiKeyHeader, _) => {
            return Err("api_key_header requires a header name (KIND:DOMAIN:HEADER)".to_string());
        }
        (_, Some(_)) => {
            return Err(format!(
                "kind {kind_str} does not take a header name; expected KIND:DOMAIN"
            ));
        }
        (_, None) => None,
    };
    Ok(InjectionDef {
        kind,
        domain: domain.to_string(),
        header,
    })
}

pub fn augment(app: clap::Command) -> clap::Command {
    app.subcommand(
        subcommand::<ConnectorArgs>("connector")
            .about("Manage the credential-connector catalog (connectable services)."),
    )
}

pub const SPEC: CommandSpec = CommandSpec {
    name: "connector",
    augment,
    run: real::run,
    announces_update_check: true,
    owns_terminal: false,
};

pub async fn run(
    cmd: &ConnectorCommand,
    cwd: &Path,
    catalog_path: &Path,
    signin: &dyn ConnectorSignIn,
    publisher: &dyn ConnectorPublisher,
    writer: &mut impl Write,
) -> Result<i32> {
    match cmd {
        ConnectorCommand::Add(args) => add(args, catalog_path, writer),
        ConnectorCommand::List => list(catalog_path, writer),
        ConnectorCommand::Remove(args) => remove(args, catalog_path, writer),
        ConnectorCommand::Connect(args) => connect(args, cwd, catalog_path, signin, writer).await,
        ConnectorCommand::Disconnect(args) => disconnect(args, cwd, writer),
        ConnectorCommand::Publish(args) => publish(args, cwd, publisher, writer).await,
    }
}

/// `lns connector publish <ref> -f <file>`: build the connector definition into a connector artifact and upload it. Publishing is inert — an artifact is only reachable once its id is added to the discovery index.
pub async fn publish(
    args: &ConnectorPublishArgs,
    cwd: &Path,
    publisher: &dyn ConnectorPublisher,
    writer: &mut impl Write,
) -> Result<i32> {
    let path = cwd.join(&args.file);
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let connector: Connector = serde_yaml::from_str(&text)
        .with_context(|| format!("parsing a connector from {}", path.display()))?;
    let built = lns_artifact::build::build_connector(&connector)?;
    let bytes: usize = built.blobs.iter().map(|blob| blob.data.len()).sum();
    if args.dry_run {
        writeln!(
            writer,
            "would push {}@{} ({bytes} bytes)",
            args.reference, built.manifest_digest
        )?;
        writeln!(writer, "dry run — built and validated; nothing uploaded")?;
        return Ok(0);
    }
    publisher.push(&built, &args.reference).await?;
    writeln!(
        writer,
        "built and pushed {}@{}",
        args.reference, built.manifest_digest
    )?;
    Ok(0)
}

/// Self-identifying so the MITM can detect it without false positives; explicit `--placeholder` is for shape-sensitive providers.
fn generate_placeholder(id: &str) -> String {
    format!("lns-placeholder-{id}-0000000000000000000000")
}

fn is_bundled(id: &str) -> bool {
    bundled_connectors().iter().any(|i| i.id == id)
}

fn load_catalog(path: &Path) -> Result<Catalog> {
    Catalog::load_or_default(path).with_context(|| format!("loading {}", path.display()))
}

fn kind_word(kind: AuthKind) -> &'static str {
    match kind {
        AuthKind::Credential => "credential",
        AuthKind::Oauth => "oauth",
    }
}

fn add(args: &ConnectorAddArgs, catalog_path: &Path, writer: &mut impl Write) -> Result<i32> {
    if is_bundled(&args.id) {
        bail!(
            "{:?} is a bundled connector and cannot be redeclared",
            args.id
        );
    }
    let mut catalog = load_catalog(catalog_path)?;
    if catalog.connectors.iter().any(|i| i.id == args.id) {
        bail!("connector {:?} already exists in your catalog", args.id);
    }
    let placeholder = match &args.placeholder {
        Some(p) if !is_self_identifying(p) => bail!(
            "placeholder {p:?} must self-identify as fake (contain \"placeholder\" or \"lns\")"
        ),
        Some(p) => p.clone(),
        None => generate_placeholder(&args.id),
    };
    let routes = args
        .route
        .iter()
        .map(|host| ConnectorRoute {
            match_pattern: host.clone(),
            transport: None,
            scheme: None,
            tls_terminate: false,
            rules: Vec::new(),
        })
        .collect();
    catalog.connectors.push(Connector {
        id: args.id.clone(),
        name: None,
        auth_kind: AuthKind::Credential,
        routes,
        credential: Some(CredentialAuth {
            env_var: args.env_var.clone(),
            placeholder,
            injections: args.inject.clone(),
        }),
        oauth: None,
        token_fallback: None,
    });
    catalog
        .save_atomic(catalog_path)
        .with_context(|| format!("writing {}", catalog_path.display()))?;
    writeln!(writer, "Declared connector {:?} in your catalog", args.id)?;
    writeln!(
        writer,
        "Connect it to a project with `lns connect {}`.",
        args.id
    )?;
    Ok(0)
}

fn list(catalog_path: &Path, writer: &mut impl Write) -> Result<i32> {
    let user = load_catalog(catalog_path)?;
    for i in bundled_connectors() {
        writeln!(writer, "{}  (bundled)  {}", i.id, kind_word(i.auth_kind))?;
    }
    for i in &user.connectors {
        // A user id that shadows a bundled one is inert (bundled wins), so don't list it as live.
        if is_bundled(&i.id) {
            continue;
        }
        writeln!(writer, "{}  (user)  {}", i.id, kind_word(i.auth_kind))?;
    }
    Ok(0)
}

fn remove(args: &ConnectorRemoveArgs, catalog_path: &Path, writer: &mut impl Write) -> Result<i32> {
    if is_bundled(&args.id) {
        bail!("{:?} is a bundled connector and cannot be removed", args.id);
    }
    let mut catalog = load_catalog(catalog_path)?;
    let before = catalog.connectors.len();
    catalog.connectors.retain(|i| i.id != args.id);
    if catalog.connectors.len() == before {
        bail!("no connector {:?} in your catalog to remove", args.id);
    }
    catalog
        .save_atomic(catalog_path)
        .with_context(|| format!("writing {}", catalog_path.display()))?;
    writeln!(writer, "Removed connector {:?}", args.id)?;
    Ok(0)
}

pub async fn connect(
    args: &ConnectArgs,
    cwd: &Path,
    catalog_path: &Path,
    signin: &dyn ConnectorSignIn,
    writer: &mut impl Write,
) -> Result<i32> {
    let user = load_catalog(catalog_path)?;
    let effective = effective_connectors(&user, &pulled::PulledCatalog::default());
    let Some(integ) = effective.iter().find(|i| i.id == args.id) else {
        bail!("unknown connector {:?}; see `lns connector list`", args.id);
    };
    // An oauth connector authenticates by an interactive sign-in; a credential connector binds its per-machine value decision through the approval-window card. Either way the id is recorded only on success.
    let closing = if integ.auth_kind == AuthKind::Oauth {
        match signin.sign_in(&args.id, writer).await? {
            SignInOutcome::ServiceUnavailable => bail!(
                "the background service must be running to sign in to {:?}; start it with `lns service start`",
                args.id
            ),
            SignInOutcome::Failed(reason) => {
                bail!("sign-in to {:?} did not complete: {reason}", args.id)
            }
            SignInOutcome::Completed => format!(
                "Connected {:?}; relaunch any running sandbox to pick it up.",
                args.id
            ),
        }
    } else {
        match signin.bind_credential(&args.id, writer).await? {
            BindOutcome::ServiceUnavailable => bail!(
                "the background service must be running to bind {:?}; start it with `lns service start`",
                args.id
            ),
            BindOutcome::Failed(reason) => {
                bail!("binding {:?} did not complete: {reason}", args.id)
            }
            BindOutcome::Completed(decision) => bind_message(&args.id, decision),
        }
    };
    let path = policy_path(args.policy.as_deref(), cwd);
    let mut policy = Policy::load_or_default(&path)
        .with_context(|| format!("loading policy from {}", path.display()))?;
    policy.connect(args.id.clone());
    policy
        .save_atomic(&path)
        .with_context(|| format!("writing policy to {}", path.display()))?;
    writeln!(writer, "{closing}")?;
    Ok(0)
}

/// Each completed value decision reads back what was bound on this machine — never an effect on any sandbox definition.
fn bind_message(id: &str, decision: lns_ipc::CredentialBindDecision) -> String {
    match decision {
        lns_ipc::CredentialBindDecision::Stored => format!(
            "Bound a value for {id:?} on this machine; the boundary injects it where a workload presents the placeholder."
        ),
        lns_ipc::CredentialBindDecision::HostDetect => format!(
            "Bound {id:?} to the host-detected value on this machine; it resolves at the boundary at request time."
        ),
        lns_ipc::CredentialBindDecision::Denied => format!(
            "Recorded a deny for {id:?} on this machine; requests carrying its placeholder will fail at the boundary."
        ),
    }
}

pub fn disconnect(args: &DisconnectArgs, cwd: &Path, writer: &mut impl Write) -> Result<i32> {
    let path = policy_path(args.policy.as_deref(), cwd);
    let mut policy = Policy::load_or_default(&path)
        .with_context(|| format!("loading policy from {}", path.display()))?;
    if !policy.disconnect(&args.id) {
        bail!("{:?} is not connected in {}", args.id, path.display());
    }
    policy
        .save_atomic(&path)
        .with_context(|| format!("writing policy to {}", path.display()))?;
    writeln!(writer, "Disconnected {:?}", args.id)?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lns_policy::connectors::{OauthAuth, OauthFlow};
    use lns_policy::providers::{InjectionDef, InjectionKind};
    use tempfile::TempDir;

    #[test]
    fn parse_injection_accepts_the_two_declarable_kinds() {
        let bearer = parse_injection("bearer_header:api.acme.corp").unwrap();
        assert_eq!(bearer.kind, InjectionKind::BearerHeader);
        assert_eq!(bearer.domain, "api.acme.corp");
        let uri = parse_injection("uri_placeholder:api.rocket.example").unwrap();
        assert_eq!(uri.kind, InjectionKind::UriPlaceholder);
    }

    #[test]
    fn parse_injection_rejects_awssigv4_with_a_clear_reason() {
        let err = parse_injection("awsSigv4:*.amazonaws.com").unwrap_err();
        assert!(err.contains("awsSigv4"), "got: {err}");
        assert!(parse_injection("aws_sigv4:x").is_err());
    }

    #[test]
    fn parse_injection_rejects_an_unknown_kind() {
        let err = parse_injection("basic_auth:api.acme.corp").unwrap_err();
        assert!(err.contains("unknown injection kind"), "got: {err}");
    }

    #[test]
    fn parse_injection_requires_a_kind_and_domain() {
        assert!(
            parse_injection("bearer_header")
                .unwrap_err()
                .contains("KIND:DOMAIN")
        );
        assert!(
            parse_injection("bearer_header:")
                .unwrap_err()
                .contains("missing a domain")
        );
    }

    #[test]
    fn parse_injection_accepts_token_header() {
        let inj = parse_injection("token_header:api.example.com").unwrap();
        assert_eq!(inj.kind, InjectionKind::TokenHeader);
        assert_eq!(inj.domain, "api.example.com");
        assert_eq!(inj.header, None);
    }

    #[test]
    fn parse_injection_accepts_basic_x_access_token() {
        let inj = parse_injection("basic_x_access_token:example.com").unwrap();
        assert_eq!(inj.kind, InjectionKind::BasicXAccessToken);
        assert_eq!(inj.domain, "example.com");
        assert_eq!(inj.header, None);
    }

    #[test]
    fn parse_injection_accepts_api_key_header_with_header_name() {
        let inj = parse_injection("api_key_header:api.example.test:x-api-key").unwrap();
        assert_eq!(inj.kind, InjectionKind::ApiKeyHeader);
        assert_eq!(inj.domain, "api.example.test");
        assert_eq!(inj.header.as_deref(), Some("x-api-key"));
    }

    #[test]
    fn parse_injection_rejects_api_key_header_without_a_header_name() {
        let err = parse_injection("api_key_header:api.example.test").unwrap_err();
        assert!(
            err.contains("api_key_header") && err.contains("header name"),
            "got: {err}"
        );
        let err = parse_injection("api_key_header:api.example.test:").unwrap_err();
        assert!(
            err.contains("api_key_header") && err.contains("header name"),
            "got: {err}"
        );
    }

    #[test]
    fn parse_injection_rejects_a_header_segment_on_kinds_that_do_not_use_one() {
        let err = parse_injection("bearer_header:api.acme.corp:x-api-key").unwrap_err();
        assert!(err.contains("does not take a header name"), "got: {err}");
    }

    fn add_args(id: &str) -> ConnectorAddArgs {
        ConnectorAddArgs {
            id: id.into(),
            env_var: "ACME_API_KEY".into(),
            inject: vec![InjectionDef {
                kind: InjectionKind::BearerHeader,
                domain: "api.acme.corp".into(),
                header: None,
            }],
            route: vec!["api.acme.corp".into()],
            placeholder: None,
        }
    }

    fn catalog_at(dir: &Path) -> std::path::PathBuf {
        dir.join(".lns-connectors.yaml")
    }

    fn load(path: &Path) -> Catalog {
        Catalog::load_or_default(path).unwrap()
    }

    fn write_user_catalog(path: &Path, connectors: Vec<Connector>) {
        Catalog { connectors }.save_atomic(path).unwrap();
    }

    fn oauth_connector(id: &str) -> Connector {
        Connector {
            id: id.into(),
            name: None,
            auth_kind: AuthKind::Oauth,
            routes: vec![ConnectorRoute {
                match_pattern: "api.somesaas.com".into(),
                transport: None,
                scheme: None,
                tls_terminate: false,
                rules: Vec::new(),
            }],
            credential: None,
            oauth: Some(OauthAuth {
                flow: OauthFlow::Device,
                client_id: Some("Iv1.somesaas".into()),
                client_secret: None,
                scopes: vec!["repo".into()],
                device_authorization_endpoint: Some(
                    "https://api.somesaas.com/login/device/code".into(),
                ),
                authorization_endpoint: None,
                token_endpoint: "https://api.somesaas.com/login/oauth/access_token".into(),
                userinfo_endpoint: None,
                account_field: None,
                env_var: "SOMESAAS_TOKEN".into(),
                placeholder: "lns-somesaas-placeholder".into(),
                injections: vec![InjectionDef {
                    kind: InjectionKind::BearerHeader,
                    domain: "api.somesaas.com".into(),
                    header: None,
                }],
            }),
            token_fallback: None,
        }
    }

    #[test]
    fn add_declares_a_new_user_connector_with_routes_and_injection() {
        let dir = TempDir::new().unwrap();
        let path = catalog_at(dir.path());
        let mut out = Vec::new();
        add(&add_args("acme"), &path, &mut out).unwrap();
        let catalog = load(&path);
        assert_eq!(catalog.connectors.len(), 1);
        let acme = &catalog.connectors[0];
        assert_eq!(acme.id, "acme");
        assert_eq!(acme.auth_kind, AuthKind::Credential);
        assert_eq!(acme.routes[0].match_pattern, "api.acme.corp");
        let cred = acme.credential.as_ref().unwrap();
        assert_eq!(cred.env_var, "ACME_API_KEY");
        assert!(is_self_identifying(&cred.placeholder));
    }

    #[test]
    fn add_keeps_an_explicit_self_identifying_placeholder() {
        let dir = TempDir::new().unwrap();
        let path = catalog_at(dir.path());
        let mut args = add_args("acme");
        args.placeholder = Some("acme_LNSPLACEHOLDER".into());
        add(&args, &path, &mut Vec::new()).unwrap();
        assert_eq!(
            load(&path).connectors[0]
                .credential
                .as_ref()
                .unwrap()
                .placeholder,
            "acme_LNSPLACEHOLDER"
        );
    }

    #[test]
    fn add_rejects_a_placeholder_that_does_not_self_identify() {
        let dir = TempDir::new().unwrap();
        let mut args = add_args("acme");
        args.placeholder = Some("real-looking-token".into());
        let err = add(&args, &catalog_at(dir.path()), &mut Vec::new()).unwrap_err();
        assert!(format!("{err:#}").contains("self-identify"));
    }

    #[test]
    fn add_rejects_a_bundled_id() {
        let dir = TempDir::new().unwrap();
        let err = add(
            &add_args("gitlab"),
            &catalog_at(dir.path()),
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("bundled"));
    }

    #[test]
    fn add_rejects_a_duplicate_user_id() {
        let dir = TempDir::new().unwrap();
        let path = catalog_at(dir.path());
        add(&add_args("acme"), &path, &mut Vec::new()).unwrap();
        let err = add(&add_args("acme"), &path, &mut Vec::new()).unwrap_err();
        assert!(format!("{err:#}").contains("already exists"));
    }

    #[test]
    fn list_shows_bundled_and_user_connectors_labelled() {
        let dir = TempDir::new().unwrap();
        let path = catalog_at(dir.path());
        add(&add_args("acme"), &path, &mut Vec::new()).unwrap();
        let mut out = Vec::new();
        list(&path, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("gitlab  (bundled)  credential"), "{text}");
        assert!(text.contains("acme  (user)  credential"), "{text}");
    }

    #[test]
    fn list_labels_an_oauth_user_connector() {
        let dir = TempDir::new().unwrap();
        let path = catalog_at(dir.path());
        write_user_catalog(&path, vec![oauth_connector("somesaas")]);
        let mut out = Vec::new();
        list(&path, &mut out).unwrap();
        assert!(
            String::from_utf8(out)
                .unwrap()
                .contains("somesaas  (user)  oauth"),
            "oauth kind must be labelled"
        );
    }

    #[test]
    fn list_skips_a_user_entry_that_shadows_a_bundled_id() {
        let dir = TempDir::new().unwrap();
        let path = catalog_at(dir.path());
        write_user_catalog(
            &path,
            vec![Connector {
                id: "gitlab".into(),
                name: None,
                auth_kind: AuthKind::Credential,
                routes: Vec::new(),
                credential: Some(CredentialAuth {
                    env_var: "EVIL".into(),
                    placeholder: "lns-evil".into(),
                    injections: Vec::new(),
                }),
                oauth: None,
                token_fallback: None,
            }],
        );
        let mut out = Vec::new();
        list(&path, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("gitlab  (bundled)"), "{text}");
        assert!(
            !text.contains("gitlab  (user)"),
            "a shadow must not be listed: {text}"
        );
    }

    #[test]
    fn list_surfaces_a_malformed_catalog_as_an_error() {
        let dir = TempDir::new().unwrap();
        let path = catalog_at(dir.path());
        std::fs::write(&path, "connectors: not-a-list\n").unwrap();
        let err = list(&path, &mut Vec::new()).unwrap_err();
        assert!(format!("{err:#}").contains("loading"));
    }

    #[test]
    fn remove_deletes_a_user_connector() {
        let dir = TempDir::new().unwrap();
        let path = catalog_at(dir.path());
        add(&add_args("acme"), &path, &mut Vec::new()).unwrap();
        remove(
            &ConnectorRemoveArgs { id: "acme".into() },
            &path,
            &mut Vec::new(),
        )
        .unwrap();
        assert!(load(&path).connectors.is_empty());
    }

    #[test]
    fn remove_rejects_a_bundled_id() {
        let dir = TempDir::new().unwrap();
        let err = remove(
            &ConnectorRemoveArgs {
                id: "gitlab".into(),
            },
            &catalog_at(dir.path()),
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("bundled"));
    }

    #[test]
    fn remove_errors_on_an_unknown_user_id() {
        let dir = TempDir::new().unwrap();
        let err = remove(
            &ConnectorRemoveArgs { id: "ghost".into() },
            &catalog_at(dir.path()),
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("ghost"));
    }

    struct FakeSignIn {
        outcome: SignInOutcome,
        bind: BindOutcome,
    }
    impl FakeSignIn {
        fn completed() -> Self {
            Self {
                outcome: SignInOutcome::Completed,
                bind: BindOutcome::Completed(lns_ipc::CredentialBindDecision::Stored),
            }
        }
        fn returning(outcome: SignInOutcome) -> Self {
            Self {
                outcome,
                bind: BindOutcome::Completed(lns_ipc::CredentialBindDecision::Stored),
            }
        }
        fn binding(bind: BindOutcome) -> Self {
            Self {
                outcome: SignInOutcome::Completed,
                bind,
            }
        }
    }
    impl ConnectorSignIn for FakeSignIn {
        fn sign_in<'a>(
            &'a self,
            id: &'a str,
            out: &'a mut dyn Write,
        ) -> super::sign_in::LocalBoxFuture<'a, Result<SignInOutcome>> {
            let outcome = self.outcome.clone();
            Box::pin(async move {
                writeln!(out, "(signing in to {id})")?;
                Ok(outcome)
            })
        }

        fn bind_credential<'a>(
            &'a self,
            id: &'a str,
            out: &'a mut dyn Write,
        ) -> super::sign_in::LocalBoxFuture<'a, Result<BindOutcome>> {
            let bind = self.bind.clone();
            Box::pin(async move {
                writeln!(out, "(binding {id})")?;
                Ok(bind)
            })
        }
    }

    #[tokio::test]
    async fn connect_writes_a_bundled_connector_id_into_the_policy() {
        let dir = TempDir::new().unwrap();
        let catalog = catalog_at(dir.path());
        let mut out = Vec::new();
        connect(
            &ConnectArgs {
                id: "gitlab".into(),
                policy: None,
            },
            dir.path(),
            &catalog,
            &FakeSignIn::completed(),
            &mut out,
        )
        .await
        .unwrap();
        let policy = Policy::load_or_default(&dir.path().join("lns-policy.yaml")).unwrap();
        assert_eq!(policy.connectors, ["gitlab"]);
    }

    #[tokio::test]
    async fn connect_rejects_an_unknown_connector() {
        let dir = TempDir::new().unwrap();
        let err = connect(
            &ConnectArgs {
                id: "nope".into(),
                policy: None,
            },
            dir.path(),
            &catalog_at(dir.path()),
            &FakeSignIn::completed(),
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("unknown connector"));
    }

    #[tokio::test]
    async fn connect_signs_in_an_oauth_connector_then_records_it() {
        let dir = TempDir::new().unwrap();
        let catalog = catalog_at(dir.path());
        write_user_catalog(&catalog, vec![oauth_connector("somesaas")]);
        connect(
            &ConnectArgs {
                id: "somesaas".into(),
                policy: None,
            },
            dir.path(),
            &catalog,
            &FakeSignIn::completed(),
            &mut Vec::new(),
        )
        .await
        .unwrap();
        let policy = Policy::load_or_default(&dir.path().join("lns-policy.yaml")).unwrap();
        assert_eq!(
            policy.connectors,
            ["somesaas"],
            "a completed sign-in records the connector"
        );
    }

    #[tokio::test]
    async fn connect_to_oauth_fails_without_recording_when_sign_in_does_not_complete() {
        let dir = TempDir::new().unwrap();
        let catalog = catalog_at(dir.path());
        write_user_catalog(&catalog, vec![oauth_connector("somesaas")]);
        let err = connect(
            &ConnectArgs {
                id: "somesaas".into(),
                policy: None,
            },
            dir.path(),
            &catalog,
            &FakeSignIn::returning(SignInOutcome::Failed("access_denied".into())),
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("access_denied"), "got: {err:#}");
        let policy = Policy::load_or_default(&dir.path().join("lns-policy.yaml")).unwrap();
        assert!(
            policy.connectors.is_empty(),
            "a failed sign-in must not record the connector"
        );
    }

    #[tokio::test]
    async fn connect_binds_a_credential_and_describes_the_machine_scope() {
        let dir = TempDir::new().unwrap();
        let mut out = Vec::new();
        connect(
            &ConnectArgs {
                id: "gitlab".into(),
                policy: None,
            },
            dir.path(),
            &catalog_at(dir.path()),
            &FakeSignIn::binding(BindOutcome::Completed(
                lns_ipc::CredentialBindDecision::HostDetect,
            )),
            &mut out,
        )
        .await
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("host-detected value on this machine"),
            "got: {text}"
        );
        assert!(
            !text.contains("relaunch any running sandbox"),
            "a bind must not claim a sandbox effect: {text}"
        );
    }

    #[tokio::test]
    async fn connect_records_an_explicit_deny_distinctly() {
        let dir = TempDir::new().unwrap();
        let mut out = Vec::new();
        connect(
            &ConnectArgs {
                id: "gitlab".into(),
                policy: None,
            },
            dir.path(),
            &catalog_at(dir.path()),
            &FakeSignIn::binding(BindOutcome::Completed(
                lns_ipc::CredentialBindDecision::Denied,
            )),
            &mut out,
        )
        .await
        .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("Recorded a deny"), "got: {text}");
    }

    #[tokio::test]
    async fn connect_to_credential_fails_without_recording_when_the_bind_does_not_complete() {
        let dir = TempDir::new().unwrap();
        let err = connect(
            &ConnectArgs {
                id: "gitlab".into(),
                policy: None,
            },
            dir.path(),
            &catalog_at(dir.path()),
            &FakeSignIn::binding(BindOutcome::Failed("the value decision timed out".into())),
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("timed out"), "got: {err:#}");
        let policy = Policy::load_or_default(&dir.path().join("lns-policy.yaml")).unwrap();
        assert!(
            policy.connectors.is_empty(),
            "a failed bind must not record the connector"
        );
    }

    #[tokio::test]
    async fn connect_to_credential_reports_when_the_service_is_unavailable() {
        let dir = TempDir::new().unwrap();
        let err = connect(
            &ConnectArgs {
                id: "gitlab".into(),
                policy: None,
            },
            dir.path(),
            &catalog_at(dir.path()),
            &FakeSignIn::binding(BindOutcome::ServiceUnavailable),
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("service must be running"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn connect_to_oauth_reports_when_the_service_is_unavailable() {
        let dir = TempDir::new().unwrap();
        let catalog = catalog_at(dir.path());
        write_user_catalog(&catalog, vec![oauth_connector("somesaas")]);
        let err = connect(
            &ConnectArgs {
                id: "somesaas".into(),
                policy: None,
            },
            dir.path(),
            &catalog,
            &FakeSignIn::returning(SignInOutcome::ServiceUnavailable),
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("service must be running"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn disconnect_removes_a_connected_connector() {
        let dir = TempDir::new().unwrap();
        let catalog = catalog_at(dir.path());
        connect(
            &ConnectArgs {
                id: "gitlab".into(),
                policy: None,
            },
            dir.path(),
            &catalog,
            &FakeSignIn::completed(),
            &mut Vec::new(),
        )
        .await
        .unwrap();
        disconnect(
            &DisconnectArgs {
                id: "gitlab".into(),
                policy: None,
            },
            dir.path(),
            &mut Vec::new(),
        )
        .unwrap();
        let policy = Policy::load_or_default(&dir.path().join("lns-policy.yaml")).unwrap();
        assert!(policy.connectors.is_empty());
    }

    #[test]
    fn disconnect_errors_when_not_connected() {
        let dir = TempDir::new().unwrap();
        let err = disconnect(
            &DisconnectArgs {
                id: "gitlab".into(),
                policy: None,
            },
            dir.path(),
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("not connected"));
    }

    #[derive(Default)]
    struct FakePublisher {
        pushed: std::cell::RefCell<Vec<String>>,
        fail: Option<String>,
    }
    impl FakePublisher {
        fn ok() -> Self {
            Self::default()
        }
        fn failing(message: &str) -> Self {
            Self {
                fail: Some(message.to_string()),
                ..Default::default()
            }
        }
    }
    impl ConnectorPublisher for FakePublisher {
        fn push<'a>(
            &'a self,
            built: &'a lns_artifact::build::BuiltArtifact,
            reference: &'a str,
        ) -> LocalBoxFuture<'a, Result<()>> {
            self.pushed
                .borrow_mut()
                .push(format!("{reference} {}", built.artifact_type));
            let fail = self.fail.clone();
            Box::pin(async move {
                match fail {
                    Some(message) => Err(anyhow::anyhow!(message)),
                    None => Ok(()),
                }
            })
        }
    }

    const CREDENTIAL_CONNECTOR_YAML: &str = "\
id: some-provider
authKind: credential
routes:
  - match: api.some-provider.example
credential:
  envVar: SOME_TOKEN
  placeholder: lns-placeholder
  injections:
    - kind: bearer_header
      domain: api.some-provider.example
";

    fn write_connector(dir: &Path, body: &str) {
        std::fs::write(dir.join("connector.yaml"), body).unwrap();
    }

    fn publish_args(dry_run: bool) -> ConnectorPublishArgs {
        ConnectorPublishArgs {
            reference: "registry.lns.run/connectors/some-provider:0.1.0".into(),
            file: "connector.yaml".into(),
            dry_run,
        }
    }

    #[tokio::test]
    async fn publish_builds_a_connector_artifact_and_uploads_it() {
        let dir = TempDir::new().unwrap();
        write_connector(dir.path(), CREDENTIAL_CONNECTOR_YAML);
        let publisher = FakePublisher::ok();
        let mut out = Vec::new();
        let code = publish(&publish_args(false), dir.path(), &publisher, &mut out)
            .await
            .unwrap();
        assert_eq!(code, 0);
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains(
                "built and pushed registry.lns.run/connectors/some-provider:0.1.0@sha256:"
            ),
            "got: {text}"
        );
        let pushed = publisher.pushed.borrow();
        assert_eq!(pushed.len(), 1);
        assert!(
            pushed[0].contains("application/vnd.lens.connector.v1+json"),
            "a connector artifact type must be uploaded: {}",
            pushed[0]
        );
    }

    #[tokio::test]
    async fn publish_dry_run_builds_without_uploading() {
        let dir = TempDir::new().unwrap();
        write_connector(dir.path(), CREDENTIAL_CONNECTOR_YAML);
        let publisher = FakePublisher::ok();
        let mut out = Vec::new();
        publish(&publish_args(true), dir.path(), &publisher, &mut out)
            .await
            .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("would push"), "got: {text}");
        assert!(text.contains("nothing uploaded"), "got: {text}");
        assert!(
            publisher.pushed.borrow().is_empty(),
            "a dry run must not upload"
        );
    }

    #[tokio::test]
    async fn publish_refuses_a_connector_carrying_an_embedded_client_secret() {
        let dir = TempDir::new().unwrap();
        write_connector(
            dir.path(),
            "\
id: some-oauth
authKind: oauth
oauth:
  clientId: some-client
  clientSecret: some-secret
  deviceAuthorizationEndpoint: https://api.some-oauth.example/device
  tokenEndpoint: https://api.some-oauth.example/token
  envVar: SOME_OAUTH_TOKEN
  placeholder: lns-placeholder
",
        );
        let publisher = FakePublisher::ok();
        let err = publish(
            &publish_args(false),
            dir.path(),
            &publisher,
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("clientSecret"), "got: {err:#}");
        assert!(
            publisher.pushed.borrow().is_empty(),
            "a refused connector must not be uploaded"
        );
    }

    #[tokio::test]
    async fn publish_rejects_an_unknown_field_in_the_definition() {
        let dir = TempDir::new().unwrap();
        write_connector(
            dir.path(),
            "id: some-provider\nauthKind: credential\nsurprise: true\n",
        );
        let err = publish(
            &publish_args(false),
            dir.path(),
            &FakePublisher::ok(),
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("parsing a connector"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn publish_reports_a_missing_definition_file() {
        let dir = TempDir::new().unwrap();
        let err = publish(
            &publish_args(false),
            dir.path(),
            &FakePublisher::ok(),
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("reading"), "got: {err:#}");
    }

    #[tokio::test]
    async fn publish_surfaces_a_publisher_failure() {
        let dir = TempDir::new().unwrap();
        write_connector(dir.path(), CREDENTIAL_CONNECTOR_YAML);
        let err = publish(
            &publish_args(false),
            dir.path(),
            &FakePublisher::failing("credential for registry.lns.run lacks push scope"),
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("push scope"), "got: {err:#}");
    }

    #[tokio::test]
    async fn run_dispatches_publish() {
        let dir = TempDir::new().unwrap();
        write_connector(dir.path(), CREDENTIAL_CONNECTOR_YAML);
        let publisher = FakePublisher::ok();
        run(
            &ConnectorCommand::Publish(publish_args(false)),
            dir.path(),
            &catalog_at(dir.path()),
            &FakeSignIn::completed(),
            &publisher,
            &mut Vec::new(),
        )
        .await
        .unwrap();
        assert_eq!(publisher.pushed.borrow().len(), 1);
    }

    #[tokio::test]
    async fn run_dispatches_list() {
        let dir = TempDir::new().unwrap();
        let mut out = Vec::new();
        run(
            &ConnectorCommand::List,
            dir.path(),
            &catalog_at(dir.path()),
            &FakeSignIn::completed(),
            &FakePublisher::ok(),
            &mut out,
        )
        .await
        .unwrap();
        assert!(
            String::from_utf8(out)
                .unwrap()
                .contains("gitlab  (bundled)")
        );
    }

    #[tokio::test]
    async fn run_dispatches_add_and_remove() {
        let dir = TempDir::new().unwrap();
        let path = catalog_at(dir.path());
        run(
            &ConnectorCommand::Add(add_args("acme")),
            dir.path(),
            &path,
            &FakeSignIn::completed(),
            &FakePublisher::ok(),
            &mut Vec::new(),
        )
        .await
        .unwrap();
        assert_eq!(load(&path).connectors.len(), 1);
        run(
            &ConnectorCommand::Remove(ConnectorRemoveArgs { id: "acme".into() }),
            dir.path(),
            &path,
            &FakeSignIn::completed(),
            &FakePublisher::ok(),
            &mut Vec::new(),
        )
        .await
        .unwrap();
        assert!(load(&path).connectors.is_empty());
    }

    #[tokio::test]
    async fn run_dispatches_connect_and_disconnect() {
        let dir = TempDir::new().unwrap();
        let path = catalog_at(dir.path());
        run(
            &ConnectorCommand::Connect(ConnectArgs {
                id: "gitlab".into(),
                policy: None,
            }),
            dir.path(),
            &path,
            &FakeSignIn::completed(),
            &FakePublisher::ok(),
            &mut Vec::new(),
        )
        .await
        .unwrap();
        assert_eq!(
            Policy::load_or_default(&dir.path().join("lns-policy.yaml"))
                .unwrap()
                .connectors,
            ["gitlab"]
        );
        run(
            &ConnectorCommand::Disconnect(DisconnectArgs {
                id: "gitlab".into(),
                policy: None,
            }),
            dir.path(),
            &path,
            &FakeSignIn::completed(),
            &FakePublisher::ok(),
            &mut Vec::new(),
        )
        .await
        .unwrap();
        assert!(
            Policy::load_or_default(&dir.path().join("lns-policy.yaml"))
                .unwrap()
                .connectors
                .is_empty()
        );
    }
}
