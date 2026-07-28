#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use crate::log;
use std::io;

use anyhow::{Context, Result};
use lns_ipc::WireFrame;
use lns_session::{
    ClientFrame, ServerFrame, SignalKind, Winsize, decode_frame as protocol_decode_frame,
    decode_length_prefix,
};
use tokio::io::AsyncReadExt;
use tokio::sync::mpsc;

mod real;
pub use real::capture_session_exec;
pub use real::capture_session_output;
pub use real::run_session_on_fd;

/// A captured session's two streams, kept apart: the guest writes them down independent fds at arbitrary boundaries, so merging them splices a partial stderr line in front of the next stdout line.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct CapturedStreams {
    pub stdout: String,
    pub stderr: String,
}

pub(super) struct CaptureBuffers {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    max_bytes: usize,
}

impl CaptureBuffers {
    pub(super) fn new(max_bytes: usize) -> Self {
        Self {
            stdout: Vec::new(),
            stderr: Vec::new(),
            max_bytes,
        }
    }

    /// Only stdout is parsed, so only stdout is worth failing over; stderr keeps a bounded tail so a chatty install never loses a finished provision to its own progress output.
    pub(super) fn push(&mut self, frame: &WireFrame) -> Result<()> {
        match frame {
            WireFrame::Stdout(bytes) => {
                anyhow::ensure!(
                    self.stdout.len() + bytes.len() <= self.max_bytes,
                    "capture output exceeded {} bytes",
                    self.max_bytes
                );
                self.stdout.extend_from_slice(bytes);
            }
            WireFrame::Stderr(bytes) => {
                self.stderr.extend_from_slice(bytes);
                let overflow = self.stderr.len().saturating_sub(self.max_bytes);
                if overflow > 0 {
                    self.stderr.drain(..overflow);
                }
            }
            WireFrame::Json(_) => {}
        }
        Ok(())
    }

    pub(super) fn finish(self) -> CapturedStreams {
        CapturedStreams {
            stdout: String::from_utf8_lossy(&self.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&self.stderr).into_owned(),
        }
    }
}

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
    pub hostname: Option<String>,
    pub tty: bool,
    pub stdin: bool,
    pub initial_winsize: Option<Winsize>,
    /// Ask the broker to drop to the guest run-as identity before exec; false for the primary session, whose workload is the supervisor.
    pub confine: bool,
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
) -> Result<Option<i32>> {
    while let Some(frame) = read_one_frame(&mut reader).await? {
        match frame {
            ServerFrame::StdoutBytes(bytes) => {
                if frame_tx.send(WireFrame::Stdout(bytes)).await.is_err() {
                    return Ok(None);
                }
            }
            ServerFrame::StderrBytes(bytes) => {
                if frame_tx.send(WireFrame::Stderr(bytes)).await.is_err() {
                    return Ok(None);
                }
            }
            ServerFrame::ExitStatus(code) => {
                log::debug!(code, "broker reported workload ExitStatus");
                return Ok(Some(code));
            }
        }
    }
    Ok(None)
}

