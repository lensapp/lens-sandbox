use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use lns_ipc::{
    LogLevel, Request, Response, SignalKind, WireFrame, decode_frame, decode_wire_frame_from_bytes,
    encode_frame, read_frame_bytes_async,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tokio::time::timeout;

use clap::FromArgMatches;

use crate::chord::{DetachChordDetector, FeedAction};
use crate::cli::{ExecArgs, RunArgs};
use crate::command::{RunCtx, RunFuture};
use lns_ipc::{ExecImageArgs, RunImageArgs};

use super::{client::ServiceClient, real, require_running_check};
use crate::run::summary::print_run_summary;

const PUBLISHED_PREFLIGHT_TIMEOUT: Duration = Duration::from_secs(30);

pub fn run_command<'a>(matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = RunArgs::from_arg_matches(matches)?;
        launch_run(args, ctx.debug).await
    })
}

pub async fn launch_run(mut args: RunArgs, debug: bool) -> Result<i32> {
    args.env = crate::run::env_file::merged_run_env(&args.env_file, &args.env)?;
    let config_path = crate::config::default_config_path()?;
    let defaults = crate::config::load_run_defaults(&config_path)?;
    let args = crate::config::apply_run_defaults(args, defaults);
    let cwd = std::env::current_dir().context("reading current directory")?;
    // Target resolution is offline; a missing or invalid definition fails fast, before the service gate.
    let target = crate::run::target::resolve(
        args.image.as_deref(),
        args.file.as_deref(),
        &crate::sandbox::real::RealFs,
        &cwd,
    )?;
    require_running().await;
    run_image(args, target, cwd, debug).await
}

pub fn exec_command<'a>(matches: &'a clap::ArgMatches, _ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = ExecArgs::from_arg_matches(matches)?;
        require_running().await;
        exec_image(args).await
    })
}

pub fn service_command<'a>(matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = super::ServiceArgs::from_arg_matches(matches)?;
        let mut out = ctx.out;
        dispatch(&args.command, &mut out).await?;
        Ok(0)
    })
}

pub async fn dispatch(cmd: &super::ServiceCommand, writer: &mut dyn std::io::Write) -> Result<()> {
    let client = real_client()?;
    match cmd {
        super::ServiceCommand::Start => super::cmd_start(&client).await,
        super::ServiceCommand::Stop => super::cmd_stop(&client).await,
        super::ServiceCommand::Status(args) => super::cmd_status(&client, args, writer).await,
        super::ServiceCommand::Enable => super::cmd_enable(&client).await,
        super::ServiceCommand::Disable => super::cmd_disable(&client).await,
    }
}

pub async fn require_running() {
    let client = match real_client() {
        Ok(c) => c,
        Err(e) => {
            crate::log::error!("{e}");
            std::process::exit(1);
        }
    };
    let alive = client.ping().await;
    if let Err(msg) = require_running_check(alive) {
        crate::log::error!("{msg}");
        std::process::exit(1);
    }
}
fn real_client() -> Result<real::RealServiceClient> {
    Ok(real::RealServiceClient::new(
        super::socket_path()?,
        super::find_service_binary(),
    ))
}

/// A detached run must never block on the KEEP/DROP prompt, so it drops undecided secrets like a no-terminal run even when launched from a TTY.
fn host_binds_interactive(detached: bool, stdin_is_tty: bool) -> bool {
    !detached && stdin_is_tty
}

fn resolve_host_binds(
    specs: &[lns_ipc::BindSpec],
    interactive: bool,
) -> Result<Vec<crate::run::host_bind::ResolvedBind>> {
    if specs.is_empty() {
        return Ok(Vec::new());
    }
    let scan = crate::run::host_bind::RealDirScan;
    let store = lns_policy::host_bind_decisions::JsonFileHostBindDecisionStore::new(
        lns_policy::host_bind_decisions::default_host_bind_decisions_path(),
    );
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    crate::run::host_bind::resolve_binds(
        specs,
        &scan,
        &store,
        interactive,
        &mut input,
        &mut std::io::stderr(),
    )
}

#[derive(Debug)]
struct PublishedTarget {
    image: String,
    defaults: crate::run::declarative::Defaults,
    filesets: Vec<crate::run::summary::FilesetSummary>,
    tools: Vec<String>,
}

fn published_target(
    reference: &str,
    inspection: lns_ipc::ArtifactInspection,
) -> Result<PublishedTarget> {
    match inspection {
        lns_ipc::ArtifactInspection::Sandbox(view) => {
            if view.digest.is_empty() {
                anyhow::bail!("published sandbox preflight returned no manifest digest");
            }
            let parsed: oci_client::Reference = reference
                .parse()
                .with_context(|| format!("invalid sandbox reference {reference}"))?;
            Ok(PublishedTarget {
                image: parsed.clone_with_digest(view.digest.clone()).to_string(),
                defaults: crate::run::declarative::Defaults::from_view(&view),
                filesets: crate::run::summary::fileset_summaries_from_view(&view),
                tools: crate::run::summary::tools_from_view(&view),
            })
        }
        lns_ipc::ArtifactInspection::Image(_) => anyhow::bail!(
            "{reference} is not a sandbox; run `lns init` to author an lns.yaml, or pass a published sandbox reference"
        ),
    }
}

async fn preflight_published(socket: &Path, reference: &str) -> Result<PublishedTarget> {
    let request = Request::InspectImage {
        image: reference.to_string(),
    };
    await_published_preflight(reference, real::send_request(socket, &request)).await
}

async fn await_published_preflight(
    reference: &str,
    response: impl std::future::Future<Output = Option<Response>>,
) -> Result<PublishedTarget> {
    let response = timeout(PUBLISHED_PREFLIGHT_TIMEOUT, response)
        .await
        .map_err(|_| {
            anyhow::anyhow!(
                "published sandbox preflight timed out after {}s while inspecting {reference}",
                PUBLISHED_PREFLIGHT_TIMEOUT.as_secs()
            )
        })?;
    match response {
        Some(Response::ImageInspected { inspection }) => published_target(reference, inspection),
        Some(Response::Error { message }) => anyhow::bail!("daemon error: {message}"),
        Some(other) => anyhow::bail!("expected sandbox preflight, got {other:?}"),
        None => anyhow::bail!("no response from lns-service during sandbox preflight"),
    }
}

