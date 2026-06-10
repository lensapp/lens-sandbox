#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use crate::log;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use anyhow::{Context, Result};
use lns_ipc::WireFrame;
use lns_session::{
    ClientFrame, ServerFrame, SignalKind, Winsize, decode_frame as protocol_decode_frame,
    decode_length_prefix,
};
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;

mod real;
#[cfg(target_os = "macos")]
pub use real::capture_session_output;
pub use real::run_session_on_fd;

#[derive(Debug)]
pub enum SessionInput {
    StdinBytes(Vec<u8>),
    Resize { rows: u16, cols: u16 },
    Signal(SignalKind),
}

pub struct SessionParams {
    pub argv: Vec<String>,
    pub env: Vec<String>,
    pub cwd: Option<String>,
    pub tty: bool,
    pub stdin: bool,
    pub initial_winsize: Option<Winsize>,
}

pub(super) fn input_to_frame(input: SessionInput) -> ClientFrame {
    match input {
        SessionInput::StdinBytes(bytes) => ClientFrame::StdinBytes(bytes),
        SessionInput::Resize { rows, cols } => ClientFrame::Resize(Winsize { rows, cols }),
        SessionInput::Signal(kind) => ClientFrame::Signal(kind),
    }
}

async fn read_one_frame<R: tokio::io::AsyncRead + Unpin>(
    reader: &mut R,
) -> Result<Option<ServerFrame>> {
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let len = decode_length_prefix(&len_buf).context("decode server length prefix")?;
    let mut body = vec![0u8; len];
    if let Err(e) = reader.read_exact(&mut body).await {
        if e.kind() == io::ErrorKind::UnexpectedEof {
            return Ok(None);
        }
        return Err(e.into());
    }
    let frame = protocol_decode_frame(&body).context("decode server frame body")?;
    Ok(Some(frame))
}

