use std::io::Read;
use std::os::fd::{AsRawFd, RawFd};

use super::{Local, Spec, parse_cmdline};
use crate::vsock::VsockError;

const CMDLINE: &str = "/proc/cmdline";

pub fn spawn_listeners() {
    let mut cmdline = String::new();
    if let Ok(mut f) = std::fs::File::open(CMDLINE) {
        let _ = f.read_to_string(&mut cmdline);
    }
    for spec in parse_cmdline(&cmdline) {
        std::thread::spawn(move || {
            if let Err(e) = serve(&spec) {
                eprintln!(
                    "lns-session-broker: revforward {:?} exited: {e}",
                    spec.local
                );
            }
        });
    }
}

fn serve(spec: &Spec) -> Result<(), VsockError> {
    match &spec.local {
        Local::Tcp { addr } => serve_tcp(addr, spec.host_port),
        Local::Unix { path, mode } => serve_unix(path, *mode, spec.host_port),
    }
}

fn serve_tcp(addr: &str, host_port: u32) -> Result<(), VsockError> {
    let listener = std::net::TcpListener::bind(addr).map_err(|err| VsockError::Syscall {
        op: format!("bind(tcp {addr})"),
        err,
    })?;
    for conn in listener.incoming() {
        match conn {
            Ok(stream) => spawn_bridge(stream.as_raw_fd(), host_port, Box::new(stream)),
            Err(e) if recoverable(&e) => continue,
            Err(err) => {
                return Err(VsockError::Syscall {
                    op: format!("accept(tcp {addr})"),
                    err,
                });
            }
        }
    }
    Ok(())
}

fn serve_unix(path: &str, mode: u32, host_port: u32) -> Result<(), VsockError> {
    let _ = std::fs::remove_file(path);
    if let Some(parent) = std::path::Path::new(path).parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let listener =
        std::os::unix::net::UnixListener::bind(path).map_err(|err| VsockError::Syscall {
            op: format!("bind(unix {path})"),
            err,
        })?;
    set_mode(path, mode);
    for conn in listener.incoming() {
        match conn {
            Ok(stream) => spawn_bridge(stream.as_raw_fd(), host_port, Box::new(stream)),
            Err(e) if recoverable(&e) => continue,
            Err(err) => {
                return Err(VsockError::Syscall {
                    op: format!("accept(unix {path})"),
                    err,
                });
            }
        }
    }
    Ok(())
}

fn set_mode(path: &str, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
}

fn recoverable(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::Interrupted
    )
}

/// `keepalive` owns the accepted socket for as long as the bridge runs; the raw fd is spliced, and dropping the owner at the end closes it.
fn spawn_bridge(local_fd: RawFd, host_port: u32, keepalive: Box<dyn Send>) {
    std::thread::spawn(move || {
        let _owner = keepalive;
        match crate::vsock::connect_host(host_port) {
            Ok(host_fd) => {
                splice(local_fd, host_fd);
                close(host_fd);
            }
            Err(e) => eprintln!("lns-session-broker: revforward dial host:{host_port}: {e}"),
        }
    });
}

fn splice(local_fd: RawFd, host_fd: RawFd) {
    let up = std::thread::spawn(move || pump(local_fd, host_fd));
    pump(host_fd, local_fd);
    let _ = up.join();
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
    // SAFETY: fd is owned by this bridge and unused after close.
    unsafe { libc::close(fd) };
}
