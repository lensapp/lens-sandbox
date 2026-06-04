#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use crate::log;
use std::os::fd::{FromRawFd, OwnedFd, RawFd};
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use anyhow::{Context, Result};
use lns_ipc::WireFrame;
use lns_session::{ClientFrame, encode_frame as protocol_encode};
use tokio::io::AsyncWriteExt;
use tokio::net::UnixStream;
use tokio::sync::{Mutex, mpsc};

use super::{SessionInput, SessionParams, input_to_frame, read_server_frames};

pub async fn run_session_on_fd(
    fd: RawFd,
    params: SessionParams,
    frame_tx: mpsc::Sender<WireFrame>,
    input_rx: mpsc::Receiver<SessionInput>,
) -> Result<i32> {
    // SAFETY: caller transfers ownership of fd; OwnedFd will close it on drop.
    let owned = unsafe { OwnedFd::from_raw_fd(fd) };
    set_nonblocking(&owned).context("set broker vsock fd nonblocking")?;
    let std_stream = std::os::unix::net::UnixStream::from(owned);
    let stream =
        UnixStream::from_std(std_stream).context("wrap broker vsock fd as tokio UnixStream")?;
    run_session(stream, params, frame_tx, input_rx).await
}

async fn run_session(
    stream: UnixStream,
    params: SessionParams,
    frame_tx: mpsc::Sender<WireFrame>,
    input_rx: mpsc::Receiver<SessionInput>,
) -> Result<i32> {
    let (read_half, write_half) = stream.into_split();
    let writer = Arc::new(Mutex::new(write_half));

    log::debug!(
        argv = ?params.argv,
        tty = params.tty,
        stdin = params.stdin,
        "sending OpenSession",
    );
    let open = ClientFrame::OpenSession {
        argv: params.argv,
        env: params.env,
        tty: params.tty,
        stdin: params.stdin,
        winsize: params.initial_winsize,
    };
    send_client_frame(&writer, &open)
        .await
        .context("send OpenSession to broker")?;
    log::debug!("OpenSession sent; waiting for ExitStatus");

    let exit_code = Arc::new(AtomicI32::new(137));
    let (stop_tx, _) = tokio::sync::broadcast::channel::<()>(1);

    let frame_tx_for_reader = frame_tx.clone();
    let exit_for_reader = exit_code.clone();
    let stop_for_reader = stop_tx.clone();
    let reader_handle = tokio::spawn(async move {
        let _ = read_server_frames(read_half, frame_tx_for_reader, exit_for_reader).await;
        // Without an explicit wake-up, detached-run input loops hang because nobody else closes input_rx.
        let _ = stop_for_reader.send(());
    });

    log::info!("SessionReady", "");

    pump_session_input(writer.clone(), input_rx, stop_tx.subscribe()).await;

    let _ = reader_handle.await;
    Ok(exit_code.load(Ordering::SeqCst))
}

async fn pump_session_input(
    writer: Arc<Mutex<tokio::net::unix::OwnedWriteHalf>>,
    mut input_rx: mpsc::Receiver<SessionInput>,
    mut stop_rx: tokio::sync::broadcast::Receiver<()>,
) {
    loop {
        tokio::select! {
            biased;
            _ = stop_rx.recv() => break,
            maybe = input_rx.recv() => {
                let Some(input) = maybe else {
                    let mut guard = writer.lock().await;
                    let _ = guard.shutdown().await;
                    break;
                };
                let frame = input_to_frame(input);
                if let Err(e) = send_client_frame(&writer, &frame).await {
                    log::warn!(error = %format_args!("{e:#}"), "send to broker failed; ending session input");
                    break;
                }
            }
        }
    }
}

async fn send_client_frame(
    writer: &Arc<Mutex<tokio::net::unix::OwnedWriteHalf>>,
    frame: &ClientFrame,
) -> Result<()> {
    let bytes = protocol_encode(frame).context("encode client frame")?;
    let mut guard = writer.lock().await;
    guard.write_all(&bytes).await.context("write client frame")
}

fn set_nonblocking(fd: &OwnedFd) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    let raw = fd.as_raw_fd();
    // SAFETY: fd is borrowed for the duration of the fcntl calls.
    let flags = unsafe { libc::fcntl(raw, libc::F_GETFL) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: fd is borrowed for the duration of the fcntl call.
    let r = unsafe { libc::fcntl(raw, libc::F_SETFL, flags | libc::O_NONBLOCK) };
    if r < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}
