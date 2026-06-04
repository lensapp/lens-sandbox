use std::net::{SocketAddr, TcpStream};
use std::os::fd::RawFd;
use std::time::Duration;

use super::{AcceptRecovery, classify_accept_error, parse_header};
use crate::vsock::VsockError;

const ACCEPT_BACKOFF: Duration = Duration::from_millis(20);
const WORKLOAD_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

pub fn spawn_listener() {
    std::thread::spawn(|| {
        if let Err(e) = serve() {
            eprintln!("lns-session-broker: port-forward listener exited: {e}");
        }
    });
}

fn serve() -> Result<(), VsockError> {
    let listen_fd = crate::vsock::listen(lns_session::FORWARD_PORT)?;
    loop {
        match crate::vsock::accept(listen_fd) {
            Ok(conn) => {
                std::thread::spawn(move || handle_conn(conn));
            }
            Err(VsockError::Syscall { op, err }) => match classify_accept_error(&err) {
                AcceptRecovery::Stop => return Err(VsockError::Syscall { op, err }),
                AcceptRecovery::Backoff => std::thread::sleep(ACCEPT_BACKOFF),
                AcceptRecovery::Retry => {}
            },
        }
    }
}

fn handle_conn(conn: RawFd) {
    let Some(port) = read_header(conn) else {
        close(conn);
        return;
    };
    let target = SocketAddr::from(([127, 0, 0, 1], port));
    let Ok(stream) = TcpStream::connect_timeout(&target, WORKLOAD_CONNECT_TIMEOUT) else {
        close(conn);
        return;
    };
    splice(conn, stream);
}

fn read_header(conn: RawFd) -> Option<u16> {
    let mut len_buf = [0u8; 4];
    if crate::vsock::read_exact(conn, &mut len_buf) != crate::vsock::ReadOutcome::Full {
        return None;
    }
    let len = lns_session::decode_length_prefix(&len_buf).ok()?;
    let mut body = vec![0u8; len];
    if crate::vsock::read_exact(conn, &mut body) != crate::vsock::ReadOutcome::Full {
        return None;
    }
    parse_header(&body)
}

fn splice(conn: RawFd, stream: std::net::TcpStream) {
    use std::os::fd::AsRawFd;
    let tcp_fd = stream.as_raw_fd();
    let host_to_guest = std::thread::spawn(move || pump(conn, tcp_fd));
    pump(tcp_fd, conn);
    let _ = host_to_guest.join();
    drop(stream);
    close(conn);
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
