use std::io::Write;
use std::path::Path;

use anyhow::{Result, bail};
use lns_ipc::{ConnectorView, Request, Response};

use crate::command::{CommandSpec, subcommand};
use crate::local_future::LocalBoxFuture;
use crate::terminal::Terminal;

mod real;

pub use real::RealConnectorService;

#[derive(clap::Args)]
pub struct ConnectorArgs {
    #[command(subcommand)]
    pub command: ConnectorCommand,
}

#[derive(clap::Subcommand)]
pub enum ConnectorCommand {
    #[command(
        about = "Make a pulled or local connector available on this machine. Installing grants nothing."
    )]
    Install(InstallArgs),
    #[command(
        about = "Remove a connector from this machine, with every connection it held. Grants stay."
    )]
    Uninstall(NameArg),
    #[command(
        about = "List what is installed: what each connector serves, and the connections held."
    )]
    List(ListArgs),
    #[command(
        about = "Sign this machine in to a connector and keep the result as a connection. Connecting is not granting."
    )]
    Connect(ConnectArgs),
    #[command(about = "Drop one connection, or every connection of a connector. Grants stay.")]
    Disconnect(DisconnectArgs),
    #[command(
        about = "Let one run use one of a connector's methods. Discloses what it applies, then asks."
    )]
    Grant(GrantArgs),
    #[command(about = "Clear one run's decision about a connector, so its next start asks again.")]
    Forget(ForgetArgs),
}

#[derive(clap::Args)]
pub struct ConnectArgs {
    #[arg(help = "The connector's name, as `lns connector list` shows it.")]
    pub name: String,
    #[arg(long, help = "Which method to connect. Omitted, you choose.")]
    pub method: Option<String>,
    #[arg(long = "as", help = "What to call the connection this stores.")]
    pub label: Option<String>,
}

#[derive(clap::Args)]
pub struct DisconnectArgs {
    #[arg(help = "The connector's name.")]
    pub name: String,
    #[arg(
        long,
        help = "Which connection to drop. Omitted, every connection is dropped."
    )]
    pub connection: Option<String>,
}

#[derive(clap::Args)]
pub struct GrantArgs {
    #[arg(help = "The connector's name.")]
    pub name: String,
    #[arg(long, help = "Which method to grant. Omitted, you choose.")]
    pub method: Option<String>,
    #[arg(
        long,
        help = "Which connection stands behind it, where the method authenticates."
    )]
    pub connection: Option<String>,
    #[arg(
        long,
        help = "Which run this grants. Its id, its name, or a unique id prefix."
    )]
    pub run: String,
    #[arg(
        short = 'y',
        long = "yes",
        default_value_t = false,
        help = "Answer the disclosure here, instead of at a terminal prompt. It still prints what the grant applies."
    )]
    pub yes: bool,
}

#[derive(clap::Args)]
pub struct ForgetArgs {
    #[arg(help = "The connector's name.")]
    pub name: String,
    #[arg(
        long,
        help = "Which run to clear. Its id, its name, or a unique id prefix."
    )]
    pub run: String,
}

#[derive(clap::Args)]
pub struct InstallArgs {
    #[arg(
        help = "A published connector reference, or a path to a directory or the document itself. A bare reference is qualified by run.registry, else the LNS hub."
    )]
    pub source: String,
}

#[derive(clap::Args)]
pub struct NameArg {
    #[arg(help = "The connector's name, as `lns connector list` shows it.")]
    pub name: String,
}

#[derive(clap::Args)]
pub struct ListArgs {
    #[command(flatten)]
    pub output: crate::output::OutputArgs,
}

pub fn augment(app: clap::Command) -> clap::Command {
    app.subcommand(
        subcommand::<ConnectorArgs>("connector").about(
            "Decide what this machine offers: install a connector, or see what is installed.",
        ),
    )
}

pub const SPEC: CommandSpec = CommandSpec {
    name: "connector",
    augment,
    run: real::run,
    announces_update_check: true,
    owns_terminal: crate::command::never_owns_terminal,
};

/// Sends one connector request to the running service; `None` means the service did not answer.
pub trait ConnectorService {
    fn request(&self, req: Request) -> LocalBoxFuture<'_, Option<Response>>;
}

/// `prompt` is where a question goes, per cli-spec §7.2; it is unbuffered so the question reaches the user before the read blocks.
pub async fn run(
    cmd: &ConnectorCommand,
    svc: &dyn ConnectorService,
    terminal: &mut dyn Terminal,
    cwd: &Path,
    registry: Option<&str>,
    writer: &mut impl Write,
    prompt: &mut impl Write,
) -> Result<i32> {
    match cmd {
        ConnectorCommand::Install(args) => {
            install(svc, &qualified_source(&args.source, registry), cwd, writer).await
        }
        ConnectorCommand::Uninstall(args) => uninstall(svc, &args.name, writer).await,
        ConnectorCommand::List(args) => list(svc, args.output.format, writer).await,
        ConnectorCommand::Connect(args) => connect(svc, args, terminal, writer, prompt).await,
        ConnectorCommand::Disconnect(args) => disconnect(svc, args, writer).await,
        ConnectorCommand::Grant(args) => grant(svc, args, terminal, writer, prompt).await,
        ConnectorCommand::Forget(args) => forget(svc, args, writer).await,
    }
}

