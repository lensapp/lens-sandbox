use std::ffi::CString;
use std::io;
use std::os::fd::RawFd;
use std::os::raw::{c_char, c_long, c_uint};
use std::ptr;
use std::sync::mpsc::SyncSender;
use std::time::Duration;

use lns_session::{ClientFrame, ServerFrame, Winsize, encode_frame};

const PTY_DRAIN_GRACE: Duration = Duration::from_millis(500);

// Bounded wait for the host to read the final frames and close the connection before we let PID 1 power off, so output and the exit frame aren't cut off mid-flight; returns as soon as the host closes (the common case is sub-millisecond).
const HOST_DRAIN_GRACE: Duration = Duration::from_secs(2);

use super::{
    Confinement, LoopAction, SessionError, SessionOutcome, SharedFd, WorkloadSpec, close,
    confinement, dispatch_frame, eof_action, keys_to_scrub, read_client_frame, signal_target,
    validate_argv, validate_open_session,
};
use crate::forker::{Fork, Forker};
use crate::pty;
use crate::vsock;

fn dup(fd: RawFd) -> RawFd {
    // SAFETY: caller-owned fd.
    unsafe { libc::dup(fd) }
}

fn child_exit(code: i32) -> ! {
    // SAFETY: post-fork child only; skips parent's at-exit handlers.
    unsafe { libc::_exit(code) }
}

fn kill(target: libc::pid_t, sig: i32) {
    // SAFETY: kill(2) only reads its args.
    unsafe { libc::kill(target, sig) };
}

fn shutdown_read(fd: RawFd) {
    // SAFETY: caller-owned socket fd.
    unsafe { libc::shutdown(fd, libc::SHUT_RD) };
}

fn writeln_stderr(msg: &str) -> io::Result<()> {
    use std::io::Write;
    writeln!(std::io::stderr(), "lns-session-broker: {msg}")
}

fn env_u32(key: &str) -> Option<u32> {
    std::env::var(key).ok().and_then(|s| s.parse().ok())
}

/// Identity-management caps a confined root workload keeps; mirrors `KEEP_CAP_MASK` in lens-sandbox-core so an exec lands on the same set as the workload it joins.
const KEEP_CAP_MASK: u64 =
    (1 << 0) | (1 << 1) | (1 << 3) | (1 << 4) | (1 << 5) | (1 << 6) | (1 << 7);

const CAPSET_V3: u32 = 0x2008_0522;

#[repr(C)]
struct CapHeader {
    version: u32,
    pid: i32,
}

#[repr(C)]
struct CapData {
    effective: u32,
    permitted: u32,
    inheritable: u32,
}

