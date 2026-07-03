use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use lns_ipc::{
    Request, Response, RunDetails, RunStatsInfo, WireFrame, decode_wire_frame_from_bytes,
    read_frame_bytes_async,
};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};

use crate::cli::{DetachChord, ExecArgs, KillArgs, LsArgs, parse_detach_keys_arg};
use crate::command::{CommandSpec, subcommand};
use crate::service::client::BoxFuture;

pub mod real;

#[derive(clap::Args)]
pub struct SandboxArgs {
    #[command(subcommand)]
    pub command: SandboxCommand,
}

#[derive(clap::Subcommand)]
pub enum SandboxCommand {
    #[command(
        visible_aliases = ["list", "ps"],
        about = "List runs (`docker ps`-style): running plus finished-until-removed. Narrow with --filter status=/name=, ids-only with -q, JSON with --format json."
    )]
    Ls(LsArgs),
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
    #[command(about = "Print a run's state and launch configuration as JSON.")]
    Inspect(SandboxInspectArgs),
    #[command(about = "Show a run's CPU and memory usage, sampled over one second.")]
    Stats(SandboxStatsArgs),
    #[command(
        about = "Remove a finished run from the list (`docker rm`-style; refuses running runs)."
    )]
    Rm(SandboxRmArgs),
    #[command(about = "Rename a run (`docker rename`-style); the new name resolves immediately.")]
    Rename(SandboxRenameArgs),
    #[command(about = "Remove all finished runs from the list (`docker container prune`-style).")]
    Prune,
}

#[derive(clap::Args)]
pub struct SandboxStopArgs {
    #[arg(
        value_name = "RUN",
        help = "Target run id or name surfaced by `lns sandbox ls`."
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
        help = "Target run id or name surfaced by `lns sandbox ls`."
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
        help = "Target run id or name surfaced by `lns sandbox ls`."
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
        help = "Target run id or name surfaced by `lns sandbox ls`."
    )]
    pub run: String,
}

#[derive(clap::Args)]
pub struct SandboxStatsArgs {
    #[arg(
        value_name = "RUN",
        help = "Target run id or name surfaced by `lns sandbox ls`."
    )]
    pub run: String,
}

#[derive(clap::Args)]
pub struct SandboxRmArgs {
    #[arg(
        value_name = "RUN",
        help = "Target finished run id or name surfaced by `lns sandbox ls`."
    )]
    pub run: String,
}

#[derive(clap::Args)]
pub struct SandboxRenameArgs {
    #[arg(value_name = "RUN", help = "Run to rename, by current id or name.")]
    pub run: String,

    #[arg(
        value_name = "NEW_NAME",
        help = "New name; must not be all digits and must be unique among listed runs."
    )]
    pub new_name: String,
}

pub fn augment(app: clap::Command) -> clap::Command {
    app.subcommand(subcommand::<SandboxArgs>("sandbox").about(
        "Manage running sandboxes: ls, exec, kill, stop, logs, attach, inspect, stats, rm, rename, prune.",
    ))
}

pub const SPEC: CommandSpec = CommandSpec {
    name: "sandbox",
    augment,
    run: real::run,
    announces_update_check: true,
    owns_terminal: false,
};

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
        SandboxCommand::Ls(args) => ls(svc, args, out).await,
        SandboxCommand::Kill(args) => kill(svc, args, out).await,
        SandboxCommand::Exec(_) => bail!("sandbox exec is dispatched on its own interactive path"),
        SandboxCommand::Stop(args) => stop(svc, args, out).await,
        SandboxCommand::Inspect(args) => inspect(svc, args, out).await,
        SandboxCommand::Stats(args) => stats(svc, args, out).await,
        SandboxCommand::Logs(args) => logs(svc, args, stdout, stderr).await,
        SandboxCommand::Attach(args) => attach(svc, args, term, stdout, stderr).await,
        SandboxCommand::Rm(args) => rm(svc, args, out).await,
        SandboxCommand::Rename(args) => rename(svc, args, out).await,
        SandboxCommand::Prune => prune(svc, out).await,
    }
}

pub(crate) fn run_label(run: &str) -> String {
    run.to_string()
}