/// Whether a run answers to this handle. The service decides it — the CLI holds no registry — so an ambiguous id prefix surfaces as the error it is (cli-spec §2.4).
async fn run_exists(svc: &dyn ConnectorService, run: &str) -> Result<bool> {
    match send(
        svc,
        Request::InspectRun {
            run: run.to_string(),
        },
    )
    .await?
    {
        Response::RunInspect { .. } => Ok(true),
        Response::RunUnknown { .. } => Ok(false),
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

/// The installed connector by name, so a verb reads its methods without guessing what it declares.
async fn installed_view(svc: &dyn ConnectorService, name: &str) -> Result<ConnectorView> {
    match send(svc, Request::ListConnectors).await? {
        Response::ConnectorList { connectors } => connectors
            .into_iter()
            .find(|c| c.name == name)
            .ok_or_else(|| anyhow::anyhow!("no connector named {name} is installed")),
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

/// The only method this version can offer, or a refusal naming every candidate — a verb never guesses between them.
fn single_offerable(
    methods: &[lns_ipc::ConnectorMethodView],
    name: &str,
    verb: &str,
) -> Result<lns_ipc::ConnectorMethodView> {
    let offerable: Vec<&lns_ipc::ConnectorMethodView> =
        methods.iter().filter(|m| m.offerable).collect();
    match offerable.as_slice() {
        [only] => Ok((*only).clone()),
        [] => bail!("{name} declares no method this version of lns can {verb}"),
        many => bail!(
            "{name} declares {} methods this version can {verb} ({}), so name one with --method",
            many.len(),
            many.iter()
                .map(|m| m.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

async fn connect(
    svc: &dyn ConnectorService,
    args: &ConnectArgs,
    terminal: &mut dyn Terminal,
    writer: &mut impl Write,
    prompt: &mut impl Write,
) -> Result<i32> {
    if !terminal.is_available() {
        bail!(
            "connecting {} asks for a secret, and there is no terminal to ask at. No flag answers it: run `lns connector connect {}` from a terminal.",
            args.name,
            args.name
        );
    }
    let (connector, method, asked_for) =
        method_to_connect(svc, &args.name, args.method.as_deref()).await?;
    writeln!(prompt, "connecting {} with {}", args.name, method.label)?;
    let connection = match args.label.clone() {
        Some(named) => named,
        None => confirm_name(&connector, &method, terminal, prompt)?,
    };
    let values = ask_for_each(&method, &asked_for, terminal, prompt)?;
    let req = Request::ConnectConnector {
        name: args.name.clone(),
        method: method.name.clone(),
        connection,
        values: lns_ipc::SecretValues(values),
    };
    match send(svc, req).await? {
        Response::ConnectorConnected {
            name,
            connection,
            invalidated,
        } => {
            writeln!(writer, "connected {name} as {connection}")?;
            report_invalidated(&invalidated, writer)?;
            writeln!(
                writer,
                "  this grants nothing: a run still decides whether to use it"
            )?;
            Ok(0)
        }
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

/// Names a connection: the mechanism suggests the first free name and the user confirms or replaces it (cli-spec §3.3).
fn confirm_name(
    connector: &ConnectorView,
    method: &lns_ipc::ConnectorMethodView,
    terminal: &mut dyn Terminal,
    prompt: &mut impl Write,
) -> Result<String> {
    let suggested = connector.free_connection_name(&method.name);
    write!(prompt, "name this connection [{suggested}]: ")?;
    prompt.flush()?;
    let typed = terminal.read_answer()?;
    let typed = typed.trim();
    Ok(if typed.is_empty() {
        suggested
    } else {
        typed.to_string()
    })
}

/// The connector and the method to connect, refused before a secret is typed when it cannot be connected at all.
async fn method_to_connect(
    svc: &dyn ConnectorService,
    name: &str,
    named: Option<&str>,
) -> Result<(ConnectorView, lns_ipc::ConnectorMethodView, String)> {
    let connector = installed_view(svc, name).await?;
    let method = match named {
        Some(named) => connector
            .methods
            .iter()
            .find(|m| m.name == named)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("{name} declares no method named {named}"))?,
        None => single_offerable(&connector.methods, name, "connect")?,
    };
    if !method.offerable {
        bail!(
            "method {} of {name} {}",
            method.name,
            why_unofferable(&method)
        );
    }
    let Some(asked_for) = method.auth_label.clone() else {
        bail!(
            "method {} of {name} has no authentication, so there is nothing to connect; grant it instead",
            method.name
        );
    };
    if method.asks.is_empty() {
        bail!(
            "method {} declares no credential, so there is no value to ask for",
            method.name
        );
    }
    Ok((connector, method, asked_for))
}

/// Ask once per value the method's `auth` produces, under the key the grant reads it back under, so the value the user types is the value the credential is armed with.
fn ask_for_each(
    method: &lns_ipc::ConnectorMethodView,
    label: &str,
    terminal: &mut dyn Terminal,
    prompt: &mut impl Write,
) -> Result<std::collections::BTreeMap<String, String>> {
    let mut values = std::collections::BTreeMap::new();
    for ask in &method.asks {
        write!(prompt, "{label} (not shown): ")?;
        prompt.flush()?;
        let value = terminal.read_secret()?.trim().to_string();
        writeln!(prompt)?;
        if value.is_empty() {
            bail!("no value was given for {label}, so nothing was connected");
        }
        values.insert(ask.clone(), value);
    }
    Ok(values)
}

/// A re-authentication that reports different authority invalidates the grants naming that connection, so the runs that must decide again are named (§3.2.4).
fn report_invalidated(invalidated: &[String], writer: &mut impl Write) -> std::io::Result<()> {
    if invalidated.is_empty() {
        return Ok(());
    }
    writeln!(
        writer,
        "  these runs must decide again, because this sign-in reported different authority: {}",
        invalidated.join(", ")
    )
}

async fn disconnect(
    svc: &dyn ConnectorService,
    args: &DisconnectArgs,
    writer: &mut impl Write,
) -> Result<i32> {
    let req = Request::DisconnectConnector {
        name: args.name.clone(),
        connection: args.connection.clone(),
    };
    match send(svc, req).await? {
        Response::ConnectorDisconnected { name, dropped: 0 } => {
            writeln!(writer, "{name} holds no connection to disconnect")?;
            Ok(1)
        }
        Response::ConnectorDisconnected { name, dropped } => {
            writeln!(
                writer,
                "disconnected {name}, dropping {}",
                connections(dropped)
            )?;
            writeln!(
                writer,
                "  {name} stays installed, and a run that granted a dropped connection keeps its grant"
            )?;
            Ok(0)
        }
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

async fn grant(
    svc: &dyn ConnectorService,
    args: &GrantArgs,
    terminal: &mut dyn Terminal,
    writer: &mut impl Write,
    prompt: &mut impl Write,
) -> Result<i32> {
    // cli-spec §3.3: a person consents at the card, at a terminal prompt, or with `--yes` on the host, and never a script on their behalf.
    if !args.yes && !terminal.is_available() {
        bail!(
            "granting {name} shows what it opens and asks you to confirm, and there is no terminal to ask at.\n       Run `lns connector grant {name} --run {run}` from a terminal, or pass --yes to answer here.",
            name = args.name,
            run = args.run
        );
    }
    let connector = installed_view(svc, &args.name).await?;
    let method = match args.method.as_deref() {
        Some(named) => connector
            .methods
            .iter()
            .find(|m| m.name == named)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("{} declares no method named {named}", args.name))?,
        None => single_offerable(&connector.methods, &args.name, "grant")?,
    };
    if !method.offerable {
        bail!(
            "method {} of {} {}",
            method.name,
            args.name,
            why_unofferable(&method)
        );
    }
    disclose(&connector, &method, &args.run, prompt)?;
    // The same predicate the service applies, so the disclosure cannot promise a reservation the write refuses.
    if !run_exists(svc, &args.run).await? && lns_ipc::validate_run_name(&args.run).is_ok() {
        writeln!(
            prompt,
            "  no run is named {run}. This reserves the decision for the run you next create with that name.",
            run = args.run
        )?;
    }
    if !args.yes {
        write!(prompt, "grant it? [y/N] ")?;
        prompt.flush()?;
        let answer = terminal.read_answer()?;
        writeln!(prompt)?;
        if !crate::terminal::is_affirmative(&answer) {
            writeln!(writer, "nothing was granted")?;
            return Ok(1);
        }
    }
    let req = Request::GrantConnector {
        name: args.name.clone(),
        run: args.run.clone(),
        method: method.name.clone(),
        connection: args.connection.clone(),
        answered_by: match args.yes {
            true => lns_ipc::AnswerSource::Flag,
            false => lns_ipc::AnswerSource::Terminal,
        },
    };
    match send(svc, req).await? {
        Response::ConnectorGranted {
            name,
            method,
            unchanged: true,
            ..
        } => {
            writeln!(
                writer,
                "{run} already grants {name} with {method}",
                run = args.run
            )?;
            Ok(1)
        }
        Response::ConnectorGranted {
            name,
            method,
            connection,
            displaced,
            reserved,
            ..
        } => {
            // The service decides this, not the probe above: a run may be created between the two.
            let verb = if reserved { "reserved" } else { "granted" };
            let run = &args.run;
            match connection {
                Some(connection) => writeln!(
                    writer,
                    "{verb} {name} with {method} as {connection} for {run}"
                )?,
                None => writeln!(writer, "{verb} {name} with {method} for {run}")?,
            }
            if let Some(displaced) = displaced {
                writeln!(
                    writer,
                    "  replaced {displaced}, which {run} no longer applies"
                )?;
            }
            Ok(0)
        }
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

/// What the card would show: the whole payload a grant applies, so consent is never given to something nobody saw (sandbox-spec §1.5).
fn disclose(
    connector: &ConnectorView,
    method: &lns_ipc::ConnectorMethodView,
    run: &str,
    prompt: &mut impl Write,
) -> Result<()> {
    writeln!(
        prompt,
        "granting {} to {run} would give it:",
        connector.name
    )?;
    labelled(prompt, "method", &method.label)?;
    disclose_list(prompt, "opens", &method.opens)?;
    disclose_list(prompt, "writes", &method.writes)?;
    disclose_list(prompt, "sets", &method.sets())?;
    // What is printed is what is granted, so a connection made for another method is not part of this disclosure.
    for connection in connector
        .connections
        .iter()
        .filter(|held| held.method == method.name)
    {
        let authority = if connection.authority.is_empty() {
            "no authority reported".to_string()
        } else {
            connection.authority.join(", ")
        };
        labelled(
            prompt,
            "connection",
            &format!("{} ({authority})", connection.label),
        )?;
    }
    Ok(())
}

/// Prints "nothing" rather than an empty line, so a payload the method does not carry is stated instead of looking omitted.
fn disclose_list(prompt: &mut impl Write, noun: &str, items: &[String]) -> std::io::Result<()> {
    let rendered = if items.is_empty() {
        "nothing".to_string()
    } else {
        items.join(", ")
    };
    labelled(prompt, noun, &rendered)
}

/// One labelled line, in the one column width the whole `lns connector` family lines up on.
fn labelled(writer: &mut impl Write, noun: &str, value: &str) -> std::io::Result<()> {
    writeln!(writer, "  {noun:8} {value}")
}

async fn forget(
    svc: &dyn ConnectorService,
    args: &ForgetArgs,
    writer: &mut impl Write,
) -> Result<i32> {
    let req = Request::ForgetConnector {
        name: args.name.clone(),
        run: args.run.clone(),
    };
    match send(svc, req).await? {
        Response::ConnectorForgotten {
            name,
            had_decision: false,
            ..
        } => {
            writeln!(
                writer,
                "{run} has decided nothing about {name}",
                run = args.run
            )?;
            Ok(1)
        }
        Response::ConnectorForgotten { name, reserved, .. } => {
            let what = if reserved { "reservation" } else { "decision" };
            writeln!(
                writer,
                "cleared {run}'s {what} about {name}",
                run = args.run
            )?;
            Ok(0)
        }
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

async fn send(svc: &dyn ConnectorService, req: Request) -> Result<Response> {
    let response = svc
        .request(req)
        .await
        .ok_or_else(|| anyhow::anyhow!("no response from lns-service (is it running?)"))?;
    if let Response::Error { message } = response {
        bail!("{message}");
    }
    Ok(response)
}

/// `1 connection` rather than `1 connection(s)`, because a count the user reads out loud should be a sentence.
fn connections(count: usize) -> String {
    match count {
        1 => "1 connection".to_string(),
        n => format!("{n} connections"),
    }
}

/// A local path names this machine, so it gains no registry; anything else is a reference and is qualified.
fn qualified_source(source: &str, registry: Option<&str>) -> String {
    if lns_artifact::sandbox::names_a_local_path(source) {
        return source.to_string();
    }
    crate::config::resolve_default_registry(source, registry)
}

async fn install(
    svc: &dyn ConnectorService,
    source: &str,
    cwd: &Path,
    writer: &mut impl Write,
) -> Result<i32> {
    let req = Request::InstallConnector {
        source: crate::run::target::root_named_directory(source, cwd, "connector directory")?,
    };
    match send(svc, req).await? {
        Response::ConnectorInstalled { connector } => {
            report_installed(&connector, writer)?;
            Ok(0)
        }
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

/// Says what installing did and did not do, because §7.1's "installing grants nothing" is the thing a user is most likely to assume otherwise.
fn report_installed(connector: &ConnectorView, writer: &mut impl Write) -> std::io::Result<()> {
    writeln!(
        writer,
        "installed {} ({})",
        connector.name, connector.digest
    )?;
    labelled(writer, "serves", &connector.serves.join(", "))?;
    for method in &connector.methods {
        labelled(writer, "method", &method_summary(method))?;
    }
    writeln!(
        writer,
        "nothing is granted yet; a run that reaches {} asks whether to use it",
        connector.serves.join(" or ")
    )
}

/// What a method needs before a run can use it, so both `install` and `list` answer "what do I do next" rather than only naming it.
fn method_summary(method: &lns_ipc::ConnectorMethodView) -> String {
    let state = match (method.offerable, method.auth_label.is_some()) {
        (false, _) => why_unofferable(method),
        (true, true) => "connect first",
        (true, false) => "ready to grant",
    };
    format!("{} — {state}", method.label)
}

/// Why this version will not apply a method. A fileset is a mechanism lns does not deliver yet, not an old build — telling that user to update would send them nowhere.
fn why_unofferable(method: &lns_ipc::ConnectorMethodView) -> &'static str {
    if method.writes.is_empty() {
        "needs a newer lns"
    } else {
        "writes files, which this lns cannot deliver yet"
    }
}

async fn uninstall(svc: &dyn ConnectorService, name: &str, writer: &mut impl Write) -> Result<i32> {
    let req = Request::UninstallConnector {
        name: name.to_string(),
    };
    match send(svc, req).await? {
        Response::ConnectorUninstalled {
            name,
            dropped_connections,
        } => {
            writeln!(writer, "uninstalled {name}")?;
            if dropped_connections > 0 {
                writeln!(writer, "  dropped {}", connections(dropped_connections))?;
            }
            writeln!(
                writer,
                "  a run that granted it keeps that decision, and reinstalling the same bytes resumes it"
            )?;
            Ok(0)
        }
        Response::ConnectorUnknown { name } => {
            writeln!(writer, "no connector named {name} is installed")?;
            Ok(1)
        }
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

async fn list(
    svc: &dyn ConnectorService,
    format: crate::output::Format,
    writer: &mut impl Write,
) -> Result<i32> {
    match send(svc, Request::ListConnectors).await? {
        Response::ConnectorList { connectors } => {
            let rows: Vec<ConnectorRow> = connectors.iter().map(ConnectorRow::new).collect();
            crate::output::emit(format, &rows, "No connectors installed.", writer)?;
            Ok(0)
        }
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ConnectorRow {
    name: String,
    digest: String,
    serves: Vec<String>,
    /// Bare labels, because `--format json` is read by programs: the table decorates them, the field does not.
    methods: Vec<String>,
    #[serde(skip)]
    method_summaries: Vec<String>,
    connections: Vec<String>,
}

impl ConnectorRow {
    fn new(connector: &ConnectorView) -> Self {
        Self {
            name: connector.name.clone(),
            digest: connector.digest.clone(),
            serves: connector.serves.clone(),
            methods: connector
                .methods
                .iter()
                .map(|method| method.label.clone())
                .collect(),
            // The same three states `install` reports, so a method reads the same whichever verb shows it.
            method_summaries: connector.methods.iter().map(method_summary).collect(),
            connections: connector
                .connections
                .iter()
                .map(|held| held.label.clone())
                .collect(),
        }
    }
}

impl crate::output::TableRow for ConnectorRow {
    const HEADERS: &'static [&'static str] = &["NAME", "SERVES", "METHODS", "CONNECTIONS"];

    fn cells(&self) -> Vec<String> {
        vec![
            self.name.clone(),
            self.serves.join(", "),
            self.method_summaries.join(", "),
            none_when_empty(&self.connections),
        ]
    }
}

/// A connector with no connection is the normal state after an install, so the column says so rather than sitting blank.
fn none_when_empty(connections: &[String]) -> String {
    if connections.is_empty() {
        return "none".to_string();
    }
    connections.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    /// Answers with whatever the test queued, so the arms that reject a wrong answer are reachable.
    struct CannedService {
        responses: Mutex<VecDeque<Option<Response>>>,
        sent: Mutex<Vec<Request>>,
        /// Answered outside the queue, so a `--run` probe never shifts the connector responses a test cans.
        run_is_unknown: bool,
        /// Overrides both, for the answers a probe should refuse rather than read.
        run_probe: Option<Response>,
    }

    impl CannedService {
        fn with(responses: impl IntoIterator<Item = Option<Response>>) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().collect()),
                sent: Mutex::new(Vec::new()),
                run_is_unknown: false,
                run_probe: None,
            }
        }

        /// A probe answered with something that is neither `RunInspect` nor `RunUnknown`.
        fn probing_with(
            answer: Response,
            responses: impl IntoIterator<Item = Option<Response>>,
        ) -> Self {
            Self {
                run_probe: Some(answer),
                ..Self::with(responses)
            }
        }

        /// The same script, against a handle no run answers to.
        fn reserving(responses: impl IntoIterator<Item = Option<Response>>) -> Self {
            Self {
                run_is_unknown: true,
                ..Self::with(responses)
            }
        }

        fn sent(&self) -> Vec<Request> {
            self.sent.lock().unwrap().clone()
        }
    }

    impl ConnectorService for CannedService {
        fn request(&self, req: Request) -> LocalBoxFuture<'_, Option<Response>> {
            self.sent.lock().unwrap().push(req.clone());
            if let Request::InspectRun { run } = req {
                if let Some(answer) = self.run_probe.clone() {
                    return Box::pin(async move { Some(answer) });
                }
                let resp = if self.run_is_unknown {
                    Response::RunUnknown { run }
                } else {
                    Response::RunInspect {
                        details: Box::new(lns_ipc::RunDetails {
                            summary: lns_ipc::RunSummary {
                                id: "1a2b3c4d0000000000000000000000aa".into(),
                                name: run,
                                image: "someimage".into(),
                                command: "sh".into(),
                                status: lns_ipc::RunStatus::Running,
                                created: "2026-08-31T00:00:00Z".into(),
                                started: "2026-08-31T00:00:00Z".into(),
                            },
                            config: lns_ipc::RunConfig::default(),
                        }),
                    }
                };
                return Box::pin(async move { Some(resp) });
            }
            let resp = self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("CannedService: no canned response left");
            Box::pin(async move { resp })
        }
    }

    fn cwd() -> std::path::PathBuf {
        std::path::PathBuf::from("/work")
    }

    #[tokio::test]
    async fn a_path_that_is_not_utf8_is_refused_rather_than_mangled() {
        // The absolutised path is the operand the service opens, so a lossy conversion would send it a path naming a directory nobody has.
        use std::os::unix::ffi::OsStrExt;
        let odd = std::path::PathBuf::from(std::ffi::OsStr::from_bytes(b"/work/\xff"));
        let svc = CannedService::with([]);
        let mut out = Vec::new();
        let err = run(
            &ConnectorCommand::Install(InstallArgs {
                source: "./some-provider".into(),
            }),
            &svc,
            &mut crate::terminal::NoTerminal,
            &odd,
            None,
            &mut out,
            &mut Vec::new(),
        )
        .await
        .expect_err("a non-utf8 path must be refused before anything is sent");
        assert!(format!("{err:#}").contains("utf-8"), "got: {err:#}");
    }

    /// A verb that answers with another verb's response is a protocol mismatch, not a refusal, so each verb says so rather than treating it as success.
    #[tokio::test]
    async fn every_verb_refuses_a_response_meant_for_another() {
        let cases: Vec<(ConnectorCommand, Response)> = vec![
            (
                ConnectorCommand::Install(InstallArgs {
                    source: "ghcr.io/acme/c:1".into(),
                }),
                Response::Pong,
            ),
            (
                ConnectorCommand::Uninstall(NameArg {
                    name: "some-provider".into(),
                }),
                Response::Pong,
            ),
            (
                ConnectorCommand::List(ListArgs {
                    output: crate::output::OutputArgs {
                        format: crate::output::Format::Table,
                    },
                }),
                Response::Pong,
            ),
        ];
        for (cmd, wrong) in cases {
            let svc = CannedService::with([Some(wrong)]);
            let mut out = Vec::new();
            let err = run(
                &cmd,
                &svc,
                &mut crate::terminal::NoTerminal,
                &cwd(),
                None,
                &mut out,
                &mut Vec::new(),
            )
            .await
            .expect_err("a mismatched response must not read as success");
            assert!(
                format!("{err:#}").contains("unexpected response"),
                "got: {err:#}"
            );
        }
    }

    #[tokio::test]
    async fn an_install_naming_a_method_this_version_cannot_offer_says_so() {
        // §3.2.2: an unknown auth.kind parses and leaves the method unofferable, so the card cannot offer it and the install has to explain the gap.
        let connector = ConnectorView {
            name: "some-provider".into(),
            digest: "sha256:abc".into(),
            serves: vec!["api.some-provider.example".into()],
            methods: vec![lns_ipc::ConnectorMethodView {
                name: "future".into(),
                label: "Future sign-in".into(),
                auth_label: Some("token".to_string()),
                offerable: false,
                opens: Vec::new(),
                writes: Vec::new(),
                env: Vec::new(),
                help: None,
                overrides: None,
                credentials: Vec::new(),
                asks: Vec::new(),
            }],
            connections: Vec::new(),
        };
        let svc = CannedService::with([Some(Response::ConnectorInstalled { connector })]);
        let mut out = Vec::new();
        let code = run(
            &ConnectorCommand::Install(InstallArgs {
                source: "ghcr.io/acme/c:1".into(),
            }),
            &svc,
            &mut crate::terminal::NoTerminal,
            &cwd(),
            None,
            &mut out,
            &mut Vec::new(),
        )
        .await
        .expect("the install itself succeeds");
        assert_eq!(code, 0);
        let text = String::from_utf8(out).expect("utf-8");
        assert!(
            text.contains("needs a newer lns"),
            "this one declares no fileset, so the version is the only thing in its way: {text}"
        );
    }

    #[test]
    fn the_connections_column_lists_what_the_machine_holds() {
        let row = ConnectorRow::new(&ConnectorView {
            name: "some-provider".into(),
            digest: "sha256:abc".into(),
            serves: vec!["api.some-provider.example".into()],
            methods: Vec::new(),
            connections: vec![
                lns_ipc::ConnectorConnectionView {
                    label: "work".into(),
                    method: "token".into(),
                    authority: Vec::new(),
                },
                lns_ipc::ConnectorConnectionView {
                    label: "personal".into(),
                    method: "token".into(),
                    authority: Vec::new(),
                },
            ],
        });
        use crate::output::TableRow;
        assert_eq!(row.cells()[3], "work, personal");
    }
    fn asking(answers: &[&str]) -> crate::terminal::ScriptedTerminal {
        crate::terminal::ScriptedTerminal::answering(answers)
    }

    fn listing(connectors: Vec<ConnectorView>) -> Response {
        Response::ConnectorList { connectors }
    }

    /// Drives one verb and returns everything the user saw — the prompt sink is stderr in production, and the disclosure lands there.
    async fn drive(
        cmd: ConnectorCommand,
        svc: &CannedService,
        answers: &[&str],
        cwd: &std::path::Path,
    ) -> Result<(i32, String)> {
        let mut out = Vec::new();
        let mut prompt = Vec::new();
        let code = run(
            &cmd,
            svc,
            &mut asking(answers),
            cwd,
            None,
            &mut out,
            &mut prompt,
        )
        .await?;
        let seen =
            String::from_utf8(prompt).expect("utf-8") + &String::from_utf8(out).expect("utf-8");
        Ok((code, seen))
    }

    fn with_methods(methods: Vec<lns_ipc::ConnectorMethodView>) -> ConnectorView {
        ConnectorView {
            name: "some-provider".into(),
            digest: "sha256:abc".into(),
            serves: vec!["api.some-provider.example".into()],
            methods,
            connections: Vec::new(),
        }
    }

    fn method(name: &str, offerable: bool) -> lns_ipc::ConnectorMethodView {
        lns_ipc::ConnectorMethodView {
            name: name.into(),
            label: name.into(),
            auth_label: Some("token".to_string()),
            offerable,
            opens: vec!["other.example".into()],
            writes: vec!["/home/agent/.netrc".into()],
            env: vec!["SOME_REGION".into()],
            help: None,
            overrides: None,
            credentials: vec!["SOME_TOKEN".into()],
            asks: vec!["token".into()],
        }
    }

    #[tokio::test]
    async fn connecting_with_one_offerable_method_needs_no_flag() {
        let svc = CannedService::with([
            Some(listing(vec![with_methods(vec![method("token", true)])])),
            Some(Response::ConnectorConnected {
                name: "some-provider".into(),
                connection: "token".into(),
                invalidated: Vec::new(),
            }),
        ]);
        let mut out = Vec::new();
        let code = run(
            &ConnectorCommand::Connect(ConnectArgs {
                name: "some-provider".into(),
                method: None,
                label: None,
            }),
            &svc,
            &mut asking(&["", "sk-live"]),
            &cwd(),
            None,
            &mut out,
            &mut Vec::new(),
        )
        .await
        .expect("one offerable method is unambiguous");
        assert_eq!(code, 0);
    }

    #[tokio::test]
    async fn connecting_with_several_offerable_methods_asks_for_the_flag() {
        let svc = CannedService::with([Some(listing(vec![with_methods(vec![
            method("token", true),
            method("session", true),
        ])]))]);
        let mut out = Vec::new();
        let err = run(
            &ConnectorCommand::Connect(ConnectArgs {
                name: "some-provider".into(),
                method: None,
                label: None,
            }),
            &svc,
            &mut asking(&["sk-live"]),
            &cwd(),
            None,
            &mut out,
            &mut Vec::new(),
        )
        .await
        .expect_err("two offerable methods must not be guessed between");
        let text = format!("{err:#}");
        assert!(text.contains("--method"), "{text}");
        assert!(
            text.contains("token, session"),
            "the choice has to be named: {text}"
        );
    }

    #[tokio::test]
    async fn connecting_a_connector_whose_methods_this_version_cannot_offer_is_refused() {
        let svc = CannedService::with([Some(listing(vec![with_methods(vec![method(
            "future", false,
        )])]))]);
        let mut out = Vec::new();
        let err = run(
            &ConnectorCommand::Connect(ConnectArgs {
                name: "some-provider".into(),
                method: None,
                label: None,
            }),
            &svc,
            &mut asking(&["sk-live"]),
            &cwd(),
            None,
            &mut out,
            &mut Vec::new(),
        )
        .await
        .expect_err("an unofferable method cannot be connected");
        assert!(format!("{err:#}").contains("no method"), "{err:#}");
    }

    #[tokio::test]
    async fn choosing_a_method_for_a_connector_that_is_not_installed_says_so() {
        let svc = CannedService::with([Some(listing(Vec::new()))]);
        let mut out = Vec::new();
        let err = run(
            &ConnectorCommand::Connect(ConnectArgs {
                name: "absent".into(),
                method: None,
                label: None,
            }),
            &svc,
            &mut asking(&["sk-live"]),
            &cwd(),
            None,
            &mut out,
            &mut Vec::new(),
        )
        .await
        .expect_err("nothing to choose from");
        assert!(format!("{err:#}").contains("absent"), "{err:#}");
    }

    #[tokio::test]
    async fn a_reauthentication_that_invalidates_grants_names_the_runs_that_must_decide_again() {
        // §3.2.4: where a re-authentication reports different authority, every grant naming that connection is invalidated. The service sends run names, never store keys.
        let svc = CannedService::with([
            Some(listing(vec![with_methods(vec![method("token", true)])])),
            Some(Response::ConnectorConnected {
                name: "some-provider".into(),
                connection: "work".into(),
                invalidated: vec!["reviewer".into(), "billing".into()],
            }),
        ]);
        let (_, seen) = drive(
            ConnectorCommand::Connect(ConnectArgs {
                name: "some-provider".into(),
                method: Some("token".into()),
                label: Some("work".into()),
            }),
            &svc,
            &["sk-live"],
            &cwd(),
        )
        .await
        .expect("connect");
        assert!(
            seen.contains("reviewer, billing"),
            "the runs are named: {seen}"
        );
    }

    #[tokio::test]
    async fn the_disclosure_names_a_connections_authority_and_says_when_it_reported_none() {
        let held = |authority: Vec<String>| ConnectorView {
            name: "some-provider".into(),
            digest: "sha256:abc".into(),
            serves: vec!["api.some-provider.example".into()],
            methods: vec![method("token", true)],
            connections: vec![lns_ipc::ConnectorConnectionView {
                label: "work".into(),
                method: "token".into(),
                authority,
            }],
        };
        for (authority, expected) in [
            (Vec::new(), "no authority reported"),
            (vec!["repo:read".to_string()], "repo:read"),
        ] {
            let svc = CannedService::with([
                Some(listing(vec![held(authority)])),
                Some(Response::ConnectorGranted {
                    name: "some-provider".into(),
                    method: "token".into(),
                    connection: Some("work".into()),
                    displaced: None,
                    unchanged: false,
                    reserved: false,
                }),
            ]);
            let (_, seen) = drive(
                ConnectorCommand::Grant(GrantArgs {
                    name: "some-provider".into(),
                    method: Some("token".into()),
                    connection: None,
                    run: "reviewer".into(),
                    yes: false,
                }),
                &svc,
                &["y"],
                &cwd(),
            )
            .await
            .expect("grant");
            assert!(seen.contains(expected), "want {expected:?}, got: {seen}");
            assert!(
                seen.contains("as work"),
                "the connection behind it is named: {seen}"
            );
        }
    }

    #[tokio::test]
    async fn an_id_that_resolves_to_nothing_is_never_offered_as_a_reservation() {
        // §2.4: only a name can name a run that does not exist yet, and the disclosure must not promise what the write will refuse.
        let svc = CannedService::reserving([Some(listing(vec![with_methods(vec![method(
            "token", true,
        )])]))]);
        let (_, seen) = drive(
            ConnectorCommand::Grant(GrantArgs {
                name: "some-provider".into(),
                method: Some("token".into()),
                connection: None,
                run: "1a2b".into(),
                yes: false,
            }),
            &svc,
            &["n"],
            &cwd(),
        )
        .await
        .expect("grant");
        assert!(
            !seen.contains("reserves the decision"),
            "an id prefix is an error at the service, so nothing may offer to reserve it: {seen}"
        );
    }

    #[tokio::test]
    async fn a_probe_answered_with_neither_answer_is_refused_rather_than_read() {
        let svc = CannedService::probing_with(
            Response::Acknowledged,
            [Some(listing(vec![with_methods(vec![method(
                "token", true,
            )])]))],
        );
        let err = run(
            &ConnectorCommand::Grant(GrantArgs {
                name: "some-provider".into(),
                method: Some("token".into()),
                connection: None,
                run: "reviewer".into(),
                yes: false,
            }),
            &svc,
            &mut asking(&["y"]),
            &cwd(),
            None,
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .await
        .expect_err("a probe answer this build does not understand");
        assert!(
            format!("{err:#}").contains("unexpected response"),
            "{err:#}"
        );
    }

    #[tokio::test]
    async fn the_run_is_probed_before_the_user_consents_never_after() {
        // cli-spec §3.3: the disclosure is what the user answers, so "this reserves the decision" cannot arrive after the y/N.
        let svc = CannedService::reserving([
            Some(listing(vec![with_methods(vec![method("token", true)])])),
            Some(Response::ConnectorGranted {
                name: "some-provider".into(),
                method: "token".into(),
                connection: None,
                displaced: None,
                unchanged: false,
                reserved: true,
            }),
        ]);
        let (_, seen) = drive(
            ConnectorCommand::Grant(GrantArgs {
                name: "some-provider".into(),
                method: Some("token".into()),
                connection: None,
                run: "revieweer".into(),
                yes: false,
            }),
            &svc,
            &["y"],
            &cwd(),
        )
        .await
        .expect("grant");
        assert!(
            seen.contains("no run is named revieweer"),
            "the reservation is disclosed: {seen}"
        );
        let probe = svc
            .sent()
            .iter()
            .position(|req| matches!(req, Request::InspectRun { .. }))
            .expect("the run is probed");
        let grant = svc
            .sent()
            .iter()
            .position(|req| matches!(req, Request::GrantConnector { .. }))
            .expect("the grant is sent");
        assert!(probe < grant, "sent: {:?}", svc.sent());
    }

    #[tokio::test]
    async fn granting_names_the_run_it_grants() {
        let svc = CannedService::with([
            Some(listing(vec![with_methods(vec![method("token", true)])])),
            Some(Response::ConnectorGranted {
                name: "some-provider".into(),
                method: "token".into(),
                connection: None,
                displaced: None,
                unchanged: false,
                reserved: false,
            }),
        ]);
        let (_, seen) = drive(
            ConnectorCommand::Grant(GrantArgs {
                name: "some-provider".into(),
                method: Some("token".into()),
                connection: None,
                run: "reviewer".into(),
                yes: false,
            }),
            &svc,
            &["y"],
            &cwd(),
        )
        .await
        .expect("grant");
        assert!(
            seen.contains("granting some-provider to reviewer"),
            "the disclosure names the run being granted: {seen}"
        );
    }

    #[tokio::test]
    async fn every_run_scoped_verb_refuses_a_response_meant_for_another() {
        let cases: Vec<ConnectorCommand> = vec![
            ConnectorCommand::Connect(ConnectArgs {
                name: "some-provider".into(),
                method: Some("token".into()),
                label: None,
            }),
            ConnectorCommand::Disconnect(DisconnectArgs {
                name: "some-provider".into(),
                connection: None,
            }),
            ConnectorCommand::Forget(ForgetArgs {
                name: "some-provider".into(),
                run: "reviewer".into(),
            }),
        ];
        for cmd in cases {
            let svc = CannedService::with([Some(Response::Pong)]);
            let mut out = Vec::new();
            let err = run(
                &cmd,
                &svc,
                &mut asking(&["y"]),
                &cwd(),
                None,
                &mut out,
                &mut Vec::new(),
            )
            .await
            .expect_err("a mismatched response must not read as success");
            assert!(
                format!("{err:#}").contains("unexpected response"),
                "got: {err:#}"
            );
        }
    }

    #[tokio::test]
    async fn a_grant_whose_disclosure_answers_with_the_wrong_shape_is_refused() {
        for wrong in [
            Response::Pong,
            Response::ConnectorUnknown { name: "x".into() },
        ] {
            let svc = CannedService::with([Some(wrong)]);
            let mut out = Vec::new();
            let err = run(
                &ConnectorCommand::Grant(GrantArgs {
                    name: "some-provider".into(),
                    method: Some("token".into()),
                    connection: None,
                    run: "reviewer".into(),
                    yes: false,
                }),
                &svc,
                &mut asking(&["y"]),
                &cwd(),
                None,
                &mut out,
                &mut Vec::new(),
            )
            .await
            .expect_err("the disclosure must not proceed on a wrong answer");
            assert!(
                format!("{err:#}").contains("unexpected response"),
                "{err:#}"
            );
        }
    }
    #[tokio::test]
    async fn choosing_a_method_refuses_a_response_that_is_not_a_listing() {
        let svc = CannedService::with([Some(Response::Pong)]);
        let mut out = Vec::new();
        let err = run(
            &ConnectorCommand::Connect(ConnectArgs {
                name: "some-provider".into(),
                method: None,
                label: None,
            }),
            &svc,
            &mut asking(&["sk-live"]),
            &cwd(),
            None,
            &mut out,
            &mut Vec::new(),
        )
        .await
        .expect_err("a method cannot be chosen from a response that lists nothing");
        assert!(
            format!("{err:#}").contains("unexpected response"),
            "{err:#}"
        );
    }

    #[tokio::test]
    async fn a_grant_refuses_a_response_meant_for_another_verb() {
        // The disclosure answered correctly, so this pins the grant's own answer rather than the listing's.
        let svc = CannedService::with([
            Some(listing(vec![with_methods(vec![method("token", true)])])),
            Some(Response::Pong),
        ]);
        let mut out = Vec::new();
        let err = run(
            &ConnectorCommand::Grant(GrantArgs {
                name: "some-provider".into(),
                method: Some("token".into()),
                connection: None,
                run: "reviewer".into(),
                yes: false,
            }),
            &svc,
            &mut asking(&["y"]),
            &cwd(),
            None,
            &mut out,
            &mut Vec::new(),
        )
        .await
        .expect_err("a mismatched grant answer must not read as consent recorded");
        assert!(
            format!("{err:#}").contains("unexpected response"),
            "{err:#}"
        );
    }
    #[tokio::test]
    async fn connecting_refuses_a_response_meant_for_another_verb() {
        // The listing answered correctly, so this pins the connect's own answer rather than the listing's.
        let svc = CannedService::with([
            Some(listing(vec![with_methods(vec![method("token", true)])])),
            Some(Response::Pong),
        ]);
        let err = drive(
            ConnectorCommand::Connect(ConnectArgs {
                name: "some-provider".into(),
                method: Some("token".into()),
                label: None,
            }),
            &svc,
            &["", "sk-live"],
            &cwd(),
        )
        .await
        .expect_err("a mismatched answer must not read as connected");
        assert!(
            format!("{err:#}").contains("unexpected response"),
            "{err:#}"
        );
    }

    #[tokio::test]
    async fn a_verb_that_needs_the_listing_refuses_a_response_that_is_not_one() {
        let svc = CannedService::with([Some(Response::Pong)]);
        let err = drive(
            ConnectorCommand::Grant(GrantArgs {
                name: "some-provider".into(),
                method: Some("token".into()),
                connection: None,
                run: "reviewer".into(),
                yes: false,
            }),
            &svc,
            &["y"],
            &cwd(),
        )
        .await
        .expect_err("a connector cannot be read from a response that lists nothing");
        assert!(
            format!("{err:#}").contains("unexpected response"),
            "{err:#}"
        );
    }

    #[tokio::test]
    async fn a_named_method_this_version_cannot_offer_is_refused_before_a_secret_is_typed() {
        // §3.2.2: an unknown auth.kind leaves the method unofferable, and no value should be asked for one that cannot be used.
        for cmd in [
            ConnectorCommand::Connect(ConnectArgs {
                name: "some-provider".into(),
                method: Some("future".into()),
                label: None,
            }),
            ConnectorCommand::Grant(GrantArgs {
                name: "some-provider".into(),
                method: Some("future".into()),
                connection: None,
                run: "reviewer".into(),
                yes: false,
            }),
        ] {
            // No fileset on this one, so the only thing this version cannot honour is its auth kind.
            let unknown_auth = lns_ipc::ConnectorMethodView {
                writes: Vec::new(),
                ..method("future", false)
            };
            let svc = CannedService::with([Some(listing(vec![with_methods(vec![unknown_auth])]))]);
            let err = drive(cmd, &svc, &["sk-live"], &cwd())
                .await
                .expect_err("an unofferable method must be refused");
            assert!(format!("{err:#}").contains("needs a newer lns"), "{err:#}");
        }
    }

    #[tokio::test]
    async fn a_method_that_writes_files_says_so_rather_than_blaming_the_version() {
        // Updating lns would not help: no version of it delivers a fileset yet, because install keeps the document alone.
        let svc = CannedService::with([Some(listing(vec![with_methods(vec![method(
            "seeded", false,
        )])]))]);
        let err = drive(
            ConnectorCommand::Grant(GrantArgs {
                name: "some-provider".into(),
                method: Some("seeded".into()),
                connection: None,
                run: "reviewer".into(),
                yes: false,
            }),
            &svc,
            &["y"],
            &cwd(),
        )
        .await
        .expect_err("a method that writes files cannot be granted yet");

        let said = format!("{err:#}");
        assert!(said.contains("writes files"), "{said}");
        assert!(
            !said.contains("newer lns"),
            "no version of lns delivers it, so pointing at an update is a dead end: {said}"
        );
    }

    #[tokio::test]
    async fn granting_with_one_offerable_method_needs_no_flag() {
        let svc = CannedService::with([
            Some(listing(vec![with_methods(vec![method("token", true)])])),
            Some(Response::ConnectorGranted {
                name: "some-provider".into(),
                method: "token".into(),
                connection: None,
                displaced: None,
                unchanged: false,
                reserved: false,
            }),
        ]);
        let (code, seen) = drive(
            ConnectorCommand::Grant(GrantArgs {
                name: "some-provider".into(),
                method: None,
                connection: None,
                run: "reviewer".into(),
                yes: false,
            }),
            &svc,
            &["y"],
            &cwd(),
        )
        .await
        .expect("one offerable method is unambiguous");
        assert_eq!(code, 0);
        assert!(seen.contains("granted some-provider"), "got: {seen}");
    }

    #[tokio::test]
    async fn a_method_declaring_no_credential_has_nothing_to_ask_for() {
        // A method that authenticates but declares no credential would otherwise prompt for a value with nowhere to put it.
        let bare = lns_ipc::ConnectorMethodView {
            name: "token".into(),
            label: "token".into(),
            auth_label: Some("token".to_string()),
            offerable: true,
            opens: Vec::new(),
            writes: Vec::new(),
            env: Vec::new(),
            help: None,
            overrides: None,
            credentials: Vec::new(),
            asks: Vec::new(),
        };
        let svc = CannedService::with([Some(listing(vec![with_methods(vec![bare])]))]);
        // Driven through `run` rather than `drive`, because what the user was asked before the refusal is the point and `drive` drops the prompt on an error.
        let mut out = Vec::new();
        let mut prompt = Vec::new();
        let err = run(
            &ConnectorCommand::Connect(ConnectArgs {
                name: "some-provider".into(),
                method: Some("token".into()),
                label: None,
            }),
            &svc,
            &mut asking(&["", "sk-live"]),
            &cwd(),
            None,
            &mut out,
            &mut prompt,
        )
        .await
        .expect_err("there is no value to ask for");

        assert!(format!("{err:#}").contains("no credential"), "{err:#}");
        let asked = String::from_utf8(prompt).expect("utf-8");
        assert!(
            !asked.contains("name this connection"),
            "a connection that cannot be made is refused before the user is asked to name it: {asked}"
        );
    }

    #[test]
    fn a_count_the_user_reads_is_a_sentence_rather_than_a_form_field() {
        // `1 connection(s)` is the shape this helper exists to avoid.
        assert_eq!(connections(1), "1 connection");
        assert_eq!(connections(2), "2 connections");
    }

    #[test]
    fn the_json_field_carries_the_method_and_the_table_carries_the_advice() {
        // `--format json` is read by programs: decorating the field would make every reader strip an em-dash back off.
        use crate::output::TableRow;
        let row = ConnectorRow::new(&with_methods(vec![method("token", true)]));

        let cells = row.cells();
        assert_eq!(row.methods, ["token"]);
        assert!(
            cells[2].contains("connect first"),
            "the table still says what to do next: {cells:?}"
        );
    }

    #[test]
    fn a_bare_reference_addresses_the_lns_hub_rather_than_docker_hub() {
        // §2.3: a bare REF is qualified, never guessed — and the guess the registry parser makes on its own is Docker Hub, which is someone else's registry.
        assert_eq!(qualified_source("acme/docs", None), "hub.lns.run/acme/docs");
    }

    #[test]
    fn a_configured_registry_is_the_one_a_bare_reference_addresses() {
        assert_eq!(
            qualified_source("acme/docs", Some("registry.example")),
            "registry.example/acme/docs"
        );
    }

    #[test]
    fn a_reference_that_already_names_a_registry_is_left_as_written() {
        assert_eq!(
            qualified_source("ghcr.io/acme/docs:1", None),
            "ghcr.io/acme/docs:1"
        );
    }

    #[test]
    fn a_local_path_is_not_a_reference_and_gains_no_registry() {
        for path in [
            ".",
            "..",
            "./connectors/docs",
            "./connectors/docs/lns.yaml",
            "/abs/docs",
        ] {
            assert_eq!(qualified_source(path, None), path, "{path}");
        }
    }

    #[tokio::test]
    async fn connecting_twice_keeps_both_accounts_rather_than_replacing_the_first() {
        // The store keys a connection by its label, so reusing one overwrites the account already under it — silently, and taking every grant that named it.
        let held = ConnectorView {
            connections: vec![lns_ipc::ConnectorConnectionView {
                label: "token".into(),
                method: "token".into(),
                authority: Vec::new(),
            }],
            ..with_methods(vec![method("token", true)])
        };
        let svc = CannedService::with([
            Some(listing(vec![held])),
            Some(Response::ConnectorConnected {
                name: "some-provider".into(),
                connection: "token-2".into(),
                invalidated: Vec::new(),
            }),
        ]);

        let (code, seen) = drive(
            ConnectorCommand::Connect(ConnectArgs {
                name: "some-provider".into(),
                method: None,
                label: None,
            }),
            &svc,
            &["", "sk-live"],
            &cwd(),
        )
        .await
        .expect("connect");

        assert_eq!(code, 0);
        assert!(
            seen.contains("token-2"),
            "the suggested name is free, and the user is shown it: {seen}"
        );
        let sent = svc.sent();
        assert!(
            matches!(&sent[1], Request::ConnectConnector { connection, .. } if connection == "token-2"),
            "{sent:?}"
        );
    }

    #[tokio::test]
    async fn a_name_the_user_types_is_the_one_the_connection_is_kept_under() {
        let held = ConnectorView {
            connections: vec![lns_ipc::ConnectorConnectionView {
                label: "token".into(),
                method: "token".into(),
                authority: Vec::new(),
            }],
            ..with_methods(vec![method("token", true)])
        };
        let svc = CannedService::with([
            Some(listing(vec![held])),
            Some(Response::ConnectorConnected {
                name: "some-provider".into(),
                connection: "personal".into(),
                invalidated: Vec::new(),
            }),
        ]);

        drive(
            ConnectorCommand::Connect(ConnectArgs {
                name: "some-provider".into(),
                method: None,
                label: None,
            }),
            &svc,
            &["personal", "sk-live"],
            &cwd(),
        )
        .await
        .expect("connect");

        let sent = svc.sent();
        assert!(
            matches!(&sent[1], Request::ConnectConnector { connection, .. } if connection == "personal"),
            "{sent:?}"
        );
    }

    #[tokio::test]
    async fn connecting_asks_once_per_value_the_auth_produces_and_not_once_per_variable() {
        // A `kind: token` auth produces one value, so two credentials drawing on it are two variables holding one secret — asking twice would collect a second value nothing reads.
        let two = lns_ipc::ConnectorMethodView {
            name: "token".into(),
            label: "token".into(),
            auth_label: Some("token".to_string()),
            offerable: true,
            opens: Vec::new(),
            writes: Vec::new(),
            env: Vec::new(),
            help: None,
            overrides: None,
            credentials: vec!["SOME_TOKEN".into(), "SOME_SECRET".into()],
            asks: vec!["token".into()],
        };
        let svc = CannedService::with([
            Some(listing(vec![with_methods(vec![two])])),
            Some(Response::ConnectorConnected {
                name: "some-provider".into(),
                connection: "token".into(),
                invalidated: Vec::new(),
            }),
        ]);
        let (_, seen) = drive(
            ConnectorCommand::Connect(ConnectArgs {
                name: "some-provider".into(),
                method: Some("token".into()),
                label: None,
            }),
            &svc,
            &["", "first", "second"],
            &cwd(),
        )
        .await
        .expect("connect");
        assert_eq!(
            seen.matches("not shown").count(),
            1,
            "one value the auth produces is one question: {seen}"
        );
        let values = svc
            .sent
            .lock()
            .unwrap()
            .iter()
            .find_map(|req| match req {
                Request::ConnectConnector { values, .. } => Some(values.0.clone()),
                _ => None,
            })
            .expect("a connect request must have been sent");
        assert_eq!(
            values,
            [("token".to_string(), "first".to_string())].into(),
            "the value travels under the auth output both credentials draw on, which is the key the grant reads it back under"
        );
    }
}
