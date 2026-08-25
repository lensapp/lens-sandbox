use anyhow::{Context, Result, bail};
use lns_ipc::{
    Request, Response, RunDetails, RunStatsInfo, WireFrame, decode_wire_frame_from_bytes,
    read_frame_bytes_async,
};
use tokio::io::{AsyncRead, AsyncWriteExt};

use crate::cli::{DetachChord, ExecArgs, KillArgs, RunArgs, parse_detach_keys_arg};
use crate::command::{CommandSpec, subcommand};
pub use crate::service::client::{BoxFuture, SandboxService, TermInfo};

pub mod real;

#[derive(clap::Args)]
pub struct SandboxArgs {
    #[command(subcommand)]
    pub command: SandboxCommand,
}

/// Only the verbs that act on a sandbox — the document one is made from is `lns artifact`'s.
#[derive(clap::Subcommand)]
pub enum SandboxCommand {
    #[command(about = "Run a sandbox in a microVM (the top-level `lns run`).")]
    Run(Box<RunArgs>),
    #[command(about = "Run another command inside a running sandbox.")]
    Exec(ExecArgs),
    #[command(about = "Send a signal to a running sandbox (`docker kill`-style).")]
    Kill(KillArgs),
    #[command(about = "Stop a sandbox gracefully: SIGTERM, then SIGKILL once the timeout passes.")]
    Stop(SandboxStopArgs),
    #[command(about = "Run a stopped sandbox again on its preserved writable layer.")]
    Start(SandboxStartArgs),
    #[command(about = "Print a sandbox's captured output; `-f` streams until the workload exits.")]
    Logs(SandboxLogsArgs),
    #[command(about = "Re-attach to a running sandbox's output (detach chord to leave again).")]
    Attach(SandboxAttachArgs),
    #[command(about = "List running sandboxes with their state, CPU and memory.")]
    Ls(SandboxLsArgs),
    #[command(about = "Print one sandbox's live state and launch configuration.")]
    Inspect(SandboxInspectArgs),
    #[command(about = "Remove a sandbox: its record and its writable layer.")]
    Rm(SandboxRmArgs),
    #[command(about = "Remove every stopped sandbox, writable layers included.")]
    Prune(SandboxPruneArgs),
}

#[derive(clap::Args)]
pub struct SandboxLsArgs {
    #[arg(
        short = 'a',
        long = "all",
        default_value_t = false,
        help = "Include the sandboxes that have stopped, not only the running ones."
    )]
    pub all: bool,

    #[command(flatten)]
    pub output: crate::output::OutputArgs,
}

#[derive(clap::Args)]
pub struct SandboxStartArgs {
    #[arg(value_name = "RUN", help = "Stopped sandbox id or name to run again.")]
    pub run: String,

    #[arg(
        short = 'a',
        long,
        default_value_t = false,
        help = "Attach to the sandbox's output and adopt the workload's exit code."
    )]
    pub attach: bool,

    #[arg(
        short = 'i',
        long,
        default_value_t = false,
        help = "With -a: forward stdin to the workload."
    )]
    pub interactive: bool,

    #[arg(
        long,
        default_value = "ctrl-p,ctrl-q",
        value_parser = parse_detach_keys_arg,
        help = "With -a: detach chord; on match the CLI detaches, leaving the sandbox running."
    )]
    pub detach_keys: DetachChord,
}

#[derive(clap::Args)]
pub struct SandboxPruneArgs {
    #[arg(
        short = 'f',
        long,
        default_value_t = false,
        help = "Remove every stopped sandbox without asking."
    )]
    pub force: bool,
}

#[derive(clap::Args)]
pub struct SandboxStopArgs {
    #[arg(
        value_name = "RUN",
        help = "Target run id or name surfaced by `lns ps`."
    )]
    pub run: String,

    #[arg(
        short = 't',
        long,
        default_value_t = 10,
        help = "Seconds to wait for a graceful exit before escalating to SIGKILL."
    )]
    pub timeout: u64,
}

#[derive(clap::Args)]
pub struct SandboxLogsArgs {
    #[arg(
        value_name = "RUN",
        help = "Target run id or name surfaced by `lns ps`."
    )]
    pub run: String,

    #[arg(
        short = 'f',
        long,
        default_value_t = false,
        help = "Keep streaming new output until the run exits."
    )]
    pub follow: bool,
}

#[derive(clap::Args)]
pub struct SandboxAttachArgs {
    #[arg(
        value_name = "RUN",
        help = "Target run id or name surfaced by `lns ps`."
    )]
    pub run: String,

    #[arg(
        long,
        default_value = "ctrl-p,ctrl-q",
        value_parser = parse_detach_keys_arg,
        help = "Detach chord; on match the CLI detaches and returns, leaving the run running (docker-attach style — no signal is sent)."
    )]
    pub detach_keys: DetachChord,
}

#[derive(clap::Args)]
pub struct SandboxInspectArgs {
    #[arg(
        value_name = "RUN",
        help = "Sandbox id or name surfaced by `lns sandbox ls`."
    )]
    pub run: String,

    #[command(flatten)]
    pub output: crate::output::OutputArgs,
}

#[derive(clap::Args)]
pub struct SandboxRmArgs {
    #[arg(
        value_name = "RUN",
        help = "Sandbox id or name to remove; a running one is refused unless `-f`."
    )]
    pub run: String,

    #[arg(
        short = 'f',
        long,
        default_value_t = false,
        help = "Stop a running sandbox first, then remove it."
    )]
    pub force: bool,
}

pub fn augment(app: clap::Command) -> clap::Command {
    app.subcommand(
        subcommand::<SandboxArgs>("sandbox")
            .about("The sandbox a document becomes when it runs — create, watch, and remove it.")
            .mut_subcommand("run", crate::run::long_help_only),
    )
}

