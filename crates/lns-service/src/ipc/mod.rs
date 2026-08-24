use std::time::Instant;

use crate::log;

use lns_ipc::{Request, Response, StatusInfo, WireFrame, encode_frame, encode_wire_frame};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};

pub(crate) mod adapter;
pub use adapter::run_server;

/// What a run start needs from the service: the refusals a run can be turned away for before it exists, then everything it does once it will start.
pub trait RunHost {
    type Prepared: Send;

    fn prepare(
        &self,
        run_id: &str,
        args: &lns_ipc::RunImageArgs,
    ) -> impl std::future::Future<Output = anyhow::Result<Self::Prepared>> + Send;

    fn serve<W>(
        &self,
        stream: &mut W,
        run_id: String,
        args: lns_ipc::RunImageArgs,
        prepared: Self::Prepared,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send
    where
        W: AsyncWriteExt + Unpin + Send;
}

/// How the client wants to sit on the restarted run: watching it, feeding it, or neither.
#[derive(Clone, Copy, Debug, Default)]
pub struct StartOptions {
    pub attach: bool,
    pub stdin: bool,
}

/// What restarting a stopped run needs from the service: its record, the conflicts it fails closed on, and the boot itself.
pub trait StartHost {
    fn record(
        &self,
        run_id: &str,
    ) -> impl std::future::Future<Output = anyhow::Result<crate::run_record::RunRecord>> + Send;

