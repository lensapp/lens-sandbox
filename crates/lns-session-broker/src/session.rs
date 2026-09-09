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
pub use real::{handle_session, refuse_session};

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
    pub(crate) cwd: Option<SessionCwd>,
    pub(crate) hostname: Option<String>,
    pub(crate) confinement: Confinement,
    pub(crate) scrub: Vec<String>,
}

/// A declared workdir keeps `-w`'s create-if-missing contract; the identity-home fallback is chdir-only, because creating it would run mkdir as guest root on a passwd-controlled path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SessionCwd {
    Declared(String),
    Fallback(String),
}

/// The run-as identity lns-init resolved at boot, read off the broker's own environment.
pub(crate) struct RunIdentity {
    pub(crate) uid: Option<u32>,
    pub(crate) gid: Option<u32>,
    pub(crate) home: Option<String>,
    pub(crate) user: Option<String>,
}

const ROOT_UID: u32 = 0;

/// What the broker owes a forked workload before `execvp`; `Inherit` keeps the broker's root and full capabilities.
#[derive(Debug, Clone, PartialEq, Eq)]
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

/// Supervisor-internal vars the guest inherits from the kernel cmdline; mirrors `INTERNAL_VAR_PREFIX` in lens-sandbox-core, which scrubs the same set before the supervisor spawns its agent.
const INTERNAL_VAR_PREFIX: &str = "LENS_SANDBOX_";

/// Inherited env keys a confined session must not carry: they hold the relay token and url, which authenticate the supervisor to the host.
pub(crate) fn keys_to_scrub<'a>(
    inherited: impl Iterator<Item = &'a str>,
    confinement: &Confinement,
) -> Vec<String> {
    if matches!(confinement, Confinement::Inherit) {
        return Vec::new();
    }
    inherited
        .filter(|k| k.starts_with(INTERNAL_VAR_PREFIX))
        .map(str::to_string)
        .collect()
}

/// One place turns an OpenSession plus the boot-resolved identity into the child's spec, so the identity, scrub, and cwd decisions cannot drift apart across session modes.
pub(crate) fn build_workload_spec<'a>(
    argv: Vec<String>,
    mut env: Vec<String>,
    cwd: Option<String>,
    hostname: Option<String>,
    confine: bool,
    identity: &RunIdentity,
    inherited: impl Iterator<Item = &'a str>,
) -> WorkloadSpec {
    let confinement = confinement(confine, identity.uid, identity.gid);
    env.extend(identity_env(
        &confinement,
        &env,
        identity.home.as_deref(),
        identity.user.as_deref(),
    ));
    let effective_home = env
        .iter()
        .find_map(|kv| kv.strip_prefix("HOME="))
        .filter(|h| !h.is_empty() && *h != "/")
        .map(str::to_string);
    WorkloadSpec {
        argv,
        cwd: session_cwd(cwd, &confinement, effective_home.as_deref()),
        hostname,
        scrub: keys_to_scrub(inherited, &confinement),
        confinement,
        env,
    }
}

/// A confined session adopts the run-as identity's HOME (runc's rule, `/` when the lookup failed) and, matching the supervisor rather than runc, its USER — unless the session env already declares them.
pub(crate) fn identity_env(
    confinement: &Confinement,
    session_env: &[String],
    home: Option<&str>,
    user: Option<&str>,
) -> Vec<String> {
    if matches!(confinement, Confinement::Inherit) {
        return Vec::new();
    }
    let declares = |key: &str| {
        session_env
            .iter()
            .any(|kv| kv.split_once('=').is_some_and(|(k, _)| k == key))
    };
    let mut extra = Vec::new();
    if !declares("HOME") {
        let home = home.filter(|h| !h.is_empty()).unwrap_or("/");
        extra.push(format!("HOME={home}"));
    }
    if let Some(user) = user.filter(|u| !u.is_empty())
        && !declares("USER")
    {
        extra.push(format!("USER={user}"));
    }
    extra
}

