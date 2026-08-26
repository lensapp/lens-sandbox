use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use lns_ipc::{Request, Response, WireFrame, decode_frame, encode_frame, read_frame_bytes_async};
use tokio::io::AsyncWriteExt;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{mpsc, oneshot};
use tokio::time::timeout;

use crate::log;
use crate::shutdown::Shutdown;
use crate::time_fmt::rfc3339_now;

use super::{
    PostPumpAction, PumpOutcome, handle_request, peer_is_authorized, post_pump_action,
    pump_responses, write_error,
};
use super::{build_session_params, register_exec_input, validate_exec};

const PEER_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const PEER_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

pub async fn run_server(
    socket_path: PathBuf,
    shutdown: Arc<Shutdown>,
    started_at: Instant,
) -> anyhow::Result<()> {
    rebuild_stopped_runs().await;
    let listener = bind_or_replace_stale(&socket_path).await?;

    loop {
        tokio::select! {
            accept_result = listener.accept() => {
                let (stream, _) = accept_result.context("accept failed")?;
                if !peer_is_trusted(&stream) {
                    continue;
                }
                let shutdown = shutdown.clone();
                let start = started_at;
                tokio::spawn(async move {
                    if let Err(e) = handle_connection(stream, shutdown, start).await {
                        log::warn!(error = %format_args!("{e:#}"), "ipc connection ended with error");
                    }
                });
            }
            _ = shutdown.wait_async() => {
                break;
            }
        }
    }

    let _ = std::fs::remove_file(&socket_path);
    Ok(())
}

async fn bind_or_replace_stale(socket_path: &Path) -> anyhow::Result<UnixListener> {
    match bind_secure(socket_path) {
        Ok(listener) => return Ok(listener),
        Err(e) if e.kind() != io::ErrorKind::AddrInUse => {
            return Err(e).with_context(|| format!("failed to bind {}", socket_path.display()));
        }
        Err(_) => {}
    }

    if is_instance_alive(socket_path).await {
        bail!(
            "service already running (socket responds at {})",
            socket_path.display()
        );
    }

    std::fs::remove_file(socket_path)
        .with_context(|| format!("removing stale socket {}", socket_path.display()))?;

    bind_secure(socket_path).with_context(|| format!("failed to rebind {}", socket_path.display()))
}

fn bind_secure(socket_path: &Path) -> io::Result<UnixListener> {
    let listener = UnixListener::bind(socket_path)?;
    set_socket_mode_0600(socket_path)?;
    Ok(listener)
}