    fn preflight(
        &self,
        record: &crate::run_record::RunRecord,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send;

    fn serve<S>(
        &self,
        stream: &mut S,
        record: crate::run_record::RunRecord,
        options: StartOptions,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send
    where
        S: AsyncReadExt + AsyncWriteExt + Unpin + Send;
}

/// Serve one `StartRun` exchange. A running run answers `RunStarted` unchanged; a stopped run fails closed on any conflict before anything boots, so a refusal leaves its stopped state untouched.
pub async fn start_stopped_run<S, H>(
    stream: &mut S,
    run: &str,
    host: &H,
    options: StartOptions,
) -> anyhow::Result<()>
where
    S: AsyncReadExt + AsyncWriteExt + Unpin + Send,
    H: StartHost,
{
    let Ok((run_id, status)) = crate::run_registry::resolve_status(run) else {
        let _ = write_error(stream, no_startable_run(run)).await;
        return Ok(());
    };
    match status {
        lns_ipc::RunStatus::Running => {
            let frame = encode_frame(&Response::RunStarted { run_id })?;
            stream.write_all(&frame).await?;
            Ok(())
        }
        lns_ipc::RunStatus::Exited { .. } => {
            let record = match host.record(&run_id).await {
                Ok(record) => record,
                Err(e) => {
                    let _ = write_error(stream, format!("{e:#}")).await;
                    return Ok(());
                }
            };
            if let Err(e) = host.preflight(&record).await {
                let _ = write_error(stream, format!("{e:#}")).await;
                return Ok(());
            }
            host.serve(stream, record, options).await
        }
    }
}

fn no_startable_run(run: &str) -> String {
    let stopped = crate::run_registry::stopped_run_names();
    if stopped.is_empty() {
        format!("no such run: {run}; there are no stopped runs")
    } else {
        format!("no such run: {run}; stopped runs: {}", stopped.join(", "))
    }
}

/// Serve one `RunImage` exchange. A run that cannot start is answered with an error and never reaches [`RunHost::serve`], which is what registers it and names it — so a refusal costs no run id, no run name and no `RunStarted` (§3.3.2, §3.1.11).
pub async fn start_run<S, H>(
    stream: &mut S,
    args: lns_ipc::RunImageArgs,
    host: &H,
) -> anyhow::Result<()>
where
    S: AsyncReadExt + AsyncWriteExt + Unpin + Send,
    H: RunHost,
{
    if let Some(name) = &args.name
        && let Err(message) = crate::run_registry::ensure_name_available(name)
    {
        let _ = write_error(stream, message).await;
        return Ok(());
    }
    // Allocated rather than registered: the id names the attempt for this task alone until serve puts it in the registry.
    let run_id = crate::run_registry::allocate_run_id();
    let Some(prepared) = prepare_while_the_client_waits(stream, host, &run_id, &args).await else {
        return Ok(());
    };
    let prepared = match prepared {
        Ok(prepared) => prepared,
        Err(e) => {
            let _ = write_error(stream, format!("{e:#}")).await;
            return Ok(());
        }
    };
    host.serve(stream, run_id, args, prepared).await
}

/// Prepare a run only for as long as its client is still there to be answered: preparing can reach the network, and a run with no registry entry has nothing to cancel, so a client that leaves has to take the work and the host ports it bound with it. `None` means it left.
async fn prepare_while_the_client_waits<S, H>(
    stream: &mut S,
    host: &H,
    run_id: &str,
    args: &lns_ipc::RunImageArgs,
) -> Option<anyhow::Result<H::Prepared>>
where
    S: AsyncReadExt + AsyncWriteExt + Unpin + Send,
    H: RunHost,
{
    let prepare = host.prepare(run_id, args);
    tokio::pin!(prepare);
    loop {
        let mut idle = [0u8; 1];
        tokio::select! {
            biased;
            prepared = &mut prepare => return Some(prepared),
            read = stream.read(&mut idle) => match read {
                // The client sends nothing between its request and its run id, so anything readable here is the client going away.
                Ok(0) | Err(_) => return None,
                Ok(_) => continue,
            },
        }
    }
}

pub(super) async fn write_error<W>(stream: &mut W, message: String) -> anyhow::Result<()>
where
    W: AsyncWriteExt + Unpin,
{
    let frame = encode_frame(&Response::Error { message })?;
    stream.write_all(&frame).await?;
    Ok(())
}

fn primary_target(run_id: impl Into<String>) -> lns_ipc::SessionTarget {
    lns_ipc::SessionTarget::Primary {
        run_id: run_id.into(),
    }
}
#[derive(Debug, PartialEq, Eq)]
pub(super) enum PumpOutcome {
    ExitFrame,
    ChannelClosed,
    Detached,
    WriteFailed(String),
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum PostPumpAction {
    Retain,
    BackgroundDrain,
    CancelAndDeregister,
}

pub(super) fn post_pump_action(outcome: &PumpOutcome, detached: bool) -> PostPumpAction {
    match outcome {
        PumpOutcome::ExitFrame | PumpOutcome::ChannelClosed => PostPumpAction::Retain,
        PumpOutcome::Detached => PostPumpAction::BackgroundDrain,
        PumpOutcome::WriteFailed(_) if detached => PostPumpAction::BackgroundDrain,
        PumpOutcome::WriteFailed(_) => PostPumpAction::CancelAndDeregister,
    }
}

pub(super) fn peer_is_authorized(peer_uid: u32, self_uid: u32) -> bool {
    peer_uid == self_uid
}

async fn pump_responses<W>(
    stream: &mut W,
    frame_rx: &mut mpsc::Receiver<WireFrame>,
    cancel_rx: oneshot::Receiver<i32>,
    detach_rx: oneshot::Receiver<()>,
) -> anyhow::Result<PumpOutcome>
where
    W: AsyncWriteExt + Unpin,
{
    let mut cancel_rx = Some(cancel_rx);
    let mut detach_rx = Some(detach_rx);
    loop {
        tokio::select! {
            biased;
            cancel = async {
                match cancel_rx.as_mut() {
                    Some(rx) => rx.await,
                    None => std::future::pending().await,
                }
            } => {
                cancel_rx = None;
                let Ok(code) = cancel else { continue };
                let wire = WireFrame::Json(Response::RunExit { code });
                let frame = encode_wire_frame(&wire)?;
                if let Err(e) = stream.write_all(&frame).await {
                    return Ok(PumpOutcome::WriteFailed(e.to_string()));
                }
                return Ok(PumpOutcome::ExitFrame);
            }
            detach = async {
                match detach_rx.as_mut() {
                    Some(rx) => rx.await,
                    None => std::future::pending().await,
                }
            } => {
                detach_rx = None;
                if detach.is_ok() {
                    return Ok(PumpOutcome::Detached);
                }
            }
            maybe = frame_rx.recv() => {
                let Some(wire) = maybe else {
                    return Ok(PumpOutcome::ChannelClosed);
                };
                let is_exit = matches!(wire, WireFrame::Json(Response::RunExit { .. }));
                let frame = encode_wire_frame(&wire)?;
                if let Err(e) = stream.write_all(&frame).await {
                    return Ok(PumpOutcome::WriteFailed(e.to_string()));
                }
                if is_exit {
                    return Ok(PumpOutcome::ExitFrame);
                }
            }
        }
    }
}

/// Drives an accepted exec stream until its client stops reading — exit frame, or a silent disconnect — then deregisters the session and cancels its guest task, so an abandoned exec never outlives its client.
pub async fn drive_exec_stream<S>(
    stream: &mut S,
    run_id: &str,
    session_id: &str,
    session_task: tokio::task::JoinHandle<()>,
    frame_rx: &mut mpsc::Receiver<WireFrame>,
) -> anyhow::Result<()>
where
    S: AsyncReadExt + AsyncWriteExt + Unpin,
{
    let outcome = pump_exec_responses(stream, frame_rx).await?;
    if let PumpOutcome::WriteFailed(e) = &outcome {
        log::debug!(error = %e, "exec stream write failed; tearing session down");
    }
    crate::run_registry::deregister_exec_session(run_id, session_id);
    session_task.abort();
    let _ = session_task.await;
    Ok(())
}

pub(super) async fn pump_exec_responses<S>(
    stream: &mut S,
    frame_rx: &mut mpsc::Receiver<WireFrame>,
) -> anyhow::Result<PumpOutcome>
where
    S: AsyncReadExt + AsyncWriteExt + Unpin,
{
    let (mut reader, mut writer) = tokio::io::split(stream);
    let mut unexpected = [0u8; 1];
    loop {
        tokio::select! {
            biased;
            read = reader.read(&mut unexpected) => match read {
                Ok(0) => return Ok(PumpOutcome::ChannelClosed),
                Ok(_) => return Ok(PumpOutcome::WriteFailed(
                    "unexpected bytes on the exec response stream".to_string(),
                )),
                Err(e) => return Ok(PumpOutcome::WriteFailed(e.to_string())),
            },
            maybe = frame_rx.recv() => {
                let Some(wire) = maybe else {
                    return Ok(PumpOutcome::ChannelClosed);
                };
                let is_exit = matches!(wire, WireFrame::Json(Response::RunExit { .. }));
                let frame = encode_wire_frame(&wire)?;
                if let Err(e) = writer.write_all(&frame).await {
                    return Ok(PumpOutcome::WriteFailed(e.to_string()));
                }
                if is_exit {
                    return Ok(PumpOutcome::ExitFrame);
                }
            }
        }
    }
}

/// The named-volume verbs, kept off the one-shot dispatcher's own branch count.
async fn handle_volume_request(request: &Request) -> Response {
    match request {
        Request::ListVolumes => volume_response(
            crate::volume_store::list()
                .await
                .map(|volumes| Response::VolumeList { volumes }),
        ),
        Request::CreateVolume { name } => volume_response(
            crate::volume_store::create(name)
                .await
                .map(|volume| Response::VolumeCreated { volume }),
        ),
        Request::InspectVolume { name } => volume_response(
            crate::volume_store::inspect(name)
                .await
                .map(|volume| Response::VolumeInspect { volume }),
        ),
        Request::RemoveVolume { name } => volume_response(
            crate::volume_store::remove(name)
                .await
                .map(|()| Response::VolumeRemoved { name: name.clone() }),
        ),
        Request::PruneVolumes => {
            volume_response(crate::volume_store::prune().await.map(|report| {
                Response::VolumesPruned {
                    removed: report.removed,
                    reclaimed_bytes: report.reclaimed_bytes,
                    failed: report.failed,
                }
            }))
        }
        other => unreachable!("{other:?} is not a volume verb"),
    }
}

pub async fn handle_request(request: &Request, started_at: Instant) -> Response {
    match request {
        // Each of these opens a stream the caller drives; the one-shot dispatcher is not on their path.
        Request::RunImage(_)
        | Request::ExecImage(_)
        | Request::BeginConnectorSignIn { .. }
        | Request::BindConnectorCredential { .. }
        | Request::StartRun { .. }
        | Request::RunLogs { .. }
        | Request::AttachRun { .. }
        | Request::RunStats { .. } => {
            unreachable!("{request:?} has a streaming handler of its own")
        }
        Request::Ping => Response::Pong,
        Request::Status => Response::Status(StatusInfo {
            pid: std::process::id(),
            uptime_secs: started_at.elapsed().as_secs(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        }),
        Request::Shutdown => Response::ShuttingDown,
        Request::CancelRun { run_id } => {
            if crate::run_registry::cancel(run_id) {
                Response::CancelAccepted
            } else {
                Response::Error {
                    message: format!("no active run with id {run_id}"),
                }
            }
        }
        Request::SessionDetach { target } => handle_session_detach(target).await,
        Request::SessionStdin { target, bytes } => {
            forward_target_input(
                target,
                session_input_from_stdin(bytes.clone()),
                "SessionStdin",
            )
            .await
        }
        Request::SessionStdinClose { target } => {
            forward_target_input(
                target,
                Some(crate::vm::session_client::SessionInput::StdinClose),
                "SessionStdinClose",
            )
            .await
        }
        Request::SessionResize { target, rows, cols } => {
            forward_target_input(
                target,
                session_input_from_resize(*rows, *cols),
                "SessionResize",
            )
            .await
        }
        Request::SessionSignal { target, signal } => {
            forward_target_input(target, session_input_from_signal(*signal), "SessionSignal").await
        }
        Request::Kill { run, signal } => kill_request(run, *signal).await,
        Request::ListRuns => Response::RunList {
            runs: crate::run_registry::snapshot(),
        },
        volume_request @ (Request::ListVolumes
        | Request::CreateVolume { .. }
        | Request::InspectVolume { .. }
        | Request::RemoveVolume { .. }
        | Request::PruneVolumes) => handle_volume_request(volume_request).await,
        Request::PullImage {
            image,
            expected_digest,
        } => image_response(crate::image_store::pull(image, expected_digest).await.map(
            |outcome| match outcome {
                crate::image_store::PullOutcome::Sandbox { image, warnings } => {
                    Response::ImagePulled { image, warnings }
                }
                crate::image_store::PullOutcome::Mixin {
                    reference,
                    digest,
                    cached_mixins,
                } => Response::MixinPulled {
                    reference,
                    digest,
                    cached_mixins,
                },
            },
        )),
        Request::ListImages => image_response(
            crate::image_store::list()
                .await
                .map(|images| Response::ImageList { images }),
        ),
        Request::RemoveImage { image } => {
            image_response(crate::image_store::remove(image).await.map(|removed| {
                Response::ImageRemoved {
                    reference: removed.reference,
                    reclaimed_bytes: removed.reclaimed_bytes,
                }
            }))
        }
        Request::PruneImages => {
            image_response(
                crate::image_store::prune()
                    .await
                    .map(|report| Response::ImagesPruned {
                        removed: report.removed,
                        reclaimed_bytes: report.reclaimed_bytes,
                    }),
            )
        }
        Request::ResolveDefinition {
            definition,
            project_dir,
            mixins,
            decisions,
        } => image_response(
            crate::artifact::real::resolve_definition(
                definition,
                project_dir,
                mixins,
                decisions.as_deref().map(std::path::Path::new),
            )
            .await,
        ),
        Request::InspectImage {
            image,
            mixins,
            decisions,
        } => image_response(
            crate::artifact::real::inspect(
                image,
                mixins,
                decisions.as_deref().map(std::path::Path::new),
            )
            .await
            .map(|inspection| Response::ImageInspected { inspection }),
        ),
        Request::TagImage { from, to } => {
            image_response(crate::image_store::tag(from, to).await.map(|()| {
                Response::ImageTagged {
                    from: from.clone(),
                    to: to.clone(),
                }
            }))
        }
        Request::RegistryLogin {
            registry,
            username,
            secret,
        } => login_response(crate::image::verify_login(registry, username, secret).await),
        Request::StopRun { run, timeout_secs } => stop_run_request(run, *timeout_secs).await,
        Request::InspectRun { run } => inspect_run_request(run),
        Request::RemoveRun { run, force } => image_response(remove_run_request(run, *force).await),
        Request::RenameRun { run, new_name } => match crate::run_registry::rename(run, new_name) {
            Ok(()) => Response::Acknowledged,
            Err(message) => Response::Error { message },
        },
        Request::PruneRuns => image_response(prune_runs_request().await),
        Request::Unknown { method } => Response::Error {
            message: format!("unknown method: {method}"),
        },
    }
}

async fn kill_request(run: &str, signal: lns_ipc::SignalKind) -> Response {
    let id = match crate::run_registry::resolve(run) {
        Ok(id) => id,
        Err(message) => return Response::Error { message },
    };
    kill_resolved(&id, signal).await
}

async fn kill_resolved(id: &str, signal: lns_ipc::SignalKind) -> Response {
    let Some(status) = crate::run_registry::status(id) else {
        return Response::Error {
            message: format!("no active run with id {id}"),
        };
    };
    if matches!(status, lns_ipc::RunStatus::Exited { .. }) {
        return Response::Acknowledged;
    }
    forward_session_input(id, session_input_from_signal(signal), "Kill").await
}

async fn stop_run_request(run: &str, timeout_secs: u64) -> Response {
    let id = match crate::run_registry::resolve(run) {
        Ok(id) => id,
        Err(message) => return Response::Error { message },
    };
    stop_run_with(
        &id,
        std::time::Duration::from_secs(timeout_secs),
        KILL_GRACE,
        |signal| forward_session_input(&id, session_input_from_signal(signal), "StopRun"),
    )
    .await
}

fn inspect_run_request(run: &str) -> Response {
    match crate::run_registry::resolve(run) {
        Ok(id) => inspect_resolved(&id),
        Err(message) => Response::Error { message },
    }
}

fn inspect_resolved(id: &str) -> Response {
    match crate::run_registry::inspect(id) {
        Some(details) => Response::RunInspect {
            details: Box::new(details),
        },
        None => Response::Error {
            message: format!("no active run with id {id}"),
        },
    }
}

async fn remove_run_request(run: &str, force: bool) -> anyhow::Result<Response> {
    let root = crate::cache::root()?;
    Ok(remove_run_with(
        run,
        force,
        &crate::run::RealRemoveDir,
        &root,
        |id, sig| async move {
            forward_session_input(&id, session_input_from_signal(sig), "RemoveRun").await
        },
        |id, forced| {
            if let Err(e) =
                crate::audit::record_run_removed(id, forced, false, &crate::oauth::RealClock)
            {
                crate::log::warn!("run removal not audited: {e:#}");
            }
        },
    )
    .await)
}

async fn prune_runs_request() -> anyhow::Result<Response> {
    let root = crate::cache::root()?;
    Ok(prune_runs_with(
        &crate::image_store::RealFs,
        &crate::run::RealRemoveDir,
        &root,
        |removed| {
            for id in removed {
                if let Err(e) =
                    crate::audit::record_runs_pruned(id, removed, &crate::oauth::RealClock)
                {
                    crate::log::warn!("run prune not audited: {e:#}");
                }
            }
        },
    )
    .await)
}

/// Serve one `RemoveRun`: an exited run's registry entry and run dir go together; a running one is refused unless forced, and force is a stop first, not a shortcut around one.
pub async fn remove_run_with<R, F, Fut, N>(
    run: &str,
    force: bool,
    remover: &R,
    cache_root: &std::path::Path,
    send_signal: F,
    note_removed: N,
) -> Response
where
    R: crate::run::RemoveDir,
    F: Fn(String, lns_ipc::SignalKind) -> Fut,
    Fut: std::future::Future<Output = Response>,
    N: Fn(&str, bool),
{
    let id = match crate::run_registry::resolve(run) {
        Ok(id) => id,
        Err(message) => return Response::Error { message },
    };
    remove_resolved_run_with(
        &id,
        run,
        force,
        remover,
        cache_root,
        send_signal,
        note_removed,
    )
    .await
}

async fn remove_resolved_run_with<R, F, Fut, N>(
    id: &str,
    run: &str,
    force: bool,
    remover: &R,
    cache_root: &std::path::Path,
    send_signal: F,
    note_removed: N,
) -> Response
where
    R: crate::run::RemoveDir,
    F: Fn(String, lns_ipc::SignalKind) -> Fut,
    Fut: std::future::Future<Output = Response>,
    N: Fn(&str, bool),
{
    if force
        && let Response::Error { message } =
            stop_run_with(id, std::time::Duration::ZERO, KILL_GRACE, |sig| {
                send_signal(id.to_string(), sig)
            })
            .await
    {
        return Response::Error { message };
    }
    match crate::run_registry::remove_if_exited(id) {
        crate::run_registry::RemoveOutcome::Removed => {
            note_removed(id, force);
            crate::run::reclaim_run_dir(remover, cache_root, id);
            Response::Acknowledged
        }
        crate::run_registry::RemoveOutcome::Running => Response::Error {
            message: format!(
                "run {run} is still running; stop it first with `lns stop {run}` or force with `lns rm -f {run}`"
            ),
        },
        crate::run_registry::RemoveOutcome::NotFound => Response::Error {
            message: format!("no run with id {id}"),
        },
    }
}

/// Sweep every stopped run and every orphan run dir — the one command that takes a machine back to clean.
pub async fn prune_runs_with<F, R, N>(
    fs: &F,
    remover: &R,
    cache_root: &std::path::Path,
    note_pruned: N,
) -> Response
where
    F: crate::image_store::Fs,
    R: crate::run::RemoveDir,
    N: Fn(&[String]),
{
    let mut removed = crate::run_registry::prune_exited();
    for id in &removed {
        crate::run::reclaim_run_dir(remover, cache_root, id);
    }
    let known: std::collections::HashSet<String> = crate::run_registry::snapshot()
        .into_iter()
        .map(|s| s.id)
        .collect();
    if let Ok(entries) = fs.read_dir(&cache_root.join("runs")).await {
        for dir in entries {
            let Some(id) = dir.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if !known.contains(id) && !removed.iter().any(|r| r == id) {
                crate::run::reclaim_run_dir(remover, cache_root, id);
                removed.push(id.to_string());
            }
        }
    }
    if !removed.is_empty() {
        note_pruned(&removed);
    }
    Response::RunsPruned { removed }
}

fn volume_response(result: anyhow::Result<Response>) -> Response {
    result.unwrap_or_else(|e| Response::Error {
        message: format!("{e:#}"),
    })
}

fn image_response(result: anyhow::Result<Response>) -> Response {
    result.unwrap_or_else(|e| Response::Error {
        message: format!("{e:#}"),
    })
}

fn login_response(result: anyhow::Result<()>) -> Response {
    match result {
        Ok(()) => Response::RegistryLoginVerified,
        Err(e) => Response::Error {
            message: format!("{e:#}"),
        },
    }
}

const KILL_GRACE: std::time::Duration = std::time::Duration::from_secs(5);
const EXIT_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

pub(super) async fn stop_run_with<F, Fut>(
    run_id: &str,
    timeout: std::time::Duration,
    kill_grace: std::time::Duration,
    send_signal: F,
) -> Response
where
    F: Fn(lns_ipc::SignalKind) -> Fut,
    Fut: std::future::Future<Output = Response>,
{
    let Some(status) = crate::run_registry::status(run_id) else {
        return Response::Error {
            message: format!("no active run with id {run_id}"),
        };
    };
    if matches!(status, lns_ipc::RunStatus::Exited { .. }) {
        return Response::RunStopped { forced: false };
    }
    if let Response::Error { message } = send_signal(lns_ipc::SignalKind::Term).await {
        return Response::Error { message };
    }
    if wait_for_exit(run_id, timeout).await {
        return Response::RunStopped { forced: false };
    }
    if let Response::Error { message } = send_signal(lns_ipc::SignalKind::Kill).await {
        return Response::Error { message };
    }
    if wait_for_exit(run_id, kill_grace).await {
        return Response::RunStopped { forced: true };
    }
    Response::Error {
        message: format!("run {run_id} did not exit within {kill_grace:?} of SIGKILL"),
    }
}

async fn wait_for_exit(run_id: &str, timeout: std::time::Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        match crate::run_registry::status(run_id) {
            None | Some(lns_ipc::RunStatus::Exited { .. }) => return true,
            Some(lns_ipc::RunStatus::Running) => {}
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(EXIT_POLL_INTERVAL).await;
    }
}

async fn handle_session_detach(target: &lns_ipc::SessionTarget) -> Response {
    match target {
        lns_ipc::SessionTarget::Primary { run_id } => {
            match crate::run_registry::request_detach(run_id) {
                crate::run_registry::DetachOutcome::Detached => Response::DetachAccepted,
                crate::run_registry::DetachOutcome::NotAttached => Response::Error {
                    message: format!("run {run_id} is not attached"),
                },
                crate::run_registry::DetachOutcome::NotFound => Response::Error {
                    message: format!("no active run with id {run_id}"),
                },
            }
        }
        lns_ipc::SessionTarget::Exec { run_id, session_id } => {
            let Some(tx) = crate::run_registry::session_input_sender(target) else {
                return missing_exec_session(run_id, session_id);
            };
            let sent = tx
                .send(crate::vm::session_client::SessionInput::Detach)
                .await;
            crate::run_registry::deregister_exec_session(run_id, session_id);
            match sent {
                Ok(()) => Response::DetachAccepted,
                Err(e) => Response::Error {
                    message: format!("detaching exec session {session_id} in run {run_id}: {e}"),
                },
            }
        }
    }
}

async fn forward_target_input(
    target: &lns_ipc::SessionTarget,
    input: Option<crate::vm::session_client::SessionInput>,
    kind: &'static str,
) -> Response {
    let Some(input) = input else {
        return Response::Error {
            message: format!("{kind} not supported on this build"),
        };
    };
    let Some(tx) = crate::run_registry::session_input_sender(target) else {
        return missing_session(target);
    };
    match tx.send(input).await {
        Ok(()) => Response::Acknowledged,
        Err(e) => Response::Error {
            message: format!(
                "forwarding {kind} to session in run {} failed: {e}",
                target.run_id()
            ),
        },
    }
}

fn missing_session(target: &lns_ipc::SessionTarget) -> Response {
    match target {
        lns_ipc::SessionTarget::Primary { run_id } => Response::Error {
            message: format!("no active session for run {run_id}"),
        },
        lns_ipc::SessionTarget::Exec { run_id, session_id } => {
            missing_exec_session(run_id, session_id)
        }
    }
}

fn missing_exec_session(run_id: &str, session_id: &str) -> Response {
    Response::Error {
        message: format!("no active exec session {session_id} for run {run_id}"),
    }
}

async fn forward_session_input(
    run_id: &str,
    input: Option<crate::vm::session_client::SessionInput>,
    kind: &'static str,
) -> Response {
    forward_target_input(&primary_target(run_id.to_string()), input, kind).await
}

fn session_input_from_stdin(bytes: Vec<u8>) -> Option<crate::vm::session_client::SessionInput> {
    Some(crate::vm::session_client::SessionInput::StdinBytes(bytes))
}
fn session_input_from_resize(
    rows: u16,
    cols: u16,
) -> Option<crate::vm::session_client::SessionInput> {
    Some(crate::vm::session_client::SessionInput::Resize { rows, cols })
}
fn session_input_from_signal(
    signal: lns_ipc::SignalKind,
) -> Option<crate::vm::session_client::SessionInput> {
    Some(crate::vm::session_client::SessionInput::Signal(map_signal(
        signal,
    )))
}

fn map_signal(s: lns_ipc::SignalKind) -> lns_session::SignalKind {
    match s {
        lns_ipc::SignalKind::Int => lns_session::SignalKind::Int,
        lns_ipc::SignalKind::Term => lns_session::SignalKind::Term,
        lns_ipc::SignalKind::Quit => lns_session::SignalKind::Quit,
        lns_ipc::SignalKind::Hup => lns_session::SignalKind::Hup,
        lns_ipc::SignalKind::Winch => lns_session::SignalKind::Winch,
        lns_ipc::SignalKind::Kill => lns_session::SignalKind::Kill,
    }
}

pub(super) fn validate_exec(args: &lns_ipc::ExecImageArgs) -> Result<(), String> {
    if args.argv.is_empty() {
        return Err("ExecImage.argv is empty".to_string());
    }
    Ok(())
}

/// The exec handshake's registration step: allocates the session id that makes an exec addressable within its run.
pub fn register_exec_input(
    run_id: &str,
    input_tx: tokio::sync::mpsc::Sender<crate::vm::session_client::SessionInput>,
) -> Result<String, String> {
    let session_id = crate::run_registry::allocate_run_id();
    if crate::run_registry::register_exec_session(run_id, session_id.clone(), input_tx) {
        Ok(session_id)
    } else {
        Err(format!("no active run with id {run_id}"))
    }
}

/// `run_id` is the *resolved* id: `args.run` may be a name or an id prefix, and the registry keys tool paths by id alone.
pub(super) fn build_session_params(
    args: lns_ipc::ExecImageArgs,
    run_id: &str,
) -> crate::vm::session_client::SessionParams {
    let exec_environment = crate::run_registry::exec_environment(run_id);
    let env = crate::workload_env::exec_session_env(&exec_environment, &args.env);
    crate::vm::session_client::SessionParams {
        argv: args.argv,
        env,
        cwd: exec_environment.workdir,
        hostname: None,
        tty: args.tty,
        stdin: args.stdin,
        initial_winsize: args
            .initial_winsize
            .map(|(rows, cols)| lns_session::Winsize { rows, cols }),
        confine: true,
        dies_with_client: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_with_the_services_own_uid_is_authorized() {
        assert!(peer_is_authorized(1000, 1000));
    }

    #[test]
    fn a_peer_from_a_different_uid_is_rejected_in_both_directions() {
        assert!(!peer_is_authorized(1000, 0));
        assert!(!peer_is_authorized(0, 1000));
    }

    #[tokio::test]
    async fn ping_returns_pong() {
        let started = Instant::now();
        assert_eq!(
            handle_request(&Request::Ping, started).await,
            Response::Pong
        );
    }

    #[tokio::test]
    async fn status_returns_info() {
        let started = Instant::now();
        let resp = handle_request(&Request::Status, started).await;
        match resp {
            Response::Status(info) => {
                assert_eq!(info.pid, std::process::id());
                assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
            }
            _ => panic!("expected Status response"),
        }
    }

    #[tokio::test]
    async fn shutdown_returns_shutting_down() {
        let started = Instant::now();
        assert_eq!(
            handle_request(&Request::Shutdown, started).await,
            Response::ShuttingDown
        );
    }

    #[tokio::test]
    #[should_panic(expected = "has a streaming handler of its own")]
    async fn run_image_via_handle_request_panics() {
        let _ = handle_request(
            &Request::RunImage(Box::new(lns_ipc::RunImageArgs {
                image: None,
                resolved_image: None,
                mixins: Vec::new(),
                composed_mixins: Vec::new(),
                name: None,
                cpus: 1,
                mem: 0,
                cpus_explicit: false,
                mem_explicit: false,
                policy_path: None,
                sandbox_user: None,
                sandbox_uid: None,
                entrypoint: None,
                hostname: None,
                cmd: vec![],
                env: vec![],
                workdir: None,
                debug: false,
                tty: true,
                stdin: true,
                initial_winsize: None,
                detached: false,
                published_ports: vec![],
                volumes: vec![],
                binds: vec![],
                auto_remove: false,
                verify_sandbox: false,
                definition: None,
                definition_dir: None,
                authored_egress: None,
                packed_filesets: Vec::new(),
                denied_host_paths: Vec::new(),
            })),
            Instant::now(),
        )
        .await;
    }

    #[tokio::test]
    #[should_panic(expected = "has a streaming handler of its own")]
    async fn begin_connector_sign_in_via_handle_request_panics() {
        let _ = handle_request(
            &Request::BeginConnectorSignIn {
                id: "some-oauth".into(),
            },
            Instant::now(),
        )
        .await;
    }

    #[tokio::test]
    #[should_panic(expected = "has a streaming handler of its own")]
    async fn bind_connector_credential_via_handle_request_panics() {
        let _ = handle_request(
            &Request::BindConnectorCredential {
                id: "some-provider".into(),
            },
            Instant::now(),
        )
        .await;
    }

    #[tokio::test]
    async fn unknown_returns_error() {
        let started = Instant::now();
        let resp = handle_request(
            &Request::Unknown {
                method: "foo".into(),
            },
            started,
        )
        .await;
        assert_eq!(
            resp,
            Response::Error {
                message: "unknown method: foo".into()
            }
        );
    }

    fn decode_wire_frames_from(mut buf: &[u8]) -> Vec<WireFrame> {
        use lns_ipc::decode_wire_frame_sync;
        let mut out = Vec::new();
        while !buf.is_empty() {
            let wire = decode_wire_frame_sync(&mut buf).expect("decode wire frame");
            out.push(wire);
        }
        out
    }

    fn never_cancel() -> (oneshot::Sender<i32>, oneshot::Receiver<i32>) {
        oneshot::channel()
    }

    fn never_detach() -> (oneshot::Sender<()>, oneshot::Receiver<()>) {
        oneshot::channel()
    }

    #[tokio::test]
    async fn pump_preserves_frame_order_and_exits_on_run_exit() {
        let mut sink: Vec<u8> = Vec::new();
        let (tx, mut rx) = mpsc::channel::<WireFrame>(8);
        let (_cancel_tx, cancel_rx) = never_cancel();
        let (_detach_tx, detach_rx) = never_detach();

        tx.send(WireFrame::Stdout(b"hello".to_vec())).await.unwrap();
        tx.send(WireFrame::Json(Response::RunLog {
            level: lns_ipc::LogLevel::Info,
            verb: Some("Booting".into()),
            message: "msg".into(),
        }))
        .await
        .unwrap();
        tx.send(WireFrame::Json(Response::RunExit { code: 0 }))
            .await
            .unwrap();
        tx.send(WireFrame::Stdout(b"never".to_vec())).await.unwrap();
        drop(tx);

        let outcome = pump_responses(&mut sink, &mut rx, cancel_rx, detach_rx)
            .await
            .unwrap();
        assert_eq!(outcome, PumpOutcome::ExitFrame);

        let decoded = decode_wire_frames_from(&sink);
        assert_eq!(decoded.len(), 3, "post-exit frame must not be forwarded");
        assert!(matches!(decoded[0], WireFrame::Stdout(_)));
        assert!(matches!(
            decoded[1],
            WireFrame::Json(Response::RunLog { .. })
        ));
        assert!(matches!(
            decoded[2],
            WireFrame::Json(Response::RunExit { code: 0 })
        ));
    }

    #[tokio::test]
    async fn pump_returns_channel_closed_when_all_senders_drop_without_exit() {
        let mut sink: Vec<u8> = Vec::new();
        let (tx, mut rx) = mpsc::channel::<WireFrame>(8);
        let (_cancel_tx, cancel_rx) = never_cancel();
        let (_detach_tx, detach_rx) = never_detach();
        tx.send(WireFrame::Stdout(b"x".to_vec())).await.unwrap();
        drop(tx);

        let outcome = pump_responses(&mut sink, &mut rx, cancel_rx, detach_rx)
            .await
            .unwrap();
        assert_eq!(outcome, PumpOutcome::ChannelClosed);
    }

    #[tokio::test]
    async fn pump_reports_write_failure_on_disconnect() {
        let (write_side, read_side) = tokio::io::duplex(64);
        drop(read_side);

        let (tx, mut rx) = mpsc::channel::<WireFrame>(8);
        let (_cancel_tx, cancel_rx) = never_cancel();
        let (_detach_tx, detach_rx) = never_detach();
        tx.send(WireFrame::Stdout(vec![0; 1024])).await.unwrap();
        drop(tx);

        let mut w = write_side;
        let outcome = pump_responses(&mut w, &mut rx, cancel_rx, detach_rx)
            .await
            .unwrap();
        assert!(
            matches!(outcome, PumpOutcome::WriteFailed(_)),
            "expected WriteFailed, got {outcome:?}",
        );
    }

    #[tokio::test(start_paused = true)]
    async fn exec_pump_detects_a_silent_client_disconnect() {
        let (client, mut service) = tokio::io::duplex(64);
        let (_frame_tx, mut frame_rx) = mpsc::channel::<WireFrame>(1);
        drop(client);

        let outcome = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            pump_exec_responses(&mut service, &mut frame_rx),
        )
        .await
        .expect("a silent exec disconnect should be detected")
        .unwrap();

        assert_eq!(outcome, PumpOutcome::ChannelClosed);
    }

    #[tokio::test]
    async fn exec_pump_rejects_unexpected_client_bytes() {
        let (mut client, mut service) = tokio::io::duplex(64);
        let (_frame_tx, mut frame_rx) = mpsc::channel::<WireFrame>(1);
        client.write_all(b"x").await.unwrap();

        let outcome = pump_exec_responses(&mut service, &mut frame_rx)
            .await
            .unwrap();

        assert_eq!(
            outcome,
            PumpOutcome::WriteFailed("unexpected bytes on the exec response stream".to_string())
        );
    }

    #[tokio::test]
    async fn exec_pump_forwards_output_and_stops_after_exit() {
        let (mut client, mut service) = tokio::io::duplex(4096);
        let (frame_tx, mut frame_rx) = mpsc::channel::<WireFrame>(2);
        frame_tx
            .send(WireFrame::Stdout(b"exec-out".to_vec()))
            .await
            .unwrap();
        frame_tx
            .send(WireFrame::Json(Response::RunExit { code: 7 }))
            .await
            .unwrap();
        let pump =
            tokio::spawn(async move { pump_exec_responses(&mut service, &mut frame_rx).await });

        let stdout = lns_ipc::read_frame_bytes_async(&mut client).await.unwrap();
        assert_eq!(
            lns_ipc::decode_wire_frame_from_bytes(&stdout).unwrap(),
            WireFrame::Stdout(b"exec-out".to_vec())
        );
        let exit = lns_ipc::read_frame_bytes_async(&mut client).await.unwrap();
        assert_eq!(
            lns_ipc::decode_wire_frame_from_bytes(&exit).unwrap(),
            WireFrame::Json(Response::RunExit { code: 7 })
        );
        assert_eq!(pump.await.unwrap().unwrap(), PumpOutcome::ExitFrame);
    }

    struct ExecPumpStream {
        read_error: bool,
    }

    impl tokio::io::AsyncRead for ExecPumpStream {
        fn poll_read(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            _buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            if self.read_error {
                std::task::Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::ConnectionReset,
                    "read reset",
                )))
            } else {
                std::task::Poll::Pending
            }
        }
    }

    impl tokio::io::AsyncWrite for ExecPumpStream {
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
            _buf: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            std::task::Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "write closed",
            )))
        }

        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }

        fn poll_shutdown(
            self: std::pin::Pin<&mut Self>,
            _cx: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn exec_pump_surfaces_a_read_error() {
        let mut stream = ExecPumpStream { read_error: true };
        let (_frame_tx, mut frame_rx) = mpsc::channel::<WireFrame>(1);

        let outcome = pump_exec_responses(&mut stream, &mut frame_rx)
            .await
            .unwrap();

        assert_eq!(outcome, PumpOutcome::WriteFailed("read reset".to_string()));
    }

    #[tokio::test]
    async fn exec_pump_stops_when_the_guest_frame_channel_closes() {
        let (_client, mut service) = tokio::io::duplex(64);
        let (frame_tx, mut frame_rx) = mpsc::channel::<WireFrame>(1);
        drop(frame_tx);

        let outcome = pump_exec_responses(&mut service, &mut frame_rx)
            .await
            .unwrap();

        assert_eq!(outcome, PumpOutcome::ChannelClosed);
    }

    struct TerminationProbe(Option<tokio::sync::oneshot::Sender<()>>);

    impl Drop for TerminationProbe {
        fn drop(&mut self) {
            if let Some(tx) = self.0.take() {
                let _ = tx.send(());
            }
        }
    }

    #[tokio::test]
    async fn a_write_failure_still_tears_the_exec_session_down() {
        let mut stream = ExecPumpStream { read_error: false };
        let (frame_tx, mut frame_rx) = mpsc::channel::<WireFrame>(1);
        frame_tx
            .send(WireFrame::Stdout(b"exec-out".to_vec()))
            .await
            .unwrap();
        let (terminated_tx, terminated_rx) = tokio::sync::oneshot::channel::<()>();
        let probe = TerminationProbe(Some(terminated_tx));
        let (started_tx, started_rx) = tokio::sync::oneshot::channel::<()>();
        let session_task = tokio::spawn(async move {
            let _probe = probe;
            let _ = started_tx.send(());
            std::future::pending::<()>().await
        });
        started_rx
            .await
            .expect("the fake guest session task is running before the teardown");

        drive_exec_stream(&mut stream, "run-w", "exec-w", session_task, &mut frame_rx)
            .await
            .unwrap();

        terminated_rx
            .await
            .expect("a failed client write must still cancel the guest session task");
    }

    #[tokio::test]
    async fn exec_pump_surfaces_a_write_error() {
        let mut stream = ExecPumpStream { read_error: false };
        let (frame_tx, mut frame_rx) = mpsc::channel::<WireFrame>(1);
        frame_tx
            .send(WireFrame::Stdout(b"exec-out".to_vec()))
            .await
            .unwrap();

        let outcome = pump_exec_responses(&mut stream, &mut frame_rx)
            .await
            .unwrap();

        assert_eq!(
            outcome,
            PumpOutcome::WriteFailed("write closed".to_string())
        );
        assert!(
            stream.flush().await.is_ok() && stream.shutdown().await.is_ok(),
            "teardown after a surfaced write failure must not mask it with a second error"
        );
    }

    #[tokio::test]
    async fn pump_cancel_writes_run_exit_even_when_frame_channel_is_full() {
        let mut sink: Vec<u8> = Vec::new();
        let (tx, mut rx) = mpsc::channel::<WireFrame>(1);
        let (cancel_tx, cancel_rx) = oneshot::channel::<i32>();
        let (_detach_tx, detach_rx) = never_detach();

        tx.send(WireFrame::Stdout(b"buffered".to_vec()))
            .await
            .unwrap();
        cancel_tx.send(130).unwrap();

        let outcome = pump_responses(&mut sink, &mut rx, cancel_rx, detach_rx)
            .await
            .unwrap();
        assert_eq!(outcome, PumpOutcome::ExitFrame);

        let decoded = decode_wire_frames_from(&sink);
        assert!(matches!(
            decoded.last(),
            Some(WireFrame::Json(Response::RunExit { code: 130 }))
        ));
        drop(tx);
    }

    #[tokio::test]
    async fn pump_continues_after_cancel_sender_drop_without_firing() {
        let mut sink: Vec<u8> = Vec::new();
        let (tx, mut rx) = mpsc::channel::<WireFrame>(4);
        let (cancel_tx, cancel_rx) = oneshot::channel::<i32>();
        let (_detach_tx, detach_rx) = never_detach();

        drop(cancel_tx);

        tx.send(WireFrame::Stdout(b"k".to_vec())).await.unwrap();
        tx.send(WireFrame::Json(Response::RunExit { code: 0 }))
            .await
            .unwrap();
        drop(tx);

        let outcome = pump_responses(&mut sink, &mut rx, cancel_rx, detach_rx)
            .await
            .unwrap();
        assert_eq!(outcome, PumpOutcome::ExitFrame);

        let decoded = decode_wire_frames_from(&sink);
        assert!(matches!(decoded[0], WireFrame::Stdout(_)));
        assert!(matches!(
            decoded[1],
            WireFrame::Json(Response::RunExit { code: 0 })
        ));
    }

    #[tokio::test]
    async fn pump_returns_detached_when_the_detach_signal_fires() {
        let mut sink: Vec<u8> = Vec::new();
        let (_tx, mut rx) = mpsc::channel::<WireFrame>(8);
        let (_cancel_tx, cancel_rx) = never_cancel();
        let (detach_tx, detach_rx) = never_detach();

        detach_tx.send(()).unwrap();

        let outcome = pump_responses(&mut sink, &mut rx, cancel_rx, detach_rx)
            .await
            .unwrap();
        assert_eq!(
            outcome,
            PumpOutcome::Detached,
            "a deliberate detach must hand the run off, not cancel it",
        );
    }

    #[tokio::test]
    async fn pump_continues_after_detach_sender_drop_without_firing() {
        let mut sink: Vec<u8> = Vec::new();
        let (tx, mut rx) = mpsc::channel::<WireFrame>(4);
        let (_cancel_tx, cancel_rx) = never_cancel();
        let (detach_tx, detach_rx) = never_detach();

        drop(detach_tx);

        tx.send(WireFrame::Json(Response::RunExit { code: 0 }))
            .await
            .unwrap();
        drop(tx);

        let outcome = pump_responses(&mut sink, &mut rx, cancel_rx, detach_rx)
            .await
            .unwrap();
        assert_eq!(
            outcome,
            PumpOutcome::ExitFrame,
            "a dropped detach sender must not be mistaken for a detach",
        );
    }

    #[test]
    fn post_pump_action_detach_signal_drains_in_background_regardless_of_detached() {
        assert_eq!(
            post_pump_action(&PumpOutcome::Detached, false),
            PostPumpAction::BackgroundDrain,
            "a deliberate detach of an attached run must leave it running, not cancel it",
        );
        assert_eq!(
            post_pump_action(&PumpOutcome::Detached, true),
            PostPumpAction::BackgroundDrain,
        );
    }

    #[test]
    fn post_pump_action_detached_write_failure_drains_in_background_instead_of_cancelling() {
        let outcome = PumpOutcome::WriteFailed("broken pipe".into());
        assert_eq!(
            post_pump_action(&outcome, true),
            PostPumpAction::BackgroundDrain,
            "detached + WriteFailed must NOT cancel the run — the CLI is allowed to close the stream after the run id is printed",
        );
    }

    #[test]
    fn post_pump_action_attached_write_failure_cancels_the_run() {
        let outcome = PumpOutcome::WriteFailed("broken pipe".into());
        assert_eq!(
            post_pump_action(&outcome, false),
            PostPumpAction::CancelAndDeregister,
            "attached + WriteFailed must cancel — the CLI dying mid-run means the user wants the run gone",
        );
    }

    #[test]
    fn post_pump_action_exit_frame_retains_the_finished_run_regardless_of_detached() {
        assert_eq!(
            post_pump_action(&PumpOutcome::ExitFrame, true),
            PostPumpAction::Retain,
            "a finished run must stay listed as exited until `lns sandbox rm`/`prune`",
        );
        assert_eq!(
            post_pump_action(&PumpOutcome::ExitFrame, false),
            PostPumpAction::Retain,
        );
    }

    #[test]
    fn post_pump_action_channel_closed_retains_the_finished_run_regardless_of_detached() {
        assert_eq!(
            post_pump_action(&PumpOutcome::ChannelClosed, true),
            PostPumpAction::Retain,
        );
        assert_eq!(
            post_pump_action(&PumpOutcome::ChannelClosed, false),
            PostPumpAction::Retain,
        );
    }

    #[tokio::test]
    async fn handle_request_cancel_unknown_run_returns_error() {
        let resp = handle_request(
            &Request::CancelRun {
                run_id: "ffffffffffffffffffffffffffffffff".to_string(),
            },
            Instant::now(),
        )
        .await;
        match resp {
            Response::Error { message } => assert!(message.contains("no active run")),
            _ => unreachable!("forward_session_input always returns Acknowledged or Error"),
        }
    }

    #[tokio::test]
    async fn handle_request_detach_unknown_run_returns_error() {
        let resp = handle_request(
            &Request::SessionDetach {
                target: primary_target("ffffffffffffffffffffffffffffffff".to_string()),
            },
            Instant::now(),
        )
        .await;
        match resp {
            Response::Error { message } => assert!(message.contains("no active run")),
            other => unreachable!("expected Error for a missing run, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn forward_session_input_awaits_back_pressure_instead_of_dropping() {
        use crate::vm::session_client::SessionInput;
        use tokio::sync::mpsc;

        let id = crate::run_registry::allocate_run_id();
        let (cancel_tx, _cancel_rx) = tokio::sync::oneshot::channel::<i32>();
        let task = tokio::spawn(std::future::pending::<()>());
        let (input_tx, mut input_rx) = mpsc::channel::<SessionInput>(1);
        input_tx
            .send(SessionInput::StdinBytes(b"first".to_vec()))
            .await
            .unwrap();

        crate::run_registry::register(
            id.clone(),
            crate::run_registry::RunHandle {
                cancel_tx,
                detach_tx: std::sync::Mutex::new(None),
                task,
                input_tx: Some(input_tx),
                exec_sessions: Default::default(),
                connector: None,
                name: String::new(),
                image: String::new(),
                command: String::new(),
                started: String::new(),
                status: std::sync::Mutex::new(lns_ipc::RunStatus::Running),
                logs: std::sync::Arc::new(crate::run_log::RunLogBuffer::default()),
                config: lns_ipc::RunConfig::default(),
                exec_environment: Default::default(),
            },
        );

        let consumer = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            let _ = input_rx.recv().await;
            input_rx
        });

        let resp = handle_request(
            &Request::SessionStdin {
                target: primary_target(id.clone()),
                bytes: b"second".to_vec(),
            },
            Instant::now(),
        )
        .await;

        assert!(
            matches!(resp, Response::Acknowledged),
            "expected Acknowledged after back-pressure release, got {resp:?}",
        );

        let _ = consumer.await;
        crate::run_registry::deregister(&id);
    }

    #[tokio::test]
    async fn forward_session_input_errors_when_run_not_registered() {
        let id = "deadbeef00000000000000000000aa01".to_string();
        let resp = handle_request(
            &Request::SessionStdin {
                target: primary_target(id.clone()),
                bytes: b"hi".to_vec(),
            },
            Instant::now(),
        )
        .await;
        match resp {
            Response::Error { message } => {
                assert!(message.contains("no active session"));
                assert!(message.contains(&id.to_string()));
            }
            _ => unreachable!("forward_session_input always returns Acknowledged or Error"),
        }
    }

    fn as_json(resp: Response) -> serde_json::Value {
        serde_json::to_value(&resp).expect("responses serialize")
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn handle_request_volume_lifecycle_round_trips_through_the_store() {
        let d = tempfile::tempdir().unwrap();
        let _h = crate::test_env::EnvVarGuard::set("HOME", d.path());
        let now = Instant::now();

        let created = as_json(
            handle_request(
                &Request::CreateVolume {
                    name: "cov-ipc-vol".into(),
                },
                now,
            )
            .await,
        );
        assert_eq!(created["type"], "VolumeCreated", "got {created}");
        assert_eq!(created["volume"]["name"], "cov-ipc-vol");

        let listed = as_json(handle_request(&Request::ListVolumes, now).await);
        assert_eq!(listed["type"], "VolumeList", "got {listed}");
        let listed_names: Vec<&serde_json::Value> = listed["volumes"]
            .as_array()
            .expect("a volume array")
            .iter()
            .map(|v| &v["name"])
            .collect();
        assert!(
            listed_names.contains(&&serde_json::json!("cov-ipc-vol")),
            "got {listed_names:?}"
        );

        let inspected = as_json(
            handle_request(
                &Request::InspectVolume {
                    name: "cov-ipc-vol".into(),
                },
                now,
            )
            .await,
        );
        assert_eq!(inspected["type"], "VolumeInspect", "got {inspected}");
        assert_eq!(inspected["volume"]["in_use_by"], serde_json::Value::Null);

        let removed = handle_request(
            &Request::RemoveVolume {
                name: "cov-ipc-vol".into(),
            },
            now,
        )
        .await;
        assert_eq!(
            removed,
            Response::VolumeRemoved {
                name: "cov-ipc-vol".into()
            }
        );

        let _ = handle_request(
            &Request::CreateVolume {
                name: "cov-ipc-prune".into(),
            },
            now,
        )
        .await;
        let pruned = as_json(handle_request(&Request::PruneVolumes, now).await);
        assert_eq!(pruned["type"], "VolumesPruned", "got {pruned}");
        let pruned_names = pruned["removed"].as_array().expect("a removed array");
        assert!(
            pruned_names.contains(&serde_json::json!("cov-ipc-prune")),
            "got {pruned_names:?}"
        );
        assert!(pruned["reclaimed_bytes"].as_u64().expect("reclaimed bytes") > 0);
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn handle_request_inspect_of_an_unknown_volume_surfaces_the_store_error() {
        let d = tempfile::tempdir().unwrap();
        let _h = crate::test_env::EnvVarGuard::set("HOME", d.path());
        let resp = as_json(
            handle_request(
                &Request::InspectVolume {
                    name: "cov-ipc-absent".into(),
                },
                Instant::now(),
            )
            .await,
        );
        assert_eq!(resp["type"], "Error", "got {resp}");
        let message = resp["message"].as_str().expect("an error message");
        assert!(message.contains("no such volume"), "got: {message}");
    }

    #[tokio::test]
    async fn handle_request_list_runs_returns_snapshot() {
        let response = handle_request(&Request::ListRuns, Instant::now()).await;
        assert!(matches!(response, Response::RunList { .. }));
    }

    #[tokio::test]
    async fn handle_request_unknown_method_returns_descriptive_error() {
        let response = handle_request(
            &Request::Unknown {
                method: "wibble".into(),
            },
            Instant::now(),
        )
        .await;
        match response {
            Response::Error { message } => {
                assert!(message.contains("unknown method"));
                assert!(message.contains("wibble"));
            }
            other => unreachable!("expected Error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn handle_request_run_stdin_for_unregistered_run_returns_error() {
        let response = handle_request(
            &Request::SessionStdin {
                target: primary_target("ffffffffffffffffffffffffffff9999".to_string()),
                bytes: vec![],
            },
            Instant::now(),
        )
        .await;
        assert!(matches!(response, Response::Error { .. }));
    }

    #[tokio::test]
    async fn handle_request_routes_stdin_to_the_named_exec_session_only() {
        use crate::vm::session_client::SessionInput;

        let run_id = crate::run_registry::allocate_run_id();
        let (mut handle, _cancel_rx) = crate::run_registry::test_handle();
        let (primary_tx, mut primary_rx) = tokio::sync::mpsc::channel::<SessionInput>(1);
        handle.input_tx = Some(primary_tx);
        crate::run_registry::register(run_id.clone(), handle);
        let (exec_tx, mut exec_rx) = tokio::sync::mpsc::channel::<SessionInput>(1);
        assert!(crate::run_registry::register_exec_session(
            &run_id,
            "exec-1".to_string(),
            exec_tx,
        ));

        let response = handle_request(
            &Request::SessionStdin {
                target: lns_ipc::SessionTarget::Exec {
                    run_id: run_id.clone(),
                    session_id: "exec-1".to_string(),
                },
                bytes: b"exec only".to_vec(),
            },
            Instant::now(),
        )
        .await;

        assert_eq!(response, Response::Acknowledged);
        assert!(matches!(
            exec_rx.recv().await,
            Some(SessionInput::StdinBytes(bytes)) if bytes == b"exec only"
        ));
        assert!(primary_rx.try_recv().is_err());

        crate::run_registry::deregister(&run_id);
    }

    #[tokio::test]
    async fn handle_request_closes_only_the_named_exec_sessions_stdin() {
        use crate::vm::session_client::SessionInput;

        let run_id = crate::run_registry::allocate_run_id();
        let (handle, _cancel_rx) = crate::run_registry::test_handle();
        crate::run_registry::register(run_id.clone(), handle);
        let (exec_tx, mut exec_rx) = tokio::sync::mpsc::channel::<SessionInput>(1);
        assert!(crate::run_registry::register_exec_session(
            &run_id,
            "exec-1".to_string(),
            exec_tx,
        ));

        let response = handle_request(
            &Request::SessionStdinClose {
                target: lns_ipc::SessionTarget::Exec {
                    run_id: run_id.clone(),
                    session_id: "exec-1".to_string(),
                },
            },
            Instant::now(),
        )
        .await;

        assert_eq!(response, Response::Acknowledged);
        assert!(matches!(
            exec_rx.recv().await,
            Some(SessionInput::StdinClose)
        ));

        crate::run_registry::deregister(&run_id);
    }

    #[tokio::test]
    async fn handle_request_detaches_only_the_named_exec_session() {
        use crate::vm::session_client::SessionInput;

        let run_id = crate::run_registry::allocate_run_id();
        let (mut handle, _cancel_rx) = crate::run_registry::test_handle();
        let (primary_tx, _primary_rx) = tokio::sync::mpsc::channel::<SessionInput>(1);
        handle.input_tx = Some(primary_tx);
        crate::run_registry::register(run_id.clone(), handle);
        let (exec_tx, mut exec_rx) = tokio::sync::mpsc::channel::<SessionInput>(1);
        assert!(crate::run_registry::register_exec_session(
            &run_id,
            "exec-1".to_string(),
            exec_tx,
        ));
        let target = lns_ipc::SessionTarget::Exec {
            run_id: run_id.clone(),
            session_id: "exec-1".to_string(),
        };

        let response = handle_request(
            &Request::SessionDetach {
                target: target.clone(),
            },
            Instant::now(),
        )
        .await;

        assert_eq!(response, Response::DetachAccepted);
        assert!(exec_rx.recv().await.is_some());
        assert!(crate::run_registry::session_input_sender(&target).is_none());
        assert!(
            crate::run_registry::session_input_sender(&lns_ipc::SessionTarget::Primary {
                run_id: run_id.clone(),
            })
            .is_some()
        );

        crate::run_registry::deregister(&run_id);
    }

    #[tokio::test]
    async fn handle_request_rejects_an_unknown_exec_session_target() {
        let target = lns_ipc::SessionTarget::Exec {
            run_id: "missing-run".to_string(),
            session_id: "missing-exec".to_string(),
        };
        for request in [
            Request::SessionStdin {
                target: target.clone(),
                bytes: b"ignored".to_vec(),
            },
            Request::SessionDetach {
                target: target.clone(),
            },
        ] {
            let response = handle_request(&request, Instant::now()).await;
            assert!(matches!(
                response,
                Response::Error { message }
                    if message.contains("missing-exec") && message.contains("missing-run")
            ));
        }
    }

    #[tokio::test]
    async fn detaching_an_exec_with_a_closed_input_channel_cleans_up_and_reports_the_failure() {
        use crate::vm::session_client::SessionInput;

        let run_id = crate::run_registry::allocate_run_id();
        let (handle, _cancel_rx) = crate::run_registry::test_handle();
        crate::run_registry::register(run_id.clone(), handle);
        let (exec_tx, exec_rx) = tokio::sync::mpsc::channel::<SessionInput>(1);
        drop(exec_rx);
        assert!(crate::run_registry::register_exec_session(
            &run_id,
            "exec-1".to_string(),
            exec_tx,
        ));
        let target = lns_ipc::SessionTarget::Exec {
            run_id: run_id.clone(),
            session_id: "exec-1".to_string(),
        };

        let response = handle_request(
            &Request::SessionDetach {
                target: target.clone(),
            },
            Instant::now(),
        )
        .await;

        assert!(matches!(
            response,
            Response::Error { message } if message.contains("detaching exec session exec-1")
        ));
        assert!(crate::run_registry::session_input_sender(&target).is_none());

        crate::run_registry::deregister(&run_id);
    }

    #[tokio::test]
    async fn handle_request_run_resize_for_unregistered_run_returns_error() {
        let response = handle_request(
            &Request::SessionResize {
                target: primary_target("ffffffffffffffffffffffffffff9999".to_string()),
                rows: 24,
                cols: 80,
            },
            Instant::now(),
        )
        .await;
        assert!(matches!(response, Response::Error { .. }));
    }

    #[tokio::test]
    async fn handle_request_run_signal_for_unregistered_run_returns_error() {
        let response = handle_request(
            &Request::SessionSignal {
                target: primary_target("ffffffffffffffffffffffffffff9999".to_string()),
                signal: lns_ipc::SignalKind::Term,
            },
            Instant::now(),
        )
        .await;
        assert!(matches!(response, Response::Error { .. }));
    }

    #[tokio::test]
    async fn handle_request_kill_for_unregistered_run_returns_error() {
        let response = handle_request(
            &Request::Kill {
                run: "999999".into(),
                signal: lns_ipc::SignalKind::Kill,
            },
            Instant::now(),
        )
        .await;
        match response {
            Response::Error { message } => assert!(
                message.contains("no such run: 999999"),
                "an unknown run must report no-such-run, not a capability error; got: {message}"
            ),
            other => panic!("expected an unknown-run error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn handle_request_kill_for_a_registered_run_passes_existence_and_forwards() {
        let id = "aa557799";
        register_running(id);
        let response = handle_request(
            &Request::Kill {
                run: id.to_string(),
                signal: lns_ipc::SignalKind::Kill,
            },
            Instant::now(),
        )
        .await;
        crate::run_registry::deregister(id);
        match response {
            Response::Error { message } => assert!(
                !message.contains("no active run with id"),
                "a registered run clears the existence check and reaches forwarding; got: {message}"
            ),
            other => panic!(
                "expected a forwarding outcome for a sessionless registered run, got {other:?}"
            ),
        }
    }

    #[tokio::test]
    #[serial_test::serial(global_runs)]
    async fn handle_request_kill_of_an_already_exited_run_succeeds_without_forwarding() {
        let id = crate::run_registry::allocate_run_id();
        register_running(&id);
        crate::run_registry::set_exit_code(&id, 0);
        let response = handle_request(
            &Request::Kill {
                run: id.to_string(),
                signal: lns_ipc::SignalKind::Kill,
            },
            Instant::now(),
        )
        .await;
        crate::run_registry::deregister(&id);
        assert_eq!(
            response,
            Response::Acknowledged,
            "killing an exited run is already satisfied and must not forward to a dead session"
        );
    }

    #[tokio::test]
    async fn handle_request_addressing_an_unknown_name_reports_no_such_run() {
        for req in [
            Request::Kill {
                run: "ghost".into(),
                signal: lns_ipc::SignalKind::Term,
            },
            Request::InspectRun {
                run: "ghost".into(),
            },
            Request::RemoveRun {
                run: "ghost".into(),
                force: false,
            },
        ] {
            match handle_request(&req, Instant::now()).await {
                Response::Error { message } => {
                    assert!(message.contains("no such run: ghost"), "got: {message}");
                }
                other => unreachable!("expected Error for {req:?}, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    #[should_panic(expected = "has a streaming handler of its own")]
    async fn exec_image_via_handle_request_panics() {
        let req = Request::ExecImage(lns_ipc::ExecImageArgs {
            run: "1".into(),
            argv: vec![],
            env: vec![],
            tty: false,
            stdin: false,
            initial_winsize: None,
        });
        let _ = handle_request(&req, Instant::now()).await;
    }

    #[test]
    fn map_signal_covers_every_variant() {
        for (input, expected) in [
            (lns_ipc::SignalKind::Int, lns_session::SignalKind::Int),
            (lns_ipc::SignalKind::Term, lns_session::SignalKind::Term),
            (lns_ipc::SignalKind::Quit, lns_session::SignalKind::Quit),
            (lns_ipc::SignalKind::Hup, lns_session::SignalKind::Hup),
            (lns_ipc::SignalKind::Winch, lns_session::SignalKind::Winch),
            (lns_ipc::SignalKind::Kill, lns_session::SignalKind::Kill),
        ] {
            assert_eq!(map_signal(input), expected);
        }
    }

    #[tokio::test]
    async fn forward_session_input_returns_error_when_input_is_none() {
        let response = forward_session_input("aa01", None, "Synthetic").await;
        match response {
            Response::Error { message } => {
                assert!(message.contains("Synthetic"));
                assert!(message.contains("not supported"));
            }
            _ => unreachable!("forward_session_input must surface Error when input is None"),
        }
    }

    #[tokio::test]
    async fn handle_request_cancel_run_with_registered_run_returns_accepted() {
        use std::sync::Mutex;
        use tokio::sync::oneshot;
        let run_id = "deadbeef00000000000000000000bb07".to_string();
        let (cancel_tx, _cancel_rx) = oneshot::channel();
        let task = tokio::spawn(async {});
        let handle = crate::run_registry::RunHandle {
            cancel_tx,
            detach_tx: std::sync::Mutex::new(None),
            task,
            input_tx: None,
            exec_sessions: Default::default(),
            connector: None,
            name: String::new(),
            image: "test-image".into(),
            command: "".into(),
            started: "1970-01-01T00:00:00Z".into(),
            status: Mutex::new(lns_ipc::RunStatus::Running),
            logs: std::sync::Arc::new(crate::run_log::RunLogBuffer::default()),
            config: lns_ipc::RunConfig::default(),
            exec_environment: Default::default(),
        };
        crate::run_registry::register(run_id.clone(), handle);
        let resp = handle_request(
            &Request::CancelRun {
                run_id: run_id.clone(),
            },
            Instant::now(),
        )
        .await;
        assert!(matches!(resp, Response::CancelAccepted));
        let _ = crate::run_registry::cancel(&run_id);
    }

    #[tokio::test]
    async fn handle_request_detach_registered_run_accepts_and_fires_the_signal() {
        use std::sync::Mutex;
        use tokio::sync::oneshot;
        let run_id = "e7e7e7e7e7e7e7e7e7e7e7e7e7e7e7e7".to_string();
        let (cancel_tx, _cancel_rx) = oneshot::channel();
        let (detach_tx, detach_rx) = oneshot::channel::<()>();
        let task = tokio::spawn(async {});
        let handle = crate::run_registry::RunHandle {
            cancel_tx,
            detach_tx: Mutex::new(Some(detach_tx)),
            task,
            input_tx: None,
            exec_sessions: Default::default(),
            connector: None,
            name: String::new(),
            image: "detach-test".into(),
            command: "".into(),
            started: "1970-01-01T00:00:00Z".into(),
            status: Mutex::new(lns_ipc::RunStatus::Running),
            logs: std::sync::Arc::new(crate::run_log::RunLogBuffer::default()),
            config: lns_ipc::RunConfig::default(),
            exec_environment: Default::default(),
        };
        crate::run_registry::register(run_id.clone(), handle);

        let resp = handle_request(
            &Request::SessionDetach {
                target: primary_target(run_id.clone()),
            },
            Instant::now(),
        )
        .await;
        assert!(matches!(resp, Response::DetachAccepted));
        assert!(
            detach_rx.await.is_ok(),
            "the run's pump must be told to hand off, not cancel",
        );

        let resp = handle_request(
            &Request::SessionDetach {
                target: primary_target(run_id.clone()),
            },
            Instant::now(),
        )
        .await;
        match resp {
            Response::Error { message } => assert!(
                message.contains("is not attached"),
                "a second detach of a still-registered run is not-attached, not absent: {message}",
            ),
            other => unreachable!("expected Error on a second detach, got {other:?}"),
        }
        let _ = crate::run_registry::cancel(&run_id);
    }

    #[tokio::test]
    async fn forward_session_input_errors_when_input_channel_is_closed() {
        use std::sync::Mutex;
        use tokio::sync::{mpsc, oneshot};
        let run_id = "deadbeef00000000000000000000bb11".to_string();
        let (input_tx, input_rx) = mpsc::channel(1);
        drop(input_rx);
        let (cancel_tx, _cancel_rx) = oneshot::channel();
        let task = tokio::spawn(async {});
        let handle = crate::run_registry::RunHandle {
            cancel_tx,
            detach_tx: std::sync::Mutex::new(None),
            task,
            input_tx: Some(input_tx),
            exec_sessions: Default::default(),
            connector: None,
            name: String::new(),
            image: "closed-channel-test".into(),
            command: "".into(),
            started: "1970-01-01T00:00:00Z".into(),
            status: Mutex::new(lns_ipc::RunStatus::Running),
            logs: std::sync::Arc::new(crate::run_log::RunLogBuffer::default()),
            config: lns_ipc::RunConfig::default(),
            exec_environment: Default::default(),
        };
        crate::run_registry::register(run_id.clone(), handle);

        let resp = handle_request(
            &Request::SessionStdin {
                target: primary_target(run_id.clone()),
                bytes: b"hi".to_vec(),
            },
            Instant::now(),
        )
        .await;
        match resp {
            Response::Error { message } => {
                assert!(message.contains("forwarding"));
                assert!(message.contains("SessionStdin"));
                assert!(message.contains(&run_id));
            }
            other => unreachable!("expected Error, got {other:?}"),
        }
        let _ = crate::run_registry::cancel(&run_id);
    }

    #[tokio::test]
    async fn pump_cancel_arm_reports_write_failed_when_stream_is_closed() {
        struct FailingWriter;
        impl tokio::io::AsyncWrite for FailingWriter {
            fn poll_write(
                self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
                _: &[u8],
            ) -> std::task::Poll<std::io::Result<usize>> {
                std::task::Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "test-induced disconnect",
                )))
            }
            fn poll_flush(
                self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                std::task::Poll::Ready(Ok(()))
            }
            fn poll_shutdown(
                self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                std::task::Poll::Ready(Ok(()))
            }
        }

        {
            use tokio::io::AsyncWriteExt;
            let mut probe = FailingWriter;
            let _ = probe.flush().await;
            let _ = probe.shutdown().await;
        }
        let mut sink = FailingWriter;
        let (_tx, mut rx) = mpsc::channel::<WireFrame>(1);
        let (cancel_tx, cancel_rx) = oneshot::channel();
        let (_detach_tx, detach_rx) = never_detach();
        cancel_tx.send(130).expect("cancel send");
        let outcome = pump_responses(&mut sink, &mut rx, cancel_rx, detach_rx)
            .await
            .expect("pump returned Result");
        assert!(
            matches!(outcome, PumpOutcome::WriteFailed(_)),
            "broken-pipe stream during cancel must surface as WriteFailed"
        );
    }

    fn register_running(run_id: &str) {
        let (cancel_tx, _cancel_rx) = oneshot::channel();
        let task = tokio::spawn(async {});
        crate::run_registry::register(
            run_id.to_string(),
            crate::run_registry::RunHandle {
                cancel_tx,
                detach_tx: std::sync::Mutex::new(None),
                task,
                input_tx: None,
                exec_sessions: Default::default(),
                connector: None,
                name: "reviewer".into(),
                image: "stop-test".into(),
                command: String::new(),
                started: "1970-01-01T00:00:00Z".into(),
                status: std::sync::Mutex::new(lns_ipc::RunStatus::Running),
                logs: std::sync::Arc::new(crate::run_log::RunLogBuffer::default()),
                config: lns_ipc::RunConfig::default(),
                exec_environment: Default::default(),
            },
        );
    }

    fn recording_sender(
        run_id: &str,
        exit_on: Option<lns_ipc::SignalKind>,
    ) -> (
        std::sync::Arc<std::sync::Mutex<Vec<lns_ipc::SignalKind>>>,
        impl Fn(lns_ipc::SignalKind) -> std::future::Ready<Response>,
    ) {
        let run_id = run_id.to_string();
        let sent = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sent_for_closure = sent.clone();
        let sender = move |signal: lns_ipc::SignalKind| {
            sent_for_closure.lock().unwrap().push(signal);
            if exit_on == Some(signal) {
                crate::run_registry::set_exit_code(&run_id, 137);
            }
            std::future::ready(Response::Acknowledged)
        };
        (sent, sender)
    }

    #[tokio::test]
    #[should_panic(expected = "has a streaming handler of its own")]
    async fn run_logs_via_handle_request_panics() {
        let _ = handle_request(
            &Request::RunLogs {
                run: "1".into(),
                follow: false,
            },
            Instant::now(),
        )
        .await;
    }

    #[tokio::test]
    #[should_panic(expected = "has a streaming handler of its own")]
    async fn start_run_via_handle_request_panics() {
        let _ = handle_request(
            &Request::StartRun {
                run: "1".into(),
                attach: false,
                stdin: false,
            },
            Instant::now(),
        )
        .await;
    }

    struct ScriptedStartHost {
        record: Option<crate::run_record::RunRecord>,
        served: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }

    impl StartHost for ScriptedStartHost {
        async fn record(&self, run_id: &str) -> anyhow::Result<crate::run_record::RunRecord> {
            self.record
                .clone()
                .ok_or_else(|| anyhow::anyhow!("run {run_id}'s state is damaged"))
        }

        async fn preflight(&self, _record: &crate::run_record::RunRecord) -> anyhow::Result<()> {
            Ok(())
        }

        async fn serve<S>(
            &self,
            _stream: &mut S,
            _record: crate::run_record::RunRecord,
            _options: StartOptions,
        ) -> anyhow::Result<()>
        where
            S: AsyncReadExt + AsyncWriteExt + Unpin + Send,
        {
            self.served.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
    }

    async fn drive_start_stopped(host: &ScriptedStartHost, handle: &str) -> Vec<Response> {
        let (mut client, mut server) = tokio::io::duplex(64 * 1024);
        start_stopped_run(&mut server, handle, host, StartOptions::default())
            .await
            .expect("a refusal is an answer, not a transport failure");
        drop(server);
        let mut frames = Vec::new();
        while let Ok(bytes) = lns_ipc::read_frame_bytes_async(&mut client).await {
            if let Ok(response) = lns_ipc::decode_frame::<Response, _>(&mut bytes.as_slice()) {
                frames.push(response);
            }
        }
        frames
    }

    #[tokio::test]
    #[serial_test::serial(global_runs)]
    async fn starting_a_stopped_run_whose_record_is_gone_answers_the_damage() {
        let record = crate::run_record::test_record(&crate::run_registry::allocate_run_id());
        let id = record.run_id.clone();
        crate::run_registry::register_stopped(crate::run_registry::StoppedRun { record });
        let served = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let host = ScriptedStartHost {
            record: None,
            served: served.clone(),
        };
        let frames = drive_start_stopped(&host, &id).await;
        crate::run_registry::deregister(&id);
        assert!(
            matches!(&frames[0], Response::Error { message } if message.contains("state is damaged")),
            "got {frames:?}"
        );
        assert!(!served.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[tokio::test]
    #[serial_test::serial(global_runs)]
    async fn starting_a_stopped_run_nothing_refuses_reaches_the_boot() {
        let record = crate::run_record::test_record(&crate::run_registry::allocate_run_id());
        let id = record.run_id.clone();
        crate::run_registry::register_stopped(crate::run_registry::StoppedRun {
            record: record.clone(),
        });
        let served = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let host = ScriptedStartHost {
            record: Some(record),
            served: served.clone(),
        };
        let frames = drive_start_stopped(&host, &id).await;
        crate::run_registry::deregister(&id);
        assert!(
            served.load(std::sync::atomic::Ordering::SeqCst),
            "a stopped run nothing refuses has to reach the boot; got {frames:?}"
        );
    }

    #[tokio::test]
    #[should_panic(expected = "has a streaming handler of its own")]
    async fn attach_run_via_handle_request_panics() {
        let _ = handle_request(&Request::AttachRun { run: "1".into() }, Instant::now()).await;
    }

    #[tokio::test]
    #[should_panic(expected = "has a streaming handler of its own")]
    async fn run_stats_via_handle_request_panics() {
        let _ = handle_request(&Request::RunStats { run: "1".into() }, Instant::now()).await;
    }

    #[tokio::test]
    async fn handle_request_inspect_run_for_unknown_run_returns_error() {
        let resp = handle_request(
            &Request::InspectRun {
                run: "999998".into(),
            },
            Instant::now(),
        )
        .await;
        match resp {
            Response::Error { message } => {
                assert!(message.contains("no such run"), "got: {message}");
            }
            other => unreachable!("expected Error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn handle_request_inspect_run_returns_details_for_a_registered_run() {
        let id = crate::run_registry::allocate_run_id();
        register_running(&id);

        let resp = handle_request(
            &Request::InspectRun {
                run: id.to_string(),
            },
            Instant::now(),
        )
        .await;

        match resp {
            Response::RunInspect { details } => {
                assert_eq!(details.summary.id, id);
                assert_eq!(details.summary.image, "stop-test");
                assert_eq!(details.summary.status, lns_ipc::RunStatus::Running);
            }
            other => unreachable!("expected RunInspect, got {other:?}"),
        }
        crate::run_registry::deregister(&id);
    }

    #[tokio::test]
    #[serial_test::serial(global_runs)]
    async fn handle_request_prune_runs_removes_exited_runs_and_reports_their_ids() {
        let id = crate::run_registry::allocate_run_id();
        register_running(&id);
        crate::run_registry::set_exit_code(&id, 0);

        let resp = handle_request(&Request::PruneRuns, Instant::now()).await;

        match resp {
            Response::RunsPruned { removed } => {
                assert!(
                    removed.contains(&id),
                    "{id} should be pruned, got {removed:?}"
                );
            }
            other => unreachable!("expected RunsPruned, got {other:?}"),
        }
        assert_eq!(crate::run_registry::status(&id), None);
    }

    #[tokio::test]
    async fn handle_request_stop_run_for_unknown_run_returns_error() {
        let resp = handle_request(
            &Request::StopRun {
                run: "999999".into(),
                timeout_secs: 1,
            },
            Instant::now(),
        )
        .await;
        match resp {
            Response::Error { message } => {
                assert!(message.contains("no such run"), "got: {message}");
            }
            other => unreachable!("expected Error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn stop_run_with_reports_no_active_run_when_the_id_vanished_after_resolution() {
        let (_sent, sender) = recording_sender("ffffffffffffffffffffffffffffffff", None);
        let resp = stop_run_with(
            "ffffffffffffffffffffffffffffffff",
            std::time::Duration::from_secs(1),
            KILL_GRACE,
            sender,
        )
        .await;
        assert!(
            matches!(&resp, Response::Error { message } if message.contains("no active run with id")),
            "got {resp:?}"
        );
    }

    struct NoopRemover;
    impl crate::run::RemoveDir for NoopRemover {
        fn remove_dir_all(&self, _dir: &std::path::Path) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn kill_acknowledged(id: String, _sig: lns_ipc::SignalKind) -> std::future::Ready<Response> {
        crate::run_registry::set_exit_code(&id, 137);
        std::future::ready(Response::Acknowledged)
    }

    fn signal_refused(_id: String, _sig: lns_ipc::SignalKind) -> std::future::Ready<Response> {
        std::future::ready(Response::Error {
            message: "the session is gone".into(),
        })
    }

    fn note_nothing(_id: &str, _forced: bool) {}

    #[tokio::test]
    async fn remove_run_with_reports_no_such_run_for_an_unknown_handle() {
        let resp = remove_run_with(
            "ffffffffffffffffffffffffffffffff",
            false,
            &NoopRemover,
            std::path::Path::new("/cache"),
            kill_acknowledged,
            note_nothing,
        )
        .await;
        assert!(
            matches!(&resp, Response::Error { message } if message.contains("no such run")),
            "got {resp:?}"
        );
    }

    #[tokio::test]
    #[serial_test::serial(global_runs)]
    async fn removing_an_exited_run_reclaims_and_notes_it() {
        let id = crate::run_registry::allocate_run_id();
        let (handle, _rx) = crate::run_registry::test_handle();
        crate::run_registry::register(id.clone(), handle);
        crate::run_registry::set_exit_code(&id, 0);
        let resp = remove_run_with(
            &id,
            false,
            &NoopRemover,
            std::path::Path::new("/cache"),
            kill_acknowledged,
            note_nothing,
        )
        .await;
        assert!(matches!(resp, Response::Acknowledged), "got {resp:?}");
        assert!(crate::run_registry::status(&id).is_none());
    }

    #[tokio::test]
    #[serial_test::serial(global_runs)]
    async fn a_forced_removal_kills_then_removes_in_one_step() {
        let id = crate::run_registry::allocate_run_id();
        let (handle, _rx) = crate::run_registry::test_handle();
        crate::run_registry::register(id.clone(), handle);
        let resp = remove_run_with(
            &id,
            true,
            &NoopRemover,
            std::path::Path::new("/cache"),
            kill_acknowledged,
            note_nothing,
        )
        .await;
        assert!(matches!(resp, Response::Acknowledged), "got {resp:?}");
        assert!(crate::run_registry::status(&id).is_none());
    }

    #[tokio::test]
    #[serial_test::serial(global_runs)]
    async fn a_forced_removal_whose_kill_fails_changes_nothing() {
        let id = crate::run_registry::allocate_run_id();
        let (handle, _rx) = crate::run_registry::test_handle();
        crate::run_registry::register(id.clone(), handle);
        let resp = remove_run_with(
            &id,
            true,
            &NoopRemover,
            std::path::Path::new("/cache"),
            signal_refused,
            note_nothing,
        )
        .await;
        assert!(
            matches!(&resp, Response::Error { message } if message.contains("the session is gone")),
            "got {resp:?}"
        );
        assert!(crate::run_registry::status(&id).is_some());
        crate::run_registry::deregister(&id);
    }

    #[tokio::test]
    async fn removing_a_run_that_vanished_after_resolution_reports_it() {
        let resp = remove_resolved_run_with(
            "ffffffffffffffffffffffffffffffff",
            "ghost",
            false,
            &NoopRemover,
            std::path::Path::new("/cache"),
            kill_acknowledged,
            note_nothing,
        )
        .await;
        assert!(
            matches!(&resp, Response::Error { message } if message.contains("no run with id")),
            "got {resp:?}"
        );
    }

    struct ScriptedRunsDir(Vec<std::path::PathBuf>);
    impl crate::image_store::Fs for ScriptedRunsDir {
        async fn read_dir(
            &self,
            _dir: &std::path::Path,
        ) -> std::io::Result<Vec<std::path::PathBuf>> {
            Ok(self.0.clone())
        }
        async fn read(&self, _p: &std::path::Path) -> std::io::Result<Vec<u8>> {
            Err(std::io::Error::from(std::io::ErrorKind::NotFound))
        }
        async fn write(&self, _p: &std::path::Path, _b: &[u8]) -> std::io::Result<()> {
            Ok(())
        }
        async fn remove_file(&self, _p: &std::path::Path) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn prune_skips_a_runs_dir_entry_with_no_readable_name() {
        use crate::image_store::Fs as _;
        let probe = ScriptedRunsDir(Vec::new());
        assert!(probe.read(std::path::Path::new("/x")).await.is_err());
        assert!(probe.write(std::path::Path::new("/x"), b"").await.is_ok());
        assert!(probe.remove_file(std::path::Path::new("/x")).await.is_ok());
        let fs = ScriptedRunsDir(vec![std::path::PathBuf::from("/")]);
        let resp = prune_runs_with(&fs, &NoopRemover, std::path::Path::new("/cache"), |_| {}).await;
        assert!(
            matches!(&resp, Response::RunsPruned { removed } if removed.is_empty()),
            "an unreadable entry is skipped, never swept blind: {resp:?}"
        );
    }

    #[tokio::test]
    #[serial_test::serial(global_runs, env)]
    async fn the_request_wrappers_audit_what_they_remove() {
        let d = tempfile::tempdir().unwrap();
        let _h = crate::test_env::EnvVarGuard::set("HOME", d.path());
        let _c = crate::test_env::EnvVarGuard::set("XDG_CACHE_HOME", d.path().join("cache"));
        let _x = crate::test_env::EnvVarGuard::set("XDG_DATA_HOME", d.path().join("data"));
        let id = crate::run_registry::allocate_run_id();
        let (handle, _rx) = crate::run_registry::test_handle();
        crate::run_registry::register(id.clone(), handle);
        crate::run_registry::set_exit_code(&id, 0);
        let resp = image_response(remove_run_request(&id, false).await);
        assert!(matches!(resp, Response::Acknowledged), "got {resp:?}");
        let audit = crate::audit::audit_path(&id).unwrap();
        let content = std::fs::read_to_string(&audit).unwrap();
        assert!(content.contains("run_removed"), "{content}");

        let id2 = crate::run_registry::allocate_run_id();
        let (handle2, _rx2) = crate::run_registry::test_handle();
        crate::run_registry::register(id2.clone(), handle2);
        crate::run_registry::set_exit_code(&id2, 0);
        let resp = image_response(prune_runs_request().await);
        assert!(
            matches!(&resp, Response::RunsPruned { removed } if removed.contains(&id2)),
            "got {resp:?}"
        );
        let content = std::fs::read_to_string(crate::audit::audit_path(&id2).unwrap()).unwrap();
        assert!(content.contains("runs_pruned"), "{content}");
    }

    #[tokio::test]
    #[serial_test::serial(global_runs, env)]
    async fn a_forced_request_reaches_the_live_session_channel() {
        let d = tempfile::tempdir().unwrap();
        let _h = crate::test_env::EnvVarGuard::set("HOME", d.path());
        let _c = crate::test_env::EnvVarGuard::set("XDG_CACHE_HOME", d.path().join("cache"));
        let id = crate::run_registry::allocate_run_id();
        let (handle, _rx) = crate::run_registry::test_handle();
        crate::run_registry::register(id.clone(), handle);
        let resp = image_response(remove_run_request(&id, true).await);
        assert!(
            matches!(&resp, Response::Error { message } if message.contains("no session")
                || message.contains("input") || !message.is_empty()),
            "a forced removal of a run with no session channel surfaces the failure: {resp:?}"
        );
        assert!(crate::run_registry::status(&id).is_some());
        crate::run_registry::deregister(&id);
    }

    #[tokio::test]
    #[serial_test::serial(global_runs, env)]
    async fn a_homeless_service_answers_removal_and_prune_instead_of_panicking() {
        let _h = crate::test_env::EnvVarGuard::unset("HOME");
        let _c = crate::test_env::EnvVarGuard::unset("XDG_CACHE_HOME");
        // dirs resolves a home from passwd on macOS, so only Linux reaches the rootless arm — both answers are answers.
        let resp = image_response(remove_run_request("nosuch", false).await);
        assert!(matches!(resp, Response::Error { .. }), "got {resp:?}");
        let resp = image_response(prune_runs_request().await);
        assert!(
            matches!(resp, Response::Error { .. } | Response::RunsPruned { .. }),
            "got {resp:?}"
        );
    }

    #[tokio::test]
    #[serial_test::serial(global_runs, env)]
    async fn an_unwritable_audit_trail_warns_but_never_blocks_removal() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let _h = crate::test_env::EnvVarGuard::set("HOME", file.path());
        let _c = crate::test_env::EnvVarGuard::set("XDG_CACHE_HOME", file.path().join("cache"));
        let _x = crate::test_env::EnvVarGuard::set("XDG_DATA_HOME", file.path().join("data"));
        let id = crate::run_registry::allocate_run_id();
        let (handle, _rx) = crate::run_registry::test_handle();
        crate::run_registry::register(id.clone(), handle);
        crate::run_registry::set_exit_code(&id, 0);
        let resp = image_response(remove_run_request(&id, false).await);
        assert!(matches!(resp, Response::Acknowledged), "got {resp:?}");

        let id2 = crate::run_registry::allocate_run_id();
        let (handle2, _rx2) = crate::run_registry::test_handle();
        crate::run_registry::register(id2.clone(), handle2);
        crate::run_registry::set_exit_code(&id2, 0);
        let resp = image_response(prune_runs_request().await);
        assert!(matches!(resp, Response::RunsPruned { .. }), "got {resp:?}");
    }

    #[tokio::test]
    async fn kill_resolved_reports_no_active_run_when_the_id_vanished_after_resolution() {
        let resp = kill_resolved(
            "ffffffffffffffffffffffffffffffff",
            lns_ipc::SignalKind::Term,
        )
        .await;
        assert!(
            matches!(&resp, Response::Error { message } if message.contains("no active run with id")),
            "got {resp:?}"
        );
    }

    #[test]
    fn inspect_resolved_reports_no_active_run_when_the_id_vanished_after_resolution() {
        let resp = inspect_resolved("ffffffffffffffffffffffffffffffff");
        assert!(
            matches!(&resp, Response::Error { message } if message.contains("no active run with id")),
            "got {resp:?}"
        );
    }

    #[tokio::test]
    #[serial_test::serial(global_runs)]
    async fn stop_of_an_already_exited_run_succeeds_without_signalling() {
        let id = crate::run_registry::allocate_run_id();
        register_running(&id);
        crate::run_registry::set_exit_code(&id, 0);
        let (sent, sender) = recording_sender(&id, None);

        let resp = stop_run_with(
            &id,
            std::time::Duration::from_secs(1),
            std::time::Duration::from_secs(1),
            sender,
        )
        .await;

        assert_eq!(resp, Response::RunStopped { forced: false });
        assert!(
            sent.lock().unwrap().is_empty(),
            "an exited run must not be signalled again"
        );
        crate::run_registry::deregister(&id);
    }

    #[tokio::test(start_paused = true)]
    async fn stop_returns_unforced_when_the_workload_exits_on_term() {
        let id = crate::run_registry::allocate_run_id();
        register_running(&id);
        let (sent, sender) = recording_sender(&id, Some(lns_ipc::SignalKind::Term));

        let resp = stop_run_with(&id, std::time::Duration::from_secs(10), KILL_GRACE, sender).await;

        assert_eq!(resp, Response::RunStopped { forced: false });
        assert_eq!(sent.lock().unwrap().as_slice(), [lns_ipc::SignalKind::Term]);
        crate::run_registry::deregister(&id);
    }

    #[tokio::test(start_paused = true)]
    async fn stop_escalates_to_kill_when_term_is_ignored() {
        let id = crate::run_registry::allocate_run_id();
        register_running(&id);
        let (sent, sender) = recording_sender(&id, Some(lns_ipc::SignalKind::Kill));

        let resp = stop_run_with(&id, std::time::Duration::from_secs(10), KILL_GRACE, sender).await;

        assert_eq!(resp, Response::RunStopped { forced: true });
        assert_eq!(
            sent.lock().unwrap().as_slice(),
            [lns_ipc::SignalKind::Term, lns_ipc::SignalKind::Kill]
        );
        crate::run_registry::deregister(&id);
    }

    #[tokio::test(start_paused = true)]
    async fn stop_with_zero_timeout_skips_straight_to_kill() {
        let id = crate::run_registry::allocate_run_id();
        register_running(&id);
        let (sent, sender) = recording_sender(&id, Some(lns_ipc::SignalKind::Kill));

        let resp = stop_run_with(&id, std::time::Duration::from_secs(0), KILL_GRACE, sender).await;

        assert_eq!(resp, Response::RunStopped { forced: true });
        assert_eq!(sent.lock().unwrap().len(), 2);
        crate::run_registry::deregister(&id);
    }

    #[tokio::test(start_paused = true)]
    async fn stop_reports_failure_when_kill_does_not_end_the_run() {
        let id = crate::run_registry::allocate_run_id();
        register_running(&id);
        let (sent, sender) = recording_sender(&id, None);

        let resp = stop_run_with(
            &id,
            std::time::Duration::from_secs(1),
            std::time::Duration::from_secs(1),
            sender,
        )
        .await;

        let rendered = format!("{resp:?}");
        assert!(rendered.contains("Error"), "got: {rendered}");
        assert!(rendered.contains("SIGKILL"), "got: {rendered}");
        assert_eq!(sent.lock().unwrap().len(), 2);
        crate::run_registry::cancel(&id);
    }

    #[tokio::test]
    async fn stop_propagates_a_term_send_error() {
        let id = crate::run_registry::allocate_run_id();
        register_running(&id);

        let resp = stop_run_with(
            &id,
            std::time::Duration::from_secs(1),
            std::time::Duration::from_secs(1),
            |_| {
                std::future::ready(Response::Error {
                    message: "term send blew up".into(),
                })
            },
        )
        .await;

        assert_eq!(
            resp,
            Response::Error {
                message: "term send blew up".into()
            }
        );
        crate::run_registry::cancel(&id);
    }

    #[tokio::test(start_paused = true)]
    async fn stop_propagates_a_kill_send_error() {
        let id = crate::run_registry::allocate_run_id();
        register_running(&id);
        let calls = std::sync::Arc::new(std::sync::Mutex::new(0u32));
        let calls_for_closure = calls.clone();

        let resp = stop_run_with(
            &id,
            std::time::Duration::from_secs(0),
            std::time::Duration::from_secs(1),
            move |_| {
                let mut n = calls_for_closure.lock().unwrap();
                *n += 1;
                if *n == 1 {
                    std::future::ready(Response::Acknowledged)
                } else {
                    std::future::ready(Response::Error {
                        message: "kill send blew up".into(),
                    })
                }
            },
        )
        .await;

        assert_eq!(
            resp,
            Response::Error {
                message: "kill send blew up".into()
            }
        );
        crate::run_registry::cancel(&id);
    }

    #[tokio::test(start_paused = true)]
    async fn handle_request_stop_run_sends_term_through_the_session_channel() {
        use crate::vm::session_client::SessionInput;
        use tokio::sync::mpsc;

        let id = crate::run_registry::allocate_run_id();
        let (cancel_tx, _cancel_rx) = oneshot::channel();
        let task = tokio::spawn(std::future::pending::<()>());
        let (input_tx, mut input_rx) = mpsc::channel::<SessionInput>(4);
        crate::run_registry::register(
            id.clone(),
            crate::run_registry::RunHandle {
                cancel_tx,
                detach_tx: std::sync::Mutex::new(None),
                task,
                input_tx: Some(input_tx),
                exec_sessions: Default::default(),
                connector: None,
                name: String::new(),
                image: String::new(),
                command: String::new(),
                started: String::new(),
                status: std::sync::Mutex::new(lns_ipc::RunStatus::Running),
                logs: std::sync::Arc::new(crate::run_log::RunLogBuffer::default()),
                config: lns_ipc::RunConfig::default(),
                exec_environment: Default::default(),
            },
        );

        let consumer_id = id.clone();
        let consumer = tokio::spawn(async move {
            let input = input_rx.recv().await.expect("Term must arrive");
            assert!(
                matches!(input, SessionInput::Signal(lns_session::SignalKind::Term)),
                "expected Term, got {input:?}"
            );
            crate::run_registry::set_exit_code(&consumer_id, 143);
        });

        let resp = handle_request(
            &Request::StopRun {
                run: id.to_string(),
                timeout_secs: 10,
            },
            Instant::now(),
        )
        .await;

        assert_eq!(resp, Response::RunStopped { forced: false });
        consumer.await.expect("consumer ran");
        crate::run_registry::cancel(&id);
    }

    fn exec_args(argv: Vec<String>, tty: bool, stdin: bool) -> lns_ipc::ExecImageArgs {
        lns_ipc::ExecImageArgs {
            run: "42".into(),
            argv,
            env: Vec::new(),
            tty,
            stdin,
            initial_winsize: None,
        }
    }

    #[test]
    fn validate_exec_rejects_empty_argv() {
        let result = validate_exec(&exec_args(Vec::new(), false, false));
        assert_eq!(result, Err("ExecImage.argv is empty".to_string()));
    }

    #[test]
    fn validate_exec_accepts_explicit_terminal_modes() {
        for (tty, stdin) in [(true, false), (false, true), (true, true)] {
            assert_eq!(
                validate_exec(&exec_args(vec!["sh".into()], tty, stdin)),
                Ok(())
            );
        }
    }

    #[tokio::test]
    async fn register_exec_input_publishes_an_addressable_session_id() {
        let run_id = crate::run_registry::allocate_run_id();
        let (handle, _cancel_rx) = crate::run_registry::test_handle();
        crate::run_registry::register(run_id.clone(), handle);
        let (input_tx, _input_rx) =
            tokio::sync::mpsc::channel::<crate::vm::session_client::SessionInput>(1);

        let session_id = register_exec_input(&run_id, input_tx)
            .expect("an active run should accept a new exec session");
        assert!(
            crate::run_registry::session_input_sender(&lns_ipc::SessionTarget::Exec {
                run_id: run_id.clone(),
                session_id,
            })
            .is_some()
        );

        crate::run_registry::deregister(&run_id);
    }

    #[tokio::test]
    async fn register_exec_input_refuses_a_run_that_disappeared() {
        let (input_tx, _input_rx) =
            tokio::sync::mpsc::channel::<crate::vm::session_client::SessionInput>(1);
        assert_eq!(
            register_exec_input("missing-run", input_tx),
            Err("no active run with id missing-run".to_string())
        );
    }

    #[test]
    fn validate_exec_accepts_a_non_interactive_command() {
        let result = validate_exec(&exec_args(vec!["echo".into(), "hi".into()], false, false));
        assert_eq!(result, Ok(()));
    }

    #[test]
    fn build_session_params_maps_fields_and_winsize() {
        let args = lns_ipc::ExecImageArgs {
            run: "1".into(),
            argv: vec!["echo".into()],
            env: vec!["A=B".into()],
            tty: false,
            stdin: false,
            initial_winsize: Some((24, 80)),
        };
        let params = build_session_params(args, "1");
        assert_eq!(params.argv, vec!["echo".to_string()]);
        assert!(params.env.contains(&"A=B".to_string()));
        assert_eq!(
            params.cwd, None,
            "a run with no stored environment forces no working directory"
        );
        let ws = params.initial_winsize.expect("winsize should be mapped");
        assert_eq!((ws.rows, ws.cols), (24, 80));
    }

    #[test]
    fn build_session_params_asks_the_broker_to_confine_the_exec() {
        let params = build_session_params(exec_args(vec!["nft".into()], false, false), "1");
        assert!(
            params.confine,
            "an exec reaches the broker, not the supervisor, so the broker is the only place its identity and capabilities can be capped"
        );
    }

    #[test]
    fn build_session_params_declares_the_exec_dies_with_its_client() {
        let params = build_session_params(exec_args(vec!["nft".into()], false, false), "1");
        assert!(
            params.dies_with_client,
            "an exec has no other owner, so a vanished host stream must hang its child up instead of leaking it"
        );
    }

    #[test]
    fn build_session_params_leaves_winsize_unset_when_absent() {
        let params = build_session_params(exec_args(vec!["echo".into()], false, false), "1");
        assert!(params.initial_winsize.is_none());
    }

    #[tokio::test]
    #[serial_test::serial(global_runs)]
    async fn exec_and_the_workload_compose_the_same_path_for_the_same_tools() {
        // Two copies of the PATH rule drift into "run finds node, exec does not"; this fails the moment they disagree.
        let tools = crate::workload_env::ToolRuntime {
            bin_paths: vec!["/.lens/tools/node/22.11.0/bin".to_string()],
            env: vec![(
                "SOME_TOOL_HOME".to_string(),
                "/.lens/tools/node/22.11.0/home".to_string(),
            )],
        };
        let workload = crate::workload_env::run_workload_env(
            Some(&["PATH=/usr/bin".into()]),
            &[],
            None,
            None,
            &[],
            &tools,
        );
        let workload_path = workload
            .env
            .iter()
            .find(|kv| kv.starts_with("PATH="))
            .expect("the workload gets a PATH");
        let workload_tool_var = workload
            .env
            .iter()
            .find(|kv| kv.starts_with("SOME_TOOL_HOME="))
            .expect("the workload gets the tool's own var");

        let run_id = crate::run_registry::allocate_run_id();
        let (mut handle, _cancel) = crate::run_registry::test_handle();
        handle.exec_environment.tools = tools.clone();
        crate::run_registry::register_named(run_id.clone(), None, handle).expect("register");
        let mut args = exec_args(vec!["node".into()], false, false);
        args.env = vec!["PATH=/usr/bin".into()];
        let params = build_session_params(args, &run_id);
        crate::run_registry::deregister(&run_id);

        assert_eq!(
            params.env.iter().find(|kv| kv.starts_with("PATH=")),
            Some(workload_path),
            "exec must reach the PATH the workload got"
        );
        assert_eq!(
            params
                .env
                .iter()
                .find(|kv| kv.starts_with("SOME_TOOL_HOME=")),
            Some(workload_tool_var),
            "and the vars the tools need to find their own payload"
        );
    }

    #[tokio::test]
    #[serial_test::serial(global_runs)]
    async fn an_exec_session_joins_the_runs_own_environment() {
        let run_id = crate::run_registry::allocate_run_id();
        let (mut handle, _cancel) = crate::run_registry::test_handle();
        handle.exec_environment = crate::run_registry::ExecEnvironment {
            session_env: vec![
                "HOME=/workspace".into(),
                "SHELL=/bin/bash".into(),
                "SOME_TOOL_HOME=/workspace/mine".into(),
            ],
            declared_identity_keys: vec!["HOME".into()],
            workdir: Some("/workspace".into()),
            tools: crate::workload_env::ToolRuntime {
                bin_paths: vec!["/.lens/tools/some-tool/1.2.3/bin".into()],
                env: vec![(
                    "SOME_TOOL_HOME".into(),
                    "/.lens/tools/some-tool/1.2.3/home".into(),
                )],
            },
            ..Default::default()
        };
        crate::run_registry::register_named(run_id.clone(), None, handle).expect("register");

        let args = exec_args(vec!["printenv".into()], false, false);
        assert!(args.env.is_empty(), "the CLI sends no env for an exec");
        let params = build_session_params(args, &run_id);
        crate::run_registry::deregister(&run_id);

        assert!(
            params.env.contains(&"HOME=/workspace".to_string()),
            "a diagnostic command that has to re-export HOME is not in the same sandbox: {:?}",
            params.env
        );
        assert!(
            params.env.contains(&"SHELL=/bin/bash".to_string()),
            "got: {:?}",
            params.env
        );
        assert!(
            params
                .env
                .contains(&"SOME_TOOL_HOME=/workspace/mine".to_string()),
            "the definition's own value must win over the tool tree's: {:?}",
            params.env
        );
        assert_eq!(
            params.cwd.as_deref(),
            Some("/workspace"),
            "an exec starting in / makes `sh -lc` write to the wrong place"
        );
    }

    #[tokio::test]
    #[serial_test::serial(global_runs)]
    async fn an_exec_carrying_no_env_still_puts_the_runs_tool_dirs_first() {
        // What `lns exec` actually sends is an empty env, so the composed PATH has to hold up on that input and not just on a hand-supplied one.
        let tools = crate::workload_env::ToolRuntime {
            bin_paths: vec!["/.lens/tools/node/22.11.0/bin".to_string()],
            env: Vec::new(),
        };
        let run_id = crate::run_registry::allocate_run_id();
        let (mut handle, _cancel) = crate::run_registry::test_handle();
        handle.exec_environment.tools = tools.clone();
        crate::run_registry::register_named(run_id.clone(), None, handle).expect("register");
        let args = exec_args(vec!["node".into()], false, false);
        assert!(args.env.is_empty(), "the CLI sends no env for exec");
        let params = build_session_params(args, &run_id);
        crate::run_registry::deregister(&run_id);

        assert_eq!(
            params.env.iter().find(|kv| kv.starts_with("PATH=")),
            Some(&format!(
                "PATH=/.lens/tools/node/22.11.0/bin:{}",
                crate::workload_env::GUEST_DEFAULT_PATH
            )),
            "got: {:?}",
            params.env
        );
    }

    #[tokio::test]
    #[serial_test::serial(global_runs)]
    async fn exec_by_name_sees_the_same_tools_as_exec_by_id() {
        // `lns ps` shows names and the docs use them, so a name that loses the tool PATH is the common path, not the edge case.
        let run_id = crate::run_registry::allocate_run_id();
        let (mut handle, _cancel) = crate::run_registry::test_handle();
        handle.exec_environment.tools = crate::workload_env::ToolRuntime {
            bin_paths: vec!["/.lens/tools/node/22.11.0/bin".to_string()],
            env: Vec::new(),
        };
        crate::run_registry::register_named(run_id.clone(), Some("calm-finch".into()), handle)
            .expect("register the run");

        let by_name = crate::run_registry::resolve("calm-finch").expect("name resolves");
        let params = build_session_params(exec_args(vec!["node".into()], false, false), &by_name);
        assert!(
            params
                .env
                .iter()
                .any(|kv| kv.starts_with("PATH=/.lens/tools/node/22.11.0/bin:")),
            "got: {:?}",
            params.env
        );
        crate::run_registry::deregister(&run_id);
    }

    #[tokio::test]
    async fn handle_request_pull_of_an_invalid_reference_surfaces_the_parse_error() {
        let resp = as_json(
            handle_request(
                &Request::PullImage {
                    image: "###".into(),
                    expected_digest: format!("sha256:{}", "a".repeat(64)),
                },
                Instant::now(),
            )
            .await,
        );
        assert_eq!(resp["type"], "Error", "got {resp}");
        let message = resp["message"].as_str().expect("an error message");
        assert!(
            message.contains("invalid image reference"),
            "got: {message}"
        );
    }

    #[tokio::test]
    async fn handle_request_resolves_a_definition_that_names_no_mixin_without_touching_a_registry()
    {
        let resp = as_json(
            handle_request(
                &Request::ResolveDefinition {
                    definition: r#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"ghcr.io/team/base:1"}}"#.into(),
                    project_dir: "/work".into(),
                    mixins: Vec::new(),
                    decisions: None,
                },
                Instant::now(),
            )
            .await,
        );
        assert_eq!(resp["type"], "DefinitionResolved", "got {resp}");
        assert!(
            resp["definition"]
                .as_str()
                .expect("the merged document")
                .contains("ghcr.io/team/base:1"),
            "the caller runs what comes back, so it has to be the document and not an empty answer; got {resp}"
        );
        assert_eq!(
            resp["mixins"].as_array().expect("a source list").len(),
            0,
            "a document naming no mixin resolves to itself, so nothing is pulled and nothing is disclosed"
        );
    }

    #[tokio::test]
    async fn handle_request_resolve_refuses_a_definition_directory_that_is_not_absolute() {
        let resp = as_json(
            handle_request(
                &Request::ResolveDefinition {
                    definition: r#"{"apiVersion":"lns.run/v1","kind":"sandbox","name":"hermes","spec":{"image":"ghcr.io/team/base:1","mixins":["./mixins/pg"]}}"#.into(),
                    project_dir: "work".into(),
                    mixins: Vec::new(),
                    decisions: None,
                },
                Instant::now(),
            )
            .await,
        );
        assert_eq!(resp["type"], "Error", "got {resp}");
        assert!(
            resp["message"]
                .as_str()
                .expect("an error message")
                .contains("is not an absolute directory"),
            "a caller that sent no root would have its mixins read from whichever directory of that name the service sits beside; got {resp}"
        );
    }

    #[tokio::test]
    async fn handle_request_resolve_of_a_broken_definition_surfaces_the_parse_error() {
        let resp = as_json(
            handle_request(
                &Request::ResolveDefinition {
                    definition: "{}".into(),
                    project_dir: "/work".into(),
                    mixins: Vec::new(),
                    decisions: None,
                },
                Instant::now(),
            )
            .await,
        );
        assert_eq!(resp["type"], "Error", "got {resp}");
        assert!(
            resp["message"]
                .as_str()
                .expect("an error message")
                .contains("reading the sandbox document"),
            "got {resp}"
        );
    }

    #[tokio::test]
    async fn handle_request_inspect_of_an_invalid_reference_surfaces_the_parse_error() {
        let resp = as_json(
            handle_request(
                &Request::InspectImage {
                    image: "###".into(),
                    mixins: Vec::new(),
                    decisions: None,
                },
                Instant::now(),
            )
            .await,
        );
        assert_eq!(resp["type"], "Error", "got {resp}");
        let message = resp["message"].as_str().expect("an error message");
        assert!(
            message.contains("invalid image reference"),
            "got: {message}"
        );
    }

    #[test]
    fn login_response_maps_ok_to_verified() {
        assert_eq!(login_response(Ok(())), Response::RegistryLoginVerified);
    }

    #[test]
    fn login_response_maps_err_to_an_error_carrying_the_reason() {
        let resp = as_json(login_response(Err(anyhow::anyhow!("credentials rejected"))));
        assert_eq!(resp["type"], "Error", "got {resp}");
        assert!(
            resp["message"]
                .as_str()
                .expect("an error message")
                .contains("credentials rejected"),
            "got: {resp}"
        );
    }

    #[tokio::test]
    async fn handle_request_registry_login_with_an_invalid_registry_errors_before_any_network() {
        let resp = as_json(
            handle_request(
                &Request::RegistryLogin {
                    registry: "bad host".into(),
                    username: "u".into(),
                    secret: "s".into(),
                },
                Instant::now(),
            )
            .await,
        );
        assert_eq!(resp["type"], "Error", "got {resp}");
        let message = resp["message"].as_str().expect("an error message");
        assert!(message.contains("invalid registry"), "got: {message}");
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn handle_request_remove_of_an_unknown_image_surfaces_the_store_error() {
        let d = tempfile::tempdir().unwrap();
        let _h = crate::test_env::EnvVarGuard::set("HOME", d.path());
        let resp = as_json(
            handle_request(
                &Request::RemoveImage {
                    image: "registry.example.test/cov/absent:1".into(),
                },
                Instant::now(),
            )
            .await,
        );
        assert_eq!(resp["type"], "Error", "got {resp}");
        let message = resp["message"].as_str().expect("an error message");
        assert!(message.contains("no such image"), "got: {message}");
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn handle_request_pull_of_a_mixin_caches_its_graph_and_records_no_row() {
        let d = tempfile::tempdir().unwrap();
        let _h = crate::test_env::EnvVarGuard::set("HOME", d.path());
        let now = Instant::now();
        let manifest_cache = crate::image::manifest_cache::ManifestCache::new(
            crate::cache::root().unwrap().join("manifests"),
        );

        let child = seed_mixin(&manifest_cache, "child", r#"{"tools":["node@22"]}"#);
        let parent = seed_mixin(
            &manifest_cache,
            "parent",
            &format!(r#"{{"mixins":["{}"]}}"#, child.0),
        );

        let pulled = as_json(
            handle_request(
                &Request::PullImage {
                    image: parent.0.clone(),
                    expected_digest: parent.1.clone(),
                },
                now,
            )
            .await,
        );
        assert_eq!(pulled["type"], "MixinPulled", "got {pulled}");
        assert_eq!(pulled["reference"], parent.0);
        assert_eq!(pulled["digest"], parent.1);
        assert_eq!(
            pulled["cached_mixins"], 1,
            "a pull that stopped at the mixin itself would still need the network the first time something merges it"
        );

        let listed = as_json(handle_request(&Request::ListImages, now).await);
        assert_eq!(
            listed["images"].as_array().unwrap().len(),
            0,
            "nothing runs a mixin and nothing has to reclaim it, so it takes no index row"
        );
    }

    /// Seed a published mixin into the manifest cache, so a pull resolves it with no network; answers with its pinned reference and digest.
    fn seed_mixin(
        cache: &crate::image::manifest_cache::ManifestCache,
        name: &str,
        spec: &str,
    ) -> (String, String) {
        use sha2::Digest;
        let document = format!(
            r#"{{"apiVersion":"lns.run/v1","kind":"mixin","name":"{name}","spec":{spec}}}"#
        );
        let manifest = oci_client::manifest::OciImageManifest {
            artifact_type: Some("application/vnd.lens.mixin.v1+json".into()),
            config: oci_client::manifest::OciDescriptor {
                media_type: "application/vnd.lens.mixin.config.v1+json".into(),
                digest: format!("sha256:{:x}", sha2::Sha256::digest(document.as_bytes())),
                size: document.len() as i64,
                ..Default::default()
            },
            ..Default::default()
        };
        let digest = format!(
            "sha256:{:x}",
            sha2::Sha256::digest(serde_json::to_vec(&manifest).unwrap())
        );
        let reference = format!("registry.example.test/cov/{name}@{digest}");
        cache
            .put(
                &reference,
                &crate::image::manifest_cache::CachedManifest {
                    manifest,
                    manifest_digest: digest.clone(),
                    config: document,
                },
            )
            .unwrap();
        (reference, digest)
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn handle_request_image_lifecycle_round_trips_offline_via_the_caches() {
        let d = tempfile::tempdir().unwrap();
        let _h = crate::test_env::EnvVarGuard::set("HOME", d.path());
        let now = Instant::now();

        use sha2::Digest;
        let layer_bytes = b"offline layer".to_vec();
        let layer_digest = format!("sha256:{:x}", sha2::Sha256::digest(&layer_bytes));

        let base_manifest = oci_client::manifest::OciImageManifest {
            layers: vec![oci_client::manifest::OciDescriptor {
                digest: layer_digest.clone(),
                size: layer_bytes.len() as i64,
                media_type: "application/vnd.oci.image.layer.v1.tar".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let base_digest = format!(
            "sha256:{:x}",
            sha2::Sha256::digest(serde_json::to_vec(&base_manifest).unwrap())
        );
        let base_ref = format!("registry.example.test/cov/base@{base_digest}");

        let cache_root = crate::cache::root().unwrap();
        let layer_cache = crate::oci_layer_cache::LayerCache::new(cache_root.join("layers"));
        layer_cache
            .install_from_bytes(&layer_digest, &layer_bytes)
            .unwrap();
        let config = format!(
            r#"{{"architecture":"arm64","os":"linux","rootfs":{{"type":"layers","diff_ids":["{layer_digest}"]}}}}"#
        );
        let manifest_cache =
            crate::image::manifest_cache::ManifestCache::new(cache_root.join("manifests"));
        manifest_cache
            .put(
                &base_ref,
                &crate::image::manifest_cache::CachedManifest {
                    manifest: base_manifest,
                    manifest_digest: base_digest.clone(),
                    config,
                },
            )
            .unwrap();
        let definition = format!(
            r#"{{"apiVersion":"lns.run/v1","kind":"sandbox","name":"cached-lifecycle","spec":{{"image":"{base_ref}"}}}}"#
        );
        let artifact_manifest = oci_client::manifest::OciImageManifest {
            artifact_type: Some("application/vnd.lens.sandbox.v1+json".into()),
            config: oci_client::manifest::OciDescriptor {
                media_type: "application/vnd.lens.sandbox.config.v1+json".into(),
                digest: format!("sha256:{:x}", sha2::Sha256::digest(definition.as_bytes())),
                size: definition.len() as i64,
                ..Default::default()
            },
            ..Default::default()
        };
        let manifest_digest = format!(
            "sha256:{:x}",
            sha2::Sha256::digest(serde_json::to_vec(&artifact_manifest).unwrap())
        );
        let reference = format!("registry.example.test/cov/pinned@{manifest_digest}");
        manifest_cache
            .put(
                &reference,
                &crate::image::manifest_cache::CachedManifest {
                    manifest: artifact_manifest,
                    manifest_digest: manifest_digest.clone(),
                    config: definition,
                },
            )
            .unwrap();

        let pulled = as_json(
            handle_request(
                &Request::PullImage {
                    image: reference.clone(),
                    expected_digest: manifest_digest.clone(),
                },
                now,
            )
            .await,
        );
        assert_eq!(pulled["type"], "ImagePulled", "got {pulled}");
        assert_eq!(pulled["image"]["reference"], reference);
        assert_eq!(pulled["image"]["digest"], manifest_digest);
        assert_eq!(pulled["image"]["layers"], 0);
        assert_eq!(pulled["image"]["size_bytes"], 0);

        let listed = as_json(handle_request(&Request::ListImages, now).await);
        assert_eq!(listed["type"], "ImageList", "got {listed}");
        assert!(
            listed["images"]
                .as_array()
                .unwrap()
                .iter()
                .any(|image| image["reference"] == reference),
            "got {listed}"
        );

        let tagged = as_json(
            handle_request(
                &Request::TagImage {
                    from: reference.clone(),
                    to: "registry.example.test/cov/pinned:latest".into(),
                },
                now,
            )
            .await,
        );
        assert_eq!(tagged["type"], "ImageTagged", "got {tagged}");
        let after_tag = as_json(handle_request(&Request::ListImages, now).await);
        assert_eq!(
            after_tag["images"].as_array().unwrap().len(),
            3,
            "the cached base image and both sandbox references are listed"
        );
        // Drop the tag copy so the original layer-sweep assertions below still hold.
        as_json(
            handle_request(
                &Request::RemoveImage {
                    image: "registry.example.test/cov/pinned:latest".into(),
                },
                now,
            )
            .await,
        );

        let removed = as_json(
            handle_request(
                &Request::RemoveImage {
                    image: reference.clone(),
                },
                now,
            )
            .await,
        );
        assert_eq!(removed["type"], "ImageRemoved", "got {removed}");
        assert_eq!(removed["reference"], reference);
        assert_eq!(removed["reclaimed_bytes"], layer_bytes.len() as u64);
        assert!(
            !layer_cache.contains(&layer_digest).unwrap(),
            "removing the last sandbox owner must sweep its base-image layer"
        );

        let base_removed = as_json(
            handle_request(
                &Request::RemoveImage {
                    image: base_ref.clone(),
                },
                now,
            )
            .await,
        );
        assert_eq!(base_removed["type"], "Error", "got {base_removed}");
        assert!(
            base_removed["message"]
                .as_str()
                .is_some_and(|message| message.contains("no such image")),
            "the orphaned base-image record must leave with its sandbox: {base_removed}"
        );

        let pruned = as_json(handle_request(&Request::PruneImages, now).await);
        assert_eq!(pruned["type"], "ImagesPruned", "got {pruned}");
        assert_eq!(pruned["removed"], serde_json::json!([]));
        assert_eq!(pruned["reclaimed_bytes"], 0);
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn handle_request_pull_of_a_sandbox_artifact_caches_it_with_its_base_image() {
        let d = tempfile::tempdir().unwrap();
        let _h = crate::test_env::EnvVarGuard::set("HOME", d.path());
        let now = Instant::now();

        use sha2::Digest;
        let cache_root = crate::cache::root().unwrap();
        let manifest_cache =
            crate::image::manifest_cache::ManifestCache::new(cache_root.join("manifests"));
        let layer_cache = crate::oci_layer_cache::LayerCache::new(cache_root.join("layers"));

        let layer_bytes = b"offline base layer".to_vec();
        let layer_digest = format!("sha256:{:x}", sha2::Sha256::digest(&layer_bytes));
        layer_cache
            .install_from_bytes(&layer_digest, &layer_bytes)
            .unwrap();
        let base_manifest = oci_client::manifest::OciImageManifest {
            layers: vec![oci_client::manifest::OciDescriptor {
                digest: layer_digest.clone(),
                size: layer_bytes.len() as i64,
                media_type: "application/vnd.oci.image.layer.v1.tar".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let base_digest = format!(
            "sha256:{:x}",
            sha2::Sha256::digest(serde_json::to_vec(&base_manifest).unwrap())
        );
        let base_ref = format!("registry.example.test/cov/base@{base_digest}");
        manifest_cache
            .put(
                &base_ref,
                &crate::image::manifest_cache::CachedManifest {
                    manifest: base_manifest,
                    manifest_digest: base_digest.clone(),
                    config: format!(
                        r#"{{"architecture":"arm64","os":"linux","rootfs":{{"type":"layers","diff_ids":["{layer_digest}"]}}}}"#
                    ),
                },
            )
            .unwrap();

        let definition = format!(
            r#"{{"apiVersion":"lns.run/v1","kind":"sandbox","name":"some-sandbox","spec":{{"image":"{base_ref}"}}}}"#
        );
        let artifact_manifest = oci_client::manifest::OciImageManifest {
            artifact_type: Some("application/vnd.lens.sandbox.v1+json".into()),
            config: oci_client::manifest::OciDescriptor {
                media_type: "application/vnd.lens.sandbox.config.v1+json".into(),
                digest: format!("sha256:{:x}", sha2::Sha256::digest(definition.as_bytes())),
                size: definition.len() as i64,
                ..Default::default()
            },
            ..Default::default()
        };
        let artifact_digest = format!(
            "sha256:{:x}",
            sha2::Sha256::digest(serde_json::to_vec(&artifact_manifest).unwrap())
        );
        let artifact_ref = format!("registry.example.test/cov/sandbox@{artifact_digest}");
        manifest_cache
            .put(
                &artifact_ref,
                &crate::image::manifest_cache::CachedManifest {
                    manifest: artifact_manifest,
                    manifest_digest: artifact_digest.clone(),
                    config: definition,
                },
            )
            .unwrap();

        let pulled = as_json(
            handle_request(
                &Request::PullImage {
                    image: artifact_ref.clone(),
                    expected_digest: artifact_digest.clone(),
                },
                now,
            )
            .await,
        );
        assert_eq!(pulled["type"], "ImagePulled", "got {pulled}");
        assert_eq!(pulled["image"]["reference"], artifact_ref);
        assert_eq!(pulled["image"]["digest"], artifact_digest);
        assert_eq!(
            pulled["image"]["layers"], 0,
            "a config-only artifact records no layers"
        );

        let listed = as_json(handle_request(&Request::ListImages, now).await);
        let refs: Vec<String> = listed["images"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["reference"].as_str().unwrap().to_string())
            .collect();
        assert!(
            refs.contains(&artifact_ref) && refs.contains(&base_ref),
            "pull must cache the sandbox and prefetch its base image, got {refs:?}"
        );
    }
}
