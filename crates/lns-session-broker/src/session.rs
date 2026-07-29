#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::ffi::CString;
use std::io;
use std::os::fd::RawFd;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use lns_session::{ClientFrame, Winsize, decode_frame, decode_length_prefix};

#[cfg(target_os = "linux")]
mod real;
#[cfg(target_os = "linux")]
pub use real::handle_session;

pub(crate) fn close(fd: RawFd) {
    // SAFETY: caller owns fd.
    unsafe { libc::close(fd) };
}

pub struct SessionOutcome {
    pub exit_code: i32,
}

pub(crate) struct WorkloadSpec {
    pub(crate) argv: Vec<String>,
    pub(crate) env: Vec<String>,
    pub(crate) cwd: Option<String>,
    pub(crate) hostname: Option<String>,
    pub(crate) confinement: Confinement,
}

const ROOT_UID: u32 = 0;

/// What the broker owes a forked workload before `execvp`; `Inherit` keeps the broker's root and full capabilities.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Confinement {
    Inherit,
    CapsOnly,
    Setuid { uid: u32, gid: u32 },
}

pub(crate) fn confinement(confine: bool, uid: Option<u32>, gid: Option<u32>) -> Confinement {
    if !confine {
        return Confinement::Inherit;
    }
    match (uid, gid) {
        (Some(uid), Some(gid)) if uid != ROOT_UID => Confinement::Setuid { uid, gid },
        _ => Confinement::CapsOnly,
    }
}

#[derive(Debug)]
pub enum SessionError {
    Io(io::Error),
    Protocol(String),
    Pty(crate::pty::PtyError),
    PeerHangup,
}

impl std::fmt::Display for SessionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "io: {e}"),
            Self::Protocol(s) => write!(f, "protocol: {s}"),
            Self::Pty(e) => write!(f, "pty: {e}"),
            Self::PeerHangup => write!(f, "peer hung up before opening session"),
        }
    }
}

impl std::error::Error for SessionError {}

