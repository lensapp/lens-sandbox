use serde::{Deserialize, Serialize};

pub const BROKER_PORT: u32 = 1029;

pub const FORWARD_PORT: u32 = 1030;

pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForwardHeader {
    pub container_port: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClientFrame {
    OpenSession {
        argv: Vec<String>,
        env: Vec<String>,
        cwd: Option<String>,
        hostname: Option<String>,
        tty: bool,
        stdin: bool,
        winsize: Option<Winsize>,
        /// Drop to the guest's run-as identity (or cap a root one) before exec; the primary session leaves it false so the supervisor keeps the privileges the cage needs.
        confine: bool,
    },
    StdinBytes(Vec<u8>),
    Resize(Winsize),
    Signal(SignalKind),
    Detach,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerFrame {
    StdoutBytes(Vec<u8>),
    StderrBytes(Vec<u8>),
    ExitStatus(i32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Winsize {
    pub rows: u16,
    pub cols: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignalKind {
    Int,
    Term,
    Quit,
    Hup,
    Winch,
    Kill,
}

impl SignalKind {
    pub fn as_libc(self) -> i32 {
        match self {
            Self::Int => 2,
            Self::Term => 15,
            Self::Quit => 3,
            Self::Hup => 1,
            Self::Winch => 28,
            Self::Kill => 9,
        }
    }
}

#[derive(Debug)]
pub enum ProtocolError {
    Codec(postcard::Error),
    FrameTooLarge(usize),
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Codec(e) => write!(f, "postcard codec error: {e}"),
            Self::FrameTooLarge(n) => write!(
                f,
                "frame body of {n} bytes exceeds maximum {MAX_FRAME_BYTES}",
            ),
        }
    }
}

impl std::error::Error for ProtocolError {}

impl From<postcard::Error> for ProtocolError {
    fn from(e: postcard::Error) -> Self {
        Self::Codec(e)
    }
}

pub fn encode_frame<T: Serialize>(frame: &T) -> Result<Vec<u8>, ProtocolError> {
    let body = postcard::to_allocvec(frame)?;
    if body.len() > MAX_FRAME_BYTES {
        return Err(ProtocolError::FrameTooLarge(body.len()));
    }
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

pub fn decode_frame<T: for<'de> Deserialize<'de>>(body: &[u8]) -> Result<T, ProtocolError> {
    Ok(postcard::from_bytes(body)?)
}

pub fn decode_length_prefix(buf: &[u8; 4]) -> Result<usize, ProtocolError> {
    let len = u32::from_be_bytes(*buf) as usize;
    if len > MAX_FRAME_BYTES {
        return Err(ProtocolError::FrameTooLarge(len));
    }
    Ok(len)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_session_round_trip() {
        let frame = ClientFrame::OpenSession {
            argv: vec!["sh".into(), "-c".into(), "echo hi".into()],
            env: vec!["FOO=bar".into()],
            cwd: Some("/app".into()),
            hostname: Some("demo".into()),
            tty: true,
            stdin: true,
            winsize: Some(Winsize { rows: 24, cols: 80 }),
            confine: true,
        };
        let bytes = encode_frame(&frame).expect("encode");
        let len = decode_length_prefix(&bytes[..4].try_into().unwrap()).expect("len");
        assert_eq!(len, bytes.len() - 4);
        let back: ClientFrame = decode_frame(&bytes[4..]).expect("decode");
        assert_eq!(back, frame);
    }

    #[test]
    fn open_session_without_a_cwd_round_trips() {
        let frame = ClientFrame::OpenSession {
            argv: vec!["sh".into()],
            env: Vec::new(),
            cwd: None,
            hostname: None,
            tty: false,
            stdin: false,
            winsize: None,
            confine: false,
        };
        let bytes = encode_frame(&frame).expect("encode");
        let back: ClientFrame = decode_frame(&bytes[4..]).expect("decode");
        assert_eq!(back, frame);
    }

    #[test]
    fn stdin_bytes_round_trip() {
        let frame = ClientFrame::StdinBytes(b"hello".to_vec());
        let bytes = encode_frame(&frame).unwrap();
        let back: ClientFrame = decode_frame(&bytes[4..]).unwrap();
        assert_eq!(back, frame);
    }

    #[test]
    fn resize_round_trip() {
        let frame = ClientFrame::Resize(Winsize {
            rows: 40,
            cols: 120,
        });
        let bytes = encode_frame(&frame).unwrap();
        let back: ClientFrame = decode_frame(&bytes[4..]).unwrap();
        assert_eq!(back, frame);
    }

    #[test]
    fn signal_round_trip() {
        for kind in [
            SignalKind::Int,
            SignalKind::Term,
            SignalKind::Quit,
            SignalKind::Hup,
            SignalKind::Winch,
            SignalKind::Kill,
        ] {
            let frame = ClientFrame::Signal(kind);
            let bytes = encode_frame(&frame).unwrap();
            let back: ClientFrame = decode_frame(&bytes[4..]).unwrap();
            assert_eq!(back, frame);
        }
    }

    #[test]
    fn detach_round_trip() {
        let frame = ClientFrame::Detach;
        let bytes = encode_frame(&frame).unwrap();
        let back: ClientFrame = decode_frame(&bytes[4..]).unwrap();
        assert_eq!(back, frame);
    }

    #[test]
    fn server_frames_round_trip() {
        for frame in [
            ServerFrame::StdoutBytes(b"hello".to_vec()),
            ServerFrame::StderrBytes(b"err".to_vec()),
            ServerFrame::ExitStatus(0),
            ServerFrame::ExitStatus(129),
        ] {
            let bytes = encode_frame(&frame).unwrap();
            let back: ServerFrame = decode_frame(&bytes[4..]).unwrap();
            assert_eq!(back, frame);
        }
    }

    #[test]
    fn signal_kind_libc_numbers_match_posix() {
        assert_eq!(SignalKind::Int.as_libc(), libc::SIGINT);
        assert_eq!(SignalKind::Term.as_libc(), libc::SIGTERM);
        assert_eq!(SignalKind::Quit.as_libc(), libc::SIGQUIT);
        assert_eq!(SignalKind::Hup.as_libc(), libc::SIGHUP);
        assert_eq!(SignalKind::Winch.as_libc(), libc::SIGWINCH);
        assert_eq!(SignalKind::Kill.as_libc(), libc::SIGKILL);
    }

    #[test]
    fn length_prefix_too_large_rejected() {
        let huge = (MAX_FRAME_BYTES as u32 + 1).to_be_bytes();
        let err = decode_length_prefix(&huge).unwrap_err();
        assert!(matches!(err, ProtocolError::FrameTooLarge(_)));
    }

    #[test]
    fn protocol_error_display_renders_both_variants() {
        let too_large = ProtocolError::FrameTooLarge(123).to_string();
        assert!(too_large.contains("frame body of 123 bytes"));
        assert!(too_large.contains("exceeds maximum"));

        let pc_err = postcard::from_bytes::<u32>(&[]).unwrap_err();
        let codec = ProtocolError::Codec(pc_err).to_string();
        assert!(codec.contains("postcard codec error"));
    }

    #[test]
    fn forward_header_round_trip() {
        let header = ForwardHeader {
            container_port: 3003,
        };
        let bytes = encode_frame(&header).unwrap();
        let back: ForwardHeader = decode_frame(&bytes[4..]).unwrap();
        assert_eq!(back, header);
    }

    #[test]
    fn protocol_error_from_postcard_error() {
        let pc_err = postcard::from_bytes::<u32>(&[]).unwrap_err();
        let proto: ProtocolError = pc_err.into();
        assert!(matches!(proto, ProtocolError::Codec(_)));
    }

    #[test]
    fn encode_frame_rejects_oversized_body() {
        let oversize = vec![0u8; MAX_FRAME_BYTES + 16];
        let frame = ClientFrame::StdinBytes(oversize);
        let err = encode_frame(&frame).unwrap_err();
        assert!(matches!(err, ProtocolError::FrameTooLarge(_)));
    }
}
