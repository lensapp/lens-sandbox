use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use lns_ipc::{
    Request, Response, RunDetails, RunStatsInfo, WireFrame, decode_wire_frame_from_bytes,
    read_frame_bytes_async,
};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};

use crate::cli::{DetachChord, ExecArgs, KillArgs, RunArgs, parse_detach_keys_arg};
use crate::command::{CommandSpec, subcommand};
use crate::service::client::BoxFuture;

pub mod author;
pub mod distribute;
pub mod fileset;
pub mod real;
#[cfg(test)]
pub(crate) mod test_support;

#[derive(clap::Args)]
pub struct SandboxArgs {
    #[command(subcommand)]
    pub command: SandboxCommand,
}

#[derive(clap::Subcommand)]
pub enum SandboxCommand {
    #[command(about = "Scaffold a default ./lns.yaml (kind: sandbox) in this directory.")]
    Init,
    #[command(about = "Validate ./lns.yaml — schema, cross-field, and secret checks, offline.")]
    Validate(SandboxValidateArgs),
    #[command(
        about = "Build ./lns.yaml and upload it to a registry as a sandbox artifact, in one step."
    )]
    Push(SandboxPushArgs),
    #[command(about = "Fetch a published sandbox and its base image into the local cache.")]
    Pull(SandboxPullArgs),
    #[command(about = "Add a tag to a cached sandbox within its current repository.")]
    Tag(SandboxTagArgs),
    #[command(about = "List running sandboxes with their CPU and memory (`docker ps`-style).")]
    Ps(PsArgs),
    #[command(
        visible_alias = "list",
        about = "List cached sandboxes (pulled or built) in the local store."
    )]
    Ls(LsArgs),
    #[command(about = "Run a sandbox in a microVM (the top-level `lns run`).")]
    Run(Box<RunArgs>),
    #[command(about = "Run another command against a running run.")]
    Exec(ExecArgs),
    #[command(about = "Send a signal to a running run (`docker kill`-style).")]
    Kill(KillArgs),
    #[command(about = "Stop a run gracefully: SIGTERM, then SIGKILL once the timeout passes.")]
    Stop(SandboxStopArgs),
    #[command(about = "Print a run's captured output; `-f` streams until the run exits.")]
    Logs(SandboxLogsArgs),
    #[command(about = "Re-attach to a running run's output (detach chord to leave again).")]
    Attach(SandboxAttachArgs),
    #[command(
        about = "Inspect a sandbox: a running run's live state as JSON, a cached artifact's kind and definition, or a local lns.yaml's effective definition (offline)."
    )]
    Inspect(SandboxInspectArgs),
    #[command(
        about = "Remove a cached sandbox and free its now-unreferenced layers; refuses a running one."
    )]
    Rm(SandboxRmArgs),
    #[command(
        about = "Remove every cached sandbox not held by a running one and reclaim the provisioned tool cache when no sandbox is live."
    )]
    Prune(SandboxPruneArgs),
}

#[derive(clap::Args)]
pub struct PsArgs {
    #[command(flatten)]
    pub output: crate::output::OutputArgs,
}

#[derive(clap::Args)]
pub struct LsArgs {
    #[command(flatten)]
    pub output: crate::output::OutputArgs,
}

#[derive(clap::Args)]
pub struct SandboxValidateArgs {
    #[arg(
        short = 'f',
        long = "file",
        value_name = "FILE",
        help = "Definition file to validate instead of ./lns.yaml; its directory roots the definition's relative filesets."
    )]
    pub file: Option<PathBuf>,
}

#[derive(clap::Args)]
pub struct SandboxPushArgs {
    #[arg(
        value_name = "REF",
        help = "Registry reference to publish the sandbox at, e.g. ghcr.io/team/hermes:1.4.0; a bare reference resolves against the `run.registry` default, else hub.lns.run."
    )]
    pub reference: String,
    #[arg(
        long = "dry-run",
        help = "Validate, pack, and build everything push would upload, print the digests, and upload nothing."
    )]
    pub dry_run: bool,
    #[arg(
        short = 'f',
        long = "file",
        value_name = "FILE",
        help = "Definition file to publish instead of ./lns.yaml; its directory roots the definition's relative filesets."
    )]
    pub file: Option<PathBuf>,
}

#[derive(clap::Args)]
pub struct SandboxPullArgs {
    #[arg(
        value_name = "REF",
        help = "Published sandbox reference to fetch, e.g. ghcr.io/team/hermes:1.4.0; a bare reference resolves against the `run.registry` default, else hub.lns.run."
    )]
    pub reference: String,
    #[arg(
        long = "yes",
        help = "Accept publisher-declared tool installation without prompting."
    )]
    pub assume_yes: bool,
}

#[derive(clap::Args)]
pub struct SandboxTagArgs {
    #[arg(value_name = "SOURCE", help = "Cached sandbox to re-reference.")]
    pub from: String,

    #[arg(
        value_name = "TARGET",
        help = "New tag in the source sandbox's registry and repository."
    )]
    pub to: String,
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
        value_name = "TARGET",
        help = "A running run's id/name (live state), a cached sandbox reference (its definition), or a path to a local definition (., lns.yaml, ./dir, ./lns.dev.yaml — rendered offline). Omit to render ./lns.yaml."
    )]
    pub run: Option<String>,

    #[arg(
        short = 'f',
        long = "file",
        value_name = "FILE",
        conflicts_with = "run",
        help = "Definition file to render instead of ./lns.yaml, offline. Cannot be combined with TARGET."
    )]
    pub file: Option<PathBuf>,

    #[arg(
        long = "mixin",
        value_name = "REF",
        help = "Resolve this mixin into the sandbox before rendering it, as `lns run --mixin` would. Repeatable."
    )]
    pub mixins: Vec<String>,
}

#[derive(clap::Args)]
pub struct SandboxRmArgs {
    #[arg(
        value_name = "REF",
        help = "Cached sandbox reference (or a running sandbox's id/name, which is refused)."
    )]
    pub run: String,
}

#[derive(clap::Args)]
pub struct SandboxPruneArgs {
    #[arg(
        short = 'f',
        long,
        default_value_t = false,
        help = "Required: confirm removing unused cached sandboxes and, when none is live, the provisioned tool cache."
    )]
    pub force: bool,
}

/// Qualifies the registry coordinate a distribution verb addresses with `registry`, or the built-in default when nothing is configured; the local-cache verbs take an id-or-ref the service resolves, so they are left alone.
pub fn apply_registry_default(command: &mut SandboxCommand, registry: Option<&str>) {
    let reference = match command {
        SandboxCommand::Push(args) => &mut args.reference,
        SandboxCommand::Pull(args) => &mut args.reference,
        _ => return,
    };
    *reference = crate::config::resolve_default_registry(reference, registry);
}

pub fn augment(app: clap::Command) -> clap::Command {
    app.subcommand(
        subcommand::<SandboxArgs>("sandbox")
            .about("The sandbox — author, distribute, run, and manage it (the complete surface).")
            .mut_subcommand("run", crate::run::long_help_only),
    )
}

/// Only four of the parent's verbs drive the tty; the rest prompt, so they need the dispatcher's real stdin instead of the `io::empty()` a terminal owner is handed.
fn verb_owns_terminal(sub: &clap::ArgMatches) -> bool {
    matches!(
        sub.subcommand_name(),
        Some("run" | "exec" | "logs" | "attach")
    )
}

