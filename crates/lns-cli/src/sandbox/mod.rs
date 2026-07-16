use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use lns_ipc::{
    Request, Response, RunDetails, RunStatsInfo, WireFrame, decode_wire_frame_from_bytes,
    read_frame_bytes_async,
};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};

use crate::cli::{DetachChord, ExecArgs, KillArgs, parse_detach_keys_arg};
use crate::command::{CommandSpec, subcommand};
use crate::service::client::BoxFuture;

pub mod author;
pub mod distribute;
pub mod real;

#[derive(clap::Args)]
pub struct SandboxArgs {
    #[command(subcommand)]
    pub command: SandboxCommand,
}

#[derive(clap::Subcommand)]
pub enum SandboxCommand {
    #[command(about = "Scaffold a default ./lns.yaml (kind: Sandbox) in this directory.")]
    Init,
    #[command(about = "Validate ./lns.yaml — schema, cross-field, and secret checks, offline.")]
    Validate,
    #[command(about = "Render ./lns.yaml's effective definition (merged config, resolved values).")]
    Show,
    #[command(
        about = "Build ./lns.yaml and upload it to a registry as a sandbox artifact, in one step."
    )]
    Push(SandboxPushArgs),
    #[command(about = "Fetch a published sandbox and its base image into the local cache.")]
    Pull(SandboxPullArgs),
    #[command(about = "Re-reference a cached sandbox under a new tag (`docker tag`-style).")]
    Tag(SandboxTagArgs),
    #[command(about = "List running sandboxes with their CPU and memory (`docker ps`-style).")]
    Ps,
    #[command(
        visible_alias = "list",
        about = "List cached sandboxes (pulled or built) in the local store."
    )]
    Ls,
    #[command(about = "Open a new session (`docker exec`-style) against a running run.")]
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
        about = "Inspect a sandbox: a running run's live state as JSON, or a cached artifact's kind and definition."
    )]
    Inspect(SandboxInspectArgs),
    #[command(
        about = "Remove a cached sandbox and free its now-unreferenced layers; refuses a running one."
    )]
    Rm(SandboxRmArgs),
    #[command(about = "Remove every cached sandbox not held by a running one, reclaiming disk.")]
    Prune(SandboxPruneArgs),
}

#[derive(clap::Args)]
pub struct SandboxPushArgs {
    #[arg(
        value_name = "REF",
        help = "Registry reference to publish the sandbox at, e.g. ghcr.io/team/hermes:1.4.0."
    )]
    pub reference: String,
}

#[derive(clap::Args)]
pub struct SandboxPullArgs {
    #[arg(
        value_name = "REF",
        help = "Published sandbox reference to fetch, e.g. ghcr.io/team/hermes:1.4.0."
    )]
    pub reference: String,
}

#[derive(clap::Args)]
pub struct SandboxTagArgs {
    #[arg(value_name = "SOURCE", help = "Cached sandbox to re-reference.")]
    pub from: String,

    #[arg(
        value_name = "TARGET",
        help = "New reference the cached sandbox also resolves under."
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
        help = "A running run's id/name (live state), or a cached sandbox reference (its definition)."
    )]
    pub run: String,
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
        help = "Required: confirm removing every cached sandbox not held by a running one."
    )]
    pub force: bool,
}

pub fn augment(app: clap::Command) -> clap::Command {
    app.subcommand(
        subcommand::<SandboxArgs>("sandbox")
            .about("The sandbox — author, distribute, run, and manage it (the complete surface)."),
    )
}

pub const SPEC: CommandSpec = CommandSpec {
    name: "sandbox",
    augment,
    run: real::run,
    announces_update_check: true,
    owns_terminal: false,
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
    owns_terminal: false,
};

pub fn augment_ps(app: clap::Command) -> clap::Command {
    app.subcommand(
        clap::Command::new("ps").about(
            "List running sandboxes with their CPU and memory (shortcut for `lns sandbox ps`).",
        ),
    )
}

pub const PS_SPEC: CommandSpec = CommandSpec {
    name: "ps",
    augment: augment_ps,
    run: real::run_ps,
    announces_update_check: true,
    owns_terminal: false,
};

