mod exit;
mod forker;
mod forward;
mod network;
mod pty;
mod session;
mod trust;
mod volumes;
mod vsock;

use std::process::ExitCode;
#[cfg(target_os = "linux")]
use std::sync::{Arc, Mutex};
#[cfg(target_os = "linux")]
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
use lns_session::BROKER_PORT;

fn main() -> ExitCode {
    let code = match run() {
        Ok(code) => exit::clamp_exit(code),
        Err(msg) => {
            eprintln!("lns-session-broker: {msg}");
            1
        }
    };
    // Explicit reboot(RB_POWER_OFF): returning from main as PID 1 would panic the kernel instead of triggering Vz's clean-stop delegate.
    #[cfg(target_os = "linux")]
    {
        eprintln!("lns-session-broker: powering off (workload exit {code})");
        // A synced-but-mounted ext4 keeps its clean bit cleared, so every later attach warns that the filesystem is unchecked.
        volumes::release_volumes();
        // SAFETY: PID 1 final-shutdown call; sync flushes buffers before reboot.
        unsafe { libc::sync() };
        // SAFETY: reboot(RB_POWER_OFF) does not return on success.
        let rc = unsafe { libc::reboot(libc::RB_POWER_OFF) };
        eprintln!("lns-session-broker: reboot(RB_POWER_OFF) returned {rc} unexpectedly");
    }
    ExitCode::from(code)
}

#[cfg(target_os = "linux")]
fn run() -> Result<i32, String> {
    let egress_allowed = network::policy_allows_egress_with(|key| std::env::var(key).ok());
    let network_refusal =
        network::bring_up_network_with(network::bring_up_eth0, egress_allowed).err();
    match &network_refusal {
        Some(reason) => eprintln!("lns-session-broker: {}", reason.explain()),
        // Must run after bring_up_eth0: it consumes the DNS the udhcpc hook stashed.
        None => {
            if let Err(e) = network::configure_dns() {
                eprintln!("lns-session-broker: best-effort DNS setup failed: {e}");
            }
        }
    }

    // Must run before any session forks: the workload inherits env naming the canonical bundle, and the supervisor appends the proxy CA to it.
    match trust::seed_trust_store() {
        Ok(seeding) => {
            if let Some(report) = seeding.report() {
                eprintln!("lns-session-broker: {report}");
            }
        }
        Err(e) => eprintln!("lns-session-broker: best-effort trust store setup failed: {e}"),
    }

    forward::spawn_listener();

    let listen_fd = vsock::listen(BROKER_PORT).map_err(|e| format!("listen: {e}"))?;

    let primary_conn = vsock::accept(listen_fd).map_err(|e| format!("accept(primary): {e}"))?;

    if let Some(reason) = network_refusal {
        let outcome = session::refuse_session(primary_conn, &reason)
            .map_err(|e| format!("refuse primary session: {e}"))?;
        return Ok(outcome.exit_code);
    }

    let exec_sessions: Arc<Mutex<Vec<session::ExecSession>>> = Arc::new(Mutex::new(Vec::new()));

    let exec_sessions_for_thread = exec_sessions.clone();
    let accept_thread = std::thread::spawn(move || {
        accept_exec_loop(listen_fd, exec_sessions_for_thread);
    });

    let forker = forker::LibcForker;
    let primary_outcome = session::handle_session(primary_conn, None, &forker);

    // SAFETY: listen_fd is owned and unused after this block.
    unsafe {
        libc::shutdown(listen_fd, libc::SHUT_RDWR);
        libc::close(listen_fd);
    }
    // Abandon rather than join the exec-accept thread: a blocking AF_VSOCK accept() is not woken by shutdown()/close(), so joining would hang run() short of main()'s sync()+poweroff and the guest would never flush its disks; the imminent poweroff reaps it.
    drop(accept_thread);

    reap_exec_sessions(&exec_sessions);

    match primary_outcome {
        Ok(out) => Ok(out.exit_code),
        Err(e) => Err(format!("primary session: {e}")),
    }
}

#[cfg(not(target_os = "linux"))]
fn run() -> Result<i32, String> {
    Err("lns-session-broker is Linux-only (vsock listener requires AF_VSOCK)".into())
}

#[cfg(target_os = "linux")]
fn accept_exec_loop(
    listen_fd: std::os::fd::RawFd,
    sessions: Arc<Mutex<Vec<session::ExecSession>>>,
) {
    loop {
        let conn = match vsock::accept(listen_fd) {
            Ok(fd) => fd,
            Err(_) => return,
        };
        let pid_slot: Arc<Mutex<Option<libc::pid_t>>> = Arc::new(Mutex::new(None));
        let pid_slot_for_worker = pid_slot.clone();
        let handle = std::thread::spawn(move || {
            run_exec_session(conn, pid_slot_for_worker);
        });
        sessions
            .lock()
            .expect("exec_sessions mutex poisoned")
            .push(session::ExecSession {
                pid: pid_slot,
                handle,
            });
    }
}

#[cfg(target_os = "linux")]
fn run_exec_session(conn: std::os::fd::RawFd, pid_slot: Arc<Mutex<Option<libc::pid_t>>>) {
    let (tx, rx) = std::sync::mpsc::sync_channel::<libc::pid_t>(1);
    let pid_slot_for_bridge = pid_slot;
    std::thread::spawn(move || {
        if let Ok(pid) = rx.recv() {
            *pid_slot_for_bridge.lock().expect("pid_slot mutex poisoned") = Some(pid);
        }
    });
    let forker = forker::LibcForker;
    if let Err(e) = session::handle_session(conn, Some(tx), &forker) {
        eprintln!("lns-session-broker: exec session ended: {e}");
    }
}

#[cfg(target_os = "linux")]
fn reap_exec_sessions(sessions: &Arc<Mutex<Vec<session::ExecSession>>>) {
    let drained: Vec<session::ExecSession> = {
        let mut guard = sessions.lock().expect("exec_sessions mutex poisoned");
        std::mem::take(&mut *guard)
    };
    for s in &drained {
        if let Some(pid) = *s.pid.lock().expect("pid slot mutex poisoned")
            && pid > 0
        {
            // SAFETY: kill(2) only reads its args.
            unsafe { libc::kill(pid, libc::SIGHUP) };
        }
    }
    let deadline = Instant::now() + Duration::from_secs(1);
    let mut remaining: Vec<session::ExecSession> = Vec::new();
    for s in drained {
        if s.handle.is_finished() {
            let _ = s.handle.join();
        } else {
            remaining.push(s);
        }
    }
    while !remaining.is_empty() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
        remaining.retain(|s| !s.handle.is_finished());
    }
    for s in remaining {
        if let Some(pid) = *s.pid.lock().expect("pid slot mutex poisoned")
            && pid > 0
        {
            // SAFETY: kill(2) only reads its args.
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
        let _ = s.handle.join();
    }
}