pub async fn run_image(
    mut args: RunArgs,
    target: crate::run::target::RunTarget,
    cwd: std::path::PathBuf,
    debug: bool,
) -> Result<i32> {
    let client = real_client()?;
    let published = match &target {
        crate::run::target::RunTarget::Reference(reference) => {
            Some(preflight_published(client.socket(), reference).await?)
        }
        crate::run::target::RunTarget::Local { .. } => {
            // A path-shaped REF resolved to a definition; the summary names its image, not the path.
            if args.image.is_some() {
                args.image = Some(target.image());
            }
            None
        }
    };
    let defaults = match (&target, &published) {
        (crate::run::target::RunTarget::Local { def, .. }, _) => {
            crate::run::declarative::Defaults::from_definition(
                def,
                lns_artifact::resources::host::probe(),
            )
        }
        (_, Some(published)) => published.defaults.clone(),
        _ => crate::run::declarative::Defaults::default(),
    };
    let consumer_mount_targets: Vec<String> = args
        .mounts
        .iter()
        .map(|mount| mount.target().to_string())
        .collect();
    let resolved = crate::run::declarative::resolve(
        &defaults,
        target.project_dir().unwrap_or(&cwd),
        dirs::home_dir().as_deref(),
        args.workdir.take(),
        std::mem::take(&mut args.mounts),
    )?;
    args.workdir = resolved.workdir;
    args.mounts = resolved.mounts;
    let local_definition = matches!(&target, crate::run::target::RunTarget::Local { .. });
    let composed = crate::run::declarative::compose_ports(
        &defaults.ports,
        std::mem::take(&mut args.publish),
        local_definition || args.publish_declared,
    )?;
    args.publish = composed.published;
    args.declared_unpublished = composed.declared_unpublished;
    args.filesets = match (&target, &published) {
        (crate::run::target::RunTarget::Local { def, .. }, _) => def
            .spec
            .filesets
            .iter()
            .map(|fileset| crate::run::summary::FilesetSummary {
                source: crate::run::summary::fileset_source_display(fileset),
                mount_path: fileset.mount_path.clone(),
                owner: crate::run::summary::fileset_owner_display(fileset.owner).to_string(),
                from_host: fileset.host_path.is_some(),
            })
            .collect(),
        (_, Some(published)) => published.filesets.clone(),
        _ => Vec::new(),
    };
    args.tools = match (&target, &published) {
        (crate::run::target::RunTarget::Local { def, .. }, _) => def.spec.tools.clone(),
        (_, Some(published)) => published.tools.clone(),
        _ => Vec::new(),
    };
    // The size travels as its own value: writing it back into args.cpus/args.mem would tell the service the user asked for it explicitly.
    let size = crate::run::summary::resolved_size(defaults.size, &args);
    let quiet = args.quiet;
    let resolved_policy = if quiet {
        let (path, _source) = crate::run::summary::resolve_policy(args.policy.as_deref(), &cwd)?;
        path
    } else {
        print_run_summary(&args, size, &cwd, &mut std::io::stderr())?
    };

    let (volumes, bind_specs) = crate::cli::split_mounts(&args.mounts);
    let interactive = host_binds_interactive(args.detach, crate::raw_mode::stdin_is_tty());
    if published.is_some() {
        let (declared_volumes, declared_binds) = crate::run::pull_confirm::artifact_declared_mounts(
            &args.mounts,
            &consumer_mount_targets,
        );
        let reference = target.image();
        let effects = crate::run::pull_confirm::PulledEffects {
            reference: &reference,
            binds: &declared_binds,
            volumes: &declared_volumes,
            filesets: &args.filesets,
            tools: &args.tools,
        };
        let stdin = std::io::stdin();
        let mut input = stdin.lock();
        crate::run::pull_confirm::confirm_pulled_effects(
            &effects,
            args.assume_yes,
            interactive,
            &mut input,
            &mut std::io::stderr(),
        )?;
    }
    let resolved_binds = resolve_host_binds(&bind_specs, interactive)?;
    if !quiet {
        let dispositions = crate::run::summary::format_bind_dispositions(&resolved_binds);
        if !dispositions.is_empty() {
            eprint!("{dispositions}");
        }
    }
    let binds: Vec<lns_ipc::BindMount> = resolved_binds.iter().map(|b| b.to_wire()).collect();

    let socket = client.socket();
    let mut stream = UnixStream::connect(socket)
        .await
        .with_context(|| format!("connecting to {}", socket.display()))?;

    let detached = args.detach;
    let tty = !detached && args.tty && crate::raw_mode::stdin_is_tty();
    let stdin = !detached && args.interactive;
    let initial_winsize = if tty {
        crate::raw_mode::host_winsize()
    } else {
        None
    };
    let detach_chord = args.detach_keys.0.clone();
    let sandbox_user = args.effective_sandbox_user();
    let sandbox_uid = args.effective_sandbox_uid();

    let request = Request::RunImage(Box::new(RunImageArgs {
        cpus: args.effective_cpus(),
        mem: args.effective_mem(),
        cpus_explicit: args.cpus.is_some(),
        mem_explicit: args.mem.is_some(),
        cmd: args.cmd,
        env: args.env,
        image: Some(target.image()),
        resolved_image: published.as_ref().map(|published| published.image.clone()),
        name: args.name,
        policy_path: Some(resolved_policy.to_string_lossy().into_owned()),
        sandbox_user,
        sandbox_uid,
        entrypoint: args.entrypoint,
        hostname: args.hostname,
        workdir: args.workdir,
        debug,
        tty,
        stdin,
        initial_winsize,
        detached,
        published_ports: args.publish,
        volumes,
        binds,
        auto_remove: args.auto_remove,
        verify_sandbox: target.verify_sandbox(),
        definition: target.definition_json(),
        definition_dir: target
            .project_dir()
            .map(|p| p.to_string_lossy().into_owned()),
    }));
    let frame = encode_frame(&request).context("encoding RunImage request")?;
    stream
        .write_all(&frame)
        .await
        .context("writing RunImage request")?;

    let bytes = read_frame_bytes_async(&mut stream)
        .await
        .context("reading RunStarted frame")?;
    let run_id = match decode_frame(&mut &bytes[..]).context("decoding RunStarted")? {
        Response::RunStarted { run_id } => run_id,
        Response::Error { message } => anyhow::bail!("daemon error: {message}"),
        other => anyhow::bail!("expected RunStarted, got {other:?}"),
    };

    let mut progress =
        crate::run::progress::ProgressRenderer::new(!quiet && std::io::stderr().is_terminal());
    let outcome =
        drive_pre_phase(&mut stream, &mut std::io::stderr(), &mut progress, quiet).await?;
    if let PrePhaseOutcome::EarlyExit(code) = outcome {
        return Ok(code);
    }

    render_started_run(&run_id, &mut std::io::stderr(), quiet).ok();

    if detached {
        println!("{run_id}");
        return Ok(0);
    }

    drive_attached_session(
        stream,
        Some(socket.to_path_buf()),
        run_id,
        tty,
        detach_chord,
        DetachBehaviour::DetachRun,
        quiet,
        StdinForwarding::of(stdin),
    )
    .await
}

pub async fn exec_image(args: ExecArgs) -> Result<i32> {
    if args.cmd.is_empty() {
        anyhow::bail!("lns exec requires a command after `--`");
    }

    let socket = super::socket_path()?;
    let mut stream = UnixStream::connect(&socket)
        .await
        .with_context(|| format!("connecting to {}", socket.display()))?;

    let target_run = args.run;
    let tty = args.tty && crate::raw_mode::stdin_is_tty();
    let stdin = args.interactive;
    let initial_winsize = if tty {
        crate::raw_mode::host_winsize()
    } else {
        None
    };
    let detach_chord = args.detach_keys.0.clone();

    let request = Request::ExecImage(ExecImageArgs {
        run: target_run,
        argv: args.cmd,
        env: Vec::new(),
        tty,
        stdin,
        initial_winsize,
    });
    let frame = encode_frame(&request).context("encoding ExecImage request")?;
    stream
        .write_all(&frame)
        .await
        .context("writing ExecImage request")?;

    let bytes = read_frame_bytes_async(&mut stream)
        .await
        .context("reading RunStarted frame")?;
    let run_id = match decode_frame(&mut &bytes[..]).context("decoding RunStarted")? {
        Response::RunStarted { run_id } => run_id,
        Response::Error { message } => anyhow::bail!("daemon error: {message}"),
        other => anyhow::bail!("expected RunStarted, got {other:?}"),
    };
    crate::log::debug!(run_id = %run_id, "exec session opened");

    drive_attached_session(
        stream,
        Some(socket),
        run_id,
        tty,
        detach_chord,
        DetachBehaviour::SignalAndDrain,
        args.quiet,
        StdinForwarding::of(stdin),
    )
    .await
}

/// Whether host stdin reaches the session's workload; a session opened without `-i` still watches for the detach chord but hands the run nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdinForwarding {
    ToRun,
    Withheld,
}

