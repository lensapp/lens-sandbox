use std::io;
use std::os::fd::RawFd;

use libc::c_int;

use super::{AF_VSOCK, ReadOutcome, SockaddrVm, VsockSyscalls, read_exact_with};
#[cfg(target_os = "linux")]
use super::{VsockError, accept_with, listen_with, write_all_with};
use crate::session::close as session_close;

struct RealVsockSyscalls;

impl VsockSyscalls for RealVsockSyscalls {
    fn socket(&self) -> io::Result<RawFd> {
        // SAFETY: socket(2) only reads its int args; SOCK_CLOEXEC keeps the listener out of the workload.
        #[cfg(target_os = "linux")]
        let fd = unsafe { libc::socket(AF_VSOCK, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
        // SAFETY: socket(2) only reads its int args; this vsock path never runs on the host build.
        #[cfg(not(target_os = "linux"))]
        let fd = unsafe { libc::socket(AF_VSOCK, libc::SOCK_STREAM, 0) };
        if fd < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(fd)
        }
    }

    fn bind(&self, fd: RawFd, addr: &SockaddrVm) -> io::Result<()> {
        // SAFETY: addr is a fully-initialised SockaddrVm of the size we pass.
        let r = unsafe {
            libc::bind(
                fd,
                addr as *const SockaddrVm as *const libc::sockaddr,
                std::mem::size_of::<SockaddrVm>() as libc::socklen_t,
            )
        };
        if r < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    fn connect(&self, fd: RawFd, addr: &SockaddrVm) -> io::Result<()> {
        // SAFETY: addr is a fully-initialised SockaddrVm of the size we pass.
        let r = unsafe {
            libc::connect(
                fd,
                addr as *const SockaddrVm as *const libc::sockaddr,
                std::mem::size_of::<SockaddrVm>() as libc::socklen_t,
            )
        };
        if r < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    fn listen(&self, fd: RawFd, backlog: c_int) -> io::Result<()> {
        // SAFETY: fd is the owned listening socket.
        let r = unsafe { libc::listen(fd, backlog) };
        if r < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    fn accept_once(&self, listen_fd: RawFd) -> io::Result<RawFd> {
        // SAFETY: null addr/addrlen skips peer-address writeback; SOCK_CLOEXEC keeps the conn out of the workload.
        #[cfg(target_os = "linux")]
        let fd = unsafe {
            libc::accept4(
                listen_fd,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                libc::SOCK_CLOEXEC,
            )
        };
        // SAFETY: null addr/addrlen skips peer-address writeback; this vsock path never runs on the host build.
        #[cfg(not(target_os = "linux"))]
        let fd = unsafe { libc::accept(listen_fd, std::ptr::null_mut(), std::ptr::null_mut()) };
        if fd < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(fd)
        }
    }

    fn close(&self, fd: RawFd) {
        session_close(fd);
    }

    fn read(&self, fd: RawFd, buf: &mut [u8]) -> io::Result<usize> {
        // SAFETY: buf is exclusively borrowed for the slice handed to read(2).
        let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut _, buf.len()) };
        if n < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(n as usize)
        }
    }

    fn write(&self, fd: RawFd, buf: &[u8]) -> io::Result<usize> {
        // SAFETY: buf is borrowed for the slice handed to write(2).
        let n = unsafe { libc::write(fd, buf.as_ptr() as *const _, buf.len()) };
        if n < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(n as usize)
        }
    }
}

#[cfg(target_os = "linux")]
pub fn listen(port: u32) -> Result<RawFd, VsockError> {
    listen_with(&RealVsockSyscalls, port)
}

#[cfg(target_os = "linux")]
pub fn connect(port: u32) -> Result<RawFd, VsockError> {
    super::connect_with(&RealVsockSyscalls, port)
}

#[cfg(target_os = "linux")]
pub fn accept(listen_fd: RawFd) -> Result<RawFd, VsockError> {
    accept_with(&RealVsockSyscalls, listen_fd)
}

pub fn read_exact(fd: RawFd, buf: &mut [u8]) -> ReadOutcome {
    read_exact_with(&RealVsockSyscalls, fd, buf)
}

#[cfg(target_os = "linux")]
pub fn write_all(fd: RawFd, buf: &[u8]) -> bool {
    write_all_with(&RealVsockSyscalls, fd, buf)
}
