use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, bail};
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
        about = "Remove a connector from this machine, with every profile it held. Grants stay."
    )]
    Uninstall(NameArg),
    #[command(about = "List what is installed: what each connector serves, and the profiles held.")]
    List(ListArgs),
    #[command(
        about = "Sign this machine in to a connector and keep the result as a profile. Connecting is not granting."
    )]
    Connect(ConnectArgs),
    #[command(about = "Drop one profile, or every profile of a connector. Grants stay.")]
    Disconnect(DisconnectArgs),
    #[command(
        about = "Let this project use one of a connector's methods. Discloses what it applies, then asks."
    )]
    Grant(GrantArgs),
    #[command(
        about = "Clear this project's decision about a connector, so the next run asks again."
    )]
    Forget(ForgetArgs),
}

#[derive(clap::Args)]
pub struct ConnectArgs {
    #[arg(help = "The connector's name, as `lns connector list` shows it.")]
    pub name: String,
    #[arg(long, help = "Which method to connect. Omitted, you choose.")]
    pub method: Option<String>,
    #[arg(long = "as", help = "What to call the profile this stores.")]
    pub label: Option<String>,
}

#[derive(clap::Args)]
pub struct DisconnectArgs {
    #[arg(help = "The connector's name.")]
    pub name: String,
    #[arg(
        long,
        help = "Which profile to drop. Omitted, every profile is dropped."
    )]
    pub profile: Option<String>,
}

#[derive(clap::Args)]
pub struct GrantArgs {
    #[arg(help = "The connector's name.")]
    pub name: String,
    #[arg(long, help = "Which method to grant. Omitted, you choose.")]
    pub method: Option<String>,
    #[arg(
        long,
        help = "Which profile stands behind it, where the method authenticates."
    )]
    pub profile: Option<String>,
    #[arg(long, help = "Act on another directory instead of this one.")]
    pub project: Option<std::path::PathBuf>,
}

#[derive(clap::Args)]
pub struct ForgetArgs {
    #[arg(help = "The connector's name.")]
    pub name: String,
    #[arg(long, help = "Act on another directory instead of this one.")]
    pub project: Option<std::path::PathBuf>,
}

#[derive(clap::Args)]
pub struct InstallArgs {
    #[arg(
        help = "A published connector reference, or a path to a directory holding its lns.yaml."
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
    writer: &mut impl Write,
    prompt: &mut impl Write,
) -> Result<i32> {
    match cmd {
        ConnectorCommand::Install(args) => install(svc, &args.source, cwd, writer).await,
        ConnectorCommand::Uninstall(args) => uninstall(svc, &args.name, writer).await,
        ConnectorCommand::List(args) => list(svc, args.output.format, writer).await,
        ConnectorCommand::Connect(args) => connect(svc, args, terminal, writer, prompt).await,
        ConnectorCommand::Disconnect(args) => disconnect(svc, args, writer).await,
        ConnectorCommand::Grant(args) => grant(svc, args, terminal, cwd, writer, prompt).await,
        ConnectorCommand::Forget(args) => forget(svc, args, cwd, writer).await,
    }
}