#[cfg(unix)]
fn set_socket_mode_0600(socket_path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn set_socket_mode_0600(_socket_path: &Path) -> io::Result<()> {
    Ok(())
}

fn peer_is_trusted(stream: &UnixStream) -> bool {
    match stream.peer_cred() {
        Ok(cred) => {
            let authorized = peer_is_authorized(cred.uid(), service_euid());
            if !authorized {
                log::warn!(
                    peer_uid = cred.uid(),
                    "rejected IPC connection from another user"
                );
            }
            authorized
        }
        Err(e) => {
            log::warn!(error = %e, "rejected IPC connection: peer credentials unavailable");
            false
        }
    }
}

fn service_euid() -> u32 {
    // SAFETY: geteuid is reentrant and always succeeds per POSIX.
    unsafe { libc::geteuid() }
}

async fn is_instance_alive(socket_path: &Path) -> bool {
    let probe = async {
        let mut stream = UnixStream::connect(socket_path).await?;
        let frame =
            encode_frame(&Request::Ping).map_err(|e| io::Error::other(format!("encode: {e}")))?;
        stream.write_all(&frame).await?;
        let bytes = read_frame_bytes_async(&mut stream)
            .await
            .map_err(|e| io::Error::other(format!("read: {e}")))?;
        let resp: Response =
            decode_frame(&mut &bytes[..]).map_err(|e| io::Error::other(format!("decode: {e}")))?;
        Ok::<bool, io::Error>(matches!(resp, Response::Pong))
    };
    matches!(timeout(PEER_PROBE_TIMEOUT, probe).await, Ok(Ok(true)))
}

async fn handle_connection(
    mut stream: UnixStream,
    shutdown: Arc<Shutdown>,
    started_at: Instant,
) -> anyhow::Result<()> {
    let bytes = timeout(PEER_REQUEST_TIMEOUT, read_frame_bytes_async(&mut stream))
        .await
        .context("client read timed out")?
        .context("reading request frame")?;

    let request: Request = decode_frame(&mut &bytes[..])?;

    match request {
        Request::RunImage(args) => handle_run(stream, *args).await,
        Request::ExecImage(args) => handle_exec(stream, args).await,
        Request::StartRun { run, attach, stdin } => {
            handle_start(stream, run, super::StartOptions { attach, stdin }).await
        }
        Request::RunLogs { run, follow } => handle_logs(stream, run, follow).await,
        Request::AttachRun { run } => handle_attach(stream, run).await,
        Request::RunStats { run } => handle_stats(stream, run).await,
        other => handle_one_shot(stream, other, shutdown, started_at).await,
    }
}

async fn handle_start(
    mut stream: UnixStream,
    run: String,
    options: super::StartOptions,
) -> anyhow::Result<()> {
    super::start_stopped_run(&mut stream, &run, &RealStartHost, options).await
}

struct RealStartHost;

impl super::StartHost for RealStartHost {
    async fn record(&self, run_id: &str) -> anyhow::Result<crate::run_record::RunRecord> {
        let root = crate::cache::root()?;
        let bytes = tokio::fs::read(crate::run_record::record_path(&root, run_id))
            .await
            .map_err(|_| {
                anyhow::anyhow!("run {run_id}'s state is damaged: its run record is missing")
            })?;
        serde_json::from_slice(&bytes).map_err(|_| {
            anyhow::anyhow!("run {run_id}'s state is damaged: its run record does not parse")
        })
    }

    async fn preflight(&self, record: &crate::run_record::RunRecord) -> anyhow::Result<()> {
        let upper = crate::cache::run_dir(&crate::cache::root()?, &record.run_id).join("upper.img");
        match tokio::fs::metadata(&upper).await {
            Ok(m) if m.len() > 0 => {}
            _ => anyhow::bail!(
                "run {}'s state is damaged: its writable layer is missing from {}",
                record.name,
                upper.display()
            ),
        }
        for volume in &record.args.volumes {
            if let Some(holder) = crate::volume_store::global().holder(&volume.name) {
                anyhow::bail!(
                    "volume {:?} is held by run {}; stop or remove it first",
                    volume.name,
                    lns_ipc::short_run_id(&holder)
                );
            }
        }
        for bind in &record.args.binds {
            if tokio::fs::metadata(&bind.host_source).await.is_err() {
                anyhow::bail!(
                    "bind source {} no longer exists on the host",
                    bind.host_source
                );
            }
        }
        for port in &record.args.published_ports {
            if tokio::net::TcpListener::bind((port.host_ip, port.host_port))
                .await
                .is_err()
            {
                anyhow::bail!("host port {} is already in use", port.host_port);
            }
        }
        Ok(())
    }

    async fn serve<S>(
        &self,
        stream: &mut S,
        record: crate::run_record::RunRecord,
        options: super::StartOptions,
    ) -> anyhow::Result<()>
    where
        S: tokio::io::AsyncReadExt + AsyncWriteExt + Unpin + Send,
    {
        let run_id = record.run_id.clone();
        let mut args = record.args.clone();
        args.detached = !options.attach;
        args.stdin = options.attach && options.stdin;
        let mode = crate::run::LaunchMode::Restart {
            pinned_descriptor_sha256: record.descriptor_sha256.clone(),
        };
        let Some(prepared) =
            super::prepare_while_the_client_waits(stream, &RealRunHost, &run_id, &args).await
        else {
            return Ok(());
        };
        let prepared = match prepared {
            Ok(prepared) => prepared,
            Err(e) => {
                let _ = write_error(stream, format!("{e:#}")).await;
                return Ok(());
            }
        };
        serve_prepared_run(stream, run_id, args, prepared, mode).await
    }
}

async fn handle_stats(mut stream: UnixStream, run: String) -> anyhow::Result<()> {
    let run_id = match crate::run_registry::resolve(&run) {
        Ok(id) => id,
        Err(e) => {
            let _ = write_error(&mut stream, e.to_string()).await;
            return Ok(());
        }
    };
    let response = match crate::run_registry::connector(&run_id) {
        None => Response::Error {
            message: format!("no active run with id {run_id}"),
        },
        Some(connector) => crate::guest_stats::response_from(
            crate::vm::session_client::capture_session_output(
                connector.as_ref(),
                crate::guest_stats::sample_argv(),
            )
            .await,
        ),
    };
    let frame = encode_frame(&response)?;
    stream.write_all(&frame).await?;
    Ok(())
}

async fn handle_logs(mut stream: UnixStream, run: String, follow: bool) -> anyhow::Result<()> {
    let run_id = match crate::run_registry::resolve(&run) {
        Ok(id) => id,
        Err(e) => {
            let _ = write_error(&mut stream, e.to_string()).await;
            return Ok(());
        }
    };
    let Some(buffer) = crate::run_registry::log_buffer(&run_id) else {
        let _ = write_error(&mut stream, format!("no active run with id {run_id}")).await;
        return Ok(());
    };
    let started = encode_frame(&Response::RunStarted { run_id })?;
    stream.write_all(&started).await?;
    crate::run_log::stream_to(&buffer, &mut stream, follow, 0).await
}

async fn handle_attach(mut stream: UnixStream, run: String) -> anyhow::Result<()> {
    let run_id = match crate::run_registry::resolve(&run) {
        Ok(id) => id,
        Err(e) => {
            let _ = write_error(&mut stream, e.to_string()).await;
            return Ok(());
        }
    };
    let Some(buffer) = crate::run_registry::log_buffer(&run_id) else {
        let _ = write_error(&mut stream, format!("no active run with id {run_id}")).await;
        return Ok(());
    };
    let started = encode_frame(&Response::RunStarted { run_id })?;
    stream.write_all(&started).await?;
    let tail = buffer.tail_seq();
    crate::run_log::stream_to(&buffer, &mut stream, true, tail).await
}

async fn handle_one_shot(
    mut stream: UnixStream,
    request: Request,
    shutdown: Arc<Shutdown>,
    started_at: Instant,
) -> anyhow::Result<()> {
    let response = handle_request(&request, started_at).await;
    let frame = encode_frame(&response)?;
    timeout(PEER_REQUEST_TIMEOUT, stream.write_all(&frame))
        .await
        .context("client write timed out")??;

    if matches!(request, Request::Shutdown) {
        shutdown.signal();
    }

    Ok(())
}

/// The production run host: what a run refuses before it starts, and what it does once it will.
struct RealRunHost;

impl super::RunHost for RealRunHost {
    type Prepared = crate::run::PreparedRun;

    async fn prepare(
        &self,
        run_id: &str,
        args: &lns_ipc::RunImageArgs,
    ) -> anyhow::Result<Self::Prepared> {
        crate::run::prepare(run_id, args).await
    }

    async fn serve<W>(
        &self,
        stream: &mut W,
        run_id: String,
        args: lns_ipc::RunImageArgs,
        prepared: Self::Prepared,
    ) -> anyhow::Result<()>
    where
        W: AsyncWriteExt + Unpin + Send,
    {
        serve_prepared_run(
            stream,
            run_id,
            args,
            prepared,
            crate::run::LaunchMode::Fresh,
        )
        .await
    }
}

async fn handle_run(mut stream: UnixStream, args: lns_ipc::RunImageArgs) -> anyhow::Result<()> {
    super::start_run(&mut stream, args, &RealRunHost).await
}

const FRAME_CHAN_BUF: usize = 512;

/// Everything a run does once it is going to start: its registry entry, the `RunStarted` its client waits for, and the boot task's frames.
async fn migrate_legacy_audit_chains() {
    if let Ok(home) = lns_ipc::lns_home() {
        for id in crate::audit::migrate_legacy_run_audit(&crate::image_store::RealFs, &home).await {
            log::info!("Migrated", "audit chain for run {id} to its new home");
        }
    }
}

async fn rebuild_stopped_runs() {
    migrate_legacy_audit_chains().await;
    let scan = match crate::cache::root() {
        Ok(root) => crate::run_record::load_all_with(&crate::image_store::RealFs, &root).await,
        Err(e) => Err(e),
    };
    match scan {
        Ok(scan) => {
            for damaged in &scan.damaged {
                let (id, reason) = (&damaged.run_id, &damaged.reason);
                log::warn!(
                    "run {id} cannot restart and will not be pruned: {reason}; repair or delete its run dir by hand"
                );
            }
            let doomed = crate::run_registry::rebuild_from_records(scan.records);
            if let Ok(root) = crate::cache::root() {
                for id in doomed {
                    crate::run::reclaim_run_dir(&crate::run::RealRemoveDir, &root, &id);
                }
            }
        }
        Err(e) => log::warn!("stopped runs not rebuilt; they stay invisible until restart: {e:#}"),
    }
}

async fn serve_prepared_run<W>(
    stream: &mut W,
    run_id: String,
    args: lns_ipc::RunImageArgs,
    prepared: crate::run::PreparedRun,
    mode: crate::run::LaunchMode,
) -> anyhow::Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    let (frame_tx, mut frame_rx) = mpsc::channel::<WireFrame>(FRAME_CHAN_BUF);
    let (cancel_tx, cancel_rx) = oneshot::channel::<i32>();
    let (detach_tx, detach_rx) = oneshot::channel::<()>();

    let detached = args.detached;

    let (input_tx, input_rx) = mpsc::channel::<crate::vm::session_client::SessionInput>(256);

    let image_label = args
        .image
        .clone()
        .unwrap_or_else(|| "<imageless>".to_string());
    let command_label = args.cmd.join(" ");
    let started_label = rfc3339_now();
    let requested_name = args.name.clone();
    let config = lns_ipc::RunConfig::from_run_args(&args);

    let logs = Arc::new(crate::run_log::RunLogBuffer::default());
    let (task_frame_tx, tee_rx) = mpsc::channel::<WireFrame>(FRAME_CHAN_BUF);
    tokio::spawn(crate::run_log::tee_frames(
        tee_rx,
        logs.clone(),
        frame_tx.clone(),
    ));
    let run_args = args;
    let task_run_id = run_id.clone();
    let (microvm_tx, microvm_rx) = oneshot::channel::<String>();
    let fallback_microvm = run_id.clone();
    let task_mode = mode.clone();
    let run_task = tokio::spawn(async move {
        let microvm = microvm_rx.await.unwrap_or(fallback_microvm);
        crate::run::handle(
            task_run_id,
            microvm,
            run_args,
            prepared,
            task_mode,
            task_frame_tx,
            input_rx,
        )
        .await;
    });

    let abort = run_task.abort_handle();
    let runtime_cache_registration = crate::image_store::lock_runtime_cache_shared().await;
    let handle = crate::run_registry::RunHandle {
        cancel_tx,
        detach_tx: std::sync::Mutex::new(Some(detach_tx)),
        task: run_task,
        input_tx: Some(input_tx),
        exec_sessions: Default::default(),
        connector: None,
        name: String::new(),
        image: image_label,
        command: command_label,
        started: started_label,
        status: std::sync::Mutex::new(lns_ipc::RunStatus::Running),
        logs,
        config,
        exec_environment: Default::default(),
    };
    let registered = match &mode {
        crate::run::LaunchMode::Fresh => {
            crate::run_registry::register_named(run_id.clone(), requested_name, handle)
        }
        crate::run::LaunchMode::Restart { .. } => {
            crate::run_registry::transition_to_live(&run_id, handle)
        }
    };
    drop(runtime_cache_registration);
    match registered {
        Ok(microvm) => {
            let _ = microvm_tx.send(microvm);
        }
        Err(message) => {
            abort.abort();
            let _ = write_error(stream, message).await;
            return Ok(());
        }
    }

    drop(frame_tx);

    let handshake: anyhow::Result<()> = async {
        let started_frame = encode_frame(&Response::RunStarted {
            run_id: run_id.clone(),
        })
        .context("encoding RunStarted frame")?;
        match timeout(PEER_REQUEST_TIMEOUT, stream.write_all(&started_frame)).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(anyhow::Error::from(e).context("writing RunStarted frame")),
            Err(elapsed) => Err(anyhow::Error::from(elapsed).context("writing RunStarted frame")),
        }
    }
    .await;
    if let Err(e) = handshake {
        crate::run_registry::cancel(&run_id);
        crate::run_registry::deregister(&run_id);
        return Err(e);
    }

    let outcome = pump_responses(stream, &mut frame_rx, cancel_rx, detach_rx).await?;
    match post_pump_action(&outcome, detached) {
        PostPumpAction::Retain => {
            crate::run_registry::mark_exited_from_log(&run_id);
        }
        PostPumpAction::BackgroundDrain => {
            if let PumpOutcome::WriteFailed(e) = &outcome {
                log::debug!(run_id = %run_id, error = %e, "detached: CLI closed stream, draining in background");
            }
            tokio::spawn(async move {
                while frame_rx.recv().await.is_some() {}
                crate::run_registry::mark_exited_from_log(&run_id);
            });
        }
        PostPumpAction::CancelAndDeregister => {
            if let PumpOutcome::WriteFailed(e) = &outcome {
                log::debug!(run_id = %run_id, error = %e, "ipc run stream write failed; cancelling run");
            }
            crate::run_registry::cancel(&run_id);
            crate::run_registry::deregister(&run_id);
        }
    }
    Ok(())
}

