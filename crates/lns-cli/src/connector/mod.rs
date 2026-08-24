use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use lns_policy::connectors::{
    AuthKind, Catalog, Connector, ConnectorRoute, CredentialAuth, bundled_connectors,
    effective_connectors,
};
use lns_policy::grants::{GrantRecord, GrantStore, GrantVerdict, JsonFileGrantStore, project_key};

use crate::command::{CommandSpec, subcommand};

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
    List(ConnectorListArgs),
    #[command(about = "Remove a user-declared connector; bundled ones cannot be removed.")]
    Remove(ConnectorRemoveArgs),
    #[command(
        about = "Bind a connector's per-machine value decision (oauth connectors sign in); records the id in this directory's policy."
    )]
    Connect(ConnectArgs),
    #[command(
        about = "Disconnect a connector from this directory's policy and forget its per-workload grants here."
    )]
    Disconnect(DisconnectArgs),
    #[command(about = "List the per-workload connector grants remembered for this project.")]
    Grants(GrantsArgs),
    #[command(about = "Forget a connector's per-workload grants in this project.")]
    Revoke(RevokeArgs),
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
        help = "Placeholder value: at least 16 characters and containing \"placeholder\" or \"lns\"; auto-generated when omitted."
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
        help = "Policy file path; defaults to `lns-local-mixin.yaml` in the current directory."
    )]
    pub policy: Option<PathBuf>,
}

#[derive(clap::Args)]
pub struct DisconnectArgs {
    #[arg(help = "Connector id to disconnect.")]
    pub id: String,
    #[arg(
        long,
        help = "Policy file path; defaults to `lns-local-mixin.yaml` in the current directory."
    )]
    pub policy: Option<PathBuf>,
}

#[derive(clap::Args)]
pub struct ConnectorListArgs {
    #[command(flatten)]
    pub output: crate::output::OutputArgs,
}

#[derive(clap::Args)]
pub struct GrantsArgs {
    #[arg(
        long,
        help = "Policy file path whose project the grants are listed for; defaults to `lns-local-mixin.yaml` in the current directory."
    )]
    pub policy: Option<PathBuf>,
    #[arg(
        long,
        help = "List grants for every project on this machine, not just this one."
    )]
    pub all: bool,
    #[command(flatten)]
    pub output: crate::output::OutputArgs,
}

#[derive(clap::Args)]
pub struct RevokeArgs {
    #[arg(help = "Connector id whose per-workload grants to forget in this project.")]
    pub id: String,
    #[arg(
        long,
        help = "Policy file path whose project the grant is revoked from; defaults to `lns-local-mixin.yaml` in the current directory."
    )]
    pub policy: Option<PathBuf>,
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
    owns_terminal: crate::command::never_owns_terminal,
};

pub async fn run(
    cmd: &ConnectorCommand,
    cwd: &Path,
    catalog_path: &Path,
    grants_path: &Path,
    signin: &dyn ConnectorSignIn,
    writer: &mut impl Write,
) -> Result<i32> {
    match cmd {
        ConnectorCommand::Add(args) => add(args, catalog_path, writer),
        ConnectorCommand::List(args) => list(args, catalog_path, writer),
        ConnectorCommand::Remove(args) => remove(args, catalog_path, writer),
        ConnectorCommand::Connect(args) => {
            connect(args, cwd, catalog_path, grants_path, signin, writer).await
        }
        ConnectorCommand::Disconnect(args) => disconnect(args, cwd, grants_path, writer),
        ConnectorCommand::Grants(args) => grants(args, cwd, grants_path, writer),
        ConnectorCommand::Revoke(args) => revoke(args, cwd, grants_path, writer),
    }
}