/// Mirrors the supervisor's workdir → home → stay-put fallback so an exec starts where its workload did.
pub(crate) fn session_cwd(
    cwd: Option<String>,
    confinement: &Confinement,
    home: Option<&str>,
) -> Option<SessionCwd> {
    if let Some(declared) = cwd.filter(|c| !c.is_empty()) {
        return Some(SessionCwd::Declared(declared));
    }
    if matches!(confinement, Confinement::Inherit) {
        return None;
    }
    home.map(|h| SessionCwd::Fallback(h.to_string()))
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
    CloseStdin,
    Resize(Winsize),
    Signal(i32),
    Detach,
    Stop,
}

/// The opener declares whether the session dies with its client; a vanished stream then hangs the child up, everything else outlives the connection.
pub(crate) fn eof_action(dies_with_client: bool) -> LoopAction {
    if dies_with_client {
        LoopAction::Detach
    } else {
        LoopAction::Stop
    }
}

pub(crate) fn dispatch_frame(frame: ClientFrame) -> LoopAction {
    match frame {
        ClientFrame::StdinBytes(bytes) => LoopAction::WriteStdin(bytes),
        ClientFrame::StdinClose => LoopAction::CloseStdin,
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

/// A command the sandbox does not have exits 127 and one it has but cannot execute exits 126, so a caller can tell a typo from a file that is there but unusable; execvp reports ENOENT for a missing interpreter too, so NotFound alone is not enough to claim 127.
pub(crate) fn exec_failure_code(kind: io::ErrorKind, command_found: bool) -> i32 {
    if kind == io::ErrorKind::NotFound && !command_found {
        127
    } else {
        126
    }
}

pub(crate) fn command_is_present(
    command: &str,
    path: Option<&str>,
    exists: &dyn Fn(&std::path::Path) -> bool,
) -> bool {
    if command.contains('/') {
        return exists(std::path::Path::new(command));
    }
    path.is_some_and(|entries| {
        entries
            .split(':')
            .any(|dir| exists(&std::path::Path::new(dir).join(command)))
    })
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
    fn a_confined_session_adopts_the_run_as_home_and_user() {
        let extra = identity_env(
            &Confinement::Setuid {
                uid: 1000,
                gid: 1000,
            },
            &["PATH=/bin".into()],
            Some("/home/node"),
            Some("node"),
        );
        assert_eq!(
            extra,
            vec!["HOME=/home/node".to_string(), "USER=node".to_string()],
            "an exec must land in the workload user's home the way its workload did, not in the broker's kernel-given HOME=/"
        );
    }

    #[test]
    fn a_session_declared_home_and_user_outrank_the_run_as_identity() {
        let extra = identity_env(
            &Confinement::Setuid {
                uid: 1000,
                gid: 1000,
            },
            &["HOME=/srv".into(), "USER=svc".into()],
            Some("/home/node"),
            Some("node"),
        );
        assert!(
            extra.is_empty(),
            "an image ENV or `-e` HOME/USER is the author's decision; the identity only fills the gap"
        );
    }

    #[test]
    fn a_confined_session_without_a_resolved_home_falls_back_to_the_root_dir() {
        let extra = identity_env(&Confinement::CapsOnly, &[], None, None);
        assert_eq!(
            extra,
            vec!["HOME=/".to_string()],
            "runc lands on '/' when the passwd lookup fails; a confined exec matches it rather than inheriting broker state"
        );
    }

    #[test]
    fn the_primary_sessions_identity_stays_the_supervisors_business() {
        let extra = identity_env(&Confinement::Inherit, &[], Some("/home/node"), Some("node"));
        assert!(extra.is_empty());
    }

    #[test]
    fn a_confined_session_without_a_workdir_starts_in_the_effective_home_best_effort() {
        let cwd = session_cwd(None, &Confinement::CapsOnly, Some("/home/node"));
        assert_eq!(
            cwd,
            Some(SessionCwd::Fallback("/home/node".into())),
            "the supervisor starts a workdir-less workload in its home, so an exec joining it must land there — but only chdir, never create a root-owned dir at a passwd-controlled path"
        );
    }

    #[test]
    fn a_declared_workdir_outranks_the_home_fallback_and_keeps_its_create_contract() {
        let cwd = session_cwd(
            Some("/app".into()),
            &Confinement::CapsOnly,
            Some("/home/node"),
        );
        assert_eq!(cwd, Some(SessionCwd::Declared("/app".into())));
    }

    #[test]
    fn an_empty_declared_workdir_is_treated_as_absent() {
        let cwd = session_cwd(Some(String::new()), &Confinement::CapsOnly, Some("/home/n"));
        assert_eq!(
            cwd,
            Some(SessionCwd::Fallback("/home/n".into())),
            "Some(\"\") must not reach create_dir_all and kill the session with 126"
        );
    }

    #[test]
    fn the_primary_sessions_cwd_stays_the_supervisors_business() {
        let cwd = session_cwd(None, &Confinement::Inherit, Some("/home/node"));
        assert_eq!(cwd, None);
    }

    #[test]
    fn a_confined_session_with_no_home_keeps_the_brokers_cwd() {
        let cwd = session_cwd(None, &Confinement::CapsOnly, None);
        assert_eq!(cwd, None);
    }

    fn open_fields(cwd: Option<String>) -> (Vec<String>, Vec<String>, Option<String>) {
        (vec!["sh".to_string()], vec!["PATH=/bin".to_string()], cwd)
    }

    #[test]
    fn a_confined_spec_carries_the_full_run_identity() {
        let (argv, env, cwd) = open_fields(None);
        let spec = build_workload_spec(
            argv,
            env,
            cwd,
            None,
            true,
            &RunIdentity {
                uid: Some(1000),
                gid: Some(1000),
                home: Some("/home/node".into()),
                user: Some("node".into()),
            },
            ["LENS_SANDBOX_TOKEN", "PATH"].into_iter(),
        );
        assert_eq!(
            spec.confinement,
            Confinement::Setuid {
                uid: 1000,
                gid: 1000
            }
        );
        assert!(
            spec.env.contains(&"HOME=/home/node".to_string())
                && spec.env.contains(&"USER=node".to_string()),
            "home and user must land in their own keys, not swapped: {:?}",
            spec.env
        );
        assert_eq!(spec.cwd, Some(SessionCwd::Fallback("/home/node".into())));
        assert_eq!(spec.scrub, vec!["LENS_SANDBOX_TOKEN".to_string()]);
    }

    #[test]
    fn a_failed_home_lookup_yields_the_root_home_and_no_cwd_fallback() {
        let (argv, env, cwd) = open_fields(None);
        let spec = build_workload_spec(
            argv,
            env,
            cwd,
            None,
            true,
            &RunIdentity {
                uid: Some(0),
                gid: Some(0),
                home: None,
                user: None,
            },
            std::iter::empty(),
        );
        assert!(spec.env.contains(&"HOME=/".to_string()));
        assert_eq!(
            spec.cwd, None,
            "the '/' sentinel means the lookup failed; chdir'ing to it would launder the failure into a decision"
        );
    }

    #[test]
    fn a_session_declared_home_steers_the_cwd_fallback_too() {
        let (argv, mut env, cwd) = open_fields(None);
        env.push("HOME=/srv".to_string());
        let spec = build_workload_spec(
            argv,
            env,
            cwd,
            None,
            true,
            &RunIdentity {
                uid: Some(1000),
                gid: Some(1000),
                home: Some("/home/node".into()),
                user: Some("node".into()),
            },
            std::iter::empty(),
        );
        assert_eq!(
            spec.cwd,
            Some(SessionCwd::Fallback("/srv".into())),
            "the supervisor's resolve_cwd follows the effective home, declared over identity"
        );
    }

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
            dies_with_client: false,
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
            dies_with_client: false,
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
    fn host_eof_hangs_up_a_session_declared_to_die_with_its_client() {
        assert_eq!(eof_action(true), LoopAction::Detach);
    }

    #[test]
    fn host_eof_leaves_a_shared_sessions_workload_running() {
        assert_eq!(eof_action(false), LoopAction::Stop);
    }

    #[test]
    fn dispatch_frame_maps_each_client_frame() {
        assert_eq!(
            dispatch_frame(ClientFrame::StdinBytes(b"x".to_vec())),
            LoopAction::WriteStdin(b"x".to_vec())
        );
        assert_eq!(
            dispatch_frame(ClientFrame::StdinClose),
            LoopAction::CloseStdin
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
            dies_with_client: false,
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
    fn a_confined_session_scrubs_the_inherited_relay_credentials() {
        let inherited = [
            "LENS_SANDBOX_TOKEN",
            "LENS_SANDBOX_WS_URL",
            "PATH",
            "HOME",
            "LENS_RUN_UID",
        ];
        let scrubbed = keys_to_scrub(inherited.into_iter(), &Confinement::CapsOnly);
        assert_eq!(
            scrubbed,
            vec![
                "LENS_SANDBOX_TOKEN".to_string(),
                "LENS_SANDBOX_WS_URL".to_string()
            ],
            "the relay token authenticates the supervisor to the host, so a confined session must not inherit it — /proc/cmdline is masked for the same reason"
        );
    }

    #[test]
    fn an_unconfined_session_keeps_the_relay_credentials() {
        let scrubbed = keys_to_scrub(["LENS_SANDBOX_TOKEN"].into_iter(), &Confinement::Inherit);
        assert!(
            scrubbed.is_empty(),
            "the primary session's workload is the supervisor, which needs the token to reach the relay at all"
        );
    }

    #[test]
    fn a_setuid_session_scrubs_them_too() {
        let scrubbed = keys_to_scrub(
            ["LENS_SANDBOX_TOKEN"].into_iter(),
            &Confinement::Setuid {
                uid: 65534,
                gid: 65534,
            },
        );
        assert_eq!(scrubbed, vec!["LENS_SANDBOX_TOKEN".to_string()]);
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

    #[test]
    fn a_missing_command_is_127_and_one_that_cannot_run_is_126() {
        assert_eq!(exec_failure_code(io::ErrorKind::NotFound, false), 127);
        for kind in [
            io::ErrorKind::PermissionDenied,
            io::ErrorKind::InvalidData,
            io::ErrorKind::IsADirectory,
        ] {
            assert_eq!(
                exec_failure_code(kind, true),
                126,
                "{kind:?} means the command was found; only a missing one is 127"
            );
        }
    }

    #[test]
    fn a_present_command_whose_interpreter_is_missing_is_126_not_127() {
        assert_eq!(
            exec_failure_code(io::ErrorKind::NotFound, true),
            126,
            "execvp reports ENOENT for a missing shebang interpreter or ELF loader too; the command itself was found, so it is not the 127 case"
        );
    }

    #[test]
    fn a_pathless_command_is_looked_up_through_every_path_entry() {
        let exists = |p: &std::path::Path| p == std::path::Path::new("/usr/local/bin/tool");
        assert!(command_is_present(
            "tool",
            Some("/bin:/usr/local/bin"),
            &exists
        ));
        assert!(!command_is_present("tool", Some("/bin:/sbin"), &exists));
        assert!(
            !command_is_present("tool", None, &exists),
            "no PATH means nowhere to find a pathless command"
        );
    }

    #[test]
    fn an_empty_path_entry_means_the_working_directory_like_execvp() {
        let exists = |p: &std::path::Path| p == std::path::Path::new("tool");
        assert!(command_is_present("tool", Some(":/bin"), &exists));
    }

    #[test]
    fn a_command_with_a_slash_is_checked_directly_and_never_path_searched() {
        let exists = |p: &std::path::Path| p == std::path::Path::new("./tool");
        assert!(command_is_present("./tool", Some("/bin"), &exists));
        assert!(!command_is_present("/bin/tool", Some("/bin"), &exists));
    }
}