async fn handle_exec(mut stream: UnixStream, args: lns_ipc::ExecImageArgs) -> anyhow::Result<()> {
    let target_run_id = match crate::run_registry::resolve(&args.run) {
        Ok(id) => id,
        Err(e) => {
            let _ = write_error(&mut stream, e.to_string()).await;
            return Ok(());
        }
    };
    let Some(connector) = crate::run_registry::connector(&target_run_id) else {
        let _ = write_error(
            &mut stream,
            format!("no active run with id {target_run_id}"),
        )
        .await;
        return Ok(());
    };

    if let Err(message) = validate_exec(&args) {
        let _ = write_error(&mut stream, message).await;
        return Ok(());
    }

    let fd = match connector
        .connect(lns_session::BROKER_PORT, std::time::Duration::from_secs(15))
        .await
    {
        Ok(fd) => fd,
        Err(e) => {
            let _ = write_error(&mut stream, format!("opening exec vsock to broker: {e:#}")).await;
            return Ok(());
        }
    };

    let (frame_tx, mut frame_rx) = mpsc::channel::<WireFrame>(FRAME_CHAN_BUF);
    let (input_tx, input_rx) = mpsc::channel::<crate::vm::session_client::SessionInput>(256);
    let session_id = match register_exec_input(&target_run_id, input_tx) {
        Ok(id) => id,
        Err(message) => {
            // SAFETY: fd was just taken from the only owner; we drop it here.
            unsafe { libc::close(fd) };
            let _ = write_error(&mut stream, message).await;
            return Ok(());
        }
    };

    let started_frame = encode_frame(&Response::ExecStarted {
        run_id: target_run_id.clone(),
        session_id: session_id.clone(),
    })
    .context("encoding ExecStarted frame")?;
    if let Err(e) = stream.write_all(&started_frame).await {
        crate::run_registry::deregister_exec_session(&target_run_id, &session_id);
        // SAFETY: fd was just taken from the only owner; we drop it here.
        unsafe { libc::close(fd) };
        return Err(anyhow::Error::from(e).context("writing exec ExecStarted frame"));
    }

    let params = build_session_params(args, &target_run_id);

    let session_frame_tx = frame_tx.clone();
    let session_task = tokio::spawn(async move {
        let code = match crate::vm::session_client::run_session_on_fd(
            fd,
            params,
            session_frame_tx.clone(),
            input_rx,
        )
        .await
        {
            Ok(c) => c,
            Err(e) => {
                let _ = session_frame_tx
                    .send(WireFrame::Json(Response::RunLog {
                        level: lns_ipc::LogLevel::Error,
                        verb: None,
                        message: format!("exec session error: {e:#}"),
                    }))
                    .await;
                1
            }
        };
        let _ = session_frame_tx
            .send(WireFrame::Json(Response::RunExit { code }))
            .await;
    });

    drop(frame_tx);

    super::drive_exec_stream(
        &mut stream,
        &target_run_id,
        &session_id,
        session_task,
        &mut frame_rx,
    )
    .await
}