/// Only four of the parent's verbs drive the tty; the rest prompt, so they need the dispatcher's real stdin instead of the `io::empty()` a terminal owner is handed.
fn verb_owns_terminal(sub: &clap::ArgMatches) -> bool {
    matches!(
        sub.subcommand_name(),
        Some("run" | "exec" | "logs" | "attach" | "start")
    )
}

pub const SPEC: CommandSpec = CommandSpec {
    name: "sandbox",
    augment,
    run: real::run,
    announces_update_check: true,
    owns_terminal: verb_owns_terminal,
};

pub fn augment_ps(app: clap::Command) -> clap::Command {
    app.subcommand(
        subcommand::<SandboxLsArgs>("ps").about(
            "List running sandboxes with their CPU and memory (shortcut for `lns sandbox ls`).",
        ),
    )
}

pub const PS_SPEC: CommandSpec = CommandSpec {
    name: "ps",
    augment: augment_ps,
    run: real::run_ps,
    announces_update_check: true,
    owns_terminal: crate::command::never_owns_terminal,
};

macro_rules! shortcut_spec {
    ($augment:ident, $const_name:ident, $args:ty, $name:literal, $run:path, $about:literal) => {
        shortcut_spec!(
            $augment,
            $const_name,
            $args,
            $name,
            $run,
            $about,
            crate::command::never_owns_terminal
        );
    };
    ($augment:ident, $const_name:ident, $args:ty, $name:literal, $run:path, $about:literal, $owns_terminal:expr) => {
        pub fn $augment(app: clap::Command) -> clap::Command {
            app.subcommand(subcommand::<$args>($name).about($about))
        }
        pub const $const_name: CommandSpec = CommandSpec {
            name: $name,
            augment: $augment,
            run: $run,
            announces_update_check: true,
            owns_terminal: $owns_terminal,
        };
    };
}

shortcut_spec!(
    augment_start,
    START_SPEC,
    SandboxStartArgs,
    "start",
    real::run_start,
    "Run a stopped sandbox again (shortcut for `lns sandbox start`).",
    crate::command::always_owns_terminal
);
shortcut_spec!(
    augment_stop,
    STOP_SPEC,
    SandboxStopArgs,
    "stop",
    real::run_stop,
    "Stop a running sandbox gracefully (shortcut for `lns sandbox stop`)."
);
shortcut_spec!(
    augment_kill,
    KILL_SPEC,
    crate::cli::KillArgs,
    "kill",
    real::run_kill,
    "Send a signal to a running sandbox (shortcut for `lns sandbox kill`)."
);
shortcut_spec!(
    augment_logs,
    LOGS_SPEC,
    SandboxLogsArgs,
    "logs",
    real::run_logs,
    "Print a running sandbox's output (shortcut for `lns sandbox logs`).",
    crate::command::always_owns_terminal
);
shortcut_spec!(
    augment_attach,
    ATTACH_SPEC,
    SandboxAttachArgs,
    "attach",
    real::run_attach,
    "Re-attach to a running sandbox (shortcut for `lns sandbox attach`).",
    crate::command::always_owns_terminal
);

pub async fn run_with_writers<S, I, W, O, E>(
    cmd: &SandboxCommand,
    svc: &S,
    term: TermInfo,
    input: &mut I,
    out: &mut W,
    stdout: &mut O,
    stderr: &mut E,
) -> Result<i32>
where
    S: SandboxService,
    I: std::io::BufRead,
    W: std::io::Write,
    O: AsyncWriteExt + Unpin,
    E: AsyncWriteExt + Unpin,
{
    if let Some(refusal) = wrong_kind_refusal(cmd) {
        stderr
            .write_all(format!("error: {refusal}\n").as_bytes())
            .await?;
        stderr.flush().await?;
        return Ok(2);
    }
    match cmd {
        SandboxCommand::Ls(args) => ps(svc, args, out).await,
        SandboxCommand::Kill(args) => kill(svc, args, out).await,
        SandboxCommand::Run(_) => bail!("sandbox run is dispatched on its own interactive path"),
        SandboxCommand::Exec(_) => bail!("sandbox exec is dispatched on its own interactive path"),
        SandboxCommand::Stop(args) => stop(svc, args, out).await,
        SandboxCommand::Start(args) => start(svc, args, term, out, stdout, stderr).await,
        SandboxCommand::Prune(args) => prune(svc, args, term, input, out, stderr).await,
        SandboxCommand::Inspect(args) => inspect(svc, &args.run, args.output.format, out).await,
        SandboxCommand::Logs(args) => logs(svc, args, stdout, stderr).await,
        SandboxCommand::Attach(args) => attach(svc, args, term, stdout, stderr).await,
        SandboxCommand::Rm(args) => rm(svc, args, out).await,
    }
}

fn run_operand(cmd: &SandboxCommand) -> Option<(&'static str, &str)> {
    match cmd {
        SandboxCommand::Exec(args) => Some(("exec", args.run.as_str())),
        SandboxCommand::Kill(args) => Some(("kill", args.run.as_str())),
        SandboxCommand::Stop(args) => Some(("stop", args.run.as_str())),
        SandboxCommand::Start(args) => Some(("start", args.run.as_str())),
        SandboxCommand::Inspect(args) => Some(("inspect", args.run.as_str())),
        SandboxCommand::Logs(args) => Some(("logs", args.run.as_str())),
        SandboxCommand::Attach(args) => Some(("attach", args.run.as_str())),
        SandboxCommand::Rm(args) => Some(("rm", args.run.as_str())),
        _ => None,
    }
}