/// The decisions file a connector verb records itself in: the one `--policy` named, rooted where it was typed, else this directory's own.
fn project_decisions_path(explicit: Option<&Path>, cwd: &Path) -> PathBuf {
    match explicit {
        Some(p) if p.is_absolute() => p.to_path_buf(),
        Some(p) => cwd.join(p),
        None => crate::run::summary::policy_path(cwd),
    }
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
    if !lns_spec::is_legal_connector_id(&args.id) {
        bail!(
            "invalid connector id {:?}: an id is one lowercase DNS label of alphanumerics and '-'",
            args.id
        );
    }
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
        Some(p) => {
            if let Err(problem) = lns_spec::credential::validate_placeholder(p, &args.id) {
                bail!(problem);
            }
            p.clone()
        }
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

fn list(args: &ConnectorListArgs, catalog_path: &Path, writer: &mut impl Write) -> Result<i32> {
    let user = load_catalog(catalog_path)?;
    let mut rows: Vec<ConnectorRow> = bundled_connectors()
        .iter()
        .map(|i| ConnectorRow::new(i, "bundled"))
        .collect();
    // A user id that shadows a bundled one is inert (bundled wins), so don't list it as live.
    rows.extend(
        user.connectors
            .iter()
            .filter(|i| !is_bundled(&i.id))
            .map(|i| ConnectorRow::new(i, "user")),
    );
    crate::output::emit(args.output.format, &rows, writer)?;
    Ok(0)
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ConnectorRow {
    id: String,
    source: &'static str,
    auth_kind: &'static str,
}

impl ConnectorRow {
    fn new(connector: &Connector, source: &'static str) -> Self {
        Self {
            id: connector.id.clone(),
            source,
            auth_kind: kind_word(connector.auth_kind),
        }
    }
}

impl crate::output::TableRow for ConnectorRow {
    const HEADERS: &'static [&'static str] = &["CONNECTOR", "SOURCE", "AUTH"];

    fn cells(&self) -> Vec<String> {
        vec![
            self.id.clone(),
            self.source.to_string(),
            self.auth_kind.to_string(),
        ]
    }
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
    grants_path: &Path,
    signin: &dyn ConnectorSignIn,
    writer: &mut impl Write,
) -> Result<i32> {
    let user = load_catalog(catalog_path)?;
    let effective = effective_connectors(&user);
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
    let path = project_decisions_path(args.policy.as_deref(), cwd);
    connect_project(grants_path, &project_key(&path), &args.id)?;
    writeln!(writer, "{closing}")?;
    // Binding the value cannot lift a workload's decline, so say what will: otherwise the connect reports success and the workload goes on being refused with nothing on screen explaining it.
    match standing_declines(grants_path, &project_key(&path), &args.id) {
        0 => {}
        n => writeln!(
            writer,
            "note: {n} workload(s) in this project declined {:?} and will keep being refused; run `lns connector revoke {}` to forget this project's grants for it.",
            args.id, args.id
        )?,
    }
    Ok(0)
}

/// How many workloads in this project have declined the connector; an unreadable sidecar reports none rather than failing a connect that has already landed.
fn standing_declines(grants_path: &Path, project: &str, connector: &str) -> usize {
    let Ok(file) = JsonFileGrantStore::new(grants_path.to_path_buf()).load() else {
        return 0;
    };
    file.for_project(project)
        .filter(|g| g.connector == connector && g.verdict == GrantVerdict::Deny)
        .count()
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

pub fn disconnect(
    args: &DisconnectArgs,
    cwd: &Path,
    grants_path: &Path,
    writer: &mut impl Write,
) -> Result<i32> {
    let path = project_decisions_path(args.policy.as_deref(), cwd);
    let project = project_key(&path);
    let cleared = disconnect_project(grants_path, &project, &args.id)?
        .ok_or_else(|| anyhow::anyhow!("{:?} is not connected in {project}", args.id))?;
    let grants_note = match cleared {
        0 => String::new(),
        n => format!(" and forgot {n} per-workload grant(s)"),
    };
    writeln!(writer, "Disconnected {:?}{grants_note}", args.id)?;
    Ok(0)
}

/// Record that this project connected a connector, in the one file that also holds what each workload in it may spend.
fn connect_project(grants_path: &Path, project: &str, connector: &str) -> Result<()> {
    JsonFileGrantStore::new(grants_path.to_path_buf())
        .update(&mut |file| file.connect(project, connector))
        .with_context(|| format!("updating grants at {}", grants_path.display()))?;
    Ok(())
}

/// Drop the project's connection and every grant under it in one write, answering with how many grants went; `None` when it was never connected.
fn disconnect_project(grants_path: &Path, project: &str, connector: &str) -> Result<Option<usize>> {
    let store = JsonFileGrantStore::new(grants_path.to_path_buf());
    let mut cleared = None;
    store
        .update(&mut |file| {
            if !file.disconnect(project, connector) {
                return false;
            }
            cleared = Some(file.revoke_project_connector(project, connector));
            true
        })
        .with_context(|| format!("updating grants at {}", grants_path.display()))?;
    Ok(cleared)
}

/// Drop every per-workload grant for one connector in a project from the sidecar, returning how many were removed; the forget is always recorded, even with nothing to remove, because a run still deciding is exactly the one whose grant this has to cancel.
fn clear_project_grants(grants_path: &Path, project: &str, connector: &str) -> Result<usize> {
    let store = JsonFileGrantStore::new(grants_path.to_path_buf());
    let mut cleared = 0;
    store
        .update(&mut |file| {
            cleared = file.revoke_project_connector(project, connector);
            true
        })
        .with_context(|| format!("updating grants at {}", grants_path.display()))?;
    Ok(cleared)
}

fn grants(
    args: &GrantsArgs,
    cwd: &Path,
    grants_path: &Path,
    writer: &mut impl Write,
) -> Result<i32> {
    let store = JsonFileGrantStore::new(grants_path.to_path_buf());
    let file = store
        .load()
        .with_context(|| format!("reading grants from {}", grants_path.display()))?;
    let project = project_key(&project_decisions_path(args.policy.as_deref(), cwd));
    let rows: Vec<&GrantRecord> = if args.all {
        file.grants.iter().collect()
    } else {
        file.for_project(&project).collect()
    };
    if args.output.format == crate::output::Format::Json {
        let json: Vec<GrantRow> = rows.iter().map(|g| GrantRow::new(g)).collect();
        crate::output::emit_object(&json, writer)?;
        return Ok(0);
    }
    if rows.is_empty() {
        let scope = if args.all { "" } else { " for this project" };
        writeln!(writer, "No connector grants{scope}.")?;
        return Ok(0);
    }
    for g in rows {
        let verdict = match g.verdict {
            GrantVerdict::Allow => "allow",
            GrantVerdict::Deny => "deny",
        };
        if args.all {
            writeln!(
                writer,
                "{}\t{}\t{}\t{verdict}",
                g.project, g.workload, g.connector
            )?;
        } else {
            writeln!(writer, "{}\t{}\t{verdict}", g.workload, g.connector)?;
        }
    }
    Ok(0)
}

/// The grants table varies its columns with `--all`, so json carries the full row regardless of scope.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct GrantRow {
    project: String,
    workload: String,
    connector: String,
    verdict: &'static str,
    env_var: String,
}

impl GrantRow {
    fn new(grant: &GrantRecord) -> Self {
        Self {
            project: grant.project.clone(),
            workload: grant.workload.clone(),
            connector: grant.connector.clone(),
            verdict: match grant.verdict {
                GrantVerdict::Allow => "allow",
                GrantVerdict::Deny => "deny",
            },
            env_var: grant.env_var.clone(),
        }
    }
}

fn revoke(
    args: &RevokeArgs,
    cwd: &Path,
    grants_path: &Path,
    writer: &mut impl Write,
) -> Result<i32> {
    let project = project_key(&project_decisions_path(args.policy.as_deref(), cwd));
    let cleared = clear_project_grants(grants_path, &project, &args.id)?;
    if cleared == 0 {
        bail!(
            "no grants for {:?} in this project; a decision a running sandbox was still holding for it is cancelled all the same",
            args.id
        );
    }
    writeln!(
        writer,
        "Revoked {cleared} grant(s) for {:?} in this project.",
        args.id
    )?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lns_policy::connectors::{OauthAuth, OauthFlow};
    use lns_policy::providers::{InjectionDef, InjectionKind};
    use tempfile::TempDir;

    fn connected_at(dir: &Path) -> Vec<String> {
        JsonFileGrantStore::new(dir.join("grants.json"))
            .load()
            .expect("the sidecar reads back")
            .connected_in(&project_key(&dir.join("lns-local-mixin.yaml")))
    }

    #[test]
    fn a_named_decisions_file_roots_where_the_developer_typed_it() {
        // A verb records the connection in the project it names, so the path is theirs and roots at their cwd — the run's own file is found beside a document instead, and never named.
        assert_eq!(
            project_decisions_path(
                Some(Path::new("../team/lns-local-mixin.yaml")),
                Path::new("/w")
            ),
            PathBuf::from("/w/../team/lns-local-mixin.yaml"),
        );
        assert_eq!(
            project_decisions_path(
                Some(Path::new("/team/lns-local-mixin.yaml")),
                Path::new("/w")
            ),
            PathBuf::from("/team/lns-local-mixin.yaml"),
            "an absolute path is already rooted, so the cwd must not be prepended",
        );
    }

    #[test]
    fn standing_declines_reports_none_when_the_sidecar_cannot_be_read() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("grants.json");
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(
            standing_declines(&path, "/proj", "some-provider"),
            0,
            "the bind has already landed by the time the note is composed, so an unreadable sidecar must cost the user a note, not the connect"
        );
    }

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

    fn row_for<'a>(text: &'a str, id: &str) -> &'a str {
        text.lines()
            .find(|l| l.starts_with(id))
            .unwrap_or_else(|| panic!("no listing row for {id} in:\n{text}"))
    }

    fn list_args() -> ConnectorListArgs {
        ConnectorListArgs {
            output: crate::output::OutputArgs {
                format: crate::output::Format::Table,
            },
        }
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
        dir.join("connectors.yaml")
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
        assert_eq!(
            lns_spec::credential::validate_placeholder(&cred.placeholder, "acme"),
            Ok(()),
            "a generated placeholder must satisfy the same rule the catalog enforces on load"
        );
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
    fn add_rejects_what_the_catalog_would_refuse_to_load_again() {
        let dir = TempDir::new().unwrap();
        let path = catalog_at(dir.path());
        let mut short = add_args("acme");
        short.placeholder = Some("lns-short".into());
        let err = add(&short, &path, &mut Vec::new()).unwrap_err();
        assert!(
            format!("{err:#}").contains("at least"),
            "a connector saved here and refused on the next load would break every later command; got: {err:#}"
        );
        let err = add(&add_args("Acme:1"), &path, &mut Vec::new()).unwrap_err();
        assert!(
            format!("{err:#}").contains("invalid connector id"),
            "got: {err:#}"
        );
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
        list(&list_args(), &path, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("CONNECTOR"), "{text}");
        assert!(
            row_for(&text, "gitlab").ends_with("bundled  credential"),
            "{text}"
        );
        assert!(
            row_for(&text, "acme").ends_with("user     credential"),
            "{text}"
        );
    }

    #[test]
    fn list_labels_an_oauth_user_connector() {
        let dir = TempDir::new().unwrap();
        let path = catalog_at(dir.path());
        write_user_catalog(&path, vec![oauth_connector("somesaas")]);
        let mut out = Vec::new();
        list(&list_args(), &path, &mut out).unwrap();
        assert!(
            String::from_utf8(out).unwrap().contains("somesaas"),
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
                    placeholder: "lns-evil-placeholder".into(),
                    injections: Vec::new(),
                }),
                oauth: None,
                token_fallback: None,
            }],
        );
        let mut out = Vec::new();
        list(&list_args(), &path, &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(row_for(&text, "gitlab").contains("bundled"), "{text}");
        assert!(
            text.lines().filter(|l| l.starts_with("gitlab")).count() == 1,
            "a shadow must not be listed: {text}"
        );
    }

    #[test]
    fn list_surfaces_a_malformed_catalog_as_an_error() {
        let dir = TempDir::new().unwrap();
        let path = catalog_at(dir.path());
        std::fs::write(&path, "connectors: not-a-list\n").unwrap();
        let err = list(&list_args(), &path, &mut Vec::new()).unwrap_err();
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
            &dir.path().join("grants.json"),
            &FakeSignIn::completed(),
            &mut out,
        )
        .await
        .unwrap();
        assert_eq!(
            connected_at(dir.path()),
            ["gitlab"],
            "connecting names a directory and no workload, so it records per project beside the per-workload grants"
        );
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
            &dir.path().join("grants.json"),
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
            &dir.path().join("grants.json"),
            &FakeSignIn::completed(),
            &mut Vec::new(),
        )
        .await
        .unwrap();
        assert_eq!(
            connected_at(dir.path()),
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
            &dir.path().join("grants.json"),
            &FakeSignIn::returning(SignInOutcome::Failed("access_denied".into())),
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("access_denied"), "got: {err:#}");
        assert!(
            connected_at(dir.path()).is_empty(),
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
            &dir.path().join("grants.json"),
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
            &dir.path().join("grants.json"),
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
            &dir.path().join("grants.json"),
            &FakeSignIn::binding(BindOutcome::Failed("the value decision timed out".into())),
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("timed out"), "got: {err:#}");
        assert!(
            connected_at(dir.path()).is_empty(),
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
            &dir.path().join("grants.json"),
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
            &dir.path().join("grants.json"),
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
            &dir.path().join("grants.json"),
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
            &dir.path().join("grants.json"),
            &mut Vec::new(),
        )
        .unwrap();
        assert!(connected_at(dir.path()).is_empty());
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
            &dir.path().join("grants.json"),
            &mut Vec::new(),
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("not connected"));
    }

    #[tokio::test]
    async fn run_dispatches_list() {
        let dir = TempDir::new().unwrap();
        let mut out = Vec::new();
        run(
            &ConnectorCommand::List(list_args()),
            dir.path(),
            &catalog_at(dir.path()),
            &dir.path().join("grants.json"),
            &FakeSignIn::completed(),
            &mut out,
        )
        .await
        .unwrap();
        assert!(String::from_utf8(out).unwrap().contains("gitlab"));
    }

    #[tokio::test]
    async fn run_dispatches_add_and_remove() {
        let dir = TempDir::new().unwrap();
        let path = catalog_at(dir.path());
        run(
            &ConnectorCommand::Add(add_args("acme")),
            dir.path(),
            &path,
            &dir.path().join("grants.json"),
            &FakeSignIn::completed(),
            &mut Vec::new(),
        )
        .await
        .unwrap();
        assert_eq!(load(&path).connectors.len(), 1);
        run(
            &ConnectorCommand::Remove(ConnectorRemoveArgs { id: "acme".into() }),
            dir.path(),
            &path,
            &dir.path().join("grants.json"),
            &FakeSignIn::completed(),
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
            &dir.path().join("grants.json"),
            &FakeSignIn::completed(),
            &mut Vec::new(),
        )
        .await
        .unwrap();
        assert_eq!(connected_at(dir.path()), ["gitlab"]);
        run(
            &ConnectorCommand::Disconnect(DisconnectArgs {
                id: "gitlab".into(),
                policy: None,
            }),
            dir.path(),
            &path,
            &dir.path().join("grants.json"),
            &FakeSignIn::completed(),
            &mut Vec::new(),
        )
        .await
        .unwrap();
        assert!(connected_at(dir.path()).is_empty());
    }
}
