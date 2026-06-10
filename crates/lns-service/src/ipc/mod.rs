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
    DeregisterOnly,
    BackgroundDrain,
    CancelAndDeregister,
}

pub(super) fn post_pump_action(outcome: &PumpOutcome, detached: bool) -> PostPumpAction {
    match outcome {
        PumpOutcome::ExitFrame | PumpOutcome::ChannelClosed => PostPumpAction::DeregisterOnly,
        PumpOutcome::WriteFailed(_) if detached => PostPumpAction::BackgroundDrain,
        PumpOutcome::WriteFailed(_) => PostPumpAction::CancelAndDeregister,
    }
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
        Request::Kill { run_id, signal } => {
            forward_session_input(*run_id, session_input_from_signal(*signal), "Kill").await
        }
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
                }
            }))
        }
        Request::Unknown { method } => Response::Error {
            message: format!("unknown method: {method}"),
        },
    }
}

fn volume_response(result: anyhow::Result<Response>) -> Response {
    result.unwrap_or_else(|e| Response::Error {
        message: format!("{e:#}"),
    })
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
            args.run_id
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
                cpus: 1,
                mem: 0,
                policy_path: None,
                sandbox_user: None,
                sandbox_uid: None,
                cmd: vec![],
                env: vec![],
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
    fn post_pump_action_exit_frame_just_deregisters_regardless_of_detached() {
        assert_eq!(
            post_pump_action(&PumpOutcome::ExitFrame, true),
            PostPumpAction::DeregisterOnly,
        );
        assert_eq!(
            post_pump_action(&PumpOutcome::ExitFrame, false),
            PostPumpAction::DeregisterOnly,
        );
    }

    #[test]
    fn post_pump_action_channel_closed_just_deregisters_regardless_of_detached() {
        assert_eq!(
            post_pump_action(&PumpOutcome::ChannelClosed, true),
            PostPumpAction::DeregisterOnly,
        );
        assert_eq!(
            post_pump_action(&PumpOutcome::ChannelClosed, false),
            PostPumpAction::DeregisterOnly,
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
                image: String::new(),
                command: String::new(),
                started: String::new(),
                status: std::sync::Mutex::new(lns_ipc::RunStatus::Running),
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
                run_id: 999_999,
                signal: lns_ipc::SignalKind::Kill,
            },
            Instant::now(),
        )
        .await;
        assert!(matches!(response, Response::Error { .. }));
    }

    #[tokio::test]
    #[should_panic(expected = "ExecImage must be dispatched via handle_exec")]
    async fn exec_image_via_handle_request_panics() {
        let req = Request::ExecImage(lns_ipc::ExecImageArgs {
            run_id: 1,
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
            image: "test-image".into(),
            command: "".into(),
            started: "1970-01-01T00:00:00Z".into(),
            status: Mutex::new(lns_ipc::RunStatus::Running),
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
            image: "closed-channel-test".into(),
            command: "".into(),
            started: "1970-01-01T00:00:00Z".into(),
            status: Mutex::new(lns_ipc::RunStatus::Running),
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

    #[cfg(target_os = "macos")]
    fn exec_args(argv: Vec<String>, tty: bool, stdin: bool) -> lns_ipc::ExecImageArgs {
        lns_ipc::ExecImageArgs {
            run_id: 42,
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
            run_id: 1,
            argv: vec!["echo".into()],
            env: vec!["A=B".into()],
            tty: false,
            stdin: false,
            initial_winsize: Some((24, 80)),
        };
        let params = build_session_params(args);
        assert_eq!(params.argv, vec!["echo".to_string()]);
        assert_eq!(params.env, vec!["A=B".to_string()]);
        let ws = params.initial_winsize.expect("winsize should be mapped");
        assert_eq!((ws.rows, ws.cols), (24, 80));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn build_session_params_leaves_winsize_unset_when_absent() {
        let params = build_session_params(exec_args(vec!["echo".into()], false, false));
        assert!(params.initial_winsize.is_none());
    }
}