pub const SPEC: CommandSpec = CommandSpec {
    name: "sandbox",
    augment,
    run: real::run,
    announces_update_check: true,
    owns_terminal: verb_owns_terminal,
};

pub fn augment_init(app: clap::Command) -> clap::Command {
    app.subcommand(
        clap::Command::new("init")
            .about("Scaffold a default ./lns.yaml (shortcut for `lns sandbox init`)."),
    )
}

pub const INIT_SPEC: CommandSpec = CommandSpec {
    name: "init",
    augment: augment_init,
    run: real::run_init,
    announces_update_check: true,
    owns_terminal: crate::command::never_owns_terminal,
};

pub fn augment_ps(app: clap::Command) -> clap::Command {
    app.subcommand(
        subcommand::<PsArgs>("ps").about(
            "List running sandboxes with their CPU and memory (shortcut for `lns sandbox ps`).",
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
    augment_rm,
    RM_SPEC,
    SandboxRmArgs,
    "rm",
    real::run_rm,
    "Remove a sandbox (shortcut for `lns sandbox rm`)."
);
shortcut_spec!(
    augment_inspect,
    INSPECT_SPEC,
    SandboxInspectArgs,
    "inspect",
    real::run_inspect,
    "Inspect a sandbox (shortcut for `lns sandbox inspect`)."
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
shortcut_spec!(
    augment_push,
    PUSH_SPEC,
    SandboxPushArgs,
    "push",
    real::run_push,
    "Build and publish ./lns.yaml as a sandbox artifact (shortcut for `lns sandbox push`)."
);
shortcut_spec!(
    augment_pull,
    PULL_SPEC,
    SandboxPullArgs,
    "pull",
    real::run_pull,
    "Fetch a published sandbox into the cache (shortcut for `lns sandbox pull`)."
);
shortcut_spec!(
    augment_tag,
    TAG_SPEC,
    SandboxTagArgs,
    "tag",
    real::run_tag,
    "Re-reference a cached sandbox (shortcut for `lns sandbox tag`)."
);

pub trait SandboxService: Send + Sync {
    type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static;
    fn one_shot(&self, request: Request) -> BoxFuture<'_, Result<Response>>;
    fn open_stream(&self, request: Request) -> BoxFuture<'_, Result<Self::Stream>>;
    fn aux_socket(&self) -> Option<PathBuf>;
    fn load_policy(&self, path: &str) -> Option<serde_json::Value>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct TermInfo {
    pub stdin_is_tty: bool,
    pub stdout_is_terminal: bool,
}

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
    match cmd {
        SandboxCommand::Init | SandboxCommand::Validate(_) => {
            bail!("author commands run offline, not through the service dispatch")
        }
        SandboxCommand::Push(_) => {
            bail!("push builds and uploads locally, not through the service dispatch")
        }
        SandboxCommand::Pull(args) => pull(svc, args, term, input, out, stderr).await,
        SandboxCommand::Tag(args) => tag(svc, args, out).await,
        SandboxCommand::Ps(args) => ps(svc, args, out).await,
        SandboxCommand::Ls(args) => ls(svc, args, out).await,
        SandboxCommand::Kill(args) => kill(svc, args, out).await,
        SandboxCommand::Run(_) => bail!("sandbox run is dispatched on its own interactive path"),
        SandboxCommand::Exec(_) => bail!("sandbox exec is dispatched on its own interactive path"),
        SandboxCommand::Stop(args) => stop(svc, args, out).await,
        SandboxCommand::Inspect(args) => {
            let Some(target) = &args.run else {
                bail!("a local definition inspect runs offline, not through the service dispatch")
            };
            inspect(svc, target, &args.mixins, out).await
        }
        SandboxCommand::Logs(args) => logs(svc, args, stdout, stderr).await,
        SandboxCommand::Attach(args) => attach(svc, args, term, stdout, stderr).await,
        SandboxCommand::Rm(args) => rm(svc, args, out).await,
        SandboxCommand::Prune(args) => prune(svc, args, out).await,
    }
}

pub(crate) fn run_label(run: &str) -> String {
    run.to_string()
}

async fn pull<I, W, E>(
    svc: &impl SandboxService,
    args: &SandboxPullArgs,
    term: TermInfo,
    input: &mut I,
    out: &mut W,
    stderr: &mut E,
) -> Result<i32>
where
    I: std::io::BufRead,
    W: std::io::Write,
    E: AsyncWriteExt + Unpin,
{
    let inspection = svc
        .one_shot(Request::InspectImage {
            image: args.reference.clone(),
            mixins: Vec::new(),
            decisions: None,
        })
        .await?;
    // A mixin pull installs nothing: it caches documents, so there is no effect to consent to and its tools are disclosed where they are installed.
    let (digest, tools) = match inspection {
        Response::ImageInspected {
            inspection: lns_ipc::ArtifactInspection::Sandbox(view),
        } if !view.digest.is_empty() => (view.digest, view.tools),
        Response::ImageInspected {
            inspection: lns_ipc::ArtifactInspection::Mixin(view),
        } if !view.digest.is_empty() => (view.digest, Vec::new()),
        Response::ImageInspected {
            inspection:
                lns_ipc::ArtifactInspection::Mixin(_) | lns_ipc::ArtifactInspection::Sandbox(_),
        } => bail!(
            "the registry did not provide a digest for {}",
            args.reference
        ),
        Response::ImageInspected {
            inspection: lns_ipc::ArtifactInspection::Image(_),
        } => bail!(
            "{} is an OCI image, not a published sandbox",
            args.reference
        ),
        Response::Error { message } => bail!("daemon error: {message}"),
        other => bail!("unexpected response from daemon: {other:?}"),
    };
    crate::run::pull_confirm::confirm_pulled_effects(
        &crate::run::pull_confirm::PulledEffects {
            reference: &args.reference,
            binds: &[],
            volumes: &[],
            filesets: &[],
            tools: &tools,
            // A pull runs no script: scripts run at boot, and `lns run` discloses them there.
            scripts: &[],
        },
        args.assume_yes,
        term.stdin_is_tty,
        input,
        out,
    )?;
    let response = svc
        .one_shot(Request::PullImage {
            image: args.reference.clone(),
            expected_digest: digest,
        })
        .await?;
    match response {
        Response::MixinPulled {
            reference,
            digest,
            cached_mixins,
        } => {
            writeln!(out, "pulled {reference}")?;
            writeln!(out, "digest: {digest}")?;
            if cached_mixins > 0 {
                writeln!(out, "cached {cached_mixins} mixin(s) it layers on")?;
            }
            Ok(0)
        }
        Response::ImagePulled { image, warnings } => {
            writeln!(out, "pulled {}", image.reference)?;
            writeln!(out, "digest: {}", image.digest)?;
            for warning in warnings {
                stderr
                    .write_all(format!("warning: {warning}\n").as_bytes())
                    .await?;
                stderr.flush().await?;
            }
            Ok(0)
        }
        Response::Error { message } => bail!("daemon error: {message}"),
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

async fn tag<W: std::io::Write>(
    svc: &impl SandboxService,
    args: &SandboxTagArgs,
    out: &mut W,
) -> Result<i32> {
    let response = svc
        .one_shot(Request::TagImage {
            from: args.from.clone(),
            to: args.to.clone(),
        })
        .await?;
    match response {
        Response::ImageTagged { from, to } => {
            writeln!(out, "tagged {from} as {to}")?;
            Ok(0)
        }
        Response::Error { message } => bail!("daemon error: {message}"),
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

async fn ps<W: std::io::Write>(
    svc: &impl SandboxService,
    args: &PsArgs,
    out: &mut W,
) -> Result<i32> {
    let running = match svc.one_shot(Request::ListRuns).await? {
        Response::RunList { runs } => runs
            .into_iter()
            .filter(|r| matches!(r.status, lns_ipc::RunStatus::Running))
            .collect::<Vec<_>>(),
        Response::Error { message } => bail!("daemon error: {message}"),
        other => bail!("unexpected response from daemon: {other:?}"),
    };
    let mut rows = Vec::with_capacity(running.len());
    for run in &running {
        let stats = match svc
            .one_shot(Request::RunStats {
                run: run.id.clone(),
            })
            .await?
        {
            Response::RunStats { stats } => Some(stats),
            Response::Error { .. } => None,
            other => bail!("unexpected response from daemon: {other:?}"),
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
                format!("{} / {}", format_bytes(used), format_bytes(total))
            }
            _ => NO_SAMPLE.to_string(),
        }
    }
}

impl crate::output::TableRow for PsRow {
    const HEADERS: &'static [&'static str] = &["ID", "NAME", "IMAGE", "CPU %", "MEM"];

    fn cells(&self) -> Vec<String> {
        vec![
            lns_ipc::short_run_id(&self.id).to_string(),
            self.name.clone(),
            self.image.clone(),
            self.cpu_permille
                .map_or_else(|| NO_SAMPLE.to_string(), format_permille),
            self.memory_cell(),
        ]
    }
}

async fn ls<W: std::io::Write>(
    svc: &impl SandboxService,
    args: &LsArgs,
    out: &mut W,
) -> Result<i32> {
    match svc.one_shot(Request::ListImages).await? {
        Response::ImageList { mut images } => {
            images.sort_by(|a, b| a.reference.cmp(&b.reference));
            let rows: Vec<SandboxRow> = images.iter().map(SandboxRow::new).collect();
            crate::output::emit(args.output.format, &rows, out)?;
            Ok(0)
        }
        Response::Error { message } => bail!("daemon error: {message}"),
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SandboxRow {
    reference: String,
    digest: String,
    size_bytes: u64,
    layers: u32,
    pulled: String,
    in_use_by: Option<String>,
}

impl SandboxRow {
    fn new(image: &lns_ipc::ImageInfo) -> Self {
        Self {
            reference: image.reference.clone(),
            digest: image.digest.clone(),
            size_bytes: image.size_bytes,
            layers: image.layers,
            pulled: image.pulled.clone(),
            in_use_by: image.in_use_by.clone(),
        }
    }
}

impl crate::output::TableRow for SandboxRow {
    const HEADERS: &'static [&'static str] = &["SANDBOX", "STATE"];

    fn cells(&self) -> Vec<String> {
        vec![self.reference.clone(), "cached".to_string()]
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

async fn rm<W: std::io::Write>(
    svc: &impl SandboxService,
    args: &SandboxRmArgs,
    out: &mut W,
) -> Result<i32> {
    match svc
        .one_shot(Request::InspectRun {
            run: args.run.clone(),
        })
        .await?
    {
        Response::RunInspect { details }
            if matches!(details.summary.status, lns_ipc::RunStatus::Running) =>
        {
            bail!(
                "{} is a running sandbox; stop it first with `lns stop {}`",
                args.run,
                args.run
            )
        }
        // An exited run is a spent record.
        Response::RunInspect { .. } => remove_run(svc, &args.run, out).await,
        // A reference the service knows no run for is a cached sandbox; any other error is a real failure, not a miss.
        Response::Error { message } if is_unknown_run(&message) => {
            remove_cached(svc, &args.run, out).await
        }
        Response::Error { message } => bail!("daemon error: {message}"),
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

async fn remove_run<W: std::io::Write>(
    svc: &impl SandboxService,
    run: &str,
    out: &mut W,
) -> Result<i32> {
    match svc
        .one_shot(Request::RemoveRun {
            run: run.to_string(),
        })
        .await?
    {
        Response::Acknowledged => {
            writeln!(out, "removed run {run}")?;
            Ok(0)
        }
        Response::Error { message } => bail!("daemon error: {message}"),
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

async fn remove_cached<W: std::io::Write>(
    svc: &impl SandboxService,
    reference: &str,
    out: &mut W,
) -> Result<i32> {
    match svc
        .one_shot(Request::RemoveImage {
            image: reference.to_string(),
        })
        .await?
    {
        Response::ImageRemoved {
            reference,
            reclaimed_bytes,
        } => {
            writeln!(out, "removed {reference}")?;
            writeln!(
                out,
                "freed {} of base-image layers",
                format_bytes(reclaimed_bytes)
            )?;
            Ok(0)
        }
        Response::Error { message } => bail!("daemon error: {message}"),
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

async fn prune<W: std::io::Write>(
    svc: &impl SandboxService,
    args: &SandboxPruneArgs,
    out: &mut W,
) -> Result<i32> {
    if !args.force {
        bail!(
            "this removes every exited run's scratch space, every cached sandbox not held by a running one and, when none is live, the provisioned tool cache; pass --force to confirm"
        );
    }
    let (mut removed_runs, runs_bytes) = match svc.one_shot(Request::PruneRuns).await? {
        Response::RunsPruned {
            removed,
            reclaimed_bytes,
        } => (removed, reclaimed_bytes),
        Response::Error { message } => bail!("daemon error: {message}"),
        other => bail!("unexpected response from daemon: {other:?}"),
    };
    removed_runs.sort_unstable();
    for run in &removed_runs {
        writeln!(out, "removed run {run}")?;
    }
    match svc.one_shot(Request::PruneImages).await? {
        Response::ImagesPruned {
            mut removed,
            reclaimed_bytes,
        } => {
            removed.sort_unstable();
            for reference in &removed {
                writeln!(out, "removed {reference}")?;
            }
            writeln!(
                out,
                "reclaimed {}",
                format_bytes(runs_bytes + reclaimed_bytes)
            )?;
            Ok(0)
        }
        Response::Error { message } => bail!("daemon error: {message}"),
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

/// `--mixin` composes a document before it boots, and neither a run that has already booted nor a file rendered offline can honour it.
pub fn refuse_mixins_unless_published(mixins: &[String]) -> Result<()> {
    if mixins.is_empty() {
        return Ok(());
    }
    bail!(
        "--mixin applies to a sandbox reference: a live run has already booted with what it merged, and a local definition renders offline, without resolving anything"
    )
}

async fn inspect<W: std::io::Write>(
    svc: &impl SandboxService,
    target: &str,
    mixins: &[String],
    out: &mut W,
) -> Result<i32> {
    match svc
        .one_shot(Request::InspectRun {
            run: target.to_string(),
        })
        .await?
    {
        Response::RunInspect { details } => {
            refuse_mixins_unless_published(mixins)?;
            let policy = details
                .config
                .policy_path
                .as_deref()
                .map(|path| policy_doc(path, svc.load_policy(path)));
            render_inspect(&details, policy, out)?;
            Ok(0)
        }
        // A reference the service knows no run for is a cached artifact; any other error is a real failure, not a miss.
        Response::Error { message } if is_unknown_run(&message) => {
            inspect_cached(svc, target, mixins, out).await
        }
        Response::Error { message } => bail!("daemon error: {message}"),
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

async fn inspect_cached<W: std::io::Write>(
    svc: &impl SandboxService,
    reference: &str,
    mixins: &[String],
    out: &mut W,
) -> Result<i32> {
    match svc
        .one_shot(Request::InspectImage {
            image: reference.to_string(),
            mixins: mixins.to_vec(),
            decisions: None,
        })
        .await?
    {
        Response::ImageInspected { inspection } => {
            render_cached_inspect(&inspection, mixins, out)?;
            Ok(0)
        }
        Response::Error { message } => bail!("daemon error: {message}"),
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

fn render_cached_inspect<W: std::io::Write>(
    inspection: &lns_ipc::ArtifactInspection,
    typed: &[String],
    out: &mut W,
) -> Result<()> {
    match inspection {
        lns_ipc::ArtifactInspection::Sandbox(view) => {
            writeln!(out, "kind: sandbox")?;
            writeln!(out, "reference: {}", view.reference)?;
            if !view.digest.is_empty() {
                writeln!(out, "digest: {}", view.digest)?;
            }
            writeln!(out, "image: {}", view.image)?;
            for mixin in
                crate::run::summary::mixin_display(&view.mixins, typed, &view.pinned_mixins)
            {
                writeln!(out, "mixin: {mixin}")?;
            }
            if let Some(user) = &view.user {
                writeln!(out, "user: {user}")?;
            }
            if let Some(workdir) = &view.workdir {
                writeln!(out, "workdir: {workdir}")?;
            }
            for entry in &view.env {
                writeln!(out, "env: {entry}")?;
            }
            render_mounts(out, &view.mounts)?;
            if !view.ports.is_empty() {
                writeln!(out, "ports: {}", declared_ports_line(&view.ports))?;
            }
            render_filesets(out, &view.filesets)?;
            render_connectors(out, &view.connectors)?;
            for credential in &view.credentials {
                writeln!(out, "credential: {}", credential_disclosure(credential))?;
            }
            for tool in &view.tools {
                writeln!(out, "tool: {tool}")?;
            }
            render_scripts(out, &view.scripts)?;
            render_policy_flags(out, &view.policy_flags)?;
        }
        lns_ipc::ArtifactInspection::Mixin(view) => {
            writeln!(out, "kind: mixin")?;
            writeln!(out, "reference: {}", view.reference)?;
            if !view.digest.is_empty() {
                writeln!(out, "digest: {}", view.digest)?;
            }
            for mixin in &view.mixins {
                writeln!(out, "mixin: {mixin}")?;
            }
            for entry in &view.env {
                writeln!(out, "env: {entry}")?;
            }
            render_mounts(out, &view.mounts)?;
            if !view.ports.is_empty() {
                writeln!(out, "ports: {}", declared_ports_line(&view.ports))?;
            }
            render_filesets(out, &view.filesets)?;
            for credential in &view.credentials {
                writeln!(out, "credential: {}", credential_disclosure(credential))?;
            }
            for tool in &view.tools {
                writeln!(out, "tool: {tool}")?;
            }
            render_scripts(out, &view.scripts)?;
            render_policy_flags(out, &view.policy_flags)?;
        }
        lns_ipc::ArtifactInspection::Image(view) => {
            writeln!(out, "kind: image")?;
            writeln!(out, "reference: {}", view.reference)?;
            writeln!(out, "digest: {}", view.digest)?;
        }
    }
    Ok(())
}

/// One renderer for both kinds, since a mixin's mounts read exactly as a sandbox's do.
fn render_mounts<W: std::io::Write>(out: &mut W, mounts: &[lns_ipc::SandboxMount]) -> Result<()> {
    for mount in mounts {
        let kind = match mount.kind {
            lns_ipc::SandboxMountKind::Bind => "bind",
            lns_ipc::SandboxMountKind::Volume => "volume",
        };
        let mode = if mount.read_only { " (read-only)" } else { "" };
        writeln!(
            out,
            "mount: {kind} {} -> {}{mode}",
            mount.source, mount.target
        )?;
    }
    Ok(())
}

fn render_filesets<W: std::io::Write>(
    out: &mut W,
    filesets: &[lns_ipc::SandboxFileset],
) -> Result<()> {
    for fileset in filesets {
        let source = crate::run::summary::fileset_view_source_display(fileset);
        let owner = crate::run::summary::fileset_view_owner_display(fileset.owner);
        writeln!(
            out,
            "fileset: {source} -> {} (owner: {owner})",
            fileset.guest_path
        )?;
    }
    Ok(())
}

fn declared_ports_line(ports: &[lns_ipc::SandboxPort]) -> String {
    ports
        .iter()
        .map(|port| match port.host {
            Some(host) => format!("{host}:{}", port.container),
            None => port.container.to_string(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// What a declared credential discloses: the variable the workload reads, and every destination its value may travel to — the two things a reader has to weigh before running it.
pub(crate) fn credential_disclosure(credential: &lns_spec::Credential) -> String {
    if credential.injections.is_empty() {
        return format!("{} (travels nowhere)", credential.env_var);
    }
    let domains: Vec<&str> = credential
        .injections
        .iter()
        .map(|injection| injection.domain.as_str())
        .collect();
    format!("{} -> {}", credential.env_var, domains.join(", "))
}

fn render_connectors<W: std::io::Write>(out: &mut W, connectors: &[String]) -> Result<()> {
    for id in connectors {
        writeln!(out, "connector: {id}")?;
    }
    Ok(())
}

/// Inspect is where a script's body is printed whole; the launch summary elides it so a long one cannot bury the rest of the approval.
fn render_scripts<W: std::io::Write>(
    out: &mut W,
    scripts: &[lns_ipc::SandboxScript],
) -> Result<()> {
    for script in scripts {
        let user = script.user.as_deref().unwrap_or("the workload user");
        writeln!(out, "script: {} (runs as {user})", script.when)?;
        if let Some(description) = &script.description {
            writeln!(out, "  {description}")?;
        }
        for line in script.run.lines() {
            writeln!(out, "  | {line}")?;
        }
    }
    Ok(())
}

fn render_policy_flags<W: std::io::Write>(out: &mut W, flags: &[String]) -> Result<()> {
    for flag in flags {
        writeln!(out, "⚠ {flag}")?;
    }
    Ok(())
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
    let rendered = serde_json::to_string_pretty(&serde_json::Value::Object(doc))?;
    writeln!(out, "{rendered}")?;
    Ok(())
}

fn format_permille(permille: u32) -> String {
    format!("{}.{}%", permille / 10, permille % 10)
}

fn format_bytes(n: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;
    if n >= GIB {
        format!("{:.1} GiB", n as f64 / GIB as f64)
    } else if n >= MIB {
        format!("{:.1} MiB", n as f64 / MIB as f64)
    } else if n >= KIB {
        format!("{:.1} KiB", n as f64 / KIB as f64)
    } else {
        format!("{n} B")
    }
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
    use lns_ipc::encode_frame;
    use std::sync::{Arc, Mutex};

    struct CannedService {
        response: Response,
        stats_response: Option<Response>,
        inspect_image_response: Option<Response>,
        remove_image_response: Option<Response>,
        remove_run_response: Option<Response>,
        prune_runs_response: Option<Response>,
        frames: Vec<Vec<u8>>,
        requests: Arc<Mutex<Vec<Request>>>,
    }

    impl CannedService {
        fn new(response: Response) -> Self {
            Self {
                response,
                stats_response: None,
                inspect_image_response: None,
                remove_image_response: None,
                remove_run_response: None,
                prune_runs_response: None,
                frames: Vec::new(),
                requests: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn with_stats(response: Response, stats_response: Response) -> Self {
            Self {
                stats_response: Some(stats_response),
                ..Self::new(response)
            }
        }

        fn with_inspect_image(run_response: Response, image_response: Response) -> Self {
            Self {
                inspect_image_response: Some(image_response),
                ..Self::new(run_response)
            }
        }

        fn with_remove_image(run_response: Response, remove_response: Response) -> Self {
            Self {
                remove_image_response: Some(remove_response),
                ..Self::new(run_response)
            }
        }

        fn with_remove_run(run_response: Response, remove_response: Response) -> Self {
            Self {
                remove_run_response: Some(remove_response),
                ..Self::new(run_response)
            }
        }

        fn with_prune_runs(images_response: Response, runs_response: Response) -> Self {
            Self {
                prune_runs_response: Some(runs_response),
                ..Self::new(images_response)
            }
        }

        fn with_frames(frames: Vec<Vec<u8>>) -> Self {
            Self {
                frames,
                ..Self::new(Response::Pong)
            }
        }
    }

    fn sandbox_inspection(tools: Vec<String>) -> Response {
        sandbox_inspection_with_digest(tools, format!("sha256:{}", "a".repeat(64)))
    }

    fn sandbox_inspection_with_digest(tools: Vec<String>, digest: String) -> Response {
        Response::ImageInspected {
            inspection: lns_ipc::ArtifactInspection::Sandbox(Box::new(lns_ipc::SandboxView {
                mixins: Vec::new(),
                pinned_mixins: Vec::new(),
                contributions: Vec::new(),
                reference: "ghcr.io/team/hermes:1.4.0".into(),
                digest,
                image: "docker.io/library/alpine@sha256:abc".into(),
                workdir: None,
                user: None,
                mounts: Vec::new(),
                ports: Vec::new(),
                filesets: Vec::new(),
                connectors: Vec::new(),
                env: Vec::new(),
                credentials: Vec::new(),
                tools,
                scripts: Vec::new(),
                policy_flags: Vec::new(),
                cpus: None,
                mem_mib: None,
                disk_bytes: None,
            })),
        }
    }

    fn pulled_response() -> Response {
        Response::ImagePulled {
            image: lns_ipc::ImageInfo {
                reference: "ghcr.io/team/hermes:1.4.0".into(),
                digest: format!("sha256:{}", "a".repeat(64)),
                size_bytes: 1024,
                layers: 1,
                pulled: "2026-01-01T00:00:00Z".into(),
                in_use_by: None,
            },
            warnings: Vec::new(),
        }
    }

    impl SandboxService for CannedService {
        type Stream = tokio::io::DuplexStream;
        fn one_shot(&self, request: Request) -> BoxFuture<'_, Result<Response>> {
            let response = match &request {
                Request::RunStats { .. } => self
                    .stats_response
                    .clone()
                    .unwrap_or_else(|| self.response.clone()),
                Request::InspectImage { .. } => self
                    .inspect_image_response
                    .clone()
                    .unwrap_or_else(|| self.response.clone()),
                Request::RemoveImage { .. } => self
                    .remove_image_response
                    .clone()
                    .unwrap_or_else(|| self.response.clone()),
                Request::RemoveRun { .. } => self
                    .remove_run_response
                    .clone()
                    .unwrap_or_else(|| self.response.clone()),
                Request::PruneRuns => self
                    .prune_runs_response
                    .clone()
                    .unwrap_or_else(|| self.response.clone()),
                _ => self.response.clone(),
            };
            self.requests.lock().unwrap().push(request);
            Box::pin(async move { Ok(response) })
        }
        fn open_stream(&self, _request: Request) -> BoxFuture<'_, Result<Self::Stream>> {
            let frames = self.frames.clone();
            Box::pin(async move {
                if frames.is_empty() {
                    bail!("the daemon refused the stream");
                }
                Ok(stream_with(&frames).await)
            })
        }
        fn aux_socket(&self) -> Option<PathBuf> {
            None
        }
        fn load_policy(&self, _path: &str) -> Option<serde_json::Value> {
            None
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
            &mut std::io::Cursor::new(""),
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
            &mut std::io::Cursor::new(""),
            &mut out,
            &mut stdout,
            &mut stderr,
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("interactive"), "got: {err:#}");
    }

    #[tokio::test]
    async fn run_with_writers_refuses_the_offline_author_verbs() {
        let svc = CannedService::new(Response::Pong);
        for cmd in [
            SandboxCommand::Init,
            SandboxCommand::Validate(SandboxValidateArgs { file: None }),
            SandboxCommand::Inspect(SandboxInspectArgs {
                run: None,
                mixins: Vec::new(),
                file: None,
            }),
        ] {
            let mut out = Vec::new();
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let err = run_with_writers(
                &cmd,
                &svc,
                TermInfo::default(),
                &mut std::io::Cursor::new(""),
                &mut out,
                &mut stdout,
                &mut stderr,
            )
            .await
            .unwrap_err();
            assert!(format!("{err:#}").contains("offline"), "got: {err:#}");
        }
    }

    #[tokio::test]
    async fn run_with_writers_refuses_push_which_runs_locally() {
        let svc = CannedService::new(Response::Pong);
        let cmd = SandboxCommand::Push(SandboxPushArgs {
            reference: "ghcr.io/team/hermes:1.4.0".into(),
            dry_run: false,
            file: None,
        });
        let mut out = Vec::new();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let err = run_with_writers(
            &cmd,
            &svc,
            TermInfo::default(),
            &mut std::io::Cursor::new(""),
            &mut out,
            &mut stdout,
            &mut stderr,
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("locally"), "got: {err:#}");
    }

    #[tokio::test]
    async fn pull_reports_the_pulled_reference_and_digest() {
        let digest = format!("sha256:{}", "a".repeat(64));
        let svc = CannedService::with_inspect_image(
            Response::ImagePulled {
                image: lns_ipc::ImageInfo {
                    reference: "ghcr.io/team/hermes:1.4.0".into(),
                    digest: digest.clone(),
                    size_bytes: 1024,
                    layers: 1,
                    pulled: "2026-01-01T00:00:00Z".into(),
                    in_use_by: None,
                },
                warnings: vec!["the sandbox is cached, but its first run needs the network".into()],
            },
            sandbox_inspection(Vec::new()),
        );
        let mut out = Vec::new();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = run_with_writers(
            &SandboxCommand::Pull(SandboxPullArgs {
                reference: "ghcr.io/team/hermes:1.4.0".into(),
                assume_yes: false,
            }),
            &svc,
            TermInfo::default(),
            &mut std::io::Cursor::new(""),
            &mut out,
            &mut stdout,
            &mut stderr,
        )
        .await
        .unwrap();
        assert_eq!(code, 0);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("sha256:"), "got: {text}");
        assert_eq!(
            String::from_utf8(stderr).unwrap(),
            "warning: the sandbox is cached, but its first run needs the network\n"
        );
        assert!(matches!(
            svc.requests.lock().unwrap().as_slice(),
            [
                Request::InspectImage { image, .. },
                Request::PullImage {
                    image: pulled,
                    expected_digest
                }
            ] if image == "ghcr.io/team/hermes:1.4.0"
                && pulled == image
                && expected_digest == &digest
        ));
    }

    #[tokio::test]
    async fn pull_discloses_declared_tools_before_requesting_provisioning() {
        let svc = CannedService::with_inspect_image(
            pulled_response(),
            sandbox_inspection(vec!["node@22".into()]),
        );
        let mut out = Vec::new();

        let code = pull(
            &svc,
            &SandboxPullArgs {
                reference: "ghcr.io/team/hermes:1.4.0".into(),
                assume_yes: false,
            },
            TermInfo {
                stdin_is_tty: true,
                stdout_is_terminal: false,
            },
            &mut std::io::Cursor::new("yes\n"),
            &mut out,
            &mut Vec::new(),
        )
        .await
        .unwrap();

        assert_eq!(code, 0);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("Tool:       node@22"), "got: {text}");
        assert!(
            matches!(
                svc.requests.lock().unwrap().as_slice(),
                [Request::InspectImage { .. }, Request::PullImage { .. }]
            ),
            "inspection must precede the provisioning pull"
        );
    }

    #[tokio::test]
    async fn declining_pulled_tools_sends_no_provisioning_request() {
        let svc = CannedService::with_inspect_image(
            pulled_response(),
            sandbox_inspection(vec!["node@22".into()]),
        );

        let err = pull(
            &svc,
            &SandboxPullArgs {
                reference: "ghcr.io/team/hermes:1.4.0".into(),
                assume_yes: false,
            },
            TermInfo {
                stdin_is_tty: true,
                stdout_is_terminal: false,
            },
            &mut std::io::Cursor::new("n\n"),
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .await
        .unwrap_err();

        assert!(err.to_string().contains("declined"), "got: {err}");
        assert!(matches!(
            svc.requests.lock().unwrap().as_slice(),
            [Request::InspectImage { .. }]
        ));
    }

    #[tokio::test]
    async fn noninteractive_tool_pull_requires_yes_and_yes_skips_the_prompt() {
        let refused = CannedService::with_inspect_image(
            pulled_response(),
            sandbox_inspection(vec!["node@22".into()]),
        );
        let err = pull(
            &refused,
            &SandboxPullArgs {
                reference: "ghcr.io/team/hermes:1.4.0".into(),
                assume_yes: false,
            },
            TermInfo::default(),
            &mut std::io::Cursor::new(""),
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("--yes"), "got: {err}");
        assert!(matches!(
            refused.requests.lock().unwrap().as_slice(),
            [Request::InspectImage { .. }]
        ));

        let accepted = CannedService::with_inspect_image(
            pulled_response(),
            sandbox_inspection(vec!["node@22".into()]),
        );
        let mut out = Vec::new();
        pull(
            &accepted,
            &SandboxPullArgs {
                reference: "ghcr.io/team/hermes:1.4.0".into(),
                assume_yes: true,
            },
            TermInfo::default(),
            &mut std::io::Cursor::new(""),
            &mut out,
            &mut Vec::new(),
        )
        .await
        .unwrap();
        let out = String::from_utf8(out).unwrap();
        assert!(!out.contains("Continue?"), "got: {out}");
        assert!(matches!(
            accepted.requests.lock().unwrap().as_slice(),
            [Request::InspectImage { .. }, Request::PullImage { .. }]
        ));
    }

    #[tokio::test]
    async fn pull_surfaces_a_daemon_error_and_rejects_an_unrelated_variant() {
        let err = pull(
            &CannedService::new(Response::Error {
                message: "registry unreachable".into(),
            }),
            &SandboxPullArgs {
                reference: "x:1".into(),
                assume_yes: false,
            },
            TermInfo::default(),
            &mut std::io::Cursor::new(""),
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("registry unreachable"));

        let err = pull(
            &CannedService::new(Response::Pong),
            &SandboxPullArgs {
                reference: "x:1".into(),
                assume_yes: false,
            },
            TermInfo::default(),
            &mut std::io::Cursor::new(""),
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("unexpected response"));
    }

    #[tokio::test]
    async fn pull_refuses_an_unpinned_sandbox_and_a_plain_image() {
        let err = pull(
            &CannedService::with_inspect_image(
                Response::Pong,
                sandbox_inspection_with_digest(Vec::new(), String::new()),
            ),
            &SandboxPullArgs {
                reference: "x:1".into(),
                assume_yes: false,
            },
            TermInfo::default(),
            &mut std::io::Cursor::new(""),
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("did not provide a digest"));

        let err = pull(
            &CannedService::with_inspect_image(
                Response::Pong,
                Response::ImageInspected {
                    inspection: lns_ipc::ArtifactInspection::Image(lns_ipc::ImageView {
                        reference: "x:1".into(),
                        digest: format!("sha256:{}", "a".repeat(64)),
                    }),
                },
            ),
            &SandboxPullArgs {
                reference: "x:1".into(),
                assume_yes: false,
            },
            TermInfo::default(),
            &mut std::io::Cursor::new(""),
            &mut Vec::new(),
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("OCI image"));
    }

    #[tokio::test]
    async fn pull_surfaces_a_post_consent_daemon_error_and_unexpected_response() {
        for (response, expected) in [
            (
                Response::Error {
                    message: "registry changed".into(),
                },
                "registry changed",
            ),
            (Response::Pong, "unexpected response"),
        ] {
            let err = pull(
                &CannedService::with_inspect_image(response, sandbox_inspection(Vec::new())),
                &SandboxPullArgs {
                    reference: "x:1".into(),
                    assume_yes: false,
                },
                TermInfo::default(),
                &mut std::io::Cursor::new(""),
                &mut Vec::new(),
                &mut Vec::new(),
            )
            .await
            .unwrap_err();
            assert!(err.to_string().contains(expected), "got: {err}");
        }
    }

    #[tokio::test]
    async fn tag_confirms_the_new_reference() {
        let svc = CannedService::new(Response::ImageTagged {
            from: "hermes:1.4.0".into(),
            to: "hermes:latest".into(),
        });
        let mut out = Vec::new();
        let code = tag(
            &svc,
            &SandboxTagArgs {
                from: "hermes:1.4.0".into(),
                to: "hermes:latest".into(),
            },
            &mut out,
        )
        .await
        .unwrap();
        assert_eq!(code, 0);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "tagged hermes:1.4.0 as hermes:latest\n"
        );
    }

    #[tokio::test]
    async fn tag_surfaces_a_daemon_error_and_rejects_an_unrelated_variant() {
        let err = tag(
            &CannedService::new(Response::Error {
                message: "no such cached sandbox".into(),
            }),
            &SandboxTagArgs {
                from: "a:1".into(),
                to: "a:2".into(),
            },
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("no such cached sandbox"));

        let err = tag(
            &CannedService::new(Response::Pong),
            &SandboxTagArgs {
                from: "a:1".into(),
                to: "a:2".into(),
            },
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("unexpected response"));
    }

    #[tokio::test]
    async fn ls_surfaces_a_daemon_error() {
        let svc = CannedService::new(Response::Error {
            message: "registry poisoned".into(),
        });
        let mut out = Vec::new();
        let err = ls(&svc, &ls_args(), &mut out).await.unwrap_err();
        assert!(format!("{err:#}").contains("registry poisoned"));
    }

    #[tokio::test]
    async fn ls_rejects_an_unrelated_response_variant() {
        let svc = CannedService::new(Response::Pong);
        let mut out = Vec::new();
        let err = ls(&svc, &ls_args(), &mut out).await.unwrap_err();
        assert!(format!("{err:#}").contains("unexpected response"));
    }

    fn ls_args() -> LsArgs {
        LsArgs {
            output: crate::output::OutputArgs {
                format: crate::output::Format::Table,
            },
        }
    }

    fn ps_args() -> PsArgs {
        PsArgs {
            output: crate::output::OutputArgs {
                format: crate::output::Format::Table,
            },
        }
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
    async fn inspect_of_a_live_run_refuses_a_mixin_rather_than_ignoring_it() {
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
        let err = inspect(&svc, "1", &["ghcr.io/acme/obs:2".to_string()], &mut out)
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("--mixin applies to a sandbox reference"),
            "a live run has already booted, so rendering it as though the flag applied would describe a composition that never ran; got: {err:#}"
        );
    }

    #[tokio::test]
    async fn inspect_rejects_an_unrelated_response_variant() {
        let svc = CannedService::new(Response::Pong);
        let mut out = Vec::new();
        let err = inspect(&svc, "1", &[], &mut out).await.unwrap_err();
        assert!(format!("{err:#}").contains("unexpected response"));
    }

    #[tokio::test]
    async fn inspect_surfaces_a_daemon_error() {
        let svc = CannedService::new(Response::Error {
            message: "no active run with id 1".into(),
        });
        let mut out = Vec::new();
        let err = inspect(&svc, "1", &[], &mut out).await.unwrap_err();
        assert!(format!("{err:#}").contains("no active run with id 1"));
    }

    #[tokio::test]
    async fn inspect_falls_back_to_a_cached_sandbox_definition() {
        let svc = CannedService::with_inspect_image(
            Response::Error {
                message: "no such run: hermes:1.4.0".into(),
            },
            Response::ImageInspected {
                inspection: lns_ipc::ArtifactInspection::Sandbox(Box::new(lns_ipc::SandboxView {
                    mixins: Vec::new(),
                    pinned_mixins: Vec::new(),
                    contributions: Vec::new(),
                    reference: "hermes:1.4.0".into(),
                    digest: format!("sha256:{}", "a".repeat(64)),
                    image: "docker.io/library/alpine@sha256:abc".into(),
                    workdir: None,
                    user: None,
                    mounts: Vec::new(),
                    ports: Vec::new(),
                    filesets: Vec::new(),
                    connectors: vec!["some-provider".into()],
                    env: Vec::new(),
                    credentials: Vec::new(),
                    tools: vec!["node@22.11.0".into()],
                    scripts: vec![
                        lns_ipc::SandboxScript {
                            when: "pre-start".into(),
                            run: "apt-get update\napt-get install -y psql".into(),
                            user: Some("root".into()),
                            description: Some("the psql the prompts assume".into()),
                        },
                        lns_ipc::SandboxScript {
                            when: "pre-start".into(),
                            run: "npm ci".into(),
                            user: None,
                            description: None,
                        },
                    ],
                    policy_flags: Vec::new(),
                    cpus: None,
                    mem_mib: None,
                    disk_bytes: None,
                })),
            },
        );
        let mut out = Vec::new();
        let code = inspect(&svc, "hermes:1.4.0", &[], &mut out).await.unwrap();
        assert_eq!(code, 0);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("kind: sandbox"), "got: {text}");
        assert!(
            text.contains("image: docker.io/library/alpine"),
            "got: {text}"
        );
        assert!(text.contains("connector: some-provider"), "got: {text}");
        assert!(text.contains("tool: node@22.11.0"), "got: {text}");
        assert!(
            text.contains("script: pre-start (runs as root)")
                && text.contains("  the psql the prompts assume")
                && text.contains("  | apt-get update")
                && text.contains("  | apt-get install -y psql"),
            "inspect answers \"so what does it actually do\", so every line of the body has to appear; got: {text}"
        );
        assert!(
            text.contains("script: pre-start (runs as the workload user)"),
            "a script naming no user runs as the workload does, and a reader should not have to infer that from a blank; got: {text}"
        );
    }

    #[tokio::test]
    async fn inspect_cached_renders_the_image_kind() {
        let image = CannedService::with_inspect_image(
            Response::Error {
                message: "no such run: x".into(),
            },
            Response::ImageInspected {
                inspection: lns_ipc::ArtifactInspection::Image(lns_ipc::ImageView {
                    reference: "alpine:3.20".into(),
                    digest: "sha256:abc".into(),
                }),
            },
        );
        let mut out = Vec::new();
        inspect(&image, "x", &[], &mut out).await.unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("kind: image"), "got: {text}");
        assert!(text.contains("digest: sha256:abc"), "got: {text}");
    }

    #[tokio::test]
    async fn inspect_cached_rejects_an_unrelated_image_response() {
        let svc = CannedService::with_inspect_image(
            Response::Error {
                message: "no such run: x".into(),
            },
            Response::Pong,
        );
        let mut out = Vec::new();
        let err = inspect(&svc, "x", &[], &mut out).await.unwrap_err();
        assert!(format!("{err:#}").contains("unexpected response"));
    }

    #[tokio::test]
    async fn inspect_surfaces_a_non_not_found_inspect_error_without_touching_the_cache() {
        let svc = CannedService::new(Response::Error {
            message: "daemon busy".into(),
        });
        let mut out = Vec::new();
        let err = inspect(&svc, "reviewer", &[], &mut out).await.unwrap_err();
        assert!(
            format!("{err:#}").contains("daemon busy"),
            "a transient InspectRun error must surface, not be masked as no-such-image: {err:#}"
        );
        let requests = svc.requests.lock().unwrap();
        assert!(
            !requests
                .iter()
                .any(|r| matches!(r, Request::InspectImage { .. })),
            "a genuine error must not fall through to the cached-inspect path: {requests:?}"
        );
    }

    fn running_inspect(status: lns_ipc::RunStatus) -> Response {
        Response::RunInspect {
            details: Box::new(RunDetails {
                summary: lns_ipc::RunSummary {
                    id: "1a2b3c4d0000000000000000000000aa".into(),
                    name: "reviewer".into(),
                    image: "some-image".into(),
                    command: String::new(),
                    status,
                    started: "2026-01-01T00:00:00Z".into(),
                },
                config: lns_ipc::RunConfig::default(),
            }),
        }
    }

    #[tokio::test]
    async fn rm_refuses_a_running_sandbox_naming_stop() {
        let svc = CannedService::new(running_inspect(lns_ipc::RunStatus::Running));
        let mut out = Vec::new();
        let err = rm(
            &svc,
            &SandboxRmArgs {
                run: "reviewer".into(),
            },
            &mut out,
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("running"), "got: {err:#}");
        assert!(format!("{err:#}").contains("lns stop"), "got: {err:#}");
    }

    #[tokio::test]
    async fn rm_removes_a_cached_sandbox_and_reports_freed_layers() {
        let svc = CannedService::with_remove_image(
            Response::Error {
                message: "no such run: hermes:1.4.0".into(),
            },
            Response::ImageRemoved {
                reference: "hermes:1.4.0".into(),
                reclaimed_bytes: 3 * 1024 * 1024,
            },
        );
        let mut out = Vec::new();
        let code = rm(
            &svc,
            &SandboxRmArgs {
                run: "hermes:1.4.0".into(),
            },
            &mut out,
        )
        .await
        .unwrap();
        assert_eq!(code, 0);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("removed hermes:1.4.0"), "got: {text}");
        assert!(text.contains("freed 3.0 MiB"), "got: {text}");
    }

    #[tokio::test]
    async fn rm_surfaces_a_non_not_found_inspect_error_without_touching_the_cache() {
        let svc = CannedService::new(Response::Error {
            message: "daemon busy".into(),
        });
        let mut out = Vec::new();
        let err = rm(
            &svc,
            &SandboxRmArgs {
                run: "reviewer".into(),
            },
            &mut out,
        )
        .await
        .unwrap_err();
        assert!(
            format!("{err:#}").contains("daemon busy"),
            "a transient InspectRun error must surface, not be masked as no-such-image: {err:#}"
        );
        let requests = svc.requests.lock().unwrap();
        assert!(
            !requests
                .iter()
                .any(|r| matches!(r, Request::RemoveImage { .. })),
            "a genuine error must not fall through to the cached-remove path: {requests:?}"
        );
    }

    #[tokio::test]
    async fn rm_of_an_exited_run_removes_the_spent_run_record() {
        let svc = CannedService::with_remove_run(
            running_inspect(lns_ipc::RunStatus::Exited { code: 0 }),
            Response::Acknowledged,
        );
        let mut out = Vec::new();
        let code = rm(
            &svc,
            &SandboxRmArgs {
                run: "reviewer".into(),
            },
            &mut out,
        )
        .await
        .unwrap();
        assert_eq!(code, 0);
        assert!(
            String::from_utf8(out)
                .unwrap()
                .contains("removed run reviewer"),
            "an exited run must be dropped via RemoveRun"
        );
        let requests = svc.requests.lock().unwrap();
        assert!(
            requests
                .iter()
                .any(|r| matches!(r, Request::RemoveRun { .. })),
            "rm of an exited run must issue RemoveRun, not RemoveImage: {requests:?}"
        );
    }

    #[tokio::test]
    async fn rm_of_an_exited_run_surfaces_the_daemon_error() {
        let svc = CannedService::with_remove_run(
            running_inspect(lns_ipc::RunStatus::Exited { code: 0 }),
            Response::Error {
                message: "run vanished mid-remove".into(),
            },
        );
        let mut out = Vec::new();
        let err = rm(
            &svc,
            &SandboxRmArgs {
                run: "reviewer".into(),
            },
            &mut out,
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("run vanished mid-remove"));
    }

    #[tokio::test]
    async fn rm_of_an_exited_run_rejects_an_unrelated_remove_response() {
        let svc = CannedService::with_remove_run(
            running_inspect(lns_ipc::RunStatus::Exited { code: 0 }),
            Response::Pong,
        );
        let mut out = Vec::new();
        let err = rm(
            &svc,
            &SandboxRmArgs {
                run: "reviewer".into(),
            },
            &mut out,
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("unexpected response"));
    }

    #[tokio::test]
    async fn rm_rejects_an_unrelated_inspect_response() {
        let svc = CannedService::new(Response::Pong);
        let mut out = Vec::new();
        let err = rm(&svc, &SandboxRmArgs { run: "1".into() }, &mut out)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("unexpected response"));
    }

    #[tokio::test]
    async fn rm_of_a_cached_sandbox_surfaces_the_daemon_error() {
        let svc = CannedService::with_remove_image(
            Response::Error {
                message: "no such run: ghcr.io/team/x:1".into(),
            },
            Response::Error {
                message: "no such image: ghcr.io/team/x:1".into(),
            },
        );
        let mut out = Vec::new();
        let err = rm(
            &svc,
            &SandboxRmArgs {
                run: "ghcr.io/team/x:1".into(),
            },
            &mut out,
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("no such image"));
    }

    #[tokio::test]
    async fn rm_rejects_an_unrelated_remove_response() {
        let svc = CannedService::with_remove_image(
            Response::Error {
                message: "no such run: reviewer".into(),
            },
            Response::Pong,
        );
        let mut out = Vec::new();
        let err = rm(
            &svc,
            &SandboxRmArgs {
                run: "reviewer".into(),
            },
            &mut out,
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("unexpected response"));
    }

    #[tokio::test]
    async fn prune_requires_force_before_touching_the_cache() {
        let svc = CannedService::new(Response::Pong);
        let mut out = Vec::new();
        let err = prune(&svc, &SandboxPruneArgs { force: false }, &mut out)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("--force"), "got: {err:#}");
    }

    #[tokio::test]
    async fn prune_with_force_lists_removed_runs_and_sandboxes_with_combined_bytes() {
        let svc = CannedService::with_prune_runs(
            Response::ImagesPruned {
                removed: vec!["b:2".into(), "a:1".into()],
                reclaimed_bytes: 64 * 1024 * 1024,
            },
            Response::RunsPruned {
                removed: vec!["ffee0000".into(), "aabb0000".into()],
                reclaimed_bytes: 16 * 1024 * 1024,
            },
        );
        let mut out = Vec::new();
        let code = prune(&svc, &SandboxPruneArgs { force: true }, &mut out)
            .await
            .unwrap();
        assert_eq!(code, 0);
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("removed run aabb0000") && text.contains("removed run ffee0000"),
            "got: {text}"
        );
        assert!(
            text.contains("removed a:1") && text.contains("removed b:2"),
            "got: {text}"
        );
        assert!(
            text.contains("reclaimed 80.0 MiB"),
            "runs and images must be summed, got: {text}"
        );
    }

    #[tokio::test]
    async fn prune_surfaces_a_daemon_error_and_rejects_an_unrelated_variant() {
        let err = prune(
            &CannedService::new(Response::Error {
                message: "registry poisoned".into(),
            }),
            &SandboxPruneArgs { force: true },
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("registry poisoned"));

        let err = prune(
            &CannedService::new(Response::Pong),
            &SandboxPruneArgs { force: true },
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("unexpected response"));

        let empty_runs = || Response::RunsPruned {
            removed: Vec::new(),
            reclaimed_bytes: 0,
        };
        let err = prune(
            &CannedService::with_prune_runs(
                Response::Error {
                    message: "image store poisoned".into(),
                },
                empty_runs(),
            ),
            &SandboxPruneArgs { force: true },
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("image store poisoned"));

        let err = prune(
            &CannedService::with_prune_runs(Response::Pong, empty_runs()),
            &SandboxPruneArgs { force: true },
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("unexpected response"));
    }

    #[test]
    fn format_bytes_picks_the_natural_unit() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2048), "2.0 KiB");
        assert_eq!(format_bytes(92_274_688), "88.0 MiB");
        assert_eq!(format_bytes(2 * 1024 * 1024 * 1024), "2.0 GiB");
    }

    #[test]
    fn format_permille_renders_tenths_of_a_percent() {
        assert_eq!(format_permille(0), "0.0%");
        assert_eq!(format_permille(125), "12.5%");
        assert_eq!(format_permille(1000), "100.0%");
    }

    async fn stream_with(frames: &[Vec<u8>]) -> tokio::io::DuplexStream {
        let (client, mut server) = tokio::io::duplex(64 * 1024);
        let payload: Vec<u8> = frames.concat();
        tokio::spawn(async move {
            let _ = server.write_all(&payload).await;
        });
        client
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
        let code = inspect(&svc, "1", &[], &mut out).await.unwrap();
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