pub(crate) fn validate_open_session(frame: &ClientFrame) -> Result<(), SessionError> {
    match frame {
        ClientFrame::OpenSession { argv, .. } if argv.is_empty() => {
            Err(SessionError::Protocol("OpenSession.argv empty".into()))
        }
        ClientFrame::OpenSession { .. } => Ok(()),
        other => Err(SessionError::Protocol(format!(
            "expected OpenSession, got {other:?}"
        ))),
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LoopAction {
    WriteStdin(Vec<u8>),
    Resize(Winsize),
    Signal(i32),
    Detach,
    Stop,
}

pub(crate) fn dispatch_frame(frame: ClientFrame) -> LoopAction {
    match frame {
        ClientFrame::StdinBytes(bytes) => LoopAction::WriteStdin(bytes),
        ClientFrame::Resize(ws) => LoopAction::Resize(ws),
        ClientFrame::Signal(kind) => LoopAction::Signal(kind.as_libc()),
        ClientFrame::Detach => LoopAction::Detach,
        ClientFrame::OpenSession { .. } => LoopAction::Stop,
    }
}

pub(crate) fn signal_target(
    foreground_pgrp: Option<libc::pid_t>,
    child_pid: libc::pid_t,
    sig: i32,
) -> Option<(libc::pid_t, i32)> {
    if sig <= 0 || child_pid <= 0 {
        return None;
    }
    let pgrp = foreground_pgrp.unwrap_or(child_pid);
    Some((-pgrp, sig))
}

pub(crate) fn validate_argv(argv: &[String]) -> Option<Vec<CString>> {
    let cargs: Vec<CString> = argv
        .iter()
        .filter_map(|s| CString::new(s.as_str()).ok())
        .collect();
    (cargs.len() == argv.len()).then_some(cargs)
}

pub fn read_client_frame(fd: RawFd) -> Option<ClientFrame> {
    let mut len_buf = [0u8; 4];
    if crate::vsock::read_exact(fd, &mut len_buf) != crate::vsock::ReadOutcome::Full {
        return None;
    }
    let len = decode_length_prefix(&len_buf).ok()?;
    let mut body = vec![0u8; len];
    if crate::vsock::read_exact(fd, &mut body) != crate::vsock::ReadOutcome::Full {
        return None;
    }
    decode_frame::<ClientFrame>(&body).ok()
}

#[derive(Clone)]
pub(crate) struct SharedFd(Arc<SharedFdInner>);

struct SharedFdInner {
    fd: Mutex<RawFd>,
}

impl SharedFd {
    pub(crate) fn new(fd: RawFd) -> Self {
        Self(Arc::new(SharedFdInner { fd: Mutex::new(fd) }))
    }

    pub(crate) fn raw(&self) -> RawFd {
        *self.0.fd.lock().expect("SharedFd mutex poisoned")
    }

    pub(crate) fn write_lock(&self) -> Option<std::sync::MutexGuard<'_, RawFd>> {
        let guard = self.0.fd.lock().expect("SharedFd mutex poisoned");
        if *guard < 0 { None } else { Some(guard) }
    }

    pub(crate) fn close(self) {
        let mut guard = self.0.fd.lock().expect("SharedFd mutex poisoned");
        if *guard >= 0 {
            close(*guard);
            *guard = -1;
        }
    }
}

pub struct ExecSession {
    pub pid: Arc<Mutex<Option<libc::pid_t>>>,
    pub handle: JoinHandle<()>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use lns_session::{SignalKind, encode_frame};
    use std::os::fd::{AsRawFd, IntoRawFd};

    #[test]
    fn session_error_display_covers_each_variant() {
        assert_eq!(
            format!("{}", SessionError::Io(io::Error::other("boom"))),
            "io: boom"
        );
        assert_eq!(
            format!("{}", SessionError::Protocol("nope".into())),
            "protocol: nope"
        );
        assert_eq!(
            format!(
                "{}",
                SessionError::Pty(crate::pty::PtyError::InteriorNul("x"))
            ),
            "pty: interior NUL in x"
        );
        assert!(format!("{}", SessionError::PeerHangup).contains("hung up"));
    }

    #[test]
    fn validate_open_session_accepts_a_well_formed_opener() {
        let frame = ClientFrame::OpenSession {
            argv: vec!["/bin/sh".into()],
            env: vec!["A=1".into()],
            cwd: Some("/app".into()),
            hostname: Some("demo".into()),
            tty: true,
            stdin: false,
            winsize: None,
            confine: false,
        };
        assert!(validate_open_session(&frame).is_ok());
    }

    #[test]
    fn validate_open_session_rejects_empty_argv() {
        let frame = ClientFrame::OpenSession {
            argv: Vec::new(),
            env: Vec::new(),
            cwd: None,
            hostname: None,
            tty: false,
            stdin: false,
            winsize: None,
            confine: false,
        };
        let err = validate_open_session(&frame).unwrap_err();
        assert!(matches!(&err, SessionError::Protocol(s) if s.contains("argv empty")));
    }

    #[test]
    fn validate_open_session_rejects_a_non_opener() {
        let err = validate_open_session(&ClientFrame::Detach).unwrap_err();
        assert!(matches!(&err, SessionError::Protocol(s) if s.contains("expected OpenSession")));
    }

    #[test]
    fn dispatch_frame_maps_each_client_frame() {
        assert_eq!(
            dispatch_frame(ClientFrame::StdinBytes(b"x".to_vec())),
            LoopAction::WriteStdin(b"x".to_vec())
        );
        assert_eq!(
            dispatch_frame(ClientFrame::Resize(Winsize { rows: 1, cols: 2 })),
            LoopAction::Resize(Winsize { rows: 1, cols: 2 })
        );
        assert_eq!(
            dispatch_frame(ClientFrame::Signal(SignalKind::Int)),
            LoopAction::Signal(SignalKind::Int.as_libc())
        );
        assert_eq!(dispatch_frame(ClientFrame::Detach), LoopAction::Detach);
        let opener = ClientFrame::OpenSession {
            argv: Vec::new(),
            env: Vec::new(),
            cwd: None,
            hostname: None,
            tty: false,
            stdin: false,
            winsize: None,
            confine: false,
        };
        assert_eq!(dispatch_frame(opener), LoopAction::Stop);
    }

    #[test]
    fn signal_target_guards_nonpositive_inputs() {
        assert_eq!(signal_target(None, 5, 0), None);
        assert_eq!(signal_target(None, 5, -1), None);
        assert_eq!(signal_target(Some(7), 0, 9), None);
    }

    #[test]
    fn signal_target_uses_foreground_pgrp_then_falls_back_to_child() {
        assert_eq!(signal_target(Some(7), 5, 9), Some((-7, 9)));
        assert_eq!(signal_target(None, 5, 9), Some((-5, 9)));
    }

    #[test]
    fn an_unconfined_session_inherits_the_brokers_privileges() {
        assert_eq!(
            confinement(false, Some(65534), Some(65534)),
            Confinement::Inherit,
            "the primary session execs the supervisor, which needs root and CAP_NET_ADMIN to install the cage"
        );
    }

    #[test]
    fn a_confined_session_setuids_to_the_run_as_ids() {
        assert_eq!(
            confinement(true, Some(65534), Some(65534)),
            Confinement::Setuid {
                uid: 65534,
                gid: 65534
            },
            "an exec must land on the same identity as the workload it joins, not the broker's root"
        );
    }

    #[test]
    fn a_confined_session_on_a_root_workload_drops_capabilities_instead() {
        assert_eq!(
            confinement(true, Some(ROOT_UID), Some(0)),
            Confinement::CapsOnly,
            "setuid(0) is a no-op that would leave CAP_NET_ADMIN in place, so a root workload's exec caps instead"
        );
    }

    #[test]
    fn a_confined_session_without_run_as_ids_still_drops_capabilities() {
        assert_eq!(confinement(true, None, None), Confinement::CapsOnly);
        assert_eq!(confinement(true, Some(1000), None), Confinement::CapsOnly);
        assert_eq!(confinement(true, None, Some(1000)), Confinement::CapsOnly);
    }

    #[test]
    fn validate_argv_accepts_clean_args_and_rejects_interior_nul() {
        let ok = validate_argv(&["echo".into(), "hi".into()]).expect("clean argv");
        assert_eq!(ok.len(), 2);
        assert!(validate_argv(&["ec\0ho".into()]).is_none());
    }

    fn pair() -> (
        std::os::unix::net::UnixStream,
        std::os::unix::net::UnixStream,
    ) {
        std::os::unix::net::UnixStream::pair().expect("socketpair")
    }

    #[test]
    fn read_client_frame_decodes_a_full_frame() {
        let (mut writer, reader) = pair();
        let frame = ClientFrame::StdinBytes(b"hello".to_vec());
        std::io::Write::write_all(&mut writer, &encode_frame(&frame).unwrap()).unwrap();
        let got = read_client_frame(reader.as_raw_fd());
        assert_eq!(got, Some(ClientFrame::StdinBytes(b"hello".to_vec())));
    }

    #[test]
    fn read_client_frame_is_none_on_peer_hangup() {
        let (writer, reader) = pair();
        drop(writer);
        assert_eq!(read_client_frame(reader.as_raw_fd()), None);
    }

    #[test]
    fn read_client_frame_is_none_when_body_truncated() {
        let (mut writer, reader) = pair();
        // Promise 8 body bytes but send only 3, then hang up.
        let mut bytes = (8u32).to_be_bytes().to_vec();
        bytes.extend_from_slice(b"abc");
        std::io::Write::write_all(&mut writer, &bytes).unwrap();
        drop(writer);
        assert_eq!(read_client_frame(reader.as_raw_fd()), None);
    }

    #[test]
    fn read_client_frame_is_none_on_undecodable_body() {
        let (mut writer, reader) = pair();
        // Valid length prefix, but a body that isn't a postcard ClientFrame.
        let mut bytes = (3u32).to_be_bytes().to_vec();
        bytes.extend_from_slice(&[0xff, 0xff, 0xff]);
        std::io::Write::write_all(&mut writer, &bytes).unwrap();
        drop(writer);
        assert_eq!(read_client_frame(reader.as_raw_fd()), None);
    }

    #[test]
    fn shared_fd_tracks_open_then_closed_state() {
        let (a, _b) = pair();
        let fd = a.into_raw_fd();
        let shared = SharedFd::new(fd);
        assert_eq!(shared.raw(), fd);
        assert!(shared.write_lock().is_some());
        let clone = shared.clone();
        clone.close();
        // After close the fd is sentinel -1 and write_lock refuses to hand it out.
        assert_eq!(shared.raw(), -1);
        assert!(shared.write_lock().is_none());
        // Closing again is a no-op (fd already -1).
        shared.close();
    }
}
