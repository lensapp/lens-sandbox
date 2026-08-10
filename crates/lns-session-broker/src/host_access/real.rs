use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;

use super::{MANIFEST_PATH, Step, parse_manifest};

/// Executes the staged manifest before the workload starts. An absent manifest means no host capability was granted, which is the common case.
pub fn apply() {
    let Ok(text) = std::fs::read_to_string(MANIFEST_PATH) else {
        return;
    };
    let home = std::env::var("LENS_RUN_HOME").unwrap_or_else(|_| "/root".to_string());
    let ids = run_ids();
    for step in parse_manifest(&text, &home) {
        if let Err(e) = run_step(&step, ids) {
            eprintln!("lns-session-broker: host access step failed: {e} ({step:?})");
        }
    }
}

fn run_ids() -> Option<(u32, u32)> {
    Some((env_u32("LENS_RUN_UID")?, env_u32("LENS_RUN_GID")?))
}

fn env_u32(key: &str) -> Option<u32> {
    std::env::var(key).ok()?.parse().ok()
}

fn run_step(step: &Step, ids: Option<(u32, u32)>) -> std::io::Result<()> {
    match step {
        Step::Dir { target, mode } => {
            std::fs::create_dir_all(target)?;
            std::fs::set_permissions(target, PermissionsExt::from_mode(*mode))?;
            chown(Path::new(target), ids);
            Ok(())
        }
        Step::File {
            staged,
            target,
            mode,
        } => {
            let bytes = std::fs::read(staged)?;
            ensure_parent(target, ids)?;
            std::fs::write(target, bytes)?;
            std::fs::set_permissions(target, PermissionsExt::from_mode(*mode))?;
            chown(Path::new(target), ids);
            Ok(())
        }
        Step::Socket { target, port } => {
            ensure_parent(target, ids)?;
            let listener = bind_socket(target, ids)?;
            let port = *port;
            std::thread::spawn(move || serve(listener, port));
            Ok(())
        }
    }
}

fn ensure_parent(target: &str, ids: Option<(u32, u32)>) -> std::io::Result<()> {
    if let Some(parent) = Path::new(target).parent() {
        std::fs::create_dir_all(parent)?;
        chown(parent, ids);
    }
    Ok(())
}

/// The socket is created 0600 and owned by the run-as user: the workload must reach it, and nothing else in the guest should. The mask is set before `bind` so no window exists in which a wider mode is reachable.
fn bind_socket(target: &str, ids: Option<(u32, u32)>) -> std::io::Result<UnixListener> {
    let _ = std::fs::remove_file(target);
    let previous = umask(0o177);
    let listener = UnixListener::bind(target);
    umask(previous);
    let listener = listener?;
    std::fs::set_permissions(target, PermissionsExt::from_mode(0o600))?;
    chown(Path::new(target), ids);
    Ok(listener)
}

fn umask(mask: libc::mode_t) -> libc::mode_t {
    // SAFETY: umask(2) only swaps this process's file-mode creation mask.
    unsafe { libc::umask(mask) }
}

fn chown(path: &Path, ids: Option<(u32, u32)>) {
    let Some((uid, gid)) = ids else {
        return;
    };
    let Ok(c_path) = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()) else {
        return;
    };
    // SAFETY: c_path is a live NUL-terminated string for the duration of the call.
    unsafe { libc::lchown(c_path.as_ptr(), uid, gid) };
}

fn serve(listener: UnixListener, port: u32) {
    for conn in listener.incoming() {
        let Ok(conn) = conn else {
            continue;
        };
        std::thread::spawn(move || handle_conn(conn, port));
    }
}

fn handle_conn(workload: UnixStream, port: u32) {
    let Ok(host) = crate::vsock::connect(port) else {
        // The workload sees a closed socket, which its client reports as an agent error.
        return;
    };
    splice(workload.as_raw_fd(), host);
    drop(workload);
    close(host);
}

fn splice(workload_fd: RawFd, host_fd: RawFd) {
    let to_host = std::thread::spawn(move || pump(workload_fd, host_fd));
    pump(host_fd, workload_fd);
    let _ = to_host.join();
}

fn pump(src: RawFd, dst: RawFd) {
    let mut buf = [0u8; 16 * 1024];
    loop {
        // SAFETY: buf is exclusively borrowed for the slice handed to read(2).
        let n = unsafe { libc::read(src, buf.as_mut_ptr() as *mut _, buf.len()) };
        if n < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        if n <= 0 {
            // SAFETY: dst is a live fd; shutdown signals EOF to the peer's reader.
            unsafe { libc::shutdown(dst, libc::SHUT_WR) };
            return;
        }
        if !crate::vsock::write_all(dst, &buf[..n as usize]) {
            // SAFETY: dst is a live fd; full shutdown unblocks the paired pump's read.
            unsafe { libc::shutdown(dst, libc::SHUT_RDWR) };
            return;
        }
    }
}

fn close(fd: RawFd) {
    // SAFETY: fd is owned by this connection and unused after close.
    unsafe { libc::close(fd) };
}