pub(super) fn session_exit_code(outcome: Result<Option<i32>>) -> Result<i32> {
    match outcome {
        Ok(Some(code)) => Ok(code),
        Ok(None) => Err(anyhow::anyhow!(
            "broker session ended before the workload reported an exit status; the guest terminated unexpectedly (crash before exec, or teardown) — re-run with `--log-level debug` to capture the guest console"
        )),
        Err(e) => Err(e.context("reading broker session frames")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lns_session::encode_frame;

    fn framed(frame: &ServerFrame) -> Vec<u8> {
        encode_frame(frame).unwrap()
    }

    #[test]
    fn interleaved_frames_never_splice_stderr_into_a_parsed_stdout_line() {
        // A progress renderer writes a partial line with no trailing newline; merged into one buffer it would eat the marker that follows.
        let mut buffers = CaptureBuffers::new(1024);
        for frame in [
            WireFrame::Stderr(b"downloading node 50%\r".to_vec()),
            WireFrame::Stdout(b"LNS_TOOL node 22.11.0 bin\n".to_vec()),
            WireFrame::Stderr(b"installed".to_vec()),
            WireFrame::Json(lns_ipc::Response::Pong),
        ] {
            buffers.push(&frame).unwrap();
        }
        let captured = buffers.finish();
        assert_eq!(captured.stdout, "LNS_TOOL node 22.11.0 bin\n");
        assert_eq!(captured.stderr, "downloading node 50%\rinstalled");
    }

    #[test]
    fn only_stdout_overflow_fails_the_capture() {
        let mut buffers = CaptureBuffers::new(4);
        buffers.push(&WireFrame::Stdout(b"1234".to_vec())).unwrap();
        let err = buffers
            .push(&WireFrame::Stdout(b"5".to_vec()))
            .expect_err("the parsed stream is bounded hard");
        assert!(err.to_string().contains("exceeded 4 bytes"), "got: {err}");
    }

    #[test]
    fn a_chatty_stderr_keeps_its_tail_instead_of_failing_the_run() {
        // A finished multi-tool install must not be thrown away because the engine printed too much progress.
        let mut buffers = CaptureBuffers::new(4);
        for chunk in [&b"aaaa"[..], b"bbbb", b"cc"] {
            buffers.push(&WireFrame::Stderr(chunk.to_vec())).unwrap();
        }
        buffers
            .push(&WireFrame::Stdout(b"LNS_"[..].to_vec()))
            .unwrap();
        let captured = buffers.finish();
        assert_eq!(captured.stderr, "bbcc", "the newest diagnostics survive");
        assert_eq!(captured.stdout, "LNS_");
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

    #[test]
    fn session_exit_code_passes_through_a_reported_status() {
        assert_eq!(session_exit_code(Ok(Some(0))).unwrap(), 0);
        assert_eq!(session_exit_code(Ok(Some(137))).unwrap(), 137);
    }

    #[test]
    fn session_exit_code_errs_when_no_status_was_reported() {
        let rendered = format!("{:#}", session_exit_code(Ok(None)).unwrap_err());
        assert!(
            rendered.contains("before the workload reported an exit status"),
            "got {rendered}"
        );
        assert!(
            rendered.contains("--log-level debug"),
            "the message must point at the debug console: {rendered}"
        );
    }

    #[test]
    fn session_exit_code_wraps_a_reader_error_with_context() {
        let rendered = format!(
            "{:#}",
            session_exit_code(Err(anyhow::anyhow!("frame decode blew up"))).unwrap_err()
        );
        assert!(
            rendered.contains("reading broker session frames"),
            "missing context: {rendered}"
        );
        assert!(
            rendered.contains("frame decode blew up"),
            "missing cause: {rendered}"
        );
    }

    #[tokio::test]
    async fn forwards_stdout_stderr_and_returns_exit_status() {
        let mut bytes = framed(&ServerFrame::StdoutBytes(b"out".to_vec()));
        bytes.extend(framed(&ServerFrame::StderrBytes(b"err".to_vec())));
        bytes.extend(framed(&ServerFrame::ExitStatus(42)));
        let (tx, mut rx) = mpsc::channel(8);
        let code = read_server_frames(io::Cursor::new(bytes), tx)
            .await
            .unwrap();
        assert!(matches!(rx.recv().await, Some(WireFrame::Stdout(b)) if b == b"out"));
        assert!(matches!(rx.recv().await, Some(WireFrame::Stderr(b)) if b == b"err"));
        assert_eq!(
            code,
            Some(42),
            "an explicit ExitStatus is reported as Some(code)"
        );
    }

    #[tokio::test]
    async fn returns_none_when_stream_closes_without_exit_status() {
        let bytes = framed(&ServerFrame::StdoutBytes(b"out".to_vec()));
        let (tx, _rx) = mpsc::channel(8);
        let code = read_server_frames(io::Cursor::new(bytes), tx)
            .await
            .expect("EOF after a frame ends the loop cleanly");
        assert_eq!(
            code, None,
            "EOF before any ExitStatus must be reported as None, not a fabricated code"
        );
    }

    #[tokio::test]
    async fn stops_with_none_when_stdout_receiver_is_gone() {
        let bytes = framed(&ServerFrame::StdoutBytes(b"out".to_vec()));
        let (tx, rx) = mpsc::channel(8);
        drop(rx);
        let code = read_server_frames(io::Cursor::new(bytes), tx)
            .await
            .expect("dropped stdout receiver ends the loop cleanly");
        assert_eq!(code, None);
    }

    #[tokio::test]
    async fn stops_with_none_when_stderr_receiver_is_gone() {
        let bytes = framed(&ServerFrame::StderrBytes(b"err".to_vec()));
        let (tx, rx) = mpsc::channel(8);
        drop(rx);
        let code = read_server_frames(io::Cursor::new(bytes), tx)
            .await
            .expect("dropped stderr receiver ends the loop cleanly");
        assert_eq!(code, None);
    }
}
