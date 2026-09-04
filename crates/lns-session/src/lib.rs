use serde::{Deserialize, Serialize};

pub const BROKER_PORT: u32 = 1029;

pub const EGRESS_ALLOWED_ENV: &str = "LENS_SANDBOX_EGRESS_ALLOWED";

pub const FORWARD_PORT: u32 = 1030;

pub const MAX_FRAME_BYTES: usize = 16 * 1024 * 1024;

/// Where the service stages the pinned public trust store and the broker reads it from; one spelling, or the guest silently keeps no store at all.
pub const STAGED_CA_BUNDLE_PATH: &str = "/.lens/ca-certificates.crt";

/// The store every workload env var names, the broker seeds, and the supervisor appends the proxy CA to; one spelling, or TLS falls back to no roots with no error.
pub const SYSTEM_CA_BUNDLE_PATH: &str = "/etc/ssl/certs/ca-certificates.crt";

/// The proxy the supervisor runs and points its workload at, so the service can point an exec session at the same one; one spelling, or an exec takes a route the workload never takes.
pub const GUEST_PROXY_URL: &str = "http://127.0.0.1:3128";

/// What the proxy variables exclude, so a workload reaching a server it started itself is not sent to a proxy that cannot connect back into the guest; one spelling, or an exec fails a request the workload can make.
pub const GUEST_NO_PROXY: &str = "localhost,127.0.0.1,::1";

/// Where the service stages each `pre-start` script and the supervisor reads them from; one spelling, or the guest runs nothing and reports success.
pub const SCRIPTS_DIR: &str = "/.lens/scripts";

/// The manifest naming every staged script in the order it runs; absent means the run declared none.
pub const SCRIPTS_MANIFEST_PATH: &str = "/.lens/scripts/steps.json";

/// The staged `pre-start` scripts in run order. Encoded as JSON by whichever side writes it, because this pair does not share the session channel's codec.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ScriptManifest {
    pub steps: Vec<ScriptManifestStep>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptManifestStep {
    /// The staged script's guest path, which the supervisor hands to `sh`.
    pub script: String,
    /// Who runs it, resolved in the guest; `None` defers to the run's own run-as identity.
    #[serde(default)]
    pub user: Option<String>,
    /// How a failure and the console identify this script, since a script has no name of its own.
    pub label: String,
}

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
        /// Drop to the guest's run-as identity (or cap a root one) and scrub the relay credentials before exec; only a supervised primary session leaves it false, because its workload is the supervisor.
        confine: bool,
        /// This session exists only for its host client: when the client's stream vanishes the broker hangs the child up instead of leaving it running.
        dies_with_client: bool,
    },
    StdinBytes(Vec<u8>),
    StdinClose,
    Resize(Winsize),
    Signal(SignalKind),
    Detach,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerFrame {
    StdoutBytes(Vec<u8>),
    StderrBytes(Vec<u8>),
    ExitStatus(i32),
    Refused(BrokerExitReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrokerExitReason {
    NoDhcpLease,
}

impl BrokerExitReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NoDhcpLease => "no_dhcp_lease",
        }
    }
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
            dies_with_client: true,
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
            dies_with_client: false,
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
            ServerFrame::Refused(BrokerExitReason::NoDhcpLease),
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