pub(super) async fn read_server_frames<R: tokio::io::AsyncRead + Unpin>(
    mut reader: R,
    frame_tx: mpsc::Sender<WireFrame>,
    exit_code: Arc<AtomicI32>,
) -> Result<()> {
    while let Some(frame) = read_one_frame(&mut reader).await? {
        match frame {
            ServerFrame::StdoutBytes(bytes) => {
                if frame_tx.send(WireFrame::Stdout(bytes)).await.is_err() {
                    return Ok(());
                }
            }
            ServerFrame::StderrBytes(bytes) => {
                if frame_tx.send(WireFrame::Stderr(bytes)).await.is_err() {
                    return Ok(());
                }
            }
            ServerFrame::ExitStatus(code) => {
                log::debug!(code, "broker reported workload ExitStatus");
                exit_code.store(code, Ordering::SeqCst);
                return Ok(());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use lns_session::encode_frame;

    fn framed(frame: &ServerFrame) -> Vec<u8> {
        encode_frame(frame).unwrap()
    }

    fn exit_cell() -> Arc<AtomicI32> {
        Arc::new(AtomicI32::new(137))
    }

    #[test]
    fn input_to_frame_maps_each_variant() {
        let stdin = input_to_frame(SessionInput::StdinBytes(b"x".to_vec()));
        assert_eq!(stdin, ClientFrame::StdinBytes(b"x".to_vec()));
        let resize = input_to_frame(SessionInput::Resize { rows: 10, cols: 20 });
        assert_eq!(resize, ClientFrame::Resize(Winsize { rows: 10, cols: 20 }));
        let signal = input_to_frame(SessionInput::Signal(SignalKind::Int));
        assert_eq!(signal, ClientFrame::Signal(SignalKind::Int));
    }

    #[tokio::test]
    async fn read_one_frame_decodes_a_valid_frame() {
        let bytes = framed(&ServerFrame::StdoutBytes(b"hi".to_vec()));
        let mut reader = io::Cursor::new(bytes);
        let frame = read_one_frame(&mut reader).await.unwrap();
        assert_eq!(frame, Some(ServerFrame::StdoutBytes(b"hi".to_vec())));
    }

    #[tokio::test]
    async fn read_one_frame_is_none_on_clean_length_eof() {
        let mut reader = io::Cursor::new(Vec::new());
        assert_eq!(read_one_frame(&mut reader).await.unwrap(), None);
    }

    #[tokio::test]
    async fn read_one_frame_is_none_on_body_eof() {
        let mut bytes = (5u32).to_be_bytes().to_vec();
        bytes.extend_from_slice(b"ab"); // only 2 of the promised 5 body bytes
        let mut reader = io::Cursor::new(bytes);
        assert_eq!(read_one_frame(&mut reader).await.unwrap(), None);
    }

    #[tokio::test]
    async fn read_one_frame_propagates_non_eof_length_error() {
        let mut reader = tokio_test::io::Builder::new()
            .read_error(io::Error::other("boom"))
            .build();
        let err = read_one_frame(&mut reader).await.unwrap_err();
        assert!(format!("{err:#}").contains("boom"), "got {err:#}");
    }

    #[tokio::test]
    async fn read_one_frame_propagates_non_eof_body_error() {
        let mut reader = tokio_test::io::Builder::new()
            .read(&(5u32).to_be_bytes())
            .read_error(io::Error::other("body boom"))
            .build();
        let err = read_one_frame(&mut reader).await.unwrap_err();
        assert!(format!("{err:#}").contains("body boom"), "got {err:#}");
    }

    #[tokio::test]
    async fn read_one_frame_rejects_oversize_length_prefix() {
        let bytes = (lns_session::MAX_FRAME_BYTES as u32 + 1)
            .to_be_bytes()
            .to_vec();
        let mut reader = io::Cursor::new(bytes);
        let err = read_one_frame(&mut reader).await.unwrap_err();
        assert!(
            format!("{err:#}").contains("exceeds maximum"),
            "got {err:#}"
        );
    }

    #[tokio::test]
    async fn forwards_stdout_stderr_and_captures_exit() {
        let mut bytes = framed(&ServerFrame::StdoutBytes(b"out".to_vec()));
        bytes.extend(framed(&ServerFrame::StderrBytes(b"err".to_vec())));
        bytes.extend(framed(&ServerFrame::ExitStatus(42)));
        let (tx, mut rx) = mpsc::channel(8);
        let exit = exit_cell();
        read_server_frames(io::Cursor::new(bytes), tx, exit.clone())
            .await
            .unwrap();
        assert!(matches!(rx.recv().await, Some(WireFrame::Stdout(b)) if b == b"out"));
        assert!(matches!(rx.recv().await, Some(WireFrame::Stderr(b)) if b == b"err"));
        assert_eq!(exit.load(Ordering::SeqCst), 42);
    }

    #[tokio::test]
    async fn loop_exits_cleanly_when_stream_closes_without_exit_status() {
        let bytes = framed(&ServerFrame::StdoutBytes(b"out".to_vec()));
        let (tx, _rx) = mpsc::channel(8);
        read_server_frames(io::Cursor::new(bytes), tx, exit_cell())
            .await
            .expect("EOF after a frame ends the loop cleanly");
    }

    #[tokio::test]
    async fn stops_when_stdout_receiver_is_gone() {
        let bytes = framed(&ServerFrame::StdoutBytes(b"out".to_vec()));
        let (tx, rx) = mpsc::channel(8);
        drop(rx);
        read_server_frames(io::Cursor::new(bytes), tx, exit_cell())
            .await
            .expect("dropped stdout receiver ends the loop cleanly");
    }

    #[tokio::test]
    async fn stops_when_stderr_receiver_is_gone() {
        let bytes = framed(&ServerFrame::StderrBytes(b"err".to_vec()));
        let (tx, rx) = mpsc::channel(8);
        drop(rx);
        read_server_frames(io::Cursor::new(bytes), tx, exit_cell())
            .await
            .expect("dropped stderr receiver ends the loop cleanly");
    }
}