macro_rules! shortcut_spec {
    ($augment:ident, $const_name:ident, $args:ty, $name:literal, $run:path, $about:literal) => {
        pub fn $augment(app: clap::Command) -> clap::Command {
            app.subcommand(subcommand::<$args>($name).about($about))
        }
        pub const $const_name: CommandSpec = CommandSpec {
            name: $name,
            augment: $augment,
            run: $run,
            announces_update_check: true,
            owns_terminal: false,
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
    "Print a running sandbox's output (shortcut for `lns sandbox logs`)."
);
shortcut_spec!(
    augment_attach,
    ATTACH_SPEC,
    SandboxAttachArgs,
    "attach",
    real::run_attach,
    "Re-attach to a running sandbox (shortcut for `lns sandbox attach`)."
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

pub async fn run_with_writers<S, W, O, E>(
    cmd: &SandboxCommand,
    svc: &S,
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
    match cmd {
        SandboxCommand::Init | SandboxCommand::Validate | SandboxCommand::Show => {
            bail!("author commands run offline, not through the service dispatch")
        }
        SandboxCommand::Push(_) => {
            bail!("push builds and uploads locally, not through the service dispatch")
        }
        SandboxCommand::Pull(args) => pull(svc, args, out).await,
        SandboxCommand::Tag(args) => tag(svc, args, out).await,
        SandboxCommand::Ps => ps(svc, out).await,
        SandboxCommand::Ls => ls(svc, out).await,
        SandboxCommand::Kill(args) => kill(svc, args, out).await,
        SandboxCommand::Exec(_) => bail!("sandbox exec is dispatched on its own interactive path"),
        SandboxCommand::Stop(args) => stop(svc, args, out).await,
        SandboxCommand::Inspect(args) => inspect(svc, args, out).await,
        SandboxCommand::Logs(args) => logs(svc, args, stdout, stderr).await,
        SandboxCommand::Attach(args) => attach(svc, args, term, stdout, stderr).await,
        SandboxCommand::Rm(args) => rm(svc, args, out).await,
        SandboxCommand::Prune(args) => prune(svc, args, out).await,
    }
}

pub(crate) fn run_label(run: &str) -> String {
    run.to_string()
}

async fn pull<W: std::io::Write>(
    svc: &impl SandboxService,
    args: &SandboxPullArgs,
    out: &mut W,
) -> Result<i32> {
    let response = svc
        .one_shot(Request::PullImage {
            image: args.reference.clone(),
        })
        .await?;
    match response {
        Response::ImagePulled { image } => {
            writeln!(out, "pulled {}", image.reference)?;
            writeln!(out, "digest: {}", image.digest)?;
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

async fn ps<W: std::io::Write>(svc: &impl SandboxService, out: &mut W) -> Result<i32> {
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
            Response::RunStats { stats } => stats,
            Response::Error { message } => bail!("daemon error: {message}"),
            other => bail!("unexpected response from daemon: {other:?}"),
        };
        rows.push((run, stats));
    }
    render_ps_table(&running, &rows, out)
}

fn render_ps_table<W: std::io::Write>(
    running: &[lns_ipc::RunSummary],
    rows: &[(&lns_ipc::RunSummary, RunStatsInfo)],
    out: &mut W,
) -> Result<i32> {
    let id_w = "ID".len().max(
        running
            .iter()
            .map(|r| lns_ipc::short_run_id(&r.id).len())
            .max()
            .unwrap_or(0),
    );
    let name_w = "NAME"
        .len()
        .max(running.iter().map(|r| r.name.len()).max().unwrap_or(0));
    let image_w = "IMAGE"
        .len()
        .max(running.iter().map(|r| r.image.len()).max().unwrap_or(0));
    writeln!(
        out,
        "{:<id_w$}  {:<name_w$}  {:<image_w$}  CPU %   MEM",
        "ID", "NAME", "IMAGE",
    )?;
    for (run, stats) in rows {
        writeln!(
            out,
            "{:<id_w$}  {:<name_w$}  {:<image_w$}  {:<6}  {} / {}",
            lns_ipc::short_run_id(&run.id),
            run.name,
            run.image,
            format_permille(stats.cpu_permille),
            format_bytes(stats.mem_used_bytes),
            format_bytes(stats.mem_total_bytes),
        )?;
    }
    Ok(0)
}

async fn ls<W: std::io::Write>(svc: &impl SandboxService, out: &mut W) -> Result<i32> {
    match svc.one_shot(Request::ListImages).await? {
        Response::ImageList { mut images } => {
            images.sort_by(|a, b| a.reference.cmp(&b.reference));
            render_cached_table(out, &images)?;
            Ok(0)
        }
        Response::Error { message } => bail!("daemon error: {message}"),
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

fn render_cached_table<W: std::io::Write>(
    out: &mut W,
    images: &[lns_ipc::ImageInfo],
) -> Result<()> {
    let ref_w = "SANDBOX"
        .len()
        .max(images.iter().map(|i| i.reference.len()).max().unwrap_or(0));
    writeln!(out, "{:<ref_w$}  STATE", "SANDBOX")?;
    for image in images {
        writeln!(out, "{:<ref_w$}  cached", image.reference)?;
    }
    Ok(())
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
        // An exited run is a spent record; a reference the service knows no run for is a cached sandbox.
        Response::RunInspect { .. } => remove_run(svc, &args.run, out).await,
        Response::Error { .. } => remove_cached(svc, &args.run, out).await,
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
            "this removes every cached sandbox not held by a running one; pass --force to confirm"
        );
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
            writeln!(out, "reclaimed {}", format_bytes(reclaimed_bytes))?;
            Ok(0)
        }
        Response::Error { message } => bail!("daemon error: {message}"),
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

async fn inspect<W: std::io::Write>(
    svc: &impl SandboxService,
    args: &SandboxInspectArgs,
    out: &mut W,
) -> Result<i32> {
    match svc
        .one_shot(Request::InspectRun {
            run: args.run.clone(),
        })
        .await?
    {
        Response::RunInspect { details } => {
            let policy = details
                .config
                .policy_path
                .as_deref()
                .map(|path| policy_doc(path, svc.load_policy(path)));
            render_inspect(&details, policy, out)?;
            Ok(0)
        }
        // Not a running sandbox — fall through to the cached artifact's definition.
        Response::Error { .. } => inspect_cached(svc, &args.run, out).await,
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

async fn inspect_cached<W: std::io::Write>(
    svc: &impl SandboxService,
    reference: &str,
    out: &mut W,
) -> Result<i32> {
    match svc
        .one_shot(Request::InspectImage {
            image: reference.to_string(),
        })
        .await?
    {
        Response::ImageInspected { inspection } => {
            render_cached_inspect(&inspection, out)?;
            Ok(0)
        }
        Response::Error { message } => bail!("daemon error: {message}"),
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

fn render_cached_inspect<W: std::io::Write>(
    inspection: &lns_ipc::ArtifactInspection,
    out: &mut W,
) -> Result<()> {
    match inspection {
        lns_ipc::ArtifactInspection::Sandbox(view) => {
            writeln!(out, "kind: Sandbox")?;
            writeln!(out, "reference: {}", view.reference)?;
            if !view.digest.is_empty() {
                writeln!(out, "digest: {}", view.digest)?;
            }
            writeln!(out, "image: {}", view.image)?;
            if let Some(workdir) = &view.workdir {
                writeln!(out, "workdir: {workdir}")?;
            }
            for mount in &view.mounts {
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
            render_integrations(out, &view.integrations)?;
            render_policy_flags(out, &view.policy_flags)?;
        }
        lns_ipc::ArtifactInspection::Image(view) => {
            writeln!(out, "kind: Image")?;
            writeln!(out, "reference: {}", view.reference)?;
            writeln!(out, "digest: {}", view.digest)?;
        }
        lns_ipc::ArtifactInspection::Bundle(view) => render_bundle(out, view)?,
    }
    Ok(())
}

fn render_bundle<W: std::io::Write>(out: &mut W, view: &lns_ipc::BundleView) -> Result<()> {
    writeln!(out, "kind: AgentSystem")?;
    writeln!(out, "reference: {}", view.reference)?;
    if let Some(base) = &view.sandbox_base_image {
        writeln!(out, "image: {base}")?;
    }
    if !view.filesets.is_empty() {
        writeln!(out, "filesets:")?;
        for fileset in &view.filesets {
            writeln!(out, "  {} -> {}", fileset.name, fileset.mount_path)?;
        }
    }
    render_integrations(out, &view.integrations)?;
    writeln!(out, "signature: {}", signature_label(view.signature))?;
    render_policy_flags(out, &view.policy_flags)?;
    Ok(())
}

fn render_integrations<W: std::io::Write>(out: &mut W, integrations: &[String]) -> Result<()> {
    for id in integrations {
        writeln!(out, "integration: {id}")?;
    }
    Ok(())
}

fn render_policy_flags<W: std::io::Write>(out: &mut W, flags: &[String]) -> Result<()> {
    for flag in flags {
        writeln!(out, "⚠ {flag}")?;
    }
    Ok(())
}

fn signature_label(signature: lns_ipc::SignatureView) -> &'static str {
    match signature {
        lns_ipc::SignatureView::Unsigned => "unsigned",
        lns_ipc::SignatureView::SignedTrusted => "signed (trusted)",
        lns_ipc::SignatureView::SignedUntrusted => "signed (untrusted signer)",
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

        fn with_frames(frames: Vec<Vec<u8>>) -> Self {
            Self {
                frames,
                ..Self::new(Response::Pong)
            }
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
            SandboxCommand::Validate,
            SandboxCommand::Show,
        ] {
            let mut out = Vec::new();
            let mut stdout = Vec::new();
            let mut stderr = Vec::new();
            let err = run_with_writers(
                &cmd,
                &svc,
                TermInfo::default(),
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
        });
        let mut out = Vec::new();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let err = run_with_writers(
            &cmd,
            &svc,
            TermInfo::default(),
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
        let svc = CannedService::new(Response::ImagePulled {
            image: lns_ipc::ImageInfo {
                reference: "ghcr.io/team/hermes:1.4.0".into(),
                digest: format!("sha256:{}", "a".repeat(64)),
                size_bytes: 1024,
                layers: 1,
                pulled: "2026-01-01T00:00:00Z".into(),
                in_use_by: None,
            },
        });
        let mut out = Vec::new();
        let code = pull(
            &svc,
            &SandboxPullArgs {
                reference: "ghcr.io/team/hermes:1.4.0".into(),
            },
            &mut out,
        )
        .await
        .unwrap();
        assert_eq!(code, 0);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("sha256:"), "got: {text}");
    }

    #[tokio::test]
    async fn pull_surfaces_a_daemon_error_and_rejects_an_unrelated_variant() {
        let err = pull(
            &CannedService::new(Response::Error {
                message: "registry unreachable".into(),
            }),
            &SandboxPullArgs {
                reference: "x:1".into(),
            },
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("registry unreachable"));

        let err = pull(
            &CannedService::new(Response::Pong),
            &SandboxPullArgs {
                reference: "x:1".into(),
            },
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("unexpected response"));
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
        let err = ls(&svc, &mut out).await.unwrap_err();
        assert!(format!("{err:#}").contains("registry poisoned"));
    }

    #[tokio::test]
    async fn ls_rejects_an_unrelated_response_variant() {
        let svc = CannedService::new(Response::Pong);
        let mut out = Vec::new();
        let err = ls(&svc, &mut out).await.unwrap_err();
        assert!(format!("{err:#}").contains("unexpected response"));
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
        let code = ps(&svc, &mut out).await.unwrap();
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
        let code = ps(&svc, &mut out).await.unwrap();
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
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("registry poisoned"));

        let err = ps(&CannedService::new(Response::Pong), &mut Vec::new())
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("unexpected response"));
    }

    #[tokio::test]
    async fn ps_surfaces_a_stats_error_and_rejects_an_unrelated_stats_variant() {
        let err = ps(
            &CannedService::with_stats(
                Response::RunList {
                    runs: vec![running_run()],
                },
                Response::Error {
                    message: "stats probe failed".into(),
                },
            ),
            &mut Vec::new(),
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("stats probe failed"));

        let err = ps(
            &CannedService::with_stats(
                Response::RunList {
                    runs: vec![running_run()],
                },
                Response::Pong,
            ),
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
    async fn inspect_rejects_an_unrelated_response_variant() {
        let svc = CannedService::new(Response::Pong);
        let mut out = Vec::new();
        let err = inspect(&svc, &SandboxInspectArgs { run: "1".into() }, &mut out)
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
        let err = inspect(&svc, &SandboxInspectArgs { run: "1".into() }, &mut out)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("no active run with id 1"));
    }

    #[tokio::test]
    async fn inspect_falls_back_to_a_cached_sandbox_definition() {
        let svc = CannedService::with_inspect_image(
            Response::Error {
                message: "no active run with id hermes:1.4.0".into(),
            },
            Response::ImageInspected {
                inspection: lns_ipc::ArtifactInspection::Sandbox(lns_ipc::SandboxView {
                    reference: "hermes:1.4.0".into(),
                    digest: format!("sha256:{}", "a".repeat(64)),
                    image: "docker.io/library/alpine@sha256:abc".into(),
                    workdir: None,
                    mounts: Vec::new(),
                    integrations: vec!["some-provider".into()],
                    policy_flags: Vec::new(),
                }),
            },
        );
        let mut out = Vec::new();
        let code = inspect(
            &svc,
            &SandboxInspectArgs {
                run: "hermes:1.4.0".into(),
            },
            &mut out,
        )
        .await
        .unwrap();
        assert_eq!(code, 0);
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("kind: Sandbox"), "got: {text}");
        assert!(
            text.contains("image: docker.io/library/alpine"),
            "got: {text}"
        );
        assert!(text.contains("integration: some-provider"), "got: {text}");
    }

    #[tokio::test]
    async fn inspect_cached_renders_image_and_bundle_kinds() {
        let image = CannedService::with_inspect_image(
            Response::Error {
                message: "not running".into(),
            },
            Response::ImageInspected {
                inspection: lns_ipc::ArtifactInspection::Image(lns_ipc::ImageView {
                    reference: "alpine:3.20".into(),
                    digest: "sha256:abc".into(),
                }),
            },
        );
        let mut out = Vec::new();
        inspect(&image, &SandboxInspectArgs { run: "x".into() }, &mut out)
            .await
            .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("kind: Image"), "got: {text}");
        assert!(text.contains("digest: sha256:abc"), "got: {text}");

        let bundle = CannedService::with_inspect_image(
            Response::Error {
                message: "not running".into(),
            },
            Response::ImageInspected {
                inspection: lns_ipc::ArtifactInspection::Bundle(lns_ipc::BundleView {
                    reference: "team/system:1".into(),
                    sandbox_base_image: Some("ghcr.io/team/base:1".into()),
                    filesets: vec![lns_ipc::FilesetView {
                        name: "settings".into(),
                        mount_path: "/root/.agent/settings.json".into(),
                    }],
                    integrations: vec!["some-provider".into()],
                    signature: lns_ipc::SignatureView::SignedUntrusted,
                    policy_flags: vec!["permissive defaultVerdict: allow".into()],
                }),
            },
        );
        let mut out = Vec::new();
        inspect(&bundle, &SandboxInspectArgs { run: "x".into() }, &mut out)
            .await
            .unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("kind: AgentSystem"), "got: {text}");
        assert!(text.contains("image: ghcr.io/team/base:1"), "got: {text}");
        assert!(
            text.contains("settings -> /root/.agent/settings.json"),
            "got: {text}"
        );
        assert!(text.contains("integration: some-provider"), "got: {text}");
        assert!(
            text.contains("signature: signed (untrusted signer)"),
            "got: {text}"
        );
        assert!(
            text.contains("⚠ permissive defaultVerdict: allow"),
            "got: {text}"
        );
    }

    #[tokio::test]
    async fn inspect_cached_rejects_an_unrelated_image_response() {
        let svc = CannedService::with_inspect_image(
            Response::Error {
                message: "not running".into(),
            },
            Response::Pong,
        );
        let mut out = Vec::new();
        let err = inspect(&svc, &SandboxInspectArgs { run: "x".into() }, &mut out)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("unexpected response"));
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
                message: "no active run with id hermes:1.4.0".into(),
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
                message: "no active run with id ghcr.io/team/x:1".into(),
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
                message: "no active run with id reviewer".into(),
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
    async fn prune_with_force_lists_removed_sandboxes_and_reclaimed_bytes() {
        let svc = CannedService::new(Response::ImagesPruned {
            removed: vec!["b:2".into(), "a:1".into()],
            reclaimed_bytes: 64 * 1024 * 1024,
        });
        let mut out = Vec::new();
        let code = prune(&svc, &SandboxPruneArgs { force: true }, &mut out)
            .await
            .unwrap();
        assert_eq!(code, 0);
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("removed a:1") && text.contains("removed b:2"),
            "got: {text}"
        );
        assert!(text.contains("reclaimed 64.0 MiB"), "got: {text}");
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
                    policy_path: Some("/work/lns-policy.yaml".into()),
                    ..Default::default()
                },
            }),
        });
        let mut out = Vec::new();
        let code = inspect(&svc, &SandboxInspectArgs { run: "1".into() }, &mut out)
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
        let doc = policy_doc("/work/lns-policy.yaml", None);
        assert_eq!(doc["path"], "/work/lns-policy.yaml");
        assert!(doc["error"].as_str().unwrap().contains("could not be read"));
    }

    #[test]
    fn policy_doc_embeds_a_parsed_policy() {
        let doc = policy_doc(
            "/work/lns-policy.yaml",
            Some(serde_json::json!({"network": {"defaultVerdict": "ask"}})),
        );
        assert_eq!(doc["policy"]["network"]["defaultVerdict"], "ask");
    }
}
