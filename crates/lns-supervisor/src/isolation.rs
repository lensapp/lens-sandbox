use std::io;
use std::os::fd::FromRawFd;

pub fn namespace() -> io::Result<std::fs::File> {
    protect_descriptor(lns_session::isolation::NAMESPACE_FD)?;
    // SAFETY: the broker transfers sole ownership of the namespace descriptor at exec.
    Ok(unsafe { std::fs::File::from_raw_fd(lns_session::isolation::NAMESPACE_FD) })
}

pub fn listener() -> io::Result<std::net::TcpListener> {
    protect_descriptor(lns_session::isolation::LISTENER_FD)?;
    // SAFETY: the broker transfers sole ownership of its nonblocking listener at exec.
    Ok(unsafe { std::net::TcpListener::from_raw_fd(lns_session::isolation::LISTENER_FD) })
}

fn protect_descriptor(fd: i32) -> io::Result<()> {
    // SAFETY: fcntl checks the inherited descriptor and prevents inheritance into workloads.
    if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
