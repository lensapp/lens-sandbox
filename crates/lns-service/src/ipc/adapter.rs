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

use super::{
    PostPumpAction, PumpOutcome, handle_request, post_pump_action, pump_responses, rfc3339_now,
};
#[cfg(target_os = "macos")]
use super::{build_session_params, validate_exec};

const PEER_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const PEER_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);

pub async fn run_server(
    socket_path: PathBuf,
    shutdown: Arc<Shutdown>,
    started_at: Instant,
) -> anyhow::Result<()> {
    let listener = bind_or_replace_stale(&socket_path).await?;

    loop {
        tokio::select! {
            accept_result = listener.accept() => {
                let (stream, _) = accept_result.context("accept failed")?;
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
    match UnixListener::bind(socket_path) {
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

    UnixListener::bind(socket_path)
        .with_context(|| format!("failed to rebind {}", socket_path.display()))
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
        Request::RunImage(args) => handle_run(stream, args).await,
        Request::ExecImage(args) => handle_exec(stream, args).await,
        Request::BeginIntegrationSignIn { id } => handle_integration_sign_in(stream, id).await,
        other => handle_one_shot(stream, other, shutdown, started_at).await,
    }
}

/// Drives an oauth integration's device sign-in host-side, streaming the verification prompt to the client and persisting the obtained token set for the next run to arm.
async fn handle_integration_sign_in(mut stream: UnixStream, id: String) -> anyhow::Result<()> {
    use crate::oauth::{
        DeviceCode, OauthConfig, RealClock, RealDeviceFlow, SignIn, SignInPivot, run_device_flow,
    };

    let user = lns_policy::integrations::Catalog::load_or_default(
        &lns_policy::integrations::default_integrations_path(),
    )
    .unwrap_or_default();
    let catalog = lns_policy::integrations::effective_integrations(&user);
    let Some(cfg) = catalog
        .iter()
        .find(|i| i.id == id)
        .and_then(|i| i.oauth.as_ref())
        .map(OauthConfig::from)
    else {
        let frame = encode_frame(&Response::OauthSignInFailed {
            reason: format!("{id:?} is not an oauth integration"),
        })?;
        stream.write_all(&frame).await?;
        return Ok(());
    };

    let (code_tx, mut code_rx) = tokio::sync::mpsc::unbounded_channel::<DeviceCode>();
    let flow = run_device_flow(
        &RealDeviceFlow,
        &cfg,
        move |code: &DeviceCode| {
            let _ = code_tx.send(code.clone());
        },
        std::future::pending::<SignInPivot>(),
    );
    tokio::pin!(flow);
    let outcome = loop {
        tokio::select! {
            biased;
            Some(code) = code_rx.recv() => {
                let frame = encode_frame(&Response::OauthVerification {
                    verification_uri: code.verification_uri,
                    user_code: code.user_code,
                    expires_in_secs: code.expires_in.as_secs(),
                })?;
                stream.write_all(&frame).await?;
            }
            res = &mut flow => break res,
        }
    };

    let response = match outcome {
        Ok(SignIn::Completed(token)) => match persist_oauth_token(&id, &token, &RealClock) {
            Ok(()) => Response::OauthSignInComplete,
            Err(e) => Response::OauthSignInFailed {
                reason: format!("storing the token failed: {e}"),
            },
        },
        Ok(SignIn::Denied) => Response::OauthSignInFailed {
            reason: "access was denied".into(),
        },
        Ok(SignIn::Expired) => Response::OauthSignInFailed {
            reason: "the device code expired before authorization".into(),
        },
        Ok(SignIn::Cancelled) => Response::OauthSignInFailed {
            reason: "the sign-in was cancelled".into(),
        },
        // This CLI-driven path streams only the browser dance; it offers no token-paste surface, so a pivot can't arise here.
        Ok(SignIn::Token(_)) => Response::OauthSignInFailed {
            reason: "token fallback is not available over this sign-in path".into(),
        },
        Err(e) => Response::OauthSignInFailed {
            reason: format!("{e:#}"),
        },
    };
    let frame = encode_frame(&response)?;
    stream.write_all(&frame).await?;
    Ok(())
}

fn persist_oauth_token(
    id: &str,
    token: &crate::oauth::TokenSet,
    clock: &dyn crate::oauth::Clock,
) -> std::io::Result<()> {
    use crate::credential_flow::store::{
        CredentialStore, JsonFileCredentialStore, default_credentials_path,
    };
    let store = JsonFileCredentialStore::new(default_credentials_path());
    let mut state = store.load()?;
    state.insert(id.to_string(), crate::oauth::entry_from_token(clock, token));
    store.save(&state)
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

const FRAME_CHAN_BUF: usize = 512;

async fn handle_run(mut stream: UnixStream, args: lns_ipc::RunImageArgs) -> anyhow::Result<()> {
    let (frame_tx, mut frame_rx) = mpsc::channel::<WireFrame>(FRAME_CHAN_BUF);
    let (cancel_tx, cancel_rx) = oneshot::channel::<i32>();

    let run_id = crate::run_registry::allocate_run_id();
    let detached = args.detached;

    #[cfg(target_os = "macos")]
    let (input_tx, input_rx) = mpsc::channel::<crate::vm::session_client::SessionInput>(256);

    let image_label = args
        .image
        .clone()
        .unwrap_or_else(|| "<imageless>".to_string());
    let command_label = args.cmd.join(" ");
    let started_label = rfc3339_now();

    let logs = Arc::new(crate::run_log::RunLogBuffer::default());
    let (task_frame_tx, tee_rx) = mpsc::channel::<WireFrame>(FRAME_CHAN_BUF);
    tokio::spawn(crate::run_log::tee_frames(
        tee_rx,
        logs.clone(),
        frame_tx.clone(),
    ));
    let run_args = args;
    let run_task = tokio::spawn(async move {
        #[cfg(target_os = "macos")]
        {
            crate::run::handle(run_id, run_args, task_frame_tx, input_rx).await;
        }
        #[cfg(not(target_os = "macos"))]
        {
            crate::run::handle(run_id, run_args, task_frame_tx).await;
        }
    });

    crate::run_registry::register(
        run_id,
        crate::run_registry::RunHandle {
            cancel_tx,
            task: run_task,
            #[cfg(target_os = "macos")]
            input_tx: Some(input_tx),
            #[cfg(target_os = "macos")]
            connector: None,
            image: image_label,
            command: command_label,
            started: started_label,
            status: std::sync::Mutex::new(lns_ipc::RunStatus::Running),
            logs,
        },
    );

    drop(frame_tx);

    let handshake: anyhow::Result<()> = async {
        let started_frame =
            encode_frame(&Response::RunStarted { run_id }).context("encoding RunStarted frame")?;
        match timeout(PEER_REQUEST_TIMEOUT, stream.write_all(&started_frame)).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(anyhow::Error::from(e).context("writing RunStarted frame")),
            Err(elapsed) => Err(anyhow::Error::from(elapsed).context("writing RunStarted frame")),
        }
    }
    .await;
    if let Err(e) = handshake {
        crate::run_registry::cancel(run_id);
        crate::run_registry::deregister(run_id);
        return Err(e);
    }

    let outcome = pump_responses(&mut stream, &mut frame_rx, cancel_rx).await?;
    match post_pump_action(&outcome, detached) {
        PostPumpAction::DeregisterOnly => {
            crate::run_registry::deregister(run_id);
        }
        PostPumpAction::BackgroundDrain => {
            if let PumpOutcome::WriteFailed(e) = &outcome {
                log::debug!(run_id, error = %e, "detached: CLI closed stream, draining in background");
            }
            tokio::spawn(async move {
                while frame_rx.recv().await.is_some() {}
                crate::run_registry::deregister(run_id);
            });
        }
        PostPumpAction::CancelAndDeregister => {
            if let PumpOutcome::WriteFailed(e) = &outcome {
                log::debug!(run_id, error = %e, "ipc run stream write failed; cancelling run");
            }
            crate::run_registry::cancel(run_id);
            crate::run_registry::deregister(run_id);
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
async fn handle_exec(mut stream: UnixStream, args: lns_ipc::ExecImageArgs) -> anyhow::Result<()> {
    let target_run_id = args.run_id;
    let Some(connector) = crate::run_registry::connector(target_run_id) else {
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

    let started_frame = encode_frame(&Response::RunStarted {
        run_id: target_run_id,
    })
    .context("encoding RunStarted frame")?;
    if let Err(e) = stream.write_all(&started_frame).await {
        // SAFETY: fd was just taken from the only owner; we drop it here.
        unsafe { libc::close(fd) };
        return Err(anyhow::Error::from(e).context("writing exec RunStarted frame"));
    }

    let (frame_tx, mut frame_rx) = mpsc::channel::<WireFrame>(FRAME_CHAN_BUF);
    let (input_tx, input_rx) = mpsc::channel::<crate::vm::session_client::SessionInput>(256);

    let params = build_session_params(args);

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

    let input_keepalive = input_tx;

    drop(frame_tx);

    let (_dead_cancel_tx, dead_cancel_rx) = oneshot::channel::<i32>();

    let outcome = pump_responses(&mut stream, &mut frame_rx, dead_cancel_rx).await?;
    if let PumpOutcome::WriteFailed(e) = &outcome {
        log::debug!(error = %e, "exec stream write failed; tearing session down");
    }

    drop(input_keepalive);
    session_task.abort();
    let _ = session_task.await;
    Ok(())
}

#[cfg(not(target_os = "macos"))]
async fn handle_exec(mut stream: UnixStream, _args: lns_ipc::ExecImageArgs) -> anyhow::Result<()> {
    let _ = write_error(
        &mut stream,
        "lns exec is macOS-only in this build".to_string(),
    )
    .await;
    Ok(())
}

async fn write_error(stream: &mut UnixStream, message: String) -> anyhow::Result<()> {
    let frame = encode_frame(&Response::Error { message })?;
    stream.write_all(&frame).await?;
    Ok(())
}