/// The project a verb acts on: this directory, or the one `--project` names.
fn project_dir(explicit: Option<&std::path::Path>, cwd: &Path) -> Result<String> {
    let dir = explicit.map_or_else(|| cwd.to_path_buf(), |p| cwd.join(p));
    lns_artifact::sandbox::fold_path(&dir)
        .to_str()
        .map(str::to_string)
        .with_context(|| format!("project directory {} is not utf-8", dir.display()))
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
            "connecting {} needs the value its authentication asks for, and there is no terminal to ask at; no flag answers it, so run `lns connector connect {}` from a terminal",
            args.name,
            args.name
        );
    }
    let (connector, method) = method_to_connect(svc, &args.name, args.method.as_deref()).await?;
    writeln!(prompt, "connecting {} with {}", args.name, method.label)?;
    let profile = match args.label.clone() {
        Some(named) => named,
        None => confirm_name(&connector, &method, terminal, prompt)?,
    };
    let values = ask_for_each(&method, terminal, prompt)?;
    let req = Request::ConnectConnector {
        name: args.name.clone(),
        method: method.name.clone(),
        profile,
        values: lns_ipc::SecretValues(values),
    };
    match send(svc, req).await? {
        Response::ConnectorConnected {
            name,
            profile,
            invalidated,
        } => {
            writeln!(writer, "connected {name} as {profile}")?;
            report_invalidated(&invalidated, writer)?;
            writeln!(
                writer,
                "  connecting is not granting; a project still decides whether to use it"
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
    let suggested = connector.free_profile_name(&method.name);
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
) -> Result<(ConnectorView, lns_ipc::ConnectorMethodView)> {
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
        bail!("method {} of {name} needs a newer lns", method.name);
    }
    if method.credentials.is_empty() && method.needs_connect {
        bail!(
            "method {} declares no credential, so there is no value to ask for",
            method.name
        );
    }
    if !method.needs_connect {
        bail!(
            "method {} of {name} has no authentication, so there is nothing to connect; grant it instead",
            method.name
        );
    }
    Ok((connector, method))
}

/// Ask once per credential the method declares, naming the variable the value is for, so a method needing two is not answered with one.
fn ask_for_each(
    method: &lns_ipc::ConnectorMethodView,
    terminal: &mut dyn Terminal,
    prompt: &mut impl Write,
) -> Result<std::collections::BTreeMap<String, String>> {
    let mut values = std::collections::BTreeMap::new();
    for credential in &method.credentials {
        write!(prompt, "{credential} (not shown): ")?;
        prompt.flush()?;
        let value = terminal.read_secret()?.trim().to_string();
        writeln!(prompt)?;
        if value.is_empty() {
            bail!("no value was given for {credential}, so nothing was connected");
        }
        values.insert(credential.clone(), value);
    }
    Ok(values)
}

/// A re-authentication that reports different authority invalidates the grants naming that profile, so the projects that must decide again are named (§3.2.4).
fn report_invalidated(invalidated: &[String], writer: &mut impl Write) -> std::io::Result<()> {
    if invalidated.is_empty() {
        return Ok(());
    }
    writeln!(
        writer,
        "  these projects must decide again, because this sign-in reported different authority: {}",
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
        profile: args.profile.clone(),
    };
    match send(svc, req).await? {
        Response::ConnectorDisconnected { name, dropped: 0 } => {
            writeln!(writer, "{name} holds no profile to disconnect")?;
            Ok(1)
        }
        Response::ConnectorDisconnected { name, dropped } => {
            writeln!(writer, "disconnected {name}, dropping {dropped} profile(s)")?;
            writeln!(
                writer,
                "  it stays installed, and projects that granted a dropped profile keep their grants"
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
    cwd: &Path,
    writer: &mut impl Write,
    prompt: &mut impl Write,
) -> Result<i32> {
    // cli-spec §3.3: `grant` is the card in a terminal, and no flag answers it, so a script cannot consent on a person's behalf.
    if !terminal.is_available() {
        bail!(
            "granting {} discloses what it opens and then asks; there is no terminal to ask at, and no flag answers a connector grant — run `lns connector grant {}` from a terminal",
            args.name,
            args.name
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
        bail!("method {} of {} needs a newer lns", method.name, args.name);
    }
    let dir = project_dir(args.project.as_deref(), cwd)?;
    disclose(&connector, &method, &dir, prompt)?;
    write!(prompt, "grant it? [y/N] ")?;
    prompt.flush()?;
    let answer = terminal.read_answer()?;
    writeln!(prompt)?;
    if !crate::terminal::is_affirmative(&answer) {
        writeln!(writer, "nothing was granted")?;
        return Ok(1);
    }
    let req = Request::GrantConnector {
        name: args.name.clone(),
        project_dir: dir,
        method: method.name.clone(),
        profile: args.profile.clone(),
    };
    match send(svc, req).await? {
        Response::ConnectorGranted {
            name,
            method,
            unchanged: true,
            ..
        } => {
            writeln!(writer, "this project already granted {name}: {method}")?;
            Ok(1)
        }
        Response::ConnectorGranted {
            name,
            method,
            profile,
            displaced,
            ..
        } => {
            match profile {
                Some(profile) => writeln!(writer, "granted {name}: {method} as {profile}")?,
                None => writeln!(writer, "granted {name}: {method}")?,
            }
            if let Some(displaced) = displaced {
                writeln!(writer, "  replaced {displaced}, whose payload is retracted")?;
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
    dir: &str,
    prompt: &mut impl Write,
) -> Result<()> {
    writeln!(prompt, "{} would be granted to {dir}", connector.name)?;
    writeln!(prompt, "  method:  {}", method.label)?;
    disclose_list(prompt, "opens", &method.opens)?;
    disclose_list(prompt, "writes", &method.writes)?;
    let sets: Vec<String> = method
        .env
        .iter()
        .chain(method.credentials.iter())
        .cloned()
        .collect();
    disclose_list(prompt, "sets", &sets)?;
    for profile in &connector.profiles {
        let authority = if profile.authority.is_empty() {
            "no authority reported".to_string()
        } else {
            profile.authority.join(", ")
        };
        writeln!(prompt, "  profile: {} ({authority})", profile.label)?;
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
    writeln!(prompt, "  {noun:8} {rendered}")
}

async fn forget(
    svc: &dyn ConnectorService,
    args: &ForgetArgs,
    cwd: &Path,
    writer: &mut impl Write,
) -> Result<i32> {
    let req = Request::ForgetConnector {
        name: args.name.clone(),
        project_dir: project_dir(args.project.as_deref(), cwd)?,
    };
    match send(svc, req).await? {
        Response::ConnectorForgotten {
            name,
            had_decision: false,
        } => {
            writeln!(writer, "this project decided nothing about {name}")?;
            Ok(1)
        }
        Response::ConnectorForgotten { name, .. } => {
            writeln!(writer, "forgot what this project decided about {name}")?;
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
    writeln!(writer, "  serves: {}", connector.serves.join(", "))?;
    for method in &connector.methods {
        let needs = if method.needs_connect {
            "connect to use"
        } else {
            "no connect needed"
        };
        let unsupported = if method.offerable {
            ""
        } else {
            " — needs a newer lns"
        };
        writeln!(writer, "  method {}: {needs}{unsupported}", method.label)?;
    }
    writeln!(writer, "nothing is granted yet")
}

async fn uninstall(svc: &dyn ConnectorService, name: &str, writer: &mut impl Write) -> Result<i32> {
    let req = Request::UninstallConnector {
        name: name.to_string(),
    };
    match send(svc, req).await? {
        Response::ConnectorUninstalled {
            name,
            dropped_profiles,
        } => {
            writeln!(writer, "uninstalled {name}")?;
            if dropped_profiles > 0 {
                writeln!(writer, "  dropped {dropped_profiles} profile(s)")?;
            }
            writeln!(
                writer,
                "  projects that granted it keep that decision; reinstalling the same bytes resumes it"
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
    methods: Vec<String>,
    profiles: Vec<String>,
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
                .map(|m| {
                    if m.needs_connect {
                        m.label.clone()
                    } else {
                        format!("{} (no connect)", m.label)
                    }
                })
                .collect(),
            profiles: connector.profiles.iter().map(|p| p.label.clone()).collect(),
        }
    }
}

impl crate::output::TableRow for ConnectorRow {
    const HEADERS: &'static [&'static str] = &["NAME", "SERVES", "METHODS", "PROFILES"];

    fn cells(&self) -> Vec<String> {
        vec![
            self.name.clone(),
            self.serves.join(", "),
            self.methods.join(", "),
            none_when_empty(&self.profiles),
        ]
    }
}

/// A connector with no profile is the normal state after an install, so the column says so rather than sitting blank.
fn none_when_empty(profiles: &[String]) -> String {
    if profiles.is_empty() {
        return "none".to_string();
    }
    profiles.join(", ")
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
    }

    impl CannedService {
        fn with(responses: impl IntoIterator<Item = Option<Response>>) -> Self {
            Self {
                responses: Mutex::new(responses.into_iter().collect()),
                sent: Mutex::new(Vec::new()),
            }
        }

        fn sent(&self) -> Vec<Request> {
            self.sent.lock().unwrap().clone()
        }
    }

    impl ConnectorService for CannedService {
        fn request(&self, req: Request) -> LocalBoxFuture<'_, Option<Response>> {
            self.sent.lock().unwrap().push(req);
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
                needs_connect: true,
                offerable: false,
                opens: Vec::new(),
                writes: Vec::new(),
                env: Vec::new(),
                help: None,
                credentials: Vec::new(),
            }],
            profiles: Vec::new(),
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
            &mut out,
            &mut Vec::new(),
        )
        .await
        .expect("the install itself succeeds");
        assert_eq!(code, 0);
        let text = String::from_utf8(out).expect("utf-8");
        assert!(text.contains("needs a newer lns"), "got: {text}");
    }

    #[test]
    fn the_profiles_column_lists_what_the_machine_holds() {
        let row = ConnectorRow::new(&ConnectorView {
            name: "some-provider".into(),
            digest: "sha256:abc".into(),
            serves: vec!["api.some-provider.example".into()],
            methods: Vec::new(),
            profiles: vec![
                lns_ipc::ConnectorProfileView {
                    label: "work".into(),
                    method: "token".into(),
                    authority: Vec::new(),
                },
                lns_ipc::ConnectorProfileView {
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
        let code = run(&cmd, svc, &mut asking(answers), cwd, &mut out, &mut prompt).await?;
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
            profiles: Vec::new(),
        }
    }

    fn method(name: &str, offerable: bool) -> lns_ipc::ConnectorMethodView {
        lns_ipc::ConnectorMethodView {
            name: name.into(),
            label: name.into(),
            needs_connect: true,
            offerable,
            opens: vec!["other.example".into()],
            writes: vec!["/home/agent/.netrc".into()],
            env: vec!["SOME_REGION".into()],
            help: None,
            credentials: vec!["SOME_TOKEN".into()],
        }
    }

    #[tokio::test]
    async fn connecting_with_one_offerable_method_needs_no_flag() {
        let svc = CannedService::with([
            Some(listing(vec![with_methods(vec![method("token", true)])])),
            Some(Response::ConnectorConnected {
                name: "some-provider".into(),
                profile: "token".into(),
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
            &mut out,
            &mut Vec::new(),
        )
        .await
        .expect_err("nothing to choose from");
        assert!(format!("{err:#}").contains("absent"), "{err:#}");
    }

    #[tokio::test]
    async fn a_reauthentication_that_invalidates_grants_names_the_projects_that_must_decide_again()
    {
        // §3.2.4: where a re-authentication reports different authority, every grant naming that profile is invalidated.
        let svc = CannedService::with([
            Some(listing(vec![with_methods(vec![method("token", true)])])),
            Some(Response::ConnectorConnected {
                name: "some-provider".into(),
                profile: "work".into(),
                invalidated: vec!["/work".into(), "/other".into()],
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
            seen.contains("/work, /other"),
            "the projects are named: {seen}"
        );
    }

    #[tokio::test]
    async fn the_disclosure_names_a_profiles_authority_and_says_when_it_reported_none() {
        let held = |authority: Vec<String>| ConnectorView {
            name: "some-provider".into(),
            digest: "sha256:abc".into(),
            serves: vec!["api.some-provider.example".into()],
            methods: vec![method("token", true)],
            profiles: vec![lns_ipc::ConnectorProfileView {
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
                    profile: Some("work".into()),
                    displaced: None,
                    unchanged: false,
                }),
            ]);
            let (_, seen) = drive(
                ConnectorCommand::Grant(GrantArgs {
                    name: "some-provider".into(),
                    method: Some("token".into()),
                    profile: None,
                    project: None,
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
                "the profile behind it is named: {seen}"
            );
        }
    }

    #[tokio::test]
    async fn granting_in_another_directory_names_that_directory() {
        let svc = CannedService::with([
            Some(listing(vec![with_methods(vec![method("token", true)])])),
            Some(Response::ConnectorGranted {
                name: "some-provider".into(),
                method: "token".into(),
                profile: None,
                displaced: None,
                unchanged: false,
            }),
        ]);
        let (_, seen) = drive(
            ConnectorCommand::Grant(GrantArgs {
                name: "some-provider".into(),
                method: Some("token".into()),
                profile: None,
                project: Some(std::path::PathBuf::from("../elsewhere")),
            }),
            &svc,
            &["y"],
            &cwd(),
        )
        .await
        .expect("grant");
        assert!(seen.contains("/elsewhere"), "got: {seen}");
    }

    #[tokio::test]
    async fn a_project_directory_that_is_not_utf8_is_refused() {
        use std::os::unix::ffi::OsStrExt;
        let odd = std::path::PathBuf::from(std::ffi::OsStr::from_bytes(b"/work/\xff"));
        let svc = CannedService::with([]);
        let mut out = Vec::new();
        let err = run(
            &ConnectorCommand::Forget(ForgetArgs {
                name: "some-provider".into(),
                project: None,
            }),
            &svc,
            &mut asking(&["y"]),
            &odd,
            &mut out,
            &mut Vec::new(),
        )
        .await
        .expect_err("a non-utf8 project directory must be refused");
        assert!(format!("{err:#}").contains("utf-8"), "{err:#}");
    }

    #[tokio::test]
    async fn every_project_verb_refuses_a_response_meant_for_another() {
        let cases: Vec<ConnectorCommand> = vec![
            ConnectorCommand::Connect(ConnectArgs {
                name: "some-provider".into(),
                method: Some("token".into()),
                label: None,
            }),
            ConnectorCommand::Disconnect(DisconnectArgs {
                name: "some-provider".into(),
                profile: None,
            }),
            ConnectorCommand::Forget(ForgetArgs {
                name: "some-provider".into(),
                project: None,
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
                    profile: None,
                    project: None,
                }),
                &svc,
                &mut asking(&["y"]),
                &cwd(),
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
                profile: None,
                project: None,
            }),
            &svc,
            &mut asking(&["y"]),
            &cwd(),
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
                profile: None,
                project: None,
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
                profile: None,
                project: None,
            }),
        ] {
            let svc = CannedService::with([Some(listing(vec![with_methods(vec![method(
                "future", false,
            )])]))]);
            let err = drive(cmd, &svc, &["sk-live"], &cwd())
                .await
                .expect_err("an unofferable method must be refused");
            assert!(format!("{err:#}").contains("needs a newer lns"), "{err:#}");
        }
    }

    #[tokio::test]
    async fn granting_with_one_offerable_method_needs_no_flag() {
        let svc = CannedService::with([
            Some(listing(vec![with_methods(vec![method("token", true)])])),
            Some(Response::ConnectorGranted {
                name: "some-provider".into(),
                method: "token".into(),
                profile: None,
                displaced: None,
                unchanged: false,
            }),
        ]);
        let (code, seen) = drive(
            ConnectorCommand::Grant(GrantArgs {
                name: "some-provider".into(),
                method: None,
                profile: None,
                project: None,
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
            needs_connect: true,
            offerable: true,
            opens: Vec::new(),
            writes: Vec::new(),
            env: Vec::new(),
            help: None,
            credentials: Vec::new(),
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

    #[tokio::test]
    async fn connecting_twice_keeps_both_accounts_rather_than_replacing_the_first() {
        // The store keys a profile by its label, so reusing one overwrites the account already under it — silently, and taking every grant that named it.
        let held = ConnectorView {
            profiles: vec![lns_ipc::ConnectorProfileView {
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
                profile: "token-2".into(),
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
            matches!(&sent[1], Request::ConnectConnector { profile, .. } if profile == "token-2"),
            "{sent:?}"
        );
    }

    #[tokio::test]
    async fn a_name_the_user_types_is_the_one_the_connection_is_kept_under() {
        let held = ConnectorView {
            profiles: vec![lns_ipc::ConnectorProfileView {
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
                profile: "personal".into(),
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
            matches!(&sent[1], Request::ConnectConnector { profile, .. } if profile == "personal"),
            "{sent:?}"
        );
    }

    #[tokio::test]
    async fn connecting_asks_once_per_credential_and_keys_each_by_its_variable() {
        // A method may declare several credentials, so one answer cannot stand for all of them.
        let two = lns_ipc::ConnectorMethodView {
            name: "token".into(),
            label: "token".into(),
            needs_connect: true,
            offerable: true,
            opens: Vec::new(),
            writes: Vec::new(),
            env: Vec::new(),
            help: None,
            credentials: vec!["SOME_TOKEN".into(), "SOME_SECRET".into()],
        };
        let svc = CannedService::with([
            Some(listing(vec![with_methods(vec![two])])),
            Some(Response::ConnectorConnected {
                name: "some-provider".into(),
                profile: "token".into(),
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
        for named in ["SOME_TOKEN", "SOME_SECRET"] {
            assert!(seen.contains(named), "{named} was never asked for: {seen}");
        }
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
        assert_eq!(values["SOME_TOKEN"], "first");
        assert_eq!(values["SOME_SECRET"], "second");
    }
}
