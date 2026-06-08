use std::os::fd::{FromRawFd, OwnedFd, RawFd};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};
use futures_util::stream::SplitSink;
use futures_util::{SinkExt, StreamExt};
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;
use tokio::sync::mpsc;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http::StatusCode;
use tokio_tungstenite::tungstenite::protocol::Message;
use tracing::Instrument;

use crate::approval_flow::protocol::HostFrame;
use crate::approval_flow::session::ApprovalSession;
use crate::credential_flow::session::CredentialSession;
use crate::log;

use super::{
    buffer_frame, current_credentials, handle_inbound, is_expected_close, seed_frames,
    validate_authorization_header,
};

pub(super) async fn accept_loop(
    mut fd_rx: mpsc::UnboundedReceiver<RawFd>,
    session: Arc<ApprovalSession>,
    credential_session: Arc<CredentialSession>,
    mut frame_rx: mpsc::UnboundedReceiver<HostFrame>,
    token: String,
    audit: PathBuf,
    user_env: Vec<String>,
) {
    let mut conn_tx: Option<mpsc::UnboundedSender<HostFrame>> = None;
    let mut pending: Vec<HostFrame> = Vec::new();
    loop {
        tokio::select! {
            biased;
            accepted = fd_rx.recv() => {
                let Some(fd) = accepted else { break };
                let (tx, rx) = mpsc::unbounded_channel::<HostFrame>();
                let seeded = seed_frames(
                    std::mem::take(&mut pending),
                    &session.current_policy(),
                    current_credentials(&credential_session),
                );
                for frame in seeded {
                    let _ = tx.send(frame);
                }
                conn_tx = Some(tx);
                let session_c = session.clone();
                let credential_session_c = credential_session.clone();
                let token_c = token.clone();
                let audit_c = audit.clone();
                let user_env_c = user_env.clone();
                let span = tracing::Span::current();
                tokio::spawn(
                    async move {
                        if let Err(e) = handle_connection(
                            fd,
                            session_c,
                            credential_session_c,
                            rx,
                            token_c,
                            audit_c,
                            user_env_c,
                        )
                        .await
                        {
                            log::warn!(error = %format_args!("{e:#}"), "relay connection ended");
                        }
                    }
                    .instrument(span),
                );
            }
            frame = frame_rx.recv() => {
                let Some(frame) = frame else { break };
                if let Some(tx) = conn_tx.as_ref() {
                    match tx.send(frame) {
                        Ok(()) => {}
                        Err(mpsc::error::SendError(frame)) => {
                            conn_tx = None;
                            buffer_frame(&mut pending, frame);
                        }
                    }
                } else {
                    buffer_frame(&mut pending, frame);
                }
            }
        }
    }
    session.withdraw_run();
    credential_session.withdraw_run();
}

#[allow(clippy::result_large_err)]
async fn handle_connection(
    fd: RawFd,
    session: Arc<ApprovalSession>,
    credential_session: Arc<CredentialSession>,
    mut frame_rx: mpsc::UnboundedReceiver<HostFrame>,
    expected_token: String,
    audit_path: PathBuf,
    user_env: Vec<String>,
) -> Result<()> {
    // SAFETY: `VZVirtioSocketConnection.fileDescriptor` hands us a kernel fd that supports stream read/write.
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    let std_stream = std::os::unix::net::UnixStream::from(owned);
    std_stream
        .set_nonblocking(true)
        .context("set vsock fd nonblocking")?;
    let stream = UnixStream::from_std(std_stream)?;

    let auth_ok = Arc::new(AtomicBool::new(false));
    let auth_ok_cb = auth_ok.clone();
    let expected = format!("Bearer {expected_token}");

    let ws = tokio_tungstenite::accept_hdr_async(
        stream,
        move |req: &Request, response: Response| -> std::result::Result<Response, ErrorResponse> {
            let header = req
                .headers()
                .get("Authorization")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            if validate_authorization_header(header.as_bytes(), expected.as_bytes()) {
                auth_ok_cb.store(true, Ordering::SeqCst);
                Ok(response)
            } else {
                let resp = tokio_tungstenite::tungstenite::http::Response::builder()
                    .status(StatusCode::UNAUTHORIZED)
                    .body(Some("invalid bearer token".to_string()))
                    .expect("static response");
                Err(resp)
            }
        },
    )
    .await
    .context("ws handshake")?;

    if !auth_ok.load(Ordering::SeqCst) {
        anyhow::bail!("ws handshake completed without auth ok");
    }

    let (mut write, mut read) = ws.split();

    let mut audit_file = crate::audit::open_audit_log_async(&audit_path).await?;
    let anchor_path = crate::audit::anchor_path_for(&audit_path);
    let resume_anchor = crate::audit::read_anchor_async(&anchor_path).await;
    let mut chain = lns_ipc::AuditChain::resuming_from_anchor(resume_anchor.as_ref());
    let mut anchor_sink = crate::audit::FileAnchorSink::new(anchor_path);
    let extra_managed: Vec<String> = {
        use crate::credential_flow::providers::Provider;
        credential_session
            .custom_providers()
            .iter()
            .map(|p| p.env_var().to_string())
            .collect()
    };
    super::write_run_env_event(
        &user_env,
        &extra_managed,
        &mut chain,
        &mut audit_file,
        &mut anchor_sink,
    )
    .await?;

    let result = serve(
        &mut write,
        &mut read,
        &session,
        &credential_session,
        &mut frame_rx,
        &mut chain,
        &mut audit_file,
        &mut anchor_sink,
    )
    .await;
    let _ = audit_file.flush().await;
    result
}

#[allow(clippy::too_many_arguments)]
async fn serve(
    write: &mut SplitSink<WebSocketStream<UnixStream>, Message>,
    read: &mut futures_util::stream::SplitStream<WebSocketStream<UnixStream>>,
    session: &Arc<ApprovalSession>,
    credential_session: &Arc<CredentialSession>,
    frame_rx: &mut mpsc::UnboundedReceiver<HostFrame>,
    chain: &mut lns_ipc::AuditChain,
    audit_file: &mut tokio::fs::File,
    anchor_sink: &mut crate::audit::FileAnchorSink,
) -> Result<()> {
    loop {
        tokio::select! {
            outbound = frame_rx.recv() => {
                let Some(frame) = outbound else { break; };
                let json = serde_json::to_string(&frame).context("serialise host frame")?;
                if write.send(Message::Text(json.into())).await.is_err() {
                    break;
                }
            }
            inbound = read.next() => {
                let Some(msg) = inbound else { break; };
                let msg = match msg {
                    Ok(m) => m,
                    Err(e) => {
                        if is_expected_close(&e) {
                            log::debug!("relay closed: {e}");
                        } else {
                            log::warn!(error = %e, "relay read error");
                        }
                        break;
                    }
                };
                if handle_inbound(msg, session, credential_session, chain, audit_file, anchor_sink).await? {
                    break;
                }
            }
        }
    }
    Ok(())
}