fn no_new_privs() -> io::Result<()> {
    // SAFETY: prctl with PR_SET_NO_NEW_PRIVS takes scalars only and cannot fail on a live process.
    if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn drop_capabilities() -> io::Result<()> {
    no_new_privs()?;
    // SAFETY: prctl with PR_CAP_AMBIENT_CLEAR_ALL takes scalars only.
    if unsafe {
        libc::prctl(
            libc::PR_CAP_AMBIENT,
            libc::PR_CAP_AMBIENT_CLEAR_ALL,
            0,
            0,
            0,
        )
    } < 0
    {
        return Err(io::Error::last_os_error());
    }
    let header = CapHeader {
        version: CAPSET_V3,
        pid: 0,
    };
    let lo = (KEEP_CAP_MASK & 0xFFFF_FFFF) as u32;
    let hi = (KEEP_CAP_MASK >> 32) as u32;
    let data = [
        CapData {
            effective: lo,
            permitted: lo,
            inheritable: 0,
        },
        CapData {
            effective: hi,
            permitted: hi,
            inheritable: 0,
        },
    ];
    // SAFETY: capset reads a v3 header plus the two 32-bit datums the version mandates; both live until the call returns.
    if unsafe { libc::syscall(libc::SYS_capset, &header as *const _, data.as_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn setuid_to(uid: u32, gid: u32) -> io::Result<()> {
    // SAFETY: each call takes scalars; setgroups before setgid/setuid so the supplementary set is cleared while still privileged.
    unsafe {
        if libc::setgroups(0, ptr::null()) < 0 || libc::setgid(gid) < 0 || libc::setuid(uid) < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    no_new_privs()
}

fn apply_confinement(confinement: &Confinement) -> io::Result<()> {
    match confinement {
        Confinement::Inherit => Ok(()),
        Confinement::CapsOnly => drop_capabilities(),
        Confinement::Setuid { uid, gid } => setuid_to(*uid, *gid),
    }
}

pub fn handle_session(
    conn: RawFd,
    pid_tx: Option<SyncSender<libc::pid_t>>,
    forker: &dyn Forker,
) -> Result<SessionOutcome, SessionError> {
    let opening = match read_client_frame(conn) {
        Some(frame) => frame,
        None => {
            close(conn);
            return Err(SessionError::PeerHangup);
        }
    };
    if let Err(e) = validate_open_session(&opening) {
        close(conn);
        return Err(e);
    }
    let ClientFrame::OpenSession {
        argv,
        env,
        cwd,
        hostname,
        tty,
        stdin,
        winsize,
        confine,
        dies_with_client,
    } = opening
    else {
        unreachable!("validate_open_session guarantees OpenSession");
    };
    let confinement = confinement(confine, env_u32("LENS_RUN_UID"), env_u32("LENS_RUN_GID"));
    let inherited: Vec<String> = std::env::vars().map(|(k, _)| k).collect();
    let spec = WorkloadSpec {
        argv,
        env,
        cwd,
        hostname,
        scrub: keys_to_scrub(inherited.iter().map(String::as_str), &confinement),
        confinement,
    };
    if tty {
        run_tty_session(conn, spec, winsize, stdin, dies_with_client, pid_tx, forker)
    } else {
        run_pipe_session(conn, spec, stdin, dies_with_client, pid_tx, forker)
    }
}

fn run_tty_session(
    conn: RawFd,
    spec: WorkloadSpec,
    winsize: Option<Winsize>,
    _stdin_enabled: bool,
    dies_with_client: bool,
    pid_tx: Option<SyncSender<libc::pid_t>>,
    forker: &dyn Forker,
) -> Result<SessionOutcome, SessionError> {
    let pty = pty::open_pty().map_err(SessionError::Pty)?;
    let pid = match forker.fork() {
        Ok(Fork::Parent(pid)) => pid,
        Ok(Fork::Child) => {
            if let Err(e) = pty::child_setup(pty.slave) {
                let _ = writeln_stderr(&format!("child_setup: {e}"));
                child_exit(127);
            }
            exec_child(&spec);
        }
        Err(err) => {
            close(pty.master);
            close(pty.slave);
            close(conn);
            return Err(SessionError::Io(err));
        }
    };
    if let Some(tx) = pid_tx {
        let _ = tx.send(pid);
    }
    close(pty.slave);
    if let Some(ws) = winsize {
        apply_winsize(pty.master, ws);
    }
    let conn = SharedFd::new(conn);
    let outcome = drive_session_tty(conn.clone(), pty.master, pid, forker, dies_with_client);
    close(pty.master);
    conn.close();
    outcome
}

fn run_pipe_session(
    conn: RawFd,
    spec: WorkloadSpec,
    stdin_enabled: bool,
    dies_with_client: bool,
    pid_tx: Option<SyncSender<libc::pid_t>>,
    forker: &dyn Forker,
) -> Result<SessionOutcome, SessionError> {
    let (stdin_r, stdin_w) = make_pipe()?;
    let (stdout_r, stdout_w) = make_pipe()?;
    let (stderr_r, stderr_w) = make_pipe()?;

    let pid = match forker.fork() {
        Ok(Fork::Parent(pid)) => pid,
        Ok(Fork::Child) => {
            close(stdin_w);
            close(stdout_r);
            close(stderr_r);
            // SAFETY: post-fork child wiring three pipe ends onto stdio.
            let dup_ok = unsafe {
                libc::dup2(stdin_r, 0) >= 0
                    && libc::dup2(stdout_w, 1) >= 0
                    && libc::dup2(stderr_w, 2) >= 0
            };
            if !dup_ok {
                child_exit(127);
            }
            close(stdin_r);
            close(stdout_w);
            close(stderr_w);
            exec_child(&spec);
        }
        Err(err) => {
            for fd in [stdin_r, stdin_w, stdout_r, stdout_w, stderr_r, stderr_w] {
                close(fd);
            }
            close(conn);
            return Err(SessionError::Io(err));
        }
    };
    if let Some(tx) = pid_tx {
        let _ = tx.send(pid);
    }
    close(stdin_r);
    close(stdout_w);
    close(stderr_w);
    if !stdin_enabled {
        close(stdin_w);
    }
    let conn = SharedFd::new(conn);
    let outcome = drive_session_pipes(
        conn.clone(),
        if stdin_enabled { Some(stdin_w) } else { None },
        stdout_r,
        stderr_r,
        pid,
        forker,
        dies_with_client,
    );
    conn.close();
    outcome
}

fn drive_session_tty(
    conn: SharedFd,
    master: RawFd,
    child_pid: libc::pid_t,
    forker: &dyn Forker,
    dies_with_client: bool,
) -> Result<SessionOutcome, SessionError> {
    let stdin_dup = dup(master);
    let ctrl_dup = dup(master);
    let out_dup = dup(master);
    if stdin_dup < 0 || ctrl_dup < 0 || out_dup < 0 {
        return Err(SessionError::Io(io::Error::last_os_error()));
    }

    let (drained_tx, drained_rx) = std::sync::mpsc::channel::<()>();
    let conn_for_out = conn.clone();
    let out_thread = std::thread::spawn(move || {
        pump_fd_to_vsock_stdout(out_dup, conn_for_out);
        let _ = drained_tx.send(());
    });

    let (host_closed_tx, host_closed_rx) = std::sync::mpsc::channel::<()>();
    let conn_for_ctrl = conn.clone();
    let ctrl_thread = std::thread::spawn(move || {
        client_loop_tty(
            conn_for_ctrl,
            stdin_dup,
            ctrl_dup,
            child_pid,
            dies_with_client,
        );
        let _ = host_closed_tx.send(());
    });

    let exit_code = forker.wait(child_pid)?;

    // Wait for the pump to flush the workload's final output, bounded because a daemonized child holding the slave open never EOFs the master and an unbounded join would hang.
    let _ = drained_rx.recv_timeout(PTY_DRAIN_GRACE);

    send_exit(&conn, exit_code);

    let _ = host_closed_rx.recv_timeout(HOST_DRAIN_GRACE);

    shutdown_read(conn.raw());
    let _ = ctrl_thread.join();
    drop(out_thread);

    Ok(SessionOutcome { exit_code })
}

fn drive_session_pipes(
    conn: SharedFd,
    stdin_w: Option<RawFd>,
    stdout_r: RawFd,
    stderr_r: RawFd,
    child_pid: libc::pid_t,
    forker: &dyn Forker,
    dies_with_client: bool,
) -> Result<SessionOutcome, SessionError> {
    let conn_out = conn.clone();
    let out_thread = std::thread::spawn(move || {
        pump_fd_to_vsock_stdout(stdout_r, conn_out);
    });
    let conn_err = conn.clone();
    let err_thread = std::thread::spawn(move || {
        pump_fd_to_vsock_stderr(stderr_r, conn_err);
    });

    let (host_closed_tx, host_closed_rx) = std::sync::mpsc::channel::<()>();
    let conn_in = conn.clone();
    let in_thread = std::thread::spawn(move || {
        client_loop_pipes(conn_in, stdin_w, child_pid, dies_with_client);
        let _ = host_closed_tx.send(());
    });

    let exit_code = forker.wait(child_pid)?;

    let _ = out_thread.join();
    let _ = err_thread.join();

    send_exit(&conn, exit_code);

    let _ = host_closed_rx.recv_timeout(HOST_DRAIN_GRACE);

    shutdown_read(conn.raw());
    let _ = in_thread.join();
    Ok(SessionOutcome { exit_code })
}

fn client_loop_tty(
    conn: SharedFd,
    master_in: RawFd,
    master_ctrl: RawFd,
    child_pid: libc::pid_t,
    dies_with_client: bool,
) {
    loop {
        let Some(frame) = read_client_frame(conn.raw()) else {
            if eof_action(dies_with_client) == LoopAction::Detach {
                deliver_signal(master_ctrl, child_pid, libc::SIGHUP);
            }
            break;
        };
        match dispatch_frame(frame) {
            LoopAction::WriteStdin(bytes) => {
                if !vsock::write_all(master_in, &bytes) {
                    break;
                }
            }
            LoopAction::CloseStdin => {
                let _ = vsock::write_all(master_in, &[0x04]);
            }
            LoopAction::Resize(ws) => apply_winsize(master_ctrl, ws),
            LoopAction::Signal(sig) => deliver_signal(master_ctrl, child_pid, sig),
            LoopAction::Detach => {
                deliver_signal(master_ctrl, child_pid, libc::SIGHUP);
                break;
            }
            LoopAction::Stop => break,
        }
    }
    close(master_in);
    close(master_ctrl);
}

fn client_loop_pipes(
    conn: SharedFd,
    mut stdin_w: Option<RawFd>,
    child_pid: libc::pid_t,
    dies_with_client: bool,
) {
    loop {
        let Some(frame) = read_client_frame(conn.raw()) else {
            if eof_action(dies_with_client) == LoopAction::Detach {
                kill(child_pid, libc::SIGHUP);
            }
            break;
        };
        match dispatch_frame(frame) {
            LoopAction::WriteStdin(bytes) => {
                if let Some(fd) = stdin_w
                    && !vsock::write_all(fd, &bytes)
                {
                    break;
                }
            }
            LoopAction::CloseStdin => {
                if let Some(fd) = stdin_w.take() {
                    close(fd);
                }
            }
            LoopAction::Resize(_) => {
                // no-op: no PTY in pipe mode
            }
            LoopAction::Signal(sig) => kill(child_pid, sig),
            LoopAction::Detach => {
                kill(child_pid, libc::SIGHUP);
                break;
            }
            LoopAction::Stop => break,
        }
    }
    if let Some(fd) = stdin_w {
        close(fd);
    }
}

fn deliver_signal(master: RawFd, child_pid: libc::pid_t, sig: i32) {
    if let Some((target, sig)) = signal_target(pty::foreground_pgrp(master), child_pid, sig) {
        kill(target, sig);
    }
}

fn apply_winsize(master: RawFd, ws: Winsize) {
    let w = libc::winsize {
        ws_row: ws.rows,
        ws_col: ws.cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: TIOCSWINSZ reads *w; master is a caller-owned PTY fd.
    unsafe { libc::ioctl(master, libc::TIOCSWINSZ, &w) };
}

fn pump_fd_to_vsock_stdout(fd: RawFd, conn: SharedFd) {
    pump_fd_into_frame(fd, conn, ServerFrame::StdoutBytes);
    close(fd);
}

fn pump_fd_to_vsock_stderr(fd: RawFd, conn: SharedFd) {
    pump_fd_into_frame(fd, conn, ServerFrame::StderrBytes);
    close(fd);
}

fn pump_fd_into_frame<F>(fd: RawFd, conn: SharedFd, wrap: F)
where
    F: Fn(Vec<u8>) -> ServerFrame,
{
    let mut buf = [0u8; 4096];
    loop {
        // SAFETY: buf is exclusively borrowed for the slice handed to read(2).
        let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut _, buf.len()) };
        if n > 0 {
            let frame = wrap(buf[..n as usize].to_vec());
            let bytes = match encode_frame(&frame) {
                Ok(b) => b,
                Err(_) => break,
            };
            let guard = match conn.write_lock() {
                Some(g) => g,
                None => break,
            };
            if !vsock::write_all(*guard, &bytes) {
                break;
            }
            continue;
        }
        if n == 0 {
            break;
        }
        if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        break;
    }
}

fn send_exit(conn: &SharedFd, code: i32) {
    let frame = ServerFrame::ExitStatus(code);
    if let Ok(bytes) = encode_frame(&frame)
        && let Some(guard) = conn.write_lock()
    {
        let _ = vsock::write_all(*guard, &bytes);
    }
}

fn make_pipe() -> Result<(RawFd, RawFd), SessionError> {
    let mut fds = [0i32; 2];
    // SAFETY: pipe2 writes a pair of fds into the 2-element array; O_CLOEXEC keeps them out of the workload exec.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } < 0 {
        return Err(SessionError::Io(io::Error::last_os_error()));
    }
    Ok((fds[0], fds[1]))
}

fn exec_child(spec: &WorkloadSpec) -> ! {
    // SAFETY: post-dup2 child; raw close_range (no musl binding) drops every fd>=3 so conn/pty.master/listeners never survive execvp.
    unsafe {
        libc::syscall(
            libc::SYS_close_range,
            3 as c_long,
            c_uint::MAX as c_long,
            0 as c_long,
        )
    };
    enter_workdir(spec.cwd.as_deref());
    set_guest_hostname(spec.hostname.as_deref());
    for key in &spec.scrub {
        if let Ok(c) = CString::new(key.as_str()) {
            // SAFETY: unsetenv copies the name; the CString outlives the call.
            unsafe { libc::unsetenv(c.as_ptr()) };
        }
    }
    for kv in &spec.env {
        if let Ok(c) = CString::new(kv.as_str()) {
            let raw = c.into_raw();
            // SAFETY: putenv keeps the pointer; child execs imminently, so the leak ends with the process image.
            unsafe { libc::putenv(raw) };
        }
    }
    if let Err(e) = apply_confinement(&spec.confinement) {
        let _ = writeln_stderr(&format!("confining the session: {e}"));
        child_exit(126);
    }
    let Some(cargs) = validate_argv(&spec.argv) else {
        let _ = writeln_stderr("argv contained interior NUL");
        child_exit(127);
    };
    let mut argv_ptrs: Vec<*const c_char> = cargs.iter().map(|c| c.as_ptr()).collect();
    argv_ptrs.push(ptr::null());
    // SAFETY: argv_ptrs is NULL-terminated; CStrings outlive the call which doesn't return on success.
    unsafe { libc::execvp(argv_ptrs[0], argv_ptrs.as_ptr()) };
    let _ = writeln_stderr(&format!(
        "execvp({:?}): {}",
        spec.argv[0],
        io::Error::last_os_error()
    ));
    child_exit(127);
}

fn set_guest_hostname(hostname: Option<&str>) {
    let Some(hostname) = hostname else { return };
    let Ok(c) = CString::new(hostname) else {
        let _ = writeln_stderr("hostname contained interior NUL");
        child_exit(126);
    };
    // SAFETY: unshare(CLONE_NEWUTS) only detaches this child's UTS namespace so the rename can't leak to other sessions in the guest.
    if unsafe { libc::unshare(libc::CLONE_NEWUTS) } < 0 {
        let _ = writeln_stderr(&format!(
            "unshare(CLONE_NEWUTS): {}",
            io::Error::last_os_error()
        ));
        child_exit(126);
    }
    // SAFETY: c points to hostname bytes without the trailing NUL, and len matches the original byte length.
    if unsafe { libc::sethostname(c.as_ptr(), hostname.len()) } < 0 {
        let _ = writeln_stderr(&format!(
            "sethostname({hostname:?}): {}",
            io::Error::last_os_error()
        ));
        child_exit(126);
    }
}

fn enter_workdir(cwd: Option<&str>) {
    let Some(dir) = cwd else { return };
    let entered = std::fs::create_dir_all(dir).and_then(|()| std::env::set_current_dir(dir));
    if let Err(e) = entered {
        let _ = writeln_stderr(&format!("workdir {dir:?}: {e}"));
        child_exit(126);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lns_session::{ClientFrame, encode_frame};
    use std::sync::mpsc;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    fn unix_socketpair() -> (RawFd, RawFd) {
        let mut fds = [0i32; 2];
        // SAFETY: socketpair writes two fds into the 2-element array.
        let r = unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, fds.as_mut_ptr()) };
        assert!(r == 0, "socketpair failed: {}", io::Error::last_os_error());
        (fds[0], fds[1])
    }

    fn send_open_session(fd: RawFd, argv: Vec<String>) {
        let frame = ClientFrame::OpenSession {
            argv,
            env: Vec::new(),
            cwd: None,
            hostname: None,
            tty: false,
            stdin: false,
            winsize: None,
            confine: false,
            dies_with_client: false,
        };
        let bytes = encode_frame(&frame).expect("encode OpenSession");
        // SAFETY: bytes is borrowed for the duration of the call.
        let n = unsafe { libc::write(fd, bytes.as_ptr() as *const _, bytes.len()) };
        assert_eq!(n as usize, bytes.len(), "short write on opener");
    }

    struct FakeForker {
        fork_calls: Arc<Mutex<u32>>,
        next_pid: libc::pid_t,
        wait_release: Arc<(Mutex<Option<i32>>, std::sync::Condvar)>,
    }

    impl FakeForker {
        fn new(pid: libc::pid_t) -> Self {
            Self {
                fork_calls: Arc::new(Mutex::new(0)),
                next_pid: pid,
                wait_release: Arc::new((Mutex::new(None), std::sync::Condvar::new())),
            }
        }

        fn release_wait(&self, code: i32) {
            let (lock, cvar) = &*self.wait_release;
            *lock.lock().unwrap() = Some(code);
            cvar.notify_all();
        }
    }

    impl Forker for FakeForker {
        fn fork(&self) -> io::Result<Fork> {
            *self.fork_calls.lock().unwrap() += 1;
            Ok(Fork::Parent(self.next_pid))
        }

        fn wait(&self, _pid: libc::pid_t) -> Result<i32, SessionError> {
            let (lock, cvar) = &*self.wait_release;
            let mut guard = lock.lock().unwrap();
            while guard.is_none() {
                guard = cvar.wait(guard).unwrap();
            }
            Ok(guard.expect("released without a code"))
        }
    }

    #[test]
    fn handle_session_publishes_child_pid_before_blocking() {
        let (client, server) = unix_socketpair();
        send_open_session(client, vec!["/bin/echo".into(), "hi".into()]);

        let fake_pid: libc::pid_t = 424242;
        let forker = Arc::new(FakeForker::new(fake_pid));
        let forker_for_thread = forker.clone();

        let (pid_tx, pid_rx) = mpsc::sync_channel::<libc::pid_t>(1);
        let handle = std::thread::spawn(move || {
            let _ = handle_session(server, Some(pid_tx), &*forker_for_thread);
        });

        let pid = pid_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("pid not received within 2s");
        assert_eq!(
            pid, fake_pid,
            "pid_tx must receive the fake forker's pid before any pump runs",
        );
        assert_eq!(
            *forker.fork_calls.lock().unwrap(),
            1,
            "exactly one fork should have occurred",
        );

        forker.release_wait(0);
        close(client);

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !handle.is_finished() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            handle.is_finished(),
            "session handler did not return within 2s of wait-release + client close",
        );
        let _ = handle.join();
    }
}
