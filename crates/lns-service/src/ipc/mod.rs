use std::time::Instant;

use lns_ipc::{Request, Response, StatusInfo, WireFrame, encode_wire_frame};
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, oneshot};

mod adapter;
pub use adapter::run_server;

#[derive(Debug, PartialEq, Eq)]
pub(super) enum PumpOutcome {
    ExitFrame,
    ChannelClosed,
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
) -> anyhow::Result<PumpOutcome>
where
    W: AsyncWriteExt + Unpin,
{
    let mut cancel_rx = Some(cancel_rx);
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

pub async fn handle_request(request: &Request, started_at: Instant) -> Response {
    match request {
        Request::Ping => Response::Pong,
        Request::Status => Response::Status(StatusInfo {
            pid: std::process::id(),
            uptime_secs: started_at.elapsed().as_secs(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        }),
        Request::Shutdown => Response::ShuttingDown,
        Request::RunImage(_) => {
            unreachable!("Request::RunImage must be dispatched via handle_run, not handle_request")
        }
        Request::ExecImage(_) => {
            unreachable!(
                "Request::ExecImage must be dispatched via handle_exec, not handle_request"
            )
        }
        Request::BeginIntegrationSignIn { .. } => {
            unreachable!(
                "Request::BeginIntegrationSignIn must be dispatched via handle_integration_sign_in, not handle_request"
            )
        }
        Request::CancelRun { run_id } => {
            if crate::run_registry::cancel(*run_id) {
                Response::CancelAccepted
            } else {
                Response::Error {
                    message: format!("no active run with id {run_id}"),
                }
            }
        }
        Request::RunStdin { run_id, bytes } => {
            forward_session_input(*run_id, session_input_from_stdin(bytes.clone()), "RunStdin")
                .await
        }
        Request::RunResize { run_id, rows, cols } => {
            forward_session_input(
                *run_id,
                session_input_from_resize(*rows, *cols),
                "RunResize",
            )
            .await
        }
        Request::RunSignal { run_id, signal } => {
            forward_session_input(*run_id, session_input_from_signal(*signal), "RunSignal").await
        }
        Request::Kill { run, signal } => kill_request(run, *signal).await,
        Request::ListRuns => Response::RunList {
            runs: crate::run_registry::snapshot(),
        },
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
        Request::PullImage { image } => image_response(
            crate::image_store::pull(image)
                .await
                .map(|image| Response::ImagePulled { image }),
        ),
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
        Request::RegistryLogin {
            registry,
            username,
            secret,
        } => login_response(crate::image::verify_login(registry, username, secret).await),
        Request::StopRun { run, timeout_secs } => stop_run_request(run, *timeout_secs).await,
        Request::InspectRun { run } => inspect_run_request(run),
        Request::RemoveRun { run } => remove_run_request(run),
        Request::RenameRun { run, new_name } => match crate::run_registry::rename(run, new_name) {
            Ok(()) => Response::Acknowledged,
            Err(message) => Response::Error { message },
        },
        Request::PruneRuns => Response::RunsPruned {
            removed: crate::run_registry::prune_exited(),
        },
        Request::RunLogs { .. } => {
            unreachable!("Request::RunLogs must be dispatched via handle_logs, not handle_request")
        }
        Request::AttachRun { .. } => {
            unreachable!(
                "Request::AttachRun must be dispatched via handle_attach, not handle_request"
            )
        }
        Request::RunStats { .. } => {
            unreachable!(
                "Request::RunStats must be dispatched via handle_stats, not handle_request"
            )
        }
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
    if crate::run_registry::status(id).is_none() {
        return Response::Error {
            message: format!("no active run with id {id}"),
        };
    }
    forward_session_input(id, session_input_from_signal(signal), "Kill").await
}

async fn stop_run_request(run: &str, timeout_secs: u64) -> Response {
    let id = match crate::run_registry::resolve(run) {
        Ok(id) => id,
        Err(message) => return Response::Error { message },
    };
    stop_run_with(
        id,
        std::time::Duration::from_secs(timeout_secs),
        KILL_GRACE,
        |signal| forward_session_input(id, session_input_from_signal(signal), "StopRun"),
    )
    .await
}

fn inspect_run_request(run: &str) -> Response {
    match crate::run_registry::resolve(run) {
        Ok(id) => match crate::run_registry::inspect(id) {
            Some(details) => Response::RunInspect {
                details: Box::new(details),
            },
            None => Response::Error {
                message: format!("no active run with id {id}"),
            },
        },
        Err(message) => Response::Error { message },
    }
}

fn remove_run_request(run: &str) -> Response {
    let id = match crate::run_registry::resolve(run) {
        Ok(id) => id,
        Err(message) => return Response::Error { message },
    };
    match crate::run_registry::remove_if_exited(id) {
        crate::run_registry::RemoveOutcome::Removed => Response::Acknowledged,
        crate::run_registry::RemoveOutcome::Running => Response::Error {
            message: format!(
                "run {id} is still running; stop it first with `lns sandbox stop {id}`"
            ),
        },
        crate::run_registry::RemoveOutcome::NotFound => Response::Error {
            message: format!("no run with id {id}"),
        },
    }
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
    run_id: u32,
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

async fn wait_for_exit(run_id: u32, timeout: std::time::Duration) -> bool {
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

#[cfg(target_os = "macos")]
async fn forward_session_input(
    run_id: u32,
    input: Option<crate::vm::session_client::SessionInput>,
    kind: &'static str,
) -> Response {
    let Some(input) = input else {
        return Response::Error {
            message: format!("{kind} not supported on this build"),
        };
    };
    let Some(tx) = crate::run_registry::input_sender(run_id) else {
        return Response::Error {
            message: format!("no active session for run {run_id}"),
        };
    };
    match tx.send(input).await {
        Ok(()) => Response::Acknowledged,
        Err(e) => Response::Error {
            message: format!("forwarding {kind} to run {run_id} failed: {e}"),
        },
    }
}

#[cfg(not(target_os = "macos"))]
async fn forward_session_input(_run_id: u32, _input: Option<()>, kind: &'static str) -> Response {
    Response::Error {
        message: format!("{kind} only supported on macOS hosts"),
    }
}

#[cfg(target_os = "macos")]
fn session_input_from_stdin(bytes: Vec<u8>) -> Option<crate::vm::session_client::SessionInput> {
    Some(crate::vm::session_client::SessionInput::StdinBytes(bytes))
}
#[cfg(target_os = "macos")]
fn session_input_from_resize(
    rows: u16,
    cols: u16,
) -> Option<crate::vm::session_client::SessionInput> {
    Some(crate::vm::session_client::SessionInput::Resize { rows, cols })
}
#[cfg(target_os = "macos")]
fn session_input_from_signal(
    signal: lns_ipc::SignalKind,
) -> Option<crate::vm::session_client::SessionInput> {
    Some(crate::vm::session_client::SessionInput::Signal(map_signal(
        signal,
    )))
}

#[cfg(target_os = "macos")]
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

#[cfg(target_os = "macos")]
pub(super) fn validate_exec(args: &lns_ipc::ExecImageArgs) -> Result<(), String> {
    if args.argv.is_empty() {
        return Err("ExecImage.argv is empty".to_string());
    }
    if args.tty || args.stdin {
        return Err(format!(
            "lns exec -t/-i against run #{} is not yet supported (input \
             routing for exec sessions awaits an IPC discriminator); for now lns exec \
             supports non-interactive commands only",
            args.run
        ));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
pub(super) fn build_session_params(
    args: lns_ipc::ExecImageArgs,
) -> crate::vm::session_client::SessionParams {
    crate::vm::session_client::SessionParams {
        argv: args.argv,
        env: args.env,
        cwd: None,
        tty: args.tty,
        stdin: args.stdin,
        initial_winsize: args
            .initial_winsize
            .map(|(rows, cols)| lns_session::Winsize { rows, cols }),
    }
}

#[cfg(not(target_os = "macos"))]
fn session_input_from_stdin(_bytes: Vec<u8>) -> Option<()> {
    None
}
#[cfg(not(target_os = "macos"))]
fn session_input_from_resize(_rows: u16, _cols: u16) -> Option<()> {
    None
}
#[cfg(not(target_os = "macos"))]
fn session_input_from_signal(_signal: lns_ipc::SignalKind) -> Option<()> {
    None
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
    #[should_panic(expected = "Request::RunImage must be dispatched via handle_run")]
    async fn run_image_via_handle_request_panics() {
        let _ = handle_request(
            &Request::RunImage(lns_ipc::RunImageArgs {
                image: None,
                name: None,
                cpus: 1,
                mem: 0,
                policy_path: None,
                sandbox_user: None,
                sandbox_uid: None,
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
            }),
            Instant::now(),
        )
        .await;
    }

    #[tokio::test]
    #[should_panic(
        expected = "BeginIntegrationSignIn must be dispatched via handle_integration_sign_in"
    )]
    async fn begin_integration_sign_in_via_handle_request_panics() {
        let _ = handle_request(
            &Request::BeginIntegrationSignIn {
                id: "some-oauth".into(),
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

    #[tokio::test]
    async fn pump_preserves_frame_order_and_exits_on_run_exit() {
        let mut sink: Vec<u8> = Vec::new();
        let (tx, mut rx) = mpsc::channel::<WireFrame>(8);
        let (_cancel_tx, cancel_rx) = never_cancel();

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

        let outcome = pump_responses(&mut sink, &mut rx, cancel_rx).await.unwrap();
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
        tx.send(WireFrame::Stdout(b"x".to_vec())).await.unwrap();
        drop(tx);

        let outcome = pump_responses(&mut sink, &mut rx, cancel_rx).await.unwrap();
        assert_eq!(outcome, PumpOutcome::ChannelClosed);
    }

    #[tokio::test]
    async fn pump_reports_write_failure_on_disconnect() {
        let (write_side, read_side) = tokio::io::duplex(64);
        drop(read_side);

        let (tx, mut rx) = mpsc::channel::<WireFrame>(8);
        let (_cancel_tx, cancel_rx) = never_cancel();
        tx.send(WireFrame::Stdout(vec![0; 1024])).await.unwrap();
        drop(tx);

        let mut w = write_side;
        let outcome = pump_responses(&mut w, &mut rx, cancel_rx).await.unwrap();
        assert!(
            matches!(outcome, PumpOutcome::WriteFailed(_)),
            "expected WriteFailed, got {outcome:?}",
        );
    }

    #[tokio::test]
    async fn pump_cancel_writes_run_exit_even_when_frame_channel_is_full() {
        let mut sink: Vec<u8> = Vec::new();
        let (tx, mut rx) = mpsc::channel::<WireFrame>(1);
        let (cancel_tx, cancel_rx) = oneshot::channel::<i32>();

        tx.send(WireFrame::Stdout(b"buffered".to_vec()))
            .await
            .unwrap();
        cancel_tx.send(130).unwrap();

        let outcome = pump_responses(&mut sink, &mut rx, cancel_rx).await.unwrap();
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

        drop(cancel_tx);

        tx.send(WireFrame::Stdout(b"k".to_vec())).await.unwrap();
        tx.send(WireFrame::Json(Response::RunExit { code: 0 }))
            .await
            .unwrap();
        drop(tx);

        let outcome = pump_responses(&mut sink, &mut rx, cancel_rx).await.unwrap();
        assert_eq!(outcome, PumpOutcome::ExitFrame);

        let decoded = decode_wire_frames_from(&sink);
        assert!(matches!(decoded[0], WireFrame::Stdout(_)));
        assert!(matches!(
            decoded[1],
            WireFrame::Json(Response::RunExit { code: 0 })
        ));
    }

    #[test]
    fn post_pump_action_detached_write_failure_drains_in_background_instead_of_cancelling() {
        let outcome = PumpOutcome::WriteFailed("broken pipe".into());
        assert_eq!(
            post_pump_action(&outcome, true),
            PostPumpAction::BackgroundDrain,
            "detached + WriteFailed must NOT cancel the run — the CLI is allowed to close the stream after `run #N`",
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
        let resp = handle_request(&Request::CancelRun { run_id: u32::MAX }, Instant::now()).await;
        match resp {
            Response::Error { message } => assert!(message.contains("no active run")),
            _ => unreachable!("forward_session_input always returns Acknowledged or Error"),
        }
    }

    #[cfg(target_os = "macos")]
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
            id,
            crate::run_registry::RunHandle {
                cancel_tx,
                task,
                input_tx: Some(input_tx),
                connector: None,
                name: String::new(),
                image: String::new(),
                command: String::new(),
                started: String::new(),
                status: std::sync::Mutex::new(lns_ipc::RunStatus::Running),
                logs: std::sync::Arc::new(crate::run_log::RunLogBuffer::default()),
                config: lns_ipc::RunConfig::default(),
            },
        );

        let consumer = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            let _ = input_rx.recv().await;
            input_rx
        });

        let resp = handle_request(
            &Request::RunStdin {
                run_id: id,
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
        crate::run_registry::deregister(id);
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn forward_session_input_errors_when_run_not_registered() {
        let id = crate::run_registry::allocate_run_id() + 3_000_000;
        let resp = handle_request(
            &Request::RunStdin {
                run_id: id,
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
        let _x = crate::test_env::EnvVarGuard::set("XDG_CACHE_HOME", d.path().join("cache"));
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
        let _x = crate::test_env::EnvVarGuard::set("XDG_CACHE_HOME", d.path().join("cache"));
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
            &Request::RunStdin {
                run_id: 999_999,
                bytes: vec![],
            },
            Instant::now(),
        )
        .await;
        assert!(matches!(response, Response::Error { .. }));
    }

    #[tokio::test]
    async fn handle_request_run_resize_for_unregistered_run_returns_error() {
        let response = handle_request(
            &Request::RunResize {
                run_id: 999_999,
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
            &Request::RunSignal {
                run_id: 999_999,
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
                message.contains("no active run with id 999999"),
                "an unknown run must report no-active-run, not a capability error; got: {message}"
            ),
            other => panic!("expected an unknown-run error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn handle_request_kill_for_a_registered_run_passes_existence_and_forwards() {
        let id = 557799;
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
    #[should_panic(expected = "ExecImage must be dispatched via handle_exec")]
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

    #[cfg(target_os = "macos")]
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

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn forward_session_input_returns_error_when_input_is_none() {
        let response = forward_session_input(1, None, "Synthetic").await;
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
        let run_id = u32::MAX - 7;
        let (cancel_tx, _cancel_rx) = oneshot::channel();
        let task = tokio::spawn(async {});
        let handle = crate::run_registry::RunHandle {
            cancel_tx,
            task,
            #[cfg(target_os = "macos")]
            input_tx: None,
            #[cfg(target_os = "macos")]
            connector: None,
            name: String::new(),
            image: "test-image".into(),
            command: "".into(),
            started: "1970-01-01T00:00:00Z".into(),
            status: Mutex::new(lns_ipc::RunStatus::Running),
            logs: std::sync::Arc::new(crate::run_log::RunLogBuffer::default()),
            config: lns_ipc::RunConfig::default(),
        };
        crate::run_registry::register(run_id, handle);
        let resp = handle_request(&Request::CancelRun { run_id }, Instant::now()).await;
        assert!(matches!(resp, Response::CancelAccepted));
        let _ = crate::run_registry::cancel(run_id);
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn forward_session_input_errors_when_input_channel_is_closed() {
        use std::sync::Mutex;
        use tokio::sync::{mpsc, oneshot};
        let run_id = u32::MAX - 11;
        let (input_tx, input_rx) = mpsc::channel(1);
        drop(input_rx);
        let (cancel_tx, _cancel_rx) = oneshot::channel();
        let task = tokio::spawn(async {});
        let handle = crate::run_registry::RunHandle {
            cancel_tx,
            task,
            input_tx: Some(input_tx),
            connector: None,
            name: String::new(),
            image: "closed-channel-test".into(),
            command: "".into(),
            started: "1970-01-01T00:00:00Z".into(),
            status: Mutex::new(lns_ipc::RunStatus::Running),
            logs: std::sync::Arc::new(crate::run_log::RunLogBuffer::default()),
            config: lns_ipc::RunConfig::default(),
        };
        crate::run_registry::register(run_id, handle);

        let resp = handle_request(
            &Request::RunStdin {
                run_id,
                bytes: b"hi".to_vec(),
            },
            Instant::now(),
        )
        .await;
        match resp {
            Response::Error { message } => {
                assert!(message.contains("forwarding"));
                assert!(message.contains("RunStdin"));
                assert!(message.contains(&run_id.to_string()));
            }
            other => unreachable!("expected Error, got {other:?}"),
        }
        let _ = crate::run_registry::cancel(run_id);
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
        cancel_tx.send(130).expect("cancel send");
        let outcome = pump_responses(&mut sink, &mut rx, cancel_rx)
            .await
            .expect("pump returned Result");
        assert!(
            matches!(outcome, PumpOutcome::WriteFailed(_)),
            "broken-pipe stream during cancel must surface as WriteFailed"
        );
    }

    #[cfg(not(target_os = "macos"))]
    #[tokio::test]
    async fn forward_session_input_on_linux_returns_not_supported_error() {
        let response = forward_session_input(1, None, "Synthetic").await;
        assert!(matches!(response, Response::Error { .. }));
        if let Response::Error { message } = &response {
            assert!(message.contains("Synthetic"), "got: {message}");
            assert!(message.to_lowercase().contains("macos"), "got: {message}");
        }
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn session_input_from_stdin_returns_none_on_linux() {
        assert!(session_input_from_stdin(b"x".to_vec()).is_none());
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn session_input_from_resize_returns_none_on_linux() {
        assert!(session_input_from_resize(24, 80).is_none());
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn session_input_from_signal_returns_none_on_linux() {
        assert!(session_input_from_signal(lns_ipc::SignalKind::Term).is_none());
    }

    fn register_running(run_id: u32) {
        let (cancel_tx, _cancel_rx) = oneshot::channel();
        let task = tokio::spawn(async {});
        crate::run_registry::register(
            run_id,
            crate::run_registry::RunHandle {
                cancel_tx,
                task,
                #[cfg(target_os = "macos")]
                input_tx: None,
                #[cfg(target_os = "macos")]
                connector: None,
                name: "reviewer".into(),
                image: "stop-test".into(),
                command: String::new(),
                started: "1970-01-01T00:00:00Z".into(),
                status: std::sync::Mutex::new(lns_ipc::RunStatus::Running),
                logs: std::sync::Arc::new(crate::run_log::RunLogBuffer::default()),
                config: lns_ipc::RunConfig::default(),
            },
        );
    }

    fn recording_sender(
        run_id: u32,
        exit_on: Option<lns_ipc::SignalKind>,
    ) -> (
        std::sync::Arc<std::sync::Mutex<Vec<lns_ipc::SignalKind>>>,
        impl Fn(lns_ipc::SignalKind) -> std::future::Ready<Response>,
    ) {
        let sent = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let sent_for_closure = sent.clone();
        let sender = move |signal: lns_ipc::SignalKind| {
            sent_for_closure.lock().unwrap().push(signal);
            if exit_on == Some(signal) {
                crate::run_registry::set_exit_code(run_id, 137);
            }
            std::future::ready(Response::Acknowledged)
        };
        (sent, sender)
    }

    #[tokio::test]
    #[should_panic(expected = "Request::RunLogs must be dispatched via handle_logs")]
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
    #[should_panic(expected = "Request::AttachRun must be dispatched via handle_attach")]
    async fn attach_run_via_handle_request_panics() {
        let _ = handle_request(&Request::AttachRun { run: "1".into() }, Instant::now()).await;
    }

    #[tokio::test]
    #[should_panic(expected = "Request::RunStats must be dispatched via handle_stats")]
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
                assert!(message.contains("no active run"), "got: {message}");
            }
            other => unreachable!("expected Error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn handle_request_inspect_run_returns_details_for_a_registered_run() {
        let id = crate::run_registry::allocate_run_id();
        register_running(id);

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
        crate::run_registry::deregister(id);
    }

    #[tokio::test]
    #[serial_test::serial(global_runs)]
    async fn handle_request_prune_runs_removes_exited_runs_and_reports_their_ids() {
        let id = crate::run_registry::allocate_run_id();
        register_running(id);
        crate::run_registry::set_exit_code(id, 0);

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
        assert_eq!(crate::run_registry::status(id), None);
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
                assert!(message.contains("no active run"), "got: {message}");
            }
            other => unreachable!("expected Error, got {other:?}"),
        }
    }

    #[tokio::test]
    #[serial_test::serial(global_runs)]
    async fn stop_of_an_already_exited_run_succeeds_without_signalling() {
        let id = crate::run_registry::allocate_run_id();
        register_running(id);
        crate::run_registry::set_exit_code(id, 0);
        let (sent, sender) = recording_sender(id, None);

        let resp = stop_run_with(
            id,
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
        crate::run_registry::deregister(id);
    }

    #[tokio::test(start_paused = true)]
    async fn stop_returns_unforced_when_the_workload_exits_on_term() {
        let id = crate::run_registry::allocate_run_id();
        register_running(id);
        let (sent, sender) = recording_sender(id, Some(lns_ipc::SignalKind::Term));

        let resp = stop_run_with(id, std::time::Duration::from_secs(10), KILL_GRACE, sender).await;

        assert_eq!(resp, Response::RunStopped { forced: false });
        assert_eq!(sent.lock().unwrap().as_slice(), [lns_ipc::SignalKind::Term]);
        crate::run_registry::deregister(id);
    }

    #[tokio::test(start_paused = true)]
    async fn stop_escalates_to_kill_when_term_is_ignored() {
        let id = crate::run_registry::allocate_run_id();
        register_running(id);
        let (sent, sender) = recording_sender(id, Some(lns_ipc::SignalKind::Kill));

        let resp = stop_run_with(id, std::time::Duration::from_secs(10), KILL_GRACE, sender).await;

        assert_eq!(resp, Response::RunStopped { forced: true });
        assert_eq!(
            sent.lock().unwrap().as_slice(),
            [lns_ipc::SignalKind::Term, lns_ipc::SignalKind::Kill]
        );
        crate::run_registry::deregister(id);
    }

    #[tokio::test(start_paused = true)]
    async fn stop_with_zero_timeout_skips_straight_to_kill() {
        let id = crate::run_registry::allocate_run_id();
        register_running(id);
        let (sent, sender) = recording_sender(id, Some(lns_ipc::SignalKind::Kill));

        let resp = stop_run_with(id, std::time::Duration::from_secs(0), KILL_GRACE, sender).await;

        assert_eq!(resp, Response::RunStopped { forced: true });
        assert_eq!(sent.lock().unwrap().len(), 2);
        crate::run_registry::deregister(id);
    }

    #[tokio::test(start_paused = true)]
    async fn stop_reports_failure_when_kill_does_not_end_the_run() {
        let id = crate::run_registry::allocate_run_id();
        register_running(id);
        let (sent, sender) = recording_sender(id, None);

        let resp = stop_run_with(
            id,
            std::time::Duration::from_secs(1),
            std::time::Duration::from_secs(1),
            sender,
        )
        .await;

        let rendered = format!("{resp:?}");
        assert!(rendered.contains("Error"), "got: {rendered}");
        assert!(rendered.contains("SIGKILL"), "got: {rendered}");
        assert_eq!(sent.lock().unwrap().len(), 2);
        crate::run_registry::cancel(id);
    }

    #[tokio::test]
    async fn stop_propagates_a_term_send_error() {
        let id = crate::run_registry::allocate_run_id();
        register_running(id);

        let resp = stop_run_with(
            id,
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
        crate::run_registry::cancel(id);
    }

    #[tokio::test(start_paused = true)]
    async fn stop_propagates_a_kill_send_error() {
        let id = crate::run_registry::allocate_run_id();
        register_running(id);
        let calls = std::sync::Arc::new(std::sync::Mutex::new(0u32));
        let calls_for_closure = calls.clone();

        let resp = stop_run_with(
            id,
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
        crate::run_registry::cancel(id);
    }

    #[cfg(target_os = "macos")]
    #[tokio::test(start_paused = true)]
    async fn handle_request_stop_run_sends_term_through_the_session_channel() {
        use crate::vm::session_client::SessionInput;
        use tokio::sync::mpsc;

        let id = crate::run_registry::allocate_run_id();
        let (cancel_tx, _cancel_rx) = oneshot::channel();
        let task = tokio::spawn(std::future::pending::<()>());
        let (input_tx, mut input_rx) = mpsc::channel::<SessionInput>(4);
        crate::run_registry::register(
            id,
            crate::run_registry::RunHandle {
                cancel_tx,
                task,
                input_tx: Some(input_tx),
                connector: None,
                name: String::new(),
                image: String::new(),
                command: String::new(),
                started: String::new(),
                status: std::sync::Mutex::new(lns_ipc::RunStatus::Running),
                logs: std::sync::Arc::new(crate::run_log::RunLogBuffer::default()),
                config: lns_ipc::RunConfig::default(),
            },
        );

        let consumer = tokio::spawn(async move {
            let input = input_rx.recv().await.expect("Term must arrive");
            assert!(
                matches!(input, SessionInput::Signal(lns_session::SignalKind::Term)),
                "expected Term, got {input:?}"
            );
            crate::run_registry::set_exit_code(id, 143);
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
        crate::run_registry::cancel(id);
    }

    #[cfg(not(target_os = "macos"))]
    #[tokio::test]
    async fn handle_request_stop_run_on_linux_reports_macos_only() {
        let id = crate::run_registry::allocate_run_id();
        register_running(id);

        let resp = handle_request(
            &Request::StopRun {
                run: id.to_string(),
                timeout_secs: 1,
            },
            Instant::now(),
        )
        .await;

        match resp {
            Response::Error { message } => {
                assert!(message.to_lowercase().contains("macos"), "got: {message}");
            }
            other => unreachable!("expected Error, got {other:?}"),
        }
        crate::run_registry::cancel(id);
    }

    #[cfg(target_os = "macos")]
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

    #[cfg(target_os = "macos")]
    #[test]
    fn validate_exec_rejects_empty_argv() {
        let result = validate_exec(&exec_args(Vec::new(), false, false));
        assert_eq!(result, Err("ExecImage.argv is empty".to_string()));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn validate_exec_rejects_interactive_exec_and_names_the_run() {
        for (tty, stdin) in [(true, false), (false, true)] {
            let err = validate_exec(&exec_args(vec!["sh".into()], tty, stdin))
                .expect_err("interactive exec is unsupported");
            assert!(err.contains("run #42"), "should name the run: {err}");
            assert!(err.contains("not yet supported"), "got: {err}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn validate_exec_accepts_a_non_interactive_command() {
        let result = validate_exec(&exec_args(vec!["echo".into(), "hi".into()], false, false));
        assert_eq!(result, Ok(()));
    }

    #[cfg(target_os = "macos")]
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
        let params = build_session_params(args);
        assert_eq!(params.argv, vec!["echo".to_string()]);
        assert_eq!(params.env, vec!["A=B".to_string()]);
        assert_eq!(
            params.cwd, None,
            "exec sessions run in the broker's default cwd"
        );
        let ws = params.initial_winsize.expect("winsize should be mapped");
        assert_eq!((ws.rows, ws.cols), (24, 80));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn build_session_params_leaves_winsize_unset_when_absent() {
        let params = build_session_params(exec_args(vec!["echo".into()], false, false));
        assert!(params.initial_winsize.is_none());
    }

    #[tokio::test]
    async fn handle_request_pull_of_an_invalid_reference_surfaces_the_parse_error() {
        let resp = as_json(
            handle_request(
                &Request::PullImage {
                    image: "###".into(),
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
        let _x = crate::test_env::EnvVarGuard::set("XDG_CACHE_HOME", d.path().join("cache"));
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
    async fn handle_request_image_lifecycle_round_trips_offline_via_the_caches() {
        let d = tempfile::tempdir().unwrap();
        let _h = crate::test_env::EnvVarGuard::set("HOME", d.path());
        let _x = crate::test_env::EnvVarGuard::set("XDG_CACHE_HOME", d.path().join("cache"));
        let now = Instant::now();

        use sha2::Digest;
        let layer_bytes = b"offline layer".to_vec();
        let layer_digest = format!("sha256:{:x}", sha2::Sha256::digest(&layer_bytes));

        let manifest = oci_client::manifest::OciImageManifest {
            layers: vec![oci_client::manifest::OciDescriptor {
                digest: layer_digest.clone(),
                size: layer_bytes.len() as i64,
                media_type: "application/vnd.oci.image.layer.v1.tar".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let manifest_digest = format!(
            "sha256:{:x}",
            sha2::Sha256::digest(serde_json::to_vec(&manifest).unwrap())
        );
        let reference = format!("registry.example.test/cov/pinned@{manifest_digest}");

        let cache_root = crate::cache::root().unwrap();
        let layer_cache = crate::oci_layer_cache::LayerCache::new(cache_root.join("layers"));
        layer_cache
            .install_from_bytes(&layer_digest, &layer_bytes)
            .unwrap();
        let config = format!(
            r#"{{"architecture":"arm64","os":"linux","rootfs":{{"type":"layers","diff_ids":["{layer_digest}"]}}}}"#
        );
        crate::image::manifest_cache::ManifestCache::new(cache_root.join("manifests"))
            .put(
                &reference,
                &crate::image::manifest_cache::CachedManifest {
                    manifest,
                    manifest_digest: manifest_digest.clone(),
                    config,
                },
            )
            .unwrap();

        let pulled = as_json(
            handle_request(
                &Request::PullImage {
                    image: reference.clone(),
                },
                now,
            )
            .await,
        );
        assert_eq!(pulled["type"], "ImagePulled", "got {pulled}");
        assert_eq!(pulled["image"]["reference"], reference);
        assert_eq!(pulled["image"]["digest"], manifest_digest);
        assert_eq!(pulled["image"]["layers"], 1);
        assert_eq!(pulled["image"]["size_bytes"], layer_bytes.len() as u64);

        let listed = as_json(handle_request(&Request::ListImages, now).await);
        assert_eq!(listed["type"], "ImageList", "got {listed}");
        assert_eq!(listed["images"][0]["reference"], reference);

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
            "the last image's layer blob must be swept with it"
        );

        let pruned = as_json(handle_request(&Request::PruneImages, now).await);
        assert_eq!(pruned["type"], "ImagesPruned", "got {pruned}");
        assert_eq!(pruned["removed"], serde_json::json!([]));
        assert_eq!(pruned["reclaimed_bytes"], 0);
    }
}