pub(crate) fn document_refusal(verb: &str, operand: &str) -> Option<String> {
    crate::run::target::is_definition_path(operand).then(|| {
        format!(
            "`lns sandbox {verb}` takes a RUN — a sandbox id or name — and \"{operand}\" names a document; `lns artifact inspect` is what reads one"
        )
    })
}

pub(crate) fn wrong_kind_refusal(cmd: &SandboxCommand) -> Option<String> {
    let (verb, operand) = run_operand(cmd)?;
    document_refusal(verb, operand)
}

pub(crate) fn run_label(run: &str) -> String {
    run.to_string()
}

/// A stopped sandbox has no guest to sample, so it is listed without a stats probe rather than with a failed one.
async fn ps<W: std::io::Write>(
    svc: &impl SandboxService,
    args: &SandboxLsArgs,
    out: &mut W,
) -> Result<i32> {
    let listed = match svc.one_shot(Request::ListRuns).await? {
        Response::RunList { runs } => runs
            .into_iter()
            .filter(|r| args.all || matches!(r.status, lns_ipc::RunStatus::Running))
            .collect::<Vec<_>>(),
        Response::Error { message } => bail!("daemon error: {message}"),
        other => bail!("unexpected response from daemon: {other:?}"),
    };
    let mut rows = Vec::with_capacity(listed.len());
    for run in &listed {
        let stats = match run.status {
            lns_ipc::RunStatus::Running => match svc
                .one_shot(Request::RunStats {
                    run: run.id.clone(),
                })
                .await?
            {
                Response::RunStats { stats } => Some(stats),
                Response::Error { .. } => None,
                other => bail!("unexpected response from daemon: {other:?}"),
            },
            _ => None,
        };
        rows.push(PsRow::new(run, stats));
    }
    crate::output::emit(args.output.format, &rows, out)?;
    Ok(0)
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct PsRow {
    id: String,
    name: String,
    image: String,
    command: String,
    status: lns_ipc::RunStatus,
    started: String,
    cpu_permille: Option<u32>,
    mem_used_bytes: Option<u64>,
    mem_total_bytes: Option<u64>,
}

const NO_SAMPLE: &str = "-";

fn state_cell(status: lns_ipc::RunStatus) -> String {
    match status {
        lns_ipc::RunStatus::Running => "running".to_string(),
        lns_ipc::RunStatus::Exited { code } => format!("stopped ({code})"),
    }
}

impl PsRow {
    fn new(run: &lns_ipc::RunSummary, stats: Option<RunStatsInfo>) -> Self {
        Self {
            id: run.id.clone(),
            name: run.name.clone(),
            image: run.image.clone(),
            command: run.command.clone(),
            status: run.status,
            started: run.started.clone(),
            cpu_permille: stats.as_ref().map(|s| s.cpu_permille),
            mem_used_bytes: stats.as_ref().map(|s| s.mem_used_bytes),
            mem_total_bytes: stats.as_ref().map(|s| s.mem_total_bytes),
        }
    }

    fn memory_cell(&self) -> String {
        match (self.mem_used_bytes, self.mem_total_bytes) {
            (Some(used), Some(total)) => {
                format!(
                    "{} / {}",
                    crate::output::format_bytes(used),
                    crate::output::format_bytes(total)
                )
            }
            _ => NO_SAMPLE.to_string(),
        }
    }
}

impl crate::output::TableRow for PsRow {
    const HEADERS: &'static [&'static str] = &["ID", "NAME", "IMAGE", "STATE", "CPU %", "MEM"];

    fn cells(&self) -> Vec<String> {
        vec![
            lns_ipc::short_run_id(&self.id).to_string(),
            self.name.clone(),
            self.image.clone(),
            state_cell(self.status),
            self.cpu_permille
                .map_or_else(|| NO_SAMPLE.to_string(), format_permille),
            self.memory_cell(),
        ]
    }
}