impl StdinForwarding {
    fn of(interactive: bool) -> Self {
        if interactive {
            Self::ToRun
        } else {
            Self::Withheld
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetachBehaviour {
    SignalAndDrain,
    LeaveRunning,
    DetachRun,
}

#[allow(clippy::too_many_arguments)] // one session-shape argument per wire field; the writer seam lives in the _with_writers twin
pub(crate) async fn drive_attached_session<S>(
    stream: S,
    aux_socket: Option<PathBuf>,
    run_id: String,
    tty: bool,
    detach_chord: Vec<u8>,
    detach: DetachBehaviour,
    quiet: bool,
    stdin: StdinForwarding,
) -> Result<i32>
where
    S: AsyncReadExt + AsyncWriteExt + Unpin + Send + 'static,
{
    let mut stdout = tokio::io::stdout();
    let mut stderr = tokio::io::stderr();
    drive_attached_session_with_writers(
        stream,
        aux_socket,
        run_id,
        tty,
        std::io::stdout().is_terminal(),
        detach_chord,
        detach,
        &mut stdout,
        &mut stderr,
        quiet,
        stdin,
    )
    .await
}

#[allow(clippy::too_many_arguments)] // explicit stream/writer/terminal seam so the attached-session loop is host-testable over in-memory duplexes
pub async fn drive_attached_session_with_writers<S, O, E>(
    mut stream: S,
    aux_socket: Option<PathBuf>,
    run_id: String,
    tty: bool,
    stdout_is_terminal: bool,
    detach_chord: Vec<u8>,
    detach: DetachBehaviour,
    stdout: &mut O,
    stderr: &mut E,
    quiet: bool,
    stdin: StdinForwarding,
) -> Result<i32>
where
    S: AsyncReadExt + AsyncWriteExt + Unpin + Send + 'static,
    O: AsyncWriteExt + Unpin,
    E: AsyncWriteExt + Unpin,
{
    let _raw_guard = if tty {
        crate::raw_mode::RawModeGuard::enable_if_tty()
    } else {
        None
    };

    let (early_exit_tx, mut early_exit_rx) = mpsc::unbounded_channel::<()>();

    let (cancel_task, winsize_task, stdin_task) = match aux_socket {
        Some(socket) => {
            let cancel_client = real_client()?;
            let cancel_run_id = run_id.clone();
            let cancel = tokio::spawn(async move {
                let _ = tokio::signal::ctrl_c().await;
                cancel_client.cancel_run(cancel_run_id).await;
            });
            let winsize_run_id = run_id.clone();
            let winsize = tty.then(|| {
                let s = socket.clone();
                tokio::spawn(async move { run_winsize_forwarder(s, winsize_run_id).await })
            });
            let pump_chord = detach_chord;
            let pump_early_exit = early_exit_tx.clone();
            let stdin_task = tokio::spawn(async move {
                let r = run_stdin_pump(socket, run_id, pump_chord, detach, stdin).await;
                if matches!(&r, Ok(true)) {
                    let _ = pump_early_exit.send(());
                }
                r
            });
            (Some(cancel), winsize, Some(stdin_task))
        }
        None => (None, None, None),
    };

    let mut last_stdout_byte: Option<u8> = None;

    let exit_code = loop {
        tokio::select! {
            biased;
            _ = early_exit_rx.recv() => {
                break exit_code_after_detach(
                    detach,
                    tty,
                    &mut stream,
                    stdout,
                    stderr,
                    &mut last_stdout_byte,
                    quiet,
                )
                .await;
            }
            res = read_frame_bytes_async(&mut stream) => {
                let bytes = res.context("reading run-response frame")?;
                let wire = decode_wire_frame_from_bytes(&bytes).context("decoding run-response")?;
                match wire {
                    WireFrame::Stdout(b) => {
                        last_stdout_byte = b.last().copied().or(last_stdout_byte);
                        stdout.write_all(&b).await?;
                        stdout.flush().await.ok();
                    }
                    WireFrame::Stderr(b) => {
                        stderr.write_all(&b).await?;
                        stderr.flush().await.ok();
                    }
                    WireFrame::Json(Response::RunLog { level, verb, message }) => {
                        render_attached_run_log(level, verb.as_deref(), &message, tty, stderr, quiet)
                            .await?;
                    }
                    WireFrame::Json(Response::RunProgress { .. }) => {}
                    WireFrame::Json(Response::RunExit { code }) => break code,
                    WireFrame::Json(Response::Error { message }) => {
                        anyhow::bail!("daemon error: {message}")
                    }
                    other => anyhow::bail!("unexpected response from daemon: {other:?}"),
                }
            }
        }
    };

    if !tty && stdout_is_terminal && needs_final_newline(last_stdout_byte) {
        stdout.write_all(b"\n").await.ok();
        stdout.flush().await.ok();
    }

    if let Some(t) = stdin_task {
        t.abort();
    }
    if let Some(t) = winsize_task {
        t.abort();
    }
    if let Some(t) = cancel_task {
        t.abort();
    }
    Ok(exit_code)
}

fn needs_final_newline(last_byte: Option<u8>) -> bool {
    matches!(last_byte, Some(b) if b != b'\n')
}

async fn render_attached_run_log<E>(
    level: LogLevel,
    verb: Option<&str>,
    message: &str,
    tty: bool,
    stderr: &mut E,
    quiet: bool,
) -> Result<()>
where
    E: AsyncWriteExt + Unpin,
{
    if matches!(level, LogLevel::Debug) {
        crate::log::debug!("{message}");
        return Ok(());
    }
    let mut line = Vec::<u8>::new();
    render_status_line(level, verb, message, &mut line, quiet)?;
    if line.is_empty() {
        return Ok(());
    }
    if tty {
        line = lf_to_crlf(&line);
    }
    stderr.write_all(&line).await?;
    stderr.flush().await.ok();
    Ok(())
}

fn lf_to_crlf(line: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(line.len() + 2);
    for &b in line {
        if b == b'\n' {
            out.push(b'\r');
        }
        out.push(b);
    }
    out
}

async fn exit_code_after_detach<S, O, E>(
    detach: DetachBehaviour,
    tty: bool,
    stream: &mut S,
    stdout: &mut O,
    stderr: &mut E,
    last_stdout_byte: &mut Option<u8>,
    quiet: bool,
) -> i32
where
    S: AsyncReadExt + Unpin,
    O: AsyncWriteExt + Unpin,
    E: AsyncWriteExt + Unpin,
{
    match detach {
        DetachBehaviour::SignalAndDrain => {
            drain_after_chord(stream, tty, stdout, stderr, last_stdout_byte, quiet).await
        }
        DetachBehaviour::LeaveRunning | DetachBehaviour::DetachRun => 0,
    }
}

async fn drain_after_chord<S, O, E>(
    stream: &mut S,
    tty: bool,
    stdout: &mut O,
    stderr: &mut E,
    last_stdout_byte: &mut Option<u8>,
    quiet: bool,
) -> i32
where
    S: AsyncReadExt + Unpin,
    O: AsyncWriteExt + Unpin,
    E: AsyncWriteExt + Unpin,
{
    match timeout(
        Duration::from_secs(5),
        drain_to_exit(stream, tty, stdout, stderr, last_stdout_byte, quiet),
    )
    .await
    {
        Ok(Ok(code)) => code,
        Ok(Err(e)) => {
            crate::log::debug!("post-chord drain error: {e}");
            0
        }
        Err(_) => {
            crate::log::debug!("post-chord drain timed out; returning 0");
            0
        }
    }
}

async fn drain_to_exit<S, O, E>(
    stream: &mut S,
    tty: bool,
    stdout: &mut O,
    stderr: &mut E,
    last_stdout_byte: &mut Option<u8>,
    quiet: bool,
) -> Result<i32>
where
    S: AsyncReadExt + Unpin,
    O: AsyncWriteExt + Unpin,
    E: AsyncWriteExt + Unpin,
{
    loop {
        let bytes = match read_frame_bytes_async(stream).await {
            Ok(b) => b,
            Err(_) => return Ok(0),
        };
        let wire = decode_wire_frame_from_bytes(&bytes).context("decoding drain-frame")?;
        match wire {
            WireFrame::Stdout(b) => {
                *last_stdout_byte = b.last().copied().or(*last_stdout_byte);
                let _ = stdout.write_all(&b).await;
            }
            WireFrame::Stderr(b) => {
                let _ = stderr.write_all(&b).await;
            }
            WireFrame::Json(Response::RunLog {
                level,
                verb,
                message,
            }) => {
                render_attached_run_log(level, verb.as_deref(), &message, tty, stderr, quiet)
                    .await?;
            }
            WireFrame::Json(Response::RunExit { code }) => return Ok(code),
            WireFrame::Json(Response::Error { message }) => {
                crate::log::debug!("drain error from daemon: {message}");
                return Ok(0);
            }
            _ => {}
        }
    }
}

async fn run_stdin_pump(
    socket: PathBuf,
    run_id: String,
    detach_chord: Vec<u8>,
    detach: DetachBehaviour,
    stdin: StdinForwarding,
) -> Result<bool> {
    let mut input = tokio::io::stdin();
    let mut buf = [0u8; 4096];
    let mut detector = (!detach_chord.is_empty()).then(|| DetachChordDetector::new(detach_chord));
    loop {
        let n = match input.read(&mut buf).await {
            Ok(0) => {
                if let Some(d) = detector.as_mut() {
                    let held = d.drain_for_eof();
                    if !held.is_empty() && stdin == StdinForwarding::ToRun {
                        let _ = send_one_shot(
                            &socket,
                            &Request::RunStdin {
                                run_id: run_id.clone(),
                                bytes: held,
                            },
                        )
                        .await;
                    }
                }
                return Ok(false);
            }
            Ok(n) => n,
            Err(e) => {
                crate::log::debug!("stdin read error: {e}");
                return Err(e.into());
            }
        };
        let chunk = &buf[..n];
        match detector.as_mut() {
            Some(d) => {
                if !pump_with_detector(&socket, &run_id, chunk, d, detach, stdin).await? {
                    return Ok(true);
                }
            }
            None => {
                if stdin == StdinForwarding::ToRun {
                    send_one_shot(
                        &socket,
                        &Request::RunStdin {
                            run_id: run_id.clone(),
                            bytes: chunk.to_vec(),
                        },
                    )
                    .await?;
                }
            }
        }
    }
}

async fn pump_with_detector(
    socket: &Path,
    run_id: &str,
    bytes: &[u8],
    detector: &mut DetachChordDetector,
    detach: DetachBehaviour,
    stdin: StdinForwarding,
) -> Result<bool> {
    let mut pending: Vec<u8> = Vec::new();
    for &b in bytes {
        let (requests, control) = plan_feed(detector.feed(b), run_id, &mut pending, detach, stdin);
        for request in &requests {
            send_one_shot(socket, request).await?;
        }
        if matches!(control, PumpControl::Detach) {
            return Ok(false);
        }
    }
    if stdin == StdinForwarding::ToRun
        && let Some(request) = drain_pending(run_id, &mut pending)
    {
        send_one_shot(socket, &request).await?;
    }
    Ok(true)
}

enum PumpControl {
    Continue,
    Detach,
}

fn drain_pending(run_id: &str, pending: &mut Vec<u8>) -> Option<Request> {
    if pending.is_empty() {
        None
    } else {
        Some(Request::RunStdin {
            run_id: run_id.to_string(),
            bytes: std::mem::take(pending),
        })
    }
}

fn plan_feed(
    action: FeedAction,
    run_id: &str,
    pending: &mut Vec<u8>,
    detach: DetachBehaviour,
    stdin: StdinForwarding,
) -> (Vec<Request>, PumpControl) {
    let stdin_requests = |bytes: Vec<u8>| match stdin {
        StdinForwarding::ToRun => vec![Request::RunStdin {
            run_id: run_id.to_string(),
            bytes,
        }],
        StdinForwarding::Withheld => Vec::new(),
    };
    let drained = |pending: &mut Vec<u8>| match stdin {
        StdinForwarding::ToRun => drain_pending(run_id, pending).into_iter().collect(),
        StdinForwarding::Withheld => {
            pending.clear();
            Vec::<Request>::new()
        }
    };
    match action {
        FeedAction::Forward(byte) => {
            pending.push(byte);
            (Vec::new(), PumpControl::Continue)
        }
        FeedAction::Hold => (drained(pending), PumpControl::Continue),
        FeedAction::Flush(held) => {
            let mut requests = drained(pending);
            requests.extend(stdin_requests(held));
            (requests, PumpControl::Continue)
        }
        FeedAction::FlushAndForward(held, current) => {
            let mut requests = drained(pending);
            let mut combined = held;
            combined.push(current);
            requests.extend(stdin_requests(combined));
            (requests, PumpControl::Continue)
        }
        FeedAction::Trigger => {
            let mut requests = drained(pending);
            match detach {
                DetachBehaviour::SignalAndDrain => requests.push(Request::RunSignal {
                    run_id: run_id.to_string(),
                    signal: SignalKind::Hup,
                }),
                DetachBehaviour::DetachRun => requests.push(Request::RunDetach {
                    run_id: run_id.to_string(),
                }),
                DetachBehaviour::LeaveRunning => {}
            }
            (requests, PumpControl::Detach)
        }
    }
}

async fn run_winsize_forwarder(socket: PathBuf, run_id: String) -> Result<()> {
    use tokio::signal::unix::{SignalKind as TokioSig, signal};
    let mut sigwinch = match signal(TokioSig::window_change()) {
        Ok(s) => s,
        Err(e) => {
            crate::log::warn!("SIGWINCH watcher not installed: {e}");
            return Ok(());
        }
    };
    while sigwinch.recv().await.is_some() {
        let Some((rows, cols)) = crate::raw_mode::host_winsize() else {
            continue;
        };
        if send_one_shot(
            &socket,
            &Request::RunResize {
                run_id: run_id.clone(),
                rows,
                cols,
            },
        )
        .await
        .is_err()
        {
            return Ok(());
        }
    }
    Ok(())
}

async fn send_one_shot(socket: &Path, request: &Request) -> Result<()> {
    let mut stream = UnixStream::connect(socket)
        .await
        .with_context(|| format!("connecting to {} for one-shot", socket.display()))?;
    write_and_await_ack(&mut stream, request).await
}

async fn write_and_await_ack<S>(stream: &mut S, request: &Request) -> Result<()>
where
    S: AsyncReadExt + AsyncWriteExt + Unpin,
{
    let frame = encode_frame(request).context("encoding one-shot request")?;
    stream
        .write_all(&frame)
        .await
        .context("writing one-shot request")?;
    // Awaiting the reply is load-bearing for RunDetach: it blocks until the service has fired detach_tx, so the CLI never closes the run stream first and trips WriteFailed → CancelAndDeregister.
    let _ = read_frame_bytes_async(stream).await;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrePhaseOutcome {
    SessionReady,
    EarlyExit(i32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrePhaseStep {
    Continue,
    SessionReady,
    EarlyExit(i32),
}

pub fn pre_phase_step<W>(
    bytes: &[u8],
    writer: &mut W,
    progress: &mut crate::run::progress::ProgressRenderer,
    quiet: bool,
) -> Result<PrePhaseStep>
where
    W: std::io::Write,
{
    let wire = decode_wire_frame_from_bytes(bytes).context("decoding pre-phase frame")?;
    if let WireFrame::Json(Response::RunProgress {
        verb,
        message,
        current,
        total,
    }) = wire
    {
        progress.update(&verb, &message, current, total, writer)?;
        return Ok(PrePhaseStep::Continue);
    }
    progress.clear(writer)?;
    match wire {
        WireFrame::Json(Response::RunLog {
            level,
            verb,
            message,
        }) => {
            render_status_line(level, verb.as_deref(), &message, writer, quiet)?;
            if verb.as_deref() == Some("SessionReady") {
                Ok(PrePhaseStep::SessionReady)
            } else {
                Ok(PrePhaseStep::Continue)
            }
        }
        WireFrame::Json(Response::OauthVerification {
            verification_uri,
            user_code,
            expires_in_secs,
        }) => {
            writeln!(
                writer,
                "Open {verification_uri} and enter code {user_code} (expires in {}m)",
                expires_in_secs / 60
            )?;
            Ok(PrePhaseStep::Continue)
        }
        WireFrame::Json(Response::OauthBrowserOpened { authorization_url }) => {
            writeln!(
                writer,
                "Opening your browser to authorize… (if it didn't open, visit {authorization_url})"
            )?;
            Ok(PrePhaseStep::Continue)
        }
        WireFrame::Json(Response::RunExit { code }) => Ok(PrePhaseStep::EarlyExit(code)),
        WireFrame::Json(Response::Error { message }) => {
            anyhow::bail!("daemon error: {message}")
        }
        other => anyhow::bail!("unexpected frame before SessionReady: {other:?}"),
    }
}

const PROGRESS_TICK: Duration = Duration::from_millis(100);

pub async fn drive_pre_phase<S, W>(
    stream: &mut S,
    writer: &mut W,
    progress: &mut crate::run::progress::ProgressRenderer,
    quiet: bool,
) -> Result<PrePhaseOutcome>
where
    S: AsyncReadExt + Unpin,
    W: std::io::Write,
{
    let mut ticker = tokio::time::interval(PROGRESS_TICK);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        let read = read_frame_bytes_async(stream);
        tokio::pin!(read);
        let bytes = loop {
            tokio::select! {
                r = &mut read => break r,
                _ = ticker.tick() => progress.tick(writer)?,
            }
        }
        .context("reading pre-phase frame")?;
        match pre_phase_step(&bytes, writer, progress, quiet)? {
            PrePhaseStep::Continue => {}
            PrePhaseStep::SessionReady => return Ok(PrePhaseOutcome::SessionReady),
            PrePhaseStep::EarlyExit(code) => return Ok(PrePhaseOutcome::EarlyExit(code)),
        }
    }
}

pub fn render_status_line(
    level: LogLevel,
    verb: Option<&str>,
    message: &str,
    writer: &mut impl std::io::Write,
    quiet: bool,
) -> std::io::Result<()> {
    if quiet && !matches!(level, LogLevel::Warn | LogLevel::Error) {
        return Ok(());
    }
    let marker = match level {
        LogLevel::Error => '✗',
        LogLevel::Warn => '⚠',
        _ => '✓',
    };
    let phrase = phrase_for_verb(verb.unwrap_or(""));
    if message.is_empty() {
        writeln!(writer, "{marker} {phrase}")
    } else {
        writeln!(writer, "{marker} {phrase} {message}")
    }
}

pub fn render_started_run(
    run_id: &str,
    writer: &mut impl std::io::Write,
    quiet: bool,
) -> std::io::Result<()> {
    render_status_line(
        LogLevel::Info,
        Some("Started"),
        &format!("run {}", lns_ipc::short_run_id(run_id)),
        writer,
        quiet,
    )
}

fn phrase_for_verb(verb: &str) -> String {
    match verb {
        "SessionReady" => "session ready".to_string(),
        "ImageCached" => "image cached".to_string(),
        _ => verb.to_lowercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;

    #[test]
    fn published_sandbox_preflight_pins_the_artifact_and_keeps_launch_defaults() {
        let digest = format!("sha256:{}", "a".repeat(64));
        let target = published_target(
            "registry.example.test/team/sandbox:1",
            lns_ipc::ArtifactInspection::Sandbox(Box::new(lns_ipc::SandboxView {
                reference: "registry.example.test/team/sandbox:1".into(),
                digest: digest.clone(),
                image: "registry.example.test/runtime:1".into(),
                workdir: Some("/workspace".into()),
                user: None,
                mounts: vec![lns_ipc::SandboxMount {
                    kind: lns_ipc::SandboxMountKind::Bind,
                    source: ".".into(),
                    target: "/workspace".into(),
                    read_only: false,
                    exclude: Vec::new(),
                    optional: false,
                }],
                ports: Vec::new(),
                filesets: vec![lns_ipc::SandboxFileset {
                    path: None,
                    reference: Some(format!(
                        "registry.example.test/team/skills@sha256:{}",
                        "b".repeat(64)
                    )),
                    inline: false,
                    host_path: None,
                    optional: false,
                    mount_path: "/root/.agent/skills".into(),
                    owner: lns_ipc::SandboxFilesetOwner::Workload,
                }],
                connectors: Vec::new(),
                env: Vec::new(),
                credentials: Vec::new(),
                tools: Vec::new(),
                policy_flags: Vec::new(),
                cpus: None,
                mem_mib: None,
            })),
        )
        .unwrap();
        assert_eq!(
            target.image,
            format!("registry.example.test/team/sandbox@{digest}")
        );
        assert_eq!(target.defaults.workdir.as_deref(), Some("/workspace"));
        assert_eq!(target.defaults.mounts[0].source, ".");
        assert_eq!(
            target.filesets,
            [crate::run::summary::FilesetSummary {
                source: format!(
                    "registry.example.test/team/skills@sha256:{}…",
                    "b".repeat(12)
                ),
                mount_path: "/root/.agent/skills".to_string(),
                owner: "workload".to_string(),
                from_host: false,
            }]
        );
    }

    #[test]
    fn published_preflight_refuses_a_plain_image() {
        let err = published_target(
            "registry.example.test/team/image:1",
            lns_ipc::ArtifactInspection::Image(lns_ipc::ImageView {
                reference: "registry.example.test/team/image:1".into(),
                digest: format!("sha256:{}", "a".repeat(64)),
            }),
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("not a sandbox"));
    }

    #[test]
    fn published_sandbox_preflight_refuses_an_unpinned_result() {
        let err = published_target(
            "registry.example.test/team/sandbox:1",
            lns_ipc::ArtifactInspection::Sandbox(Box::new(lns_ipc::SandboxView {
                reference: "registry.example.test/team/sandbox:1".into(),
                digest: String::new(),
                image: "registry.example.test/runtime:1".into(),
                workdir: Some("/workspace".into()),
                user: None,
                mounts: Vec::new(),
                ports: Vec::new(),
                filesets: Vec::new(),
                connectors: Vec::new(),
                env: Vec::new(),
                credentials: Vec::new(),
                tools: Vec::new(),
                policy_flags: Vec::new(),
                cpus: None,
                mem_mib: None,
            })),
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("no manifest digest"));
    }

    #[tokio::test(start_paused = true)]
    async fn published_preflight_times_out_when_the_service_never_responds() {
        let err = await_published_preflight(
            "registry.example.test/team/sandbox:1",
            std::future::pending::<Option<Response>>(),
        )
        .await
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "published sandbox preflight timed out after 30s while inspecting registry.example.test/team/sandbox:1"
        );
    }

    #[tokio::test]
    async fn drive_attached_session_returns_exit_code_from_run_exit_frame() {
        let (client, mut server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            let frame = encode_frame(&Response::RunExit { code: 7 }).expect("encode RunExit");
            server.write_all(&frame).await.expect("write RunExit");
        });
        let code = drive_attached_session(
            client,
            None,
            "2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a".to_string(),
            false,
            Vec::new(),
            DetachBehaviour::SignalAndDrain,
            false,
            StdinForwarding::ToRun,
        )
        .await
        .expect("drive_attached_session");
        assert_eq!(code, 7);
    }

    fn run_log(verb: &str, message: &str) -> Response {
        Response::RunLog {
            level: LogLevel::Info,
            verb: Some(verb.to_string()),
            message: message.to_string(),
        }
    }

    #[test]
    fn render_pre_phase_log_emits_check_marker_and_lowercased_verb() {
        let mut buf = Vec::<u8>::new();
        render_status_line(
            LogLevel::Info,
            Some("Resolved"),
            "ubuntu:latest",
            &mut buf,
            false,
        )
        .unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(s, "✓ resolved ubuntu:latest\n");
    }

    #[test]
    fn render_pre_phase_log_uses_cross_marker_for_error_level() {
        let mut buf = Vec::<u8>::new();
        render_status_line(
            LogLevel::Error,
            Some("Resolve"),
            "registry timeout",
            &mut buf,
            false,
        )
        .unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(s, "✗ resolve registry timeout\n");
    }

    #[test]
    fn render_pre_phase_log_special_cases_session_ready_phrase() {
        let mut buf = Vec::<u8>::new();
        render_status_line(LogLevel::Info, Some("SessionReady"), "", &mut buf, false).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(s, "✓ session ready\n");
    }

    #[test]
    fn render_pre_phase_log_special_cases_image_cached_phrase() {
        let mut buf = Vec::<u8>::new();
        render_status_line(LogLevel::Info, Some("ImageCached"), "", &mut buf, false).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(s, "✓ image cached\n");
    }

    #[test]
    fn render_pre_phase_log_omits_trailing_space_when_message_empty() {
        let mut buf = Vec::<u8>::new();
        render_status_line(LogLevel::Info, Some("Booted"), "", &mut buf, false).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(s, "✓ booted\n");
    }

    #[test]
    fn render_pre_phase_log_emits_no_ansi_escape_sequences() {
        let mut buf = Vec::<u8>::new();
        render_status_line(
            LogLevel::Info,
            Some("Booted"),
            "microVM (1.1s)",
            &mut buf,
            false,
        )
        .unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(!s.contains('\x1b'), "ANSI escape leaked: {s:?}");
    }

    #[test]
    fn render_pre_phase_log_falls_back_to_lowercased_verb_for_unknown_verbs() {
        let mut buf = Vec::<u8>::new();
        render_status_line(LogLevel::Info, None, "unverbed", &mut buf, false).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(s, "✓  unverbed\n");
    }

    async fn write_response(server: &mut tokio::io::DuplexStream, resp: Response) {
        let frame = encode_frame(&resp).expect("encode");
        server.write_all(&frame).await.expect("write");
    }

    fn no_progress() -> crate::run::progress::ProgressRenderer {
        crate::run::progress::ProgressRenderer::new(false)
    }

    fn run_progress(verb: &str, message: &str, current: u64, total: u64) -> Response {
        Response::RunProgress {
            verb: verb.to_string(),
            message: message.to_string(),
            current,
            total,
        }
    }

    #[tokio::test]
    async fn drive_pre_phase_renders_log_frames_and_returns_session_ready() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            write_response(&mut server, run_log("Resolved", "ubuntu:latest")).await;
            write_response(&mut server, run_log("Pulled", "7 layers")).await;
            write_response(&mut server, run_log("Booted", "microVM")).await;
            write_response(&mut server, run_log("SessionReady", "")).await;
        });
        let mut buf = Vec::<u8>::new();
        let outcome = drive_pre_phase(&mut client, &mut buf, &mut no_progress(), false)
            .await
            .unwrap();
        assert_eq!(outcome, PrePhaseOutcome::SessionReady);
        let s = String::from_utf8(buf).unwrap();
        assert!(
            s.contains("✓ resolved ubuntu:latest"),
            "missing resolved: {s}"
        );
        assert!(s.contains("✓ pulled 7 layers"), "missing pulled: {s}");
        assert!(s.contains("✓ booted microVM"), "missing booted: {s}");
        assert!(s.contains("✓ session ready"), "missing session ready: {s}");
    }

    #[tokio::test]
    async fn drive_pre_phase_returns_early_exit_when_run_exits_before_session_ready() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            write_response(&mut server, run_log("Resolving", "ubuntu:latest")).await;
            write_response(&mut server, Response::RunExit { code: 42 }).await;
        });
        let mut buf = Vec::<u8>::new();
        let outcome = drive_pre_phase(&mut client, &mut buf, &mut no_progress(), false)
            .await
            .unwrap();
        assert_eq!(outcome, PrePhaseOutcome::EarlyExit(42));
    }

    #[tokio::test]
    async fn drive_pre_phase_bubbles_up_daemon_error_before_session_ready() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            write_response(
                &mut server,
                Response::Error {
                    message: "no such image".to_string(),
                },
            )
            .await;
        });
        let mut buf = Vec::<u8>::new();
        let err = drive_pre_phase(&mut client, &mut buf, &mut no_progress(), false)
            .await
            .expect_err("daemon Error must surface");
        assert!(format!("{err:#}").contains("no such image"));
    }

    #[tokio::test]
    async fn drive_pre_phase_rejects_stdout_frames_before_session_ready() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            let frame = lns_ipc::encode_wire_frame(&WireFrame::Stdout(b"early bytes".to_vec()))
                .expect("encode wire frame");
            server.write_all(&frame).await.expect("write");
        });
        let mut buf = Vec::<u8>::new();
        let err = drive_pre_phase(&mut client, &mut buf, &mut no_progress(), false)
            .await
            .expect_err("stdout before SessionReady is a protocol violation");
        assert!(format!("{err:#}").contains("unexpected frame"));
    }

    #[tokio::test]
    async fn drive_pre_phase_renders_a_device_sign_in_prompt_and_continues() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            write_response(
                &mut server,
                Response::OauthVerification {
                    verification_uri: "https://api.some-oauth.example/device".into(),
                    user_code: "SOME-CODE".into(),
                    expires_in_secs: 900,
                },
            )
            .await;
            write_response(&mut server, run_log("SessionReady", "")).await;
        });
        let mut buf = Vec::<u8>::new();
        let outcome = drive_pre_phase(&mut client, &mut buf, &mut no_progress(), false)
            .await
            .unwrap();
        assert_eq!(outcome, PrePhaseOutcome::SessionReady);
        let s = String::from_utf8(buf).unwrap();
        assert!(
            s.contains("Open https://api.some-oauth.example/device and enter code SOME-CODE (expires in 15m)"),
            "got: {s}"
        );
    }

    #[tokio::test]
    async fn drive_pre_phase_renders_a_browser_sign_in_line_and_continues() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            write_response(
                &mut server,
                Response::OauthBrowserOpened {
                    authorization_url: "https://api.some-oauth.example/authorize".into(),
                },
            )
            .await;
            write_response(&mut server, run_log("SessionReady", "")).await;
        });
        let mut buf = Vec::<u8>::new();
        let outcome = drive_pre_phase(&mut client, &mut buf, &mut no_progress(), false)
            .await
            .unwrap();
        assert_eq!(outcome, PrePhaseOutcome::SessionReady);
        let s = String::from_utf8(buf).unwrap();
        assert!(
            s.contains("visit https://api.some-oauth.example/authorize"),
            "got: {s}"
        );
    }

    #[tokio::test]
    async fn drive_pre_phase_ignores_progress_frames_when_stderr_is_not_a_tty() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            write_response(&mut server, run_progress("Pulling", "", 5, 10)).await;
            write_response(&mut server, run_progress("Booting", "microVM", 0, 0)).await;
            write_response(&mut server, run_log("SessionReady", "")).await;
        });
        let mut buf = Vec::<u8>::new();
        let outcome = drive_pre_phase(&mut client, &mut buf, &mut no_progress(), false)
            .await
            .unwrap();
        assert_eq!(outcome, PrePhaseOutcome::SessionReady);
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(
            s, "✓ session ready\n",
            "piped output must contain only final status lines",
        );
    }

    #[tokio::test]
    async fn drive_pre_phase_renders_then_clears_progress_before_the_next_status_line() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            write_response(&mut server, run_progress("Pulling", "", 5, 10)).await;
            write_response(&mut server, run_log("Pulled", "2 layers")).await;
            write_response(&mut server, run_log("SessionReady", "")).await;
        });
        let mut buf = Vec::<u8>::new();
        let mut progress = crate::run::progress::ProgressRenderer::new(true);
        let outcome = drive_pre_phase(&mut client, &mut buf, &mut progress, false)
            .await
            .unwrap();
        assert_eq!(outcome, PrePhaseOutcome::SessionReady);
        let s = String::from_utf8(buf).unwrap();
        assert!(s.contains("pulling"), "live bar must render: {s:?}");
        assert!(s.contains("50%"), "{s:?}");
        let check_idx = s.find('✓').unwrap();
        assert!(
            s[..check_idx].ends_with('\r'),
            "the bar must be erased so the status line starts at column 0: {s:?}",
        );
        assert!(s.contains("✓ pulled 2 layers"), "{s:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn drive_pre_phase_animates_the_spinner_while_waiting_between_frames() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            write_response(&mut server, run_progress("Booting", "microVM", 0, 0)).await;
            tokio::time::sleep(Duration::from_millis(350)).await;
            write_response(&mut server, run_log("SessionReady", "")).await;
        });
        let mut buf = Vec::<u8>::new();
        let mut progress = crate::run::progress::ProgressRenderer::new(true);
        let outcome = drive_pre_phase(&mut client, &mut buf, &mut progress, false)
            .await
            .unwrap();
        assert_eq!(outcome, PrePhaseOutcome::SessionReady);
        let s = String::from_utf8(buf).unwrap();
        let spinner_glyphs: std::collections::HashSet<char> = s
            .chars()
            .filter(|c| ('\u{2800}'..='\u{28FF}').contains(c))
            .collect();
        assert!(
            spinner_glyphs.len() >= 2,
            "the spinner must visibly animate while the service is silent: {s:?}",
        );
        assert!(s.contains("✓ session ready"), "{s:?}");
    }

    #[tokio::test]
    async fn drive_attached_session_tolerates_progress_frames_after_session_ready() {
        let (client, mut server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            let progress = encode_frame(&run_progress("Pulling", "", 1, 2)).expect("encode");
            server.write_all(&progress).await.expect("write progress");
            let exit = encode_frame(&Response::RunExit { code: 0 }).expect("encode RunExit");
            server.write_all(&exit).await.expect("write RunExit");
        });
        let code = drive_attached_session(
            client,
            None,
            "2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a".to_string(),
            false,
            Vec::new(),
            DetachBehaviour::SignalAndDrain,
            false,
            StdinForwarding::ToRun,
        )
        .await
        .expect("a stray progress frame must not kill an attached session");
        assert_eq!(code, 0);
    }

    #[tokio::test]
    async fn drive_attached_session_bubbles_up_a_daemon_error_frame() {
        let (client, mut server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            let frame = encode_frame(&Response::Error {
                message: "policy file is unreadable".to_string(),
            })
            .expect("encode Error");
            server.write_all(&frame).await.expect("write Error");
        });
        let err = drive_attached_session(
            client,
            None,
            "2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a2a".to_string(),
            false,
            Vec::new(),
            DetachBehaviour::SignalAndDrain,
            false,
            StdinForwarding::ToRun,
        )
        .await
        .expect_err("daemon Error must surface as anyhow::Error");
        assert!(
            format!("{err:#}").contains("policy file is unreadable"),
            "error context lost"
        );
    }

    #[test]
    fn render_status_line_renders_started_run_in_check_cadence_not_right_aligned() {
        let mut buf = Vec::<u8>::new();
        render_status_line(
            LogLevel::Info,
            Some("Started"),
            "run 2a2a2a2a2a2a",
            &mut buf,
            false,
        )
        .unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(s, "✓ started run 2a2a2a2a2a2a\n");
        assert!(
            !s.contains("     Started"),
            "right-aligned padding leaked: {s:?}"
        );
    }

    #[test]
    fn render_started_run_routes_run_id_through_the_check_renderer() {
        let mut buf = Vec::<u8>::new();
        render_started_run("07eeeeee000000000000000000000000", &mut buf, false).unwrap();
        assert_eq!(
            String::from_utf8(buf).unwrap(),
            "✓ started run 07eeeeee0000\n"
        );
    }

    #[test]
    fn render_status_line_renders_finished_in_check_cadence() {
        let mut buf = Vec::<u8>::new();
        render_status_line(
            LogLevel::Info,
            Some("Finished"),
            "in 2.53s",
            &mut buf,
            false,
        )
        .unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(s, "✓ finished in 2.53s\n");
    }

    #[test]
    fn render_status_line_uses_cross_marker_for_error_level() {
        let mut buf = Vec::<u8>::new();
        render_status_line(
            LogLevel::Error,
            Some("Resolve"),
            "registry timeout",
            &mut buf,
            false,
        )
        .unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(s, "✗ resolve registry timeout\n");
    }

    fn render_marker(level: LogLevel) -> char {
        let mut buf = Vec::<u8>::new();
        render_status_line(level, Some("Verb"), "msg", &mut buf, false).unwrap();
        String::from_utf8(buf)
            .unwrap()
            .chars()
            .next()
            .expect("status line has a leading marker")
    }

    #[test]
    fn render_status_line_marker_truth_table_distinguishes_each_level() {
        assert_eq!(render_marker(LogLevel::Info), '✓');
        assert_eq!(
            render_marker(LogLevel::Warn),
            '⚠',
            "a warning must not wear the success check",
        );
        assert_eq!(render_marker(LogLevel::Error), '✗');
    }

    #[test]
    fn render_status_line_phrases_session_ready() {
        let mut buf = Vec::<u8>::new();
        render_status_line(LogLevel::Info, Some("SessionReady"), "", &mut buf, false).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert_eq!(s, "✓ session ready\n");
        assert!(!s.contains("SessionReady"), "raw verb leaked: {s:?}");
    }

    #[tokio::test]
    async fn render_attached_run_log_keeps_debug_frames_off_the_user_writer() {
        let mut stderr = Vec::<u8>::new();
        render_attached_run_log(
            LogLevel::Debug,
            None,
            "broker handshake",
            false,
            &mut stderr,
            false,
        )
        .await
        .expect("render debug");
        assert!(
            stderr.is_empty(),
            "debug run-log must not reach the user's status writer: {stderr:?}",
        );
    }

    #[tokio::test]
    async fn render_attached_run_log_writes_info_status_to_the_writer() {
        let mut stderr = Vec::<u8>::new();
        render_attached_run_log(
            LogLevel::Info,
            Some("Finished"),
            "in 1.10s",
            false,
            &mut stderr,
            false,
        )
        .await
        .expect("render info");
        assert_eq!(stderr, b"\xe2\x9c\x93 finished in 1.10s\n");
    }

    #[tokio::test]
    async fn render_attached_run_log_uses_crlf_when_the_terminal_is_raw() {
        let mut stderr = Vec::<u8>::new();
        render_attached_run_log(
            LogLevel::Warn,
            None,
            "workload exited with code 1",
            true,
            &mut stderr,
            false,
        )
        .await
        .expect("render warn");
        let rendered = String::from_utf8(stderr).expect("utf8");
        assert!(
            rendered.ends_with("\r\n"),
            "in raw mode a status line must carriage-return so the next line starts at column 0: {rendered:?}",
        );
        assert!(
            !rendered.contains("\n\r") && rendered.matches('\n').count() == 1,
            "exactly one CRLF, no stray bare LF: {rendered:?}",
        );
    }

    #[test]
    fn lf_to_crlf_expands_every_newline_and_leaves_other_bytes() {
        assert_eq!(lf_to_crlf(b"a\nb\n"), b"a\r\nb\r\n");
        assert_eq!(lf_to_crlf(b"no newline"), b"no newline");
    }

    #[test]
    fn host_binds_interactive_is_suppressed_for_a_detached_run_even_with_a_tty() {
        assert!(
            host_binds_interactive(false, true),
            "attached + tty prompts"
        );
        assert!(
            !host_binds_interactive(true, true),
            "a detached run must never block on the secret prompt, even from a terminal"
        );
        assert!(
            !host_binds_interactive(false, false),
            "no terminal cannot prompt"
        );
    }

    #[test]
    fn needs_final_newline_truth_table() {
        assert!(
            !needs_final_newline(None),
            "no stdout means stderr status lines already left the cursor at column 0",
        );
        assert!(
            needs_final_newline(Some(b'o')),
            "non-newline last byte needs a newline"
        );
        assert!(
            !needs_final_newline(Some(b'\n')),
            "newline-terminated output does not need another",
        );
    }

    #[tokio::test]
    async fn drive_attached_session_appends_trailing_newline_when_workload_omits_one() {
        let (client, mut server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            let out = lns_ipc::encode_wire_frame(&WireFrame::Stdout(b"hello".to_vec()))
                .expect("encode stdout");
            server.write_all(&out).await.expect("write stdout");
            let exit = encode_frame(&Response::RunExit { code: 0 }).expect("encode exit");
            server.write_all(&exit).await.expect("write exit");
        });
        let mut captured = Vec::<u8>::new();
        let mut status = Vec::<u8>::new();
        let code = drive_attached_session_with_writers(
            client,
            None,
            "1".to_string(),
            false,
            true,
            Vec::new(),
            DetachBehaviour::SignalAndDrain,
            &mut captured,
            &mut status,
            false,
            StdinForwarding::ToRun,
        )
        .await
        .expect("drive");
        assert_eq!(code, 0);
        assert_eq!(
            captured, b"hello\n",
            "expected exactly one appended newline"
        );
    }

    #[tokio::test]
    async fn exit_code_after_detach_leave_running_returns_zero_without_draining() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        let exit = encode_frame(&Response::RunExit { code: 9 }).expect("encode exit");
        server.write_all(&exit).await.expect("write exit");

        let mut captured = Vec::<u8>::new();
        let mut status = Vec::<u8>::new();
        let mut last = None;
        let code = exit_code_after_detach(
            DetachBehaviour::LeaveRunning,
            false,
            &mut client,
            &mut captured,
            &mut status,
            &mut last,
            false,
        )
        .await;

        assert_eq!(code, 0, "a docker-style detach returns 0 at once");
        assert!(
            captured.is_empty(),
            "LeaveRunning must leave the run running, not drain its output"
        );
        let bytes = read_frame_bytes_async(&mut client)
            .await
            .expect("the workload's frame is still pending — it was not drained");
        assert!(matches!(
            decode_wire_frame_from_bytes(&bytes).expect("decode"),
            WireFrame::Json(Response::RunExit { code: 9 })
        ));
    }

    #[tokio::test]
    async fn exit_code_after_detach_detach_run_returns_zero_without_draining() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        let exit = encode_frame(&Response::RunExit { code: 9 }).expect("encode exit");
        server.write_all(&exit).await.expect("write exit");

        let mut captured = Vec::<u8>::new();
        let mut status = Vec::<u8>::new();
        let mut last = None;
        let code = exit_code_after_detach(
            DetachBehaviour::DetachRun,
            false,
            &mut client,
            &mut captured,
            &mut status,
            &mut last,
            false,
        )
        .await;

        assert_eq!(code, 0, "detaching a run returns 0 at once");
        assert!(
            captured.is_empty(),
            "DetachRun must leave the run running, not drain its output"
        );
    }

    #[tokio::test]
    async fn exit_code_after_detach_signal_and_drain_adopts_the_drained_exit_code() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        let exit = encode_frame(&Response::RunExit { code: 7 }).expect("encode exit");
        server.write_all(&exit).await.expect("write exit");

        let mut captured = Vec::<u8>::new();
        let mut status = Vec::<u8>::new();
        let mut last = None;
        let code = exit_code_after_detach(
            DetachBehaviour::SignalAndDrain,
            true,
            &mut client,
            &mut captured,
            &mut status,
            &mut last,
            false,
        )
        .await;

        assert_eq!(
            code, 7,
            "SignalAndDrain drains through to the real exit code"
        );
    }

    #[tokio::test]
    async fn drive_attached_session_does_not_double_newline_when_workload_ends_with_one() {
        let (client, mut server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            let out = lns_ipc::encode_wire_frame(&WireFrame::Stdout(b"hello\n".to_vec()))
                .expect("encode stdout");
            server.write_all(&out).await.expect("write stdout");
            let exit = encode_frame(&Response::RunExit { code: 0 }).expect("encode exit");
            server.write_all(&exit).await.expect("write exit");
        });
        let mut captured = Vec::<u8>::new();
        let mut status = Vec::<u8>::new();
        let code = drive_attached_session_with_writers(
            client,
            None,
            "1".to_string(),
            false,
            true,
            Vec::new(),
            DetachBehaviour::SignalAndDrain,
            &mut captured,
            &mut status,
            false,
            StdinForwarding::ToRun,
        )
        .await
        .expect("drive");
        assert_eq!(code, 0);
        assert_eq!(captured, b"hello\n", "must not add a second newline");
    }

    #[tokio::test]
    async fn drive_attached_session_relays_piped_stdout_byte_for_byte_without_appending() {
        let (client, mut server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            let out = lns_ipc::encode_wire_frame(&WireFrame::Stdout(b"hello".to_vec()))
                .expect("encode stdout");
            server.write_all(&out).await.expect("write stdout");
            let exit = encode_frame(&Response::RunExit { code: 0 }).expect("encode exit");
            server.write_all(&exit).await.expect("write exit");
        });
        let mut captured = Vec::<u8>::new();
        let mut status = Vec::<u8>::new();
        let code = drive_attached_session_with_writers(
            client,
            None,
            "1".to_string(),
            false,
            false,
            Vec::new(),
            DetachBehaviour::SignalAndDrain,
            &mut captured,
            &mut status,
            false,
            StdinForwarding::ToRun,
        )
        .await
        .expect("drive");
        assert_eq!(code, 0);
        assert_eq!(
            captured, b"hello",
            "a redirected/piped stdout must receive the workload's exact bytes",
        );
    }

    #[tokio::test]
    async fn drive_attached_session_renders_post_session_runlog_in_check_form() {
        let (client, mut server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            let frame = encode_frame(&run_log("Finished", "in 2.53s")).expect("encode finished");
            server.write_all(&frame).await.expect("write finished");
            let exit = encode_frame(&Response::RunExit { code: 0 }).expect("encode exit");
            server.write_all(&exit).await.expect("write exit");
        });
        let mut captured = Vec::<u8>::new();
        let mut status = Vec::<u8>::new();
        let code = drive_attached_session_with_writers(
            client,
            None,
            "1".to_string(),
            false,
            true,
            Vec::new(),
            DetachBehaviour::SignalAndDrain,
            &mut captured,
            &mut status,
            false,
            StdinForwarding::ToRun,
        )
        .await
        .expect("drive");
        assert_eq!(code, 0);
        let s = String::from_utf8(status).unwrap();
        assert!(
            s.contains("✓ finished in 2.53s"),
            "missing check form: {s:?}"
        );
        assert!(
            !s.contains("     Finished"),
            "right-aligned padding leaked: {s:?}"
        );
    }

    #[test]
    fn a_session_opened_without_stdin_hands_the_run_nothing_it_typed() {
        for action in [
            FeedAction::Flush(b"echo pwned\n".to_vec()),
            FeedAction::FlushAndForward(b"echo pwned".to_vec(), b'\n'),
            FeedAction::Hold,
        ] {
            let mut pending = b"already typed".to_vec();
            let (requests, control) = plan_feed(
                action,
                "1",
                &mut pending,
                DetachBehaviour::SignalAndDrain,
                StdinForwarding::Withheld,
            );
            assert!(
                requests.is_empty(),
                "a non-interactive exec must not feed the run's own shell: {requests:?}"
            );
            assert!(matches!(control, PumpControl::Continue));
        }
    }

    #[test]
    fn a_session_opened_without_stdin_still_honours_the_detach_chord() {
        let mut pending = b"already typed".to_vec();
        let (requests, control) = plan_feed(
            FeedAction::Trigger,
            "9",
            &mut pending,
            DetachBehaviour::SignalAndDrain,
            StdinForwarding::Withheld,
        );
        assert_eq!(
            requests,
            vec![Request::RunSignal {
                run_id: "9".to_string(),
                signal: SignalKind::Hup
            }],
            "the chord must still detach, and must not flush the withheld bytes on its way out"
        );
        assert!(matches!(control, PumpControl::Detach));
    }

    #[test]
    fn plan_feed_forward_accumulates_without_sending() {
        let mut pending = Vec::new();
        let (requests, control) = plan_feed(
            FeedAction::Forward(b'a'),
            "1",
            &mut pending,
            DetachBehaviour::SignalAndDrain,
            StdinForwarding::ToRun,
        );
        assert!(requests.is_empty());
        assert!(matches!(control, PumpControl::Continue));
        assert_eq!(pending, vec![b'a']);
    }

    #[test]
    fn plan_feed_hold_flushes_accumulated_bytes() {
        let mut pending = vec![b'x', b'y'];
        let (requests, _) = plan_feed(
            FeedAction::Hold,
            "7",
            &mut pending,
            DetachBehaviour::SignalAndDrain,
            StdinForwarding::ToRun,
        );
        assert_eq!(
            requests,
            vec![Request::RunStdin {
                run_id: "7".to_string(),
                bytes: vec![b'x', b'y']
            }]
        );
        assert!(pending.is_empty());
    }

    #[test]
    fn plan_feed_hold_with_nothing_pending_sends_nothing() {
        let mut pending = Vec::new();
        let (requests, _) = plan_feed(
            FeedAction::Hold,
            "7",
            &mut pending,
            DetachBehaviour::SignalAndDrain,
            StdinForwarding::ToRun,
        );
        assert!(requests.is_empty());
    }

    #[test]
    fn plan_feed_flush_sends_pending_before_the_held_bytes() {
        let mut pending = vec![b'p'];
        let (requests, _) = plan_feed(
            FeedAction::Flush(vec![b'h']),
            "3",
            &mut pending,
            DetachBehaviour::SignalAndDrain,
            StdinForwarding::ToRun,
        );
        assert_eq!(
            requests,
            vec![
                Request::RunStdin {
                    run_id: "3".to_string(),
                    bytes: vec![b'p']
                },
                Request::RunStdin {
                    run_id: "3".to_string(),
                    bytes: vec![b'h']
                },
            ]
        );
        assert!(pending.is_empty());
    }

    #[test]
    fn plan_feed_flush_and_forward_appends_the_current_byte() {
        let mut pending = Vec::new();
        let (requests, _) = plan_feed(
            FeedAction::FlushAndForward(vec![b'h'], b'c'),
            "3",
            &mut pending,
            DetachBehaviour::SignalAndDrain,
            StdinForwarding::ToRun,
        );
        assert_eq!(
            requests,
            vec![Request::RunStdin {
                run_id: "3".to_string(),
                bytes: vec![b'h', b'c']
            }]
        );
    }

    #[test]
    fn plan_feed_trigger_signals_hup_then_detaches_for_run_and_exec() {
        let mut pending = vec![b'p'];
        let (requests, control) = plan_feed(
            FeedAction::Trigger,
            "9",
            &mut pending,
            DetachBehaviour::SignalAndDrain,
            StdinForwarding::ToRun,
        );
        assert_eq!(
            requests,
            vec![
                Request::RunStdin {
                    run_id: "9".to_string(),
                    bytes: vec![b'p']
                },
                Request::RunSignal {
                    run_id: "9".to_string(),
                    signal: SignalKind::Hup
                },
            ]
        );
        assert!(matches!(control, PumpControl::Detach));
        assert!(pending.is_empty());
    }

    #[test]
    fn plan_feed_trigger_leaves_the_run_running_without_signalling_for_attach() {
        let mut pending = vec![b'p'];
        let (requests, control) = plan_feed(
            FeedAction::Trigger,
            "9",
            &mut pending,
            DetachBehaviour::LeaveRunning,
            StdinForwarding::ToRun,
        );
        assert_eq!(
            requests,
            vec![Request::RunStdin {
                run_id: "9".to_string(),
                bytes: vec![b'p']
            }],
            "a docker-style detach flushes pending input but never signals the workload"
        );
        assert!(matches!(control, PumpControl::Detach));
        assert!(pending.is_empty());
    }

    #[test]
    fn plan_feed_trigger_sends_run_detach_without_signalling_for_run() {
        let mut pending = vec![b'p'];
        let (requests, control) = plan_feed(
            FeedAction::Trigger,
            "9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a",
            &mut pending,
            DetachBehaviour::DetachRun,
            StdinForwarding::ToRun,
        );
        assert_eq!(
            requests,
            vec![
                Request::RunStdin {
                    run_id: "9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a".to_string(),
                    bytes: vec![b'p']
                },
                Request::RunDetach {
                    run_id: "9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a9a".to_string(),
                },
            ],
            "detaching a run flushes pending input then asks the service to keep it running — no SIGHUP",
        );
        assert!(matches!(control, PumpControl::Detach));
        assert!(pending.is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn write_and_await_ack_blocks_until_the_service_responds() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        let detach = Request::RunDetach {
            run_id: "7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a".to_string(),
        };
        let mut ack = std::pin::pin!(write_and_await_ack(&mut client, &detach));

        assert!(
            timeout(Duration::from_millis(50), &mut ack).await.is_err(),
            "the detach round-trip must not complete before the service acknowledges — \
             a fire-and-forget RunDetach would let the CLI close the run stream first and \
             reintroduce terminate-on-detach",
        );

        let req = read_frame_bytes_async(&mut server)
            .await
            .expect("server reads the request");
        let decoded: Request = decode_frame(&mut &req[..]).expect("decode request");
        assert_eq!(
            decoded,
            Request::RunDetach {
                run_id: "7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a7a".to_string(),
            }
        );

        let frame = encode_frame(&Response::DetachAccepted).expect("encode ack");
        server.write_all(&frame).await.expect("server writes ack");

        ack.await
            .expect("the round-trip completes once the service acknowledges");
    }

    #[test]
    fn render_status_line_quiet_suppresses_info() {
        let mut buf = Vec::<u8>::new();
        render_status_line(
            LogLevel::Info,
            Some("Resolved"),
            "ubuntu:latest",
            &mut buf,
            true,
        )
        .unwrap();
        assert!(buf.is_empty(), "quiet must suppress Info lines");
    }

    #[test]
    fn render_status_line_quiet_still_emits_warn() {
        let mut buf = Vec::<u8>::new();
        render_status_line(LogLevel::Warn, Some("SlowPull"), "retrying", &mut buf, true).unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(
            s.contains("⚠ slowpull retrying"),
            "quiet must still emit Warn: {s:?}"
        );
    }

    #[test]
    fn render_status_line_quiet_still_emits_error() {
        let mut buf = Vec::<u8>::new();
        render_status_line(
            LogLevel::Error,
            Some("Resolve"),
            "not found",
            &mut buf,
            true,
        )
        .unwrap();
        let s = String::from_utf8(buf).unwrap();
        assert!(
            s.contains("✗ resolve not found"),
            "quiet must still emit Error: {s:?}"
        );
    }

    #[test]
    fn render_started_run_quiet_suppresses_output() {
        let mut buf = Vec::<u8>::new();
        render_started_run("07eeeeee000000000000000000000000", &mut buf, true).unwrap();
        assert!(buf.is_empty(), "quiet must suppress the started-run line");
    }

    #[tokio::test]
    async fn drive_pre_phase_quiet_suppresses_info_but_detects_session_ready() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            write_response(&mut server, run_log("Resolved", "ubuntu:latest")).await;
            write_response(&mut server, run_log("Pulled", "7 layers")).await;
            write_response(&mut server, run_log("Booted", "microVM")).await;
            write_response(&mut server, run_log("SessionReady", "")).await;
        });
        let mut buf = Vec::<u8>::new();
        let outcome = drive_pre_phase(&mut client, &mut buf, &mut no_progress(), true)
            .await
            .unwrap();
        assert_eq!(outcome, PrePhaseOutcome::SessionReady);
        assert!(
            buf.is_empty(),
            "quiet must suppress all Info status lines: {buf:?}"
        );
    }
}
