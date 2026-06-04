#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::io;

use crate::session::SessionError;

pub enum Fork {
    Parent(libc::pid_t),
    Child,
}

pub trait Forker: Send + Sync {
    fn fork(&self) -> io::Result<Fork>;
    fn wait(&self, pid: libc::pid_t) -> Result<i32, SessionError>;
}

pub struct LibcForker;

#[cfg(target_os = "linux")]
impl Forker for LibcForker {
    fn fork(&self) -> io::Result<Fork> {
        // SAFETY: caller is single-threaded at this point (broker before any thread spawn).
        let pid = unsafe { libc::fork() };
        if pid < 0 {
            Err(io::Error::last_os_error())
        } else if pid == 0 {
            Ok(Fork::Child)
        } else {
            Ok(Fork::Parent(pid))
        }
    }

    fn wait(&self, pid: libc::pid_t) -> Result<i32, SessionError> {
        let mut status: libc::c_int = 0;
        loop {
            // SAFETY: status is exclusively borrowed for the duration of waitpid.
            let r = unsafe { libc::waitpid(pid, &mut status, 0) };
            if r == pid {
                return Ok(crate::exit::translate_exit(status));
            }
            if r < 0 {
                let err = io::Error::last_os_error();
                if err.raw_os_error() == Some(libc::EINTR) {
                    continue;
                }
                return Err(SessionError::Io(err));
            }
        }
    }
}

#[cfg(not(target_os = "linux"))]
impl Forker for LibcForker {
    fn fork(&self) -> io::Result<Fork> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "lns-session-broker fork is Linux-only",
        ))
    }
    fn wait(&self, _pid: libc::pid_t) -> Result<i32, SessionError> {
        Err(SessionError::Protocol(
            "lns-session-broker wait is Linux-only".into(),
        ))
    }
}