async fn kill<W: std::io::Write>(
    svc: &impl SandboxService,
    args: &KillArgs,
    out: &mut W,
) -> Result<i32> {
    let signal = crate::service::parse_signal_name(&args.signal)?;
    let response = svc
        .one_shot(Request::Kill {
            run: args.run.clone(),
            signal,
        })
        .await?;
    match response {
        Response::Acknowledged => {
            writeln!(out, "killed run {}", run_label(&args.run))?;
            Ok(0)
        }
        Response::Error { message } => bail!("daemon error: {message}"),
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

async fn stop<W: std::io::Write>(
    svc: &impl SandboxService,
    args: &SandboxStopArgs,
    out: &mut W,
) -> Result<i32> {
    let response = svc
        .one_shot(Request::StopRun {
            run: args.run.clone(),
            timeout_secs: args.timeout,
        })
        .await?;
    match response {
        Response::RunStopped { forced: false } => {
            writeln!(out, "stopped run {}", run_label(&args.run))?;
            Ok(0)
        }
        Response::RunStopped { forced: true } => {
            writeln!(
                out,
                "killed run {} after the {}s timeout",
                run_label(&args.run),
                args.timeout
            )?;
            Ok(0)
        }
        Response::Error { message } => bail!("daemon error: {message}"),
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

/// The service reports a handle that isn't a live run as either "no such run: …" (nothing resolved) or "no active run with id …" (resolved then gone); only those misses fall through to the cached-artifact path, so a genuine daemon error surfaces instead of masquerading as "no such image".
pub(crate) fn is_unknown_run(message: &str) -> bool {
    message.contains("no such run") || message.contains("no active run with id")
}

/// The service owns the running-or-stopped decision, so `rm` asks it once rather than acting on a snapshot that could be stale by the time it does.
async fn rm<W: std::io::Write>(
    svc: &impl SandboxService,
    args: &SandboxRmArgs,
    out: &mut W,
) -> Result<i32> {
    match svc
        .one_shot(Request::RemoveRun {
            run: args.run.clone(),
            force: args.force,
        })
        .await?
    {
        Response::Acknowledged => {
            writeln!(out, "removed sandbox {}", args.run)?;
            Ok(0)
        }
        Response::Error { message } => bail!("daemon error: {message}"),
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

async fn start<S, W, O, E>(
    svc: &S,
    args: &SandboxStartArgs,
    term: TermInfo,
    out: &mut W,
    stdout: &mut O,
    stderr: &mut E,
) -> Result<i32>
where
    S: SandboxService,
    W: std::io::Write,
    O: AsyncWriteExt + Unpin,
    E: AsyncWriteExt + Unpin,
{
    let mut stream = svc
        .open_stream(Request::StartRun {
            run: args.run.clone(),
            attach: args.attach,
            stdin: args.interactive,
        })
        .await?;
    let run_id = expect_run_started(&mut stream).await?;
    if !args.attach {
        writeln!(out, "{}", run_label(&args.run))?;
        return Ok(0);
    }
    crate::service::drive_attached_session_with_writers(
        stream,
        svc.aux_socket(),
        run_id,
        term.stdin_is_tty,
        term.stdout_is_terminal,
        args.detach_keys.0.clone(),
        crate::service::DetachBehaviour::LeaveRunning,
        stdout,
        stderr,
        false,
        crate::service::StdinForwarding::of(args.interactive),
    )
    .await
}

async fn prune<I: std::io::BufRead, W: std::io::Write, E: AsyncWriteExt + Unpin>(
    svc: &impl SandboxService,
    args: &SandboxPruneArgs,
    term: TermInfo,
    input: &mut I,
    out: &mut W,
    stderr: &mut E,
) -> Result<i32> {
    if !args.force {
        if !term.stdin_is_tty {
            bail!(
                "this removes every stopped sandbox, writable layers included; there is no terminal to ask at, so pass --force to confirm"
            );
        }
        let stopped = stopped_run_names(svc).await?;
        if stopped.is_empty() {
            writeln!(out, "No stopped sandboxes.")?;
            return Ok(0);
        }
        crate::output::announce_prune_candidates(&stopped, stderr).await?;
        if !confirm_prune(input, stderr).await? {
            return Ok(0);
        }
    }
    match svc.one_shot(Request::PruneRuns).await? {
        Response::RunsPruned { mut removed } => {
            removed.sort_unstable();
            for run in &removed {
                writeln!(out, "removed sandbox {run}")?;
            }
            if removed.is_empty() {
                writeln!(out, "No stopped sandboxes.")?;
            }
            Ok(0)
        }
        Response::Error { message } => bail!("daemon error: {message}"),
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

async fn stopped_run_names(svc: &impl SandboxService) -> Result<Vec<String>> {
    match svc.one_shot(Request::ListRuns).await? {
        Response::RunList { runs } => {
            let mut names: Vec<String> = runs
                .into_iter()
                .filter(|run| !matches!(run.status, lns_ipc::RunStatus::Running))
                .map(|run| {
                    if run.name.is_empty() {
                        lns_ipc::short_run_id(&run.id).to_string()
                    } else {
                        run.name
                    }
                })
                .collect();
            names.sort_unstable();
            Ok(names)
        }
        Response::Error { message } => bail!("daemon error: {message}"),
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

async fn confirm_prune<I: std::io::BufRead, E: AsyncWriteExt + Unpin>(
    input: &mut I,
    err: &mut E,
) -> Result<bool> {
    err.write_all(
        b"This removes every stopped sandbox, writable layers included. Continue? [y/N] ",
    )
    .await?;
    err.flush().await?;
    let mut line = String::new();
    input.read_line(&mut line)?;
    let answer = line.trim();
    let yes = answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes");
    if !yes {
        err.write_all(b"Aborted.\n").await?;
        err.flush().await?;
    }
    Ok(yes)
}

async fn inspect<W: std::io::Write>(
    svc: &impl SandboxService,
    target: &str,
    format: crate::output::Format,
    out: &mut W,
) -> Result<i32> {
    match svc
        .one_shot(Request::InspectRun {
            run: target.to_string(),
        })
        .await?
    {
        Response::RunInspect { details } => {
            let policy = details
                .config
                .policy_path
                .as_deref()
                .map(|path| policy_doc(path, svc.load_policy(path)));
            render_inspect(&details, policy, format, out)?;
            Ok(0)
        }
        Response::Error { message } => bail!("daemon error: {message}"),
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

fn policy_doc(path: &str, loaded: Option<serde_json::Value>) -> serde_json::Value {
    match loaded {
        Some(doc) => serde_json::json!({ "path": path, "policy": doc }),
        None => serde_json::json!({ "path": path, "error": "policy file could not be read" }),
    }
}

fn render_inspect<W: std::io::Write>(
    details: &RunDetails,
    policy: Option<serde_json::Value>,
    format: crate::output::Format,
    out: &mut W,
) -> Result<()> {
    let mut config = serde_json::Map::new();
    config.insert("cpus".into(), details.config.cpus.into());
    config.insert("memMib".into(), details.config.mem_mib.into());
    config.insert("env".into(), serde_json::to_value(&details.config.env)?);
    config.insert(
        "workdir".into(),
        serde_json::to_value(&details.config.workdir)?,
    );
    config.insert(
        "publishedPorts".into(),
        serde_json::to_value(&details.config.published_ports)?,
    );
    config.insert(
        "volumes".into(),
        serde_json::to_value(&details.config.volumes)?,
    );
    config.insert("binds".into(), serde_json::to_value(&details.config.binds)?);
    config.insert(
        "sandboxUser".into(),
        serde_json::to_value(&details.config.sandbox_user)?,
    );
    config.insert(
        "sandboxUid".into(),
        serde_json::to_value(details.config.sandbox_uid)?,
    );
    config.insert("detached".into(), details.config.detached.into());

    let mut doc = serde_json::Map::new();
    doc.insert("id".into(), details.summary.id.clone().into());
    doc.insert("image".into(), details.summary.image.clone().into());
    doc.insert("command".into(), details.summary.command.clone().into());
    doc.insert(
        "status".into(),
        serde_json::to_value(details.summary.status)?,
    );
    doc.insert("started".into(), details.summary.started.clone().into());
    doc.insert(
        "uptime".into(),
        format!("since {}", details.summary.started).into(),
    );
    doc.insert("config".into(), config.into());
    doc.insert("policy".into(), policy.unwrap_or(serde_json::Value::Null));
    let fields = inspect_fields(details);
    crate::output::emit_fields(format, &fields, &serde_json::Value::Object(doc), out)
}

/// The table is the summary a reader scans; the JSON stays the record, and carries the launch configuration and the resolved policy the table has no room for.
fn inspect_fields(details: &RunDetails) -> Vec<(&'static str, String)> {
    vec![
        ("ID", details.summary.id.clone()),
        ("NAME", details.summary.name.clone()),
        ("IMAGE", details.summary.image.clone()),
        ("COMMAND", details.summary.command.clone()),
        (
            "STATUS",
            format!("{:?}", details.summary.status).to_lowercase(),
        ),
        (
            "UPTIME",
            crate::service::friendly_started(&details.summary.started),
        ),
        ("CPUS", details.config.cpus.to_string()),
        (
            "MEM",
            crate::output::format_bytes(details.config.mem_mib as u64 * 1024 * 1024),
        ),
        (
            "WORKDIR",
            details.config.workdir.clone().unwrap_or_else(|| "-".into()),
        ),
        (
            "USER",
            details
                .config
                .sandbox_user
                .clone()
                .unwrap_or_else(|| "-".into()),
        ),
    ]
}

fn format_permille(permille: u32) -> String {
    format!("{}.{}%", permille / 10, permille % 10)
}

async fn logs<S, O, E>(
    svc: &S,
    args: &SandboxLogsArgs,
    stdout: &mut O,
    stderr: &mut E,
) -> Result<i32>
where
    S: SandboxService,
    O: AsyncWriteExt + Unpin,
    E: AsyncWriteExt + Unpin,
{
    let mut stream = svc
        .open_stream(Request::RunLogs {
            run: args.run.clone(),
            follow: args.follow,
        })
        .await?;
    expect_run_started(&mut stream).await?;
    drive_logs(stream, stdout, stderr).await
}

async fn attach<S, O, E>(
    svc: &S,
    args: &SandboxAttachArgs,
    term: TermInfo,
    stdout: &mut O,
    stderr: &mut E,
) -> Result<i32>
where
    S: SandboxService,
    O: AsyncWriteExt + Unpin,
    E: AsyncWriteExt + Unpin,
{
    let mut stream = svc
        .open_stream(Request::AttachRun {
            run: args.run.clone(),
        })
        .await?;
    let run_id = expect_run_started(&mut stream).await?;
    crate::service::drive_attached_session_with_writers(
        stream,
        svc.aux_socket(),
        run_id,
        term.stdin_is_tty,
        term.stdout_is_terminal,
        args.detach_keys.0.clone(),
        crate::service::DetachBehaviour::LeaveRunning,
        stdout,
        stderr,
        false,
        crate::service::StdinForwarding::ToRun,
    )
    .await
}

async fn expect_run_started<S: AsyncRead + Unpin>(stream: &mut S) -> Result<String> {
    let bytes = read_frame_bytes_async(stream)
        .await
        .context("reading stream handshake")?;
    match decode_wire_frame_from_bytes(&bytes).context("decoding stream handshake")? {
        WireFrame::Json(Response::RunStarted { run_id }) => Ok(run_id),
        WireFrame::Json(Response::Error { message }) => bail!("daemon error: {message}"),
        other => bail!("expected RunStarted, got {other:?}"),
    }
}

async fn drive_logs<S, O, E>(mut stream: S, stdout: &mut O, stderr: &mut E) -> Result<i32>
where
    S: AsyncRead + Unpin,
    O: AsyncWriteExt + Unpin,
    E: AsyncWriteExt + Unpin,
{
    loop {
        let bytes = read_frame_bytes_async(&mut stream)
            .await
            .context("reading logs frame")?;
        match decode_wire_frame_from_bytes(&bytes).context("decoding logs frame")? {
            WireFrame::Stdout(b) => {
                stdout.write_all(&b).await?;
                stdout.flush().await.ok();
            }
            WireFrame::Stderr(b) => {
                stderr.write_all(&b).await?;
                stderr.flush().await.ok();
            }
            WireFrame::Json(Response::Acknowledged) => return Ok(0),
            WireFrame::Json(Response::RunExit { .. }) => return Ok(0),
            WireFrame::Json(Response::Error { message }) => bail!("daemon error: {message}"),
            other => bail!("unexpected logs frame: {other:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_service::{CannedService, stream_with};
    use lns_ipc::encode_frame;

    fn parsed_command(argv: &[&str]) -> SandboxCommand {
        let mut full = vec!["lns", "sandbox"];
        full.extend_from_slice(argv);
        let args: SandboxArgs = crate::command::parse_args(full).unwrap();
        args.command
    }

    #[test]
    fn every_run_verb_refuses_a_document_operand_by_naming_the_artifact_reader() {
        for argv in [
            vec!["exec", "./lns.yaml", "sh"],
            vec!["kill", "./lns.yaml"],
            vec!["stop", "../lns.yaml"],
            vec!["start", "/tmp/lns.yaml"],
            vec!["inspect", "."],
            vec!["logs", "sub/lns.yaml"],
            vec!["attach", ".."],
            vec!["rm", "./lns.dev.yaml"],
        ] {
            let refusal = wrong_kind_refusal(&parsed_command(&argv))
                .unwrap_or_else(|| panic!("{argv:?} must refuse a document operand"));
            assert!(refusal.contains("takes a RUN"), "{argv:?}: {refusal}");
            assert!(
                refusal.contains("lns artifact inspect"),
                "{argv:?}: {refusal}"
            );
            assert!(
                refusal.contains(&format!("lns sandbox {}", argv[0])),
                "{argv:?}: {refusal}"
            );
        }
    }

    #[test]
    fn a_dotted_name_or_a_verb_without_a_run_operand_is_not_redirected() {
        assert_eq!(
            wrong_kind_refusal(&parsed_command(&["inspect", "v1.2-agent"])),
            None
        );
        assert_eq!(wrong_kind_refusal(&parsed_command(&["ls"])), None);
    }

    fn running_run() -> lns_ipc::RunSummary {
        lns_ipc::RunSummary {
            id: "1a2b3c4d0000000000000000000000aa".into(),
            name: "reviewer".into(),
            image: "some-image".into(),
            command: "cmd".into(),
            status: lns_ipc::RunStatus::Running,
            started: "2026-01-01T00:00:00Z".into(),
        }
    }

    fn ps_args() -> SandboxLsArgs {
        SandboxLsArgs {
            all: false,
            output: crate::output::OutputArgs {
                format: crate::output::Format::Table,
            },
        }
    }

    fn stop_args(run_id: u32) -> SandboxStopArgs {
        SandboxStopArgs {
            run: run_id.to_string(),
            timeout: 10,
        }
    }

    #[tokio::test]
    async fn run_with_writers_refuses_the_interactive_exec_verb() {
        let svc = CannedService::new(Response::Pong);
        let cmd = SandboxCommand::Exec(crate::cli::ExecArgs {
            run: "1".into(),
            interactive: false,
            tty: false,
            detach_keys: crate::cli::DetachChord(Vec::new()),
            quiet: false,
            cmd: vec!["echo".into()],
        });
        let mut out = Vec::new();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let err = run_with_writers(
            &cmd,
            &svc,
            TermInfo::default(),
            &mut std::io::empty(),
            &mut out,
            &mut stdout,
            &mut stderr,
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("interactive"), "got: {err:#}");
    }

    #[tokio::test]
    async fn run_with_writers_refuses_the_interactive_run_verb() {
        let svc = CannedService::new(Response::Pong);
        let run_args: RunArgs = crate::command::parse_args(["lns", "run", "alpine"]).unwrap();
        let cmd = SandboxCommand::Run(Box::new(run_args));
        let mut out = Vec::new();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let err = run_with_writers(
            &cmd,
            &svc,
            TermInfo::default(),
            &mut std::io::empty(),
            &mut out,
            &mut stdout,
            &mut stderr,
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("interactive"), "got: {err:#}");
    }

    #[tokio::test]
    async fn ps_renders_running_sandboxes_with_cpu_and_memory() {
        let svc = CannedService::with_stats(
            Response::RunList {
                runs: vec![running_run()],
            },
            Response::RunStats {
                stats: RunStatsInfo {
                    cpu_permille: 125,
                    mem_used_bytes: 92_274_688,
                    mem_total_bytes: 536_870_912,
                },
            },
        );
        let mut out = Vec::new();
        let code = ps(&svc, &ps_args(), &mut out).await.unwrap();
        assert_eq!(code, 0);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("CPU") && text.contains("MEM"), "got: {text}");
        assert!(text.contains("12.5%"), "got: {text}");
        assert!(text.contains("88.0 MiB"), "got: {text}");
    }

    #[tokio::test]
    async fn ps_renders_only_the_header_when_nothing_is_running() {
        let svc = CannedService::new(Response::RunList {
            runs: vec![lns_ipc::RunSummary {
                status: lns_ipc::RunStatus::Exited { code: 0 },
                ..running_run()
            }],
        });
        let mut out = Vec::new();
        let code = ps(&svc, &ps_args(), &mut out).await.unwrap();
        assert_eq!(code, 0);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("CPU"), "got: {text}");
        assert!(
            !text.contains("reviewer"),
            "an exited run must not list: {text}"
        );
    }

    #[tokio::test]
    async fn ps_surfaces_a_listing_error_and_rejects_an_unrelated_variant() {
        let err = ps(
            &CannedService::new(Response::Error {
                message: "registry poisoned".into(),
            }),
            &ps_args(),
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("registry poisoned"));

        let err = ps(
            &CannedService::new(Response::Pong),
            &ps_args(),
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("unexpected response"));
    }

    #[tokio::test]
    async fn ps_keeps_a_run_whose_guest_stopped_answering_the_stats_probe() {
        let mut out = Vec::new();
        let code = ps(
            &CannedService::with_stats(
                Response::RunList {
                    runs: vec![running_run()],
                },
                Response::Error {
                    message: "sampling guest stats failed: connect_to_guest_port(1029) timed out"
                        .into(),
                },
            ),
            &ps_args(),
            &mut out,
        )
        .await
        .expect("a dead guest must not fail the whole listing");
        let text = String::from_utf8(out).expect("utf8");
        assert_eq!(code, 0);
        let row = text
            .lines()
            .find(|l| l.contains("reviewer"))
            .unwrap_or_else(|| panic!("row lost: {text}"));
        let cells: Vec<&str> = row.split_whitespace().collect();
        assert_eq!(
            &cells[cells.len() - 2..],
            [NO_SAMPLE, NO_SAMPLE],
            "an unsampled CPU and MEM must each render as a cell, or the trailing-empty-cell \
             trim shortens the row and misaligns it under the header: {row}"
        );
        assert!(
            !text.contains("sampling guest stats failed"),
            "scripted cleanup must stay quiet: {text}"
        );
    }

    #[tokio::test]
    async fn ps_rejects_a_stats_variant_that_is_neither_a_sample_nor_an_error() {
        let err = ps(
            &CannedService::with_stats(
                Response::RunList {
                    runs: vec![running_run()],
                },
                Response::Pong,
            ),
            &ps_args(),
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("unexpected response"));
    }

    #[tokio::test]
    async fn kill_surfaces_a_daemon_error() {
        let svc = CannedService::new(Response::Error {
            message: "no active session for run 1".into(),
        });
        let mut out = Vec::new();
        let err = kill(
            &svc,
            &crate::cli::KillArgs {
                run: "1".into(),
                signal: "TERM".into(),
            },
            &mut out,
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("no active session"));
    }

    #[tokio::test]
    async fn kill_rejects_an_unrelated_response_variant() {
        let svc = CannedService::new(Response::Pong);
        let mut out = Vec::new();
        let err = kill(
            &svc,
            &crate::cli::KillArgs {
                run: "1".into(),
                signal: "TERM".into(),
            },
            &mut out,
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("unexpected response"));
    }

    #[tokio::test]
    async fn stop_rejects_an_unrelated_response_variant() {
        let svc = CannedService::new(Response::Pong);
        let mut out = Vec::new();
        let err = stop(&svc, &stop_args(1), &mut out).await.unwrap_err();
        assert!(format!("{err:#}").contains("unexpected response"));
    }

    #[tokio::test]
    async fn prune_surfaces_the_services_refusal_and_rejects_an_unrelated_answer() {
        let svc = CannedService::new(Response::Error {
            message: "runs dir unreadable".into(),
        });
        let err = prune(
            &svc,
            &SandboxPruneArgs { force: true },
            TermInfo::default(),
            &mut std::io::empty(),
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("runs dir unreadable"));

        let svc = CannedService::new(Response::Pong);
        let err = prune(
            &svc,
            &SandboxPruneArgs { force: true },
            TermInfo::default(),
            &mut std::io::empty(),
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("unexpected response"));
    }

    #[tokio::test]
    async fn stopped_run_names_skip_the_running_and_fall_back_to_the_short_id() {
        let svc = CannedService::new(Response::RunList {
            runs: vec![
                lns_ipc::RunSummary {
                    id: format!("{:032x}", 0xbeefu32),
                    name: String::new(),
                    image: "some-image".into(),
                    command: "cmd".into(),
                    status: lns_ipc::RunStatus::Exited { code: 0 },
                    started: "2026-01-01T00:00:00Z".into(),
                },
                running_run(),
                lns_ipc::RunSummary {
                    id: format!("{:032x}", 3),
                    name: "scribe".into(),
                    image: "some-image".into(),
                    command: "cmd".into(),
                    status: lns_ipc::RunStatus::Exited { code: 1 },
                    started: "2026-01-01T00:00:00Z".into(),
                },
            ],
        });
        let names = stopped_run_names(&svc).await.unwrap();
        assert_eq!(
            names,
            vec![
                lns_ipc::short_run_id(&format!("{:032x}", 0xbeefu32)).to_string(),
                "scribe".to_string()
            ]
        );
    }

    #[tokio::test]
    async fn stopped_run_listing_surfaces_a_daemon_error_and_rejects_an_unrelated_variant() {
        let refused = CannedService::new(Response::Error {
            message: "runs dir unreadable".into(),
        });
        let err = stopped_run_names(&refused).await.unwrap_err();
        assert!(format!("{err:#}").contains("runs dir unreadable"));

        let odd = CannedService::new(Response::Pong);
        let err = stopped_run_names(&odd).await.unwrap_err();
        assert!(format!("{err:#}").contains("unexpected response"));
    }

    #[tokio::test]
    async fn rm_asks_the_service_to_remove_the_sandbox_and_names_it_back() {
        let svc = CannedService::new(Response::Acknowledged);
        let mut out = Vec::new();
        let code = rm(
            &svc,
            &SandboxRmArgs {
                run: "reviewer".into(),
                force: false,
            },
            &mut out,
        )
        .await
        .unwrap();
        assert_eq!(code, 0);
        assert!(
            String::from_utf8(out)
                .unwrap()
                .contains("removed sandbox reviewer"),
            "the answer names what it acted on"
        );
        assert!(matches!(
            svc.requests.lock().unwrap().as_slice(),
            [Request::RemoveRun { force: false, .. }]
        ));
    }

    #[tokio::test]
    async fn rm_f_asks_the_service_to_stop_it_first() {
        let svc = CannedService::new(Response::Acknowledged);
        let mut out = Vec::new();
        rm(
            &svc,
            &SandboxRmArgs {
                run: "reviewer".into(),
                force: true,
            },
            &mut out,
        )
        .await
        .unwrap();
        assert!(
            matches!(
                svc.requests.lock().unwrap().as_slice(),
                [Request::RemoveRun { force: true, .. }]
            ),
            "-f is the service's to act on: only it knows whether the sandbox is still running"
        );
    }

    #[tokio::test]
    async fn rm_surfaces_the_services_refusal_and_rejects_an_unrelated_answer() {
        let svc = CannedService::new(Response::Error {
            message: "reviewer is running; stop it first, or pass -f".into(),
        });
        let err = rm(
            &svc,
            &SandboxRmArgs {
                run: "reviewer".into(),
                force: false,
            },
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("is running"));

        let svc = CannedService::new(Response::Pong);
        let err = rm(
            &svc,
            &SandboxRmArgs {
                run: "reviewer".into(),
                force: false,
            },
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("unexpected response"));
    }

    #[tokio::test]
    async fn inspect_rejects_an_unrelated_response_variant() {
        let svc = CannedService::new(Response::Pong);
        let mut out = Vec::new();
        let err = inspect(&svc, "1", crate::output::Format::Json, &mut out)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("unexpected response"));
    }

    #[tokio::test]
    async fn inspect_surfaces_a_daemon_error() {
        let svc = CannedService::new(Response::Error {
            message: "no active run with id 1".into(),
        });
        let mut out = Vec::new();
        let err = inspect(&svc, "1", crate::output::Format::Table, &mut out)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("no active run with id 1"));
    }

    #[test]
    fn format_permille_renders_tenths_of_a_percent() {
        assert_eq!(format_permille(0), "0.0%");
        assert_eq!(format_permille(125), "12.5%");
        assert_eq!(format_permille(1000), "100.0%");
    }

    #[tokio::test]
    async fn handshake_rejects_an_unexpected_first_frame() {
        let frame = lns_ipc::encode_wire_frame(&WireFrame::Stdout(b"early".to_vec())).unwrap();
        let mut stream = stream_with(&[frame]).await;
        let err = expect_run_started(&mut stream).await.unwrap_err();
        assert!(format!("{err:#}").contains("expected RunStarted"));
    }

    #[tokio::test]
    async fn drive_logs_rejects_an_unexpected_json_frame() {
        let frame = encode_frame(&Response::Pong).unwrap();
        let stream = stream_with(&[frame]).await;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let err = drive_logs(stream, &mut stdout, &mut stderr)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("unexpected logs frame"));
    }

    #[tokio::test]
    async fn drive_logs_surfaces_a_mid_stream_daemon_error() {
        let frame = encode_frame(&Response::Error {
            message: "buffer dropped".into(),
        })
        .unwrap();
        let stream = stream_with(&[frame]).await;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let err = drive_logs(stream, &mut stdout, &mut stderr)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("buffer dropped"));
    }

    #[tokio::test]
    async fn drive_logs_writes_stderr_chunks_to_the_stderr_writer() {
        let frames = [
            lns_ipc::encode_wire_frame(&WireFrame::Stderr(b"warning!".to_vec())).unwrap(),
            encode_frame(&Response::Acknowledged).unwrap(),
        ];
        let stream = stream_with(&frames).await;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = drive_logs(stream, &mut stdout, &mut stderr).await.unwrap();
        assert_eq!(code, 0);
        assert_eq!(stderr, b"warning!");
        assert!(stdout.is_empty());
    }

    #[tokio::test]
    async fn logs_surfaces_a_failure_to_open_the_stream() {
        let svc = CannedService::with_frames(Vec::new());
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let err = logs(
            &svc,
            &SandboxLogsArgs {
                run: "1".into(),
                follow: false,
            },
            &mut stdout,
            &mut stderr,
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("refused the stream"));
    }

    #[tokio::test]
    async fn attach_relays_output_and_adopts_the_workloads_exit_code() {
        let svc = CannedService::with_frames(vec![
            encode_frame(&Response::RunStarted {
                run_id: "5e6f7a8b0000000000000000000000bb".to_string(),
            })
            .unwrap(),
            lns_ipc::encode_wire_frame(&WireFrame::Stdout(b"live".to_vec())).unwrap(),
            encode_frame(&Response::RunExit { code: 4 }).unwrap(),
        ]);
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = attach(
            &svc,
            &SandboxAttachArgs {
                run: "9".into(),
                detach_keys: crate::cli::DetachChord(Vec::new()),
            },
            TermInfo::default(),
            &mut stdout,
            &mut stderr,
        )
        .await
        .unwrap();
        assert_eq!(code, 4);
        assert_eq!(stdout, b"live");
        assert!(
            svc.aux_socket().is_none(),
            "the canned service offers no aux socket"
        );
    }

    #[tokio::test]
    async fn inspect_marks_the_policy_unreadable_when_the_loader_finds_nothing() {
        let svc = CannedService::new(Response::RunInspect {
            details: Box::new(RunDetails {
                summary: lns_ipc::RunSummary {
                    id: "1a2b3c4d0000000000000000000000aa".into(),
                    name: "reviewer".into(),
                    image: "some-image".into(),
                    command: String::new(),
                    status: lns_ipc::RunStatus::Running,
                    started: "2026-01-01T00:00:00Z".into(),
                },
                config: lns_ipc::RunConfig {
                    policy_path: Some("/work/lns-local-mixin.yaml".into()),
                    ..Default::default()
                },
            }),
        });
        let mut out = Vec::new();
        let code = inspect(&svc, "1", crate::output::Format::Json, &mut out)
            .await
            .unwrap();
        assert_eq!(code, 0);
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("policy file could not be read"),
            "got: {text}"
        );
    }

    #[test]
    fn policy_doc_marks_an_unreadable_file() {
        let doc = policy_doc("/work/lns-local-mixin.yaml", None);
        assert_eq!(doc["path"], "/work/lns-local-mixin.yaml");
        assert!(doc["error"].as_str().unwrap().contains("could not be read"));
    }

    #[test]
    fn policy_doc_embeds_a_parsed_policy() {
        let doc = policy_doc(
            "/work/lns-local-mixin.yaml",
            Some(serde_json::json!({"network": {"egress": {"http": []}}})),
        );
        assert_eq!(
            doc["policy"]["network"]["egress"]["http"],
            serde_json::json!([])
        );
    }
}
