use crate::log;
use std::os::fd::{FromRawFd, OwnedFd, RawFd};
use std::path::PathBuf;

use anyhow::{Context, Result};
use tracing::Instrument;

use super::tee;

pub fn spawn(log_path: PathBuf, echo: bool) -> Result<RawFd> {
    let mut fds = [0i32; 2];
    // SAFETY: socketpair writes two fds into the 2-element array.
    let r = unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, fds.as_mut_ptr()) };
    if r < 0 {
        return Err(
            anyhow::Error::from(std::io::Error::last_os_error()).context("socketpair for hvc0 tee")
        );
    }
    let host_fd = fds[0];
    let guest_fd = fds[1];

    tokio::spawn(tee_loop(host_fd, log_path, echo).instrument(tracing::Span::current()));

    Ok(guest_fd)
}

async fn tee_loop(host_fd: RawFd, log_path: PathBuf, echo: bool) {
    if let Err(e) = run(host_fd, &log_path, echo).await {
        log::warn!(
            error = %format_args!("{e:#}"),
            path = %log_path.display(),
            "diag console tee task failed",
        );
    }
}

async fn run(host_fd: RawFd, log_path: &PathBuf, echo: bool) -> Result<()> {
    // SAFETY: caller transfers ownership of host_fd; OwnedFd will close it on drop.
    let owned = unsafe { OwnedFd::from_raw_fd(host_fd) };
    set_nonblocking(&owned).context("set hvc0 host fd nonblocking")?;
    let std_stream = std::os::unix::net::UnixStream::from(owned);
    let mut stream = tokio::net::UnixStream::from_std(std_stream)
        .context("wrap hvc0 host fd as tokio UnixStream")?;

    if let Some(parent) = log_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("create {}", parent.display()))?;
    }
    let mut log_file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .await
        .with_context(|| format!("opening {}", log_path.display()))?;

    tee(&mut stream, &mut log_file, echo).await
}

fn set_nonblocking(fd: &OwnedFd) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;
    let raw = fd.as_raw_fd();
    // SAFETY: fd is borrowed for the duration of the fcntl calls.
    let flags = unsafe { libc::fcntl(raw, libc::F_GETFL) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: fd is borrowed for the duration of the fcntl call.
    let r = unsafe { libc::fcntl(raw, libc::F_SETFL, flags | libc::O_NONBLOCK) };
    if r < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}