async fn ls<W: std::io::Write>(
    svc: &impl SandboxService,
    args: &LsArgs,
    out: &mut W,
) -> Result<i32> {
    let response = svc.one_shot(Request::ListRuns).await?;
    match response {
        Response::RunList { runs } => {
            crate::service::render_run_list(out, &runs, args)?;
            Ok(0)
        }
        Response::Error { message } => bail!("daemon error: {message}"),
        other => bail!("unexpected response from daemon: {other:?}"),
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
            let run = run_label(&args.run);
            let secs = args.timeout;
            writeln!(out, "killed run {run} after the {secs}s timeout")?;
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
    let response = svc
        .one_shot(Request::RemoveRun {
            run: args.run.clone(),
        })
        .await?;
    match response {
        Response::Acknowledged => {
            writeln!(out, "removed run {}", run_label(&args.run))?;
            Ok(0)
        }
        Response::Error { message } => bail!("daemon error: {message}"),
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

async fn rename<W: std::io::Write>(
    svc: &impl SandboxService,
    args: &SandboxRenameArgs,
    out: &mut W,
) -> Result<i32> {
    let response = svc
        .one_shot(Request::RenameRun {
            run: args.run.clone(),
            new_name: args.new_name.clone(),
        })
        .await?;
    match response {
        Response::Acknowledged => {
            let run = run_label(&args.run);
            let new_name = &args.new_name;
            writeln!(out, "renamed run {run} to {new_name}")?;
            Ok(0)
        }
        Response::Error { message } => bail!("daemon error: {message}"),
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

async fn prune<W: std::io::Write>(svc: &impl SandboxService, out: &mut W) -> Result<i32> {
    let response = svc.one_shot(Request::PruneRuns).await?;
    match response {
        Response::RunsPruned { mut removed } => {
            if removed.is_empty() {
                writeln!(out, "no finished runs to remove")?;
            } else {
                removed.sort_unstable();
                for id in removed {
                    writeln!(out, "removed run {}", lns_ipc::short_run_id(&id))?;
                }
            }
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
    let response = svc
        .one_shot(Request::InspectRun {
            run: args.run.clone(),
        })
        .await?;
    match response {
        Response::RunInspect { details } => {
            let policy = details
                .config
                .policy_path
                .as_deref()
                .map(|path| policy_doc(path, svc.load_policy(path)));
            render_inspect(&details, policy, out)?;
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
    out: &mut W,
) -> Result<()> {
    let mut config = serde_json::Map::new();
    config.insert("cpus".into(), details.config.cpus.into());
    config.insert("memMib".into(), details.config.mem_mib.into());
    config.insert("env".into(), serde_json::to_value(&details.config.env)?);
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
    doc.insert("config".into(), config.into());
    doc.insert("policy".into(), policy.unwrap_or(serde_json::Value::Null));
    let rendered = serde_json::to_string_pretty(&serde_json::Value::Object(doc))?;
    writeln!(out, "{rendered}")?;
    Ok(())
}

async fn stats<W: std::io::Write>(
    svc: &impl SandboxService,
    args: &SandboxStatsArgs,
    out: &mut W,
) -> Result<i32> {
    let response = svc
        .one_shot(Request::RunStats {
            run: args.run.clone(),
        })
        .await?;
    match response {
        Response::RunStats { stats } => {
            render_stats(&stats, out)?;
            Ok(0)
        }
        Response::Error { message } => bail!("daemon error: {message}"),
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

fn render_stats<W: std::io::Write>(stats: &RunStatsInfo, out: &mut W) -> Result<()> {
    writeln!(out, "CPU %   MEM USAGE / LIMIT")?;
    writeln!(
        out,
        "{:<6}  {} / {}",
        format_permille(stats.cpu_permille),
        format_bytes(stats.mem_used_bytes),
        format_bytes(stats.mem_total_bytes),
    )?;
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
        frames: Vec<Vec<u8>>,
        requests: Arc<Mutex<Vec<Request>>>,
    }

    impl CannedService {
        fn new(response: Response) -> Self {
            Self {
                response,
                frames: Vec::new(),
                requests: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn with_frames(frames: Vec<Vec<u8>>) -> Self {
            Self {
                response: Response::Pong,
                frames,
                requests: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    impl SandboxService for CannedService {
        type Stream = tokio::io::DuplexStream;
        fn one_shot(&self, request: Request) -> BoxFuture<'_, Result<Response>> {
            self.requests.lock().unwrap().push(request);
            let response = self.response.clone();
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

    fn ls_args() -> LsArgs {
        LsArgs {
            all: false,
            quiet: false,
            filters: Vec::new(),
            format: None,
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
    async fn stats_rejects_an_unrelated_response_variant() {
        let svc = CannedService::new(Response::Pong);
        let mut out = Vec::new();
        let err = stats(&svc, &SandboxStatsArgs { run: "1".into() }, &mut out)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("unexpected response"));
    }

    #[tokio::test]
    async fn stats_surfaces_a_daemon_error() {
        let svc = CannedService::new(Response::Error {
            message: "macOS-only".into(),
        });
        let mut out = Vec::new();
        let err = stats(&svc, &SandboxStatsArgs { run: "1".into() }, &mut out)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("macOS-only"));
    }

    #[tokio::test]
    async fn rm_rejects_an_unrelated_response_variant() {
        let svc = CannedService::new(Response::Pong);
        let mut out = Vec::new();
        let err = rm(&svc, &SandboxRmArgs { run: "1".into() }, &mut out)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("unexpected response"));
    }

    #[tokio::test]
    async fn rename_rejects_an_unrelated_response_variant() {
        let svc = CannedService::new(Response::Pong);
        let mut out = Vec::new();
        let err = rename(
            &svc,
            &SandboxRenameArgs {
                run: "reviewer".into(),
                new_name: "auditor".into(),
            },
            &mut out,
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("unexpected response"));
    }

    #[tokio::test]
    async fn rename_confirms_with_the_handle_and_new_name() {
        let svc = CannedService::new(Response::Acknowledged);
        let mut out = Vec::new();
        let code = rename(
            &svc,
            &SandboxRenameArgs {
                run: "reviewer".into(),
                new_name: "auditor".into(),
            },
            &mut out,
        )
        .await
        .unwrap();
        assert_eq!(code, 0);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "renamed run reviewer to auditor\n"
        );
    }

    #[tokio::test]
    async fn prune_surfaces_a_daemon_error() {
        let svc = CannedService::new(Response::Error {
            message: "registry poisoned".into(),
        });
        let mut out = Vec::new();
        let err = prune(&svc, &mut out).await.unwrap_err();
        assert!(format!("{err:#}").contains("registry poisoned"));
    }

    #[tokio::test]
    async fn prune_rejects_an_unrelated_response_variant() {
        let svc = CannedService::new(Response::Pong);
        let mut out = Vec::new();
        let err = prune(&svc, &mut out).await.unwrap_err();
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
                    published_ports: Vec::new(),
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
