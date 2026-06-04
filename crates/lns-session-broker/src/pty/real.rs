use std::ffi::CStr;
use std::io;
use std::os::fd::RawFd;

use super::{Pty, PtyError, PtySyscalls, child_setup_with, foreground_pgrp_with, open_pty_with};
use crate::session::close;

fn rc(v: libc::c_int) -> io::Result<()> {
    if v < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

struct RealPtySyscalls;

impl PtySyscalls for RealPtySyscalls {
    fn posix_openpt(&self) -> io::Result<RawFd> {
        // SAFETY: posix_openpt allocates a new master fd.
        let fd = unsafe { libc::posix_openpt(libc::O_RDWR | libc::O_NOCTTY) };
        if fd < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(fd)
        }
    }

    fn grantpt(&self, master: RawFd) -> io::Result<()> {
        // SAFETY: master is the caller-owned PTY fd.
        rc(unsafe { libc::grantpt(master) })
    }

    fn unlockpt(&self, master: RawFd) -> io::Result<()> {
        // SAFETY: master is the caller-owned PTY fd.
        rc(unsafe { libc::unlockpt(master) })
    }

    fn ptsname(&self, master: RawFd) -> io::Result<String> {
        // SAFETY: ptsname returns a pointer to a static libc-owned buffer.
        let p = unsafe { libc::ptsname(master) };
        if p.is_null() {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: p was NUL-checked and points to a NUL-terminated libc-owned buffer.
        let cstr = unsafe { CStr::from_ptr(p) };
        cstr.to_str()
            .map(|s| s.to_string())
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    fn open_slave(&self, path: &CStr) -> io::Result<RawFd> {
        // SAFETY: path is a valid NUL-terminated CString.
        let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDWR | libc::O_NOCTTY) };
        if fd < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(fd)
        }
    }

    fn close(&self, fd: RawFd) {
        close(fd);
    }

    fn setsid(&self) -> io::Result<()> {
        // SAFETY: post-fork child call; no other threads.
        rc(unsafe { libc::setsid() })
    }

    fn ioctl_set_controlling_tty(&self, slave: RawFd) -> io::Result<()> {
        // SAFETY: slave is a caller-owned PTY slave fd.
        rc(unsafe { libc::ioctl(slave, libc::TIOCSCTTY, 0) })
    }

    fn getpgrp(&self) -> libc::pid_t {
        // SAFETY: getpgrp reads no arguments.
        unsafe { libc::getpgrp() }
    }

    fn tcsetpgrp(&self, slave: RawFd, pgrp: libc::pid_t) -> io::Result<()> {
        // SAFETY: slave is a caller-owned PTY slave fd; pgrp is a real process group.
        rc(unsafe { libc::tcsetpgrp(slave, pgrp) })
    }

    fn tcgetpgrp(&self, master: RawFd) -> libc::pid_t {
        // SAFETY: master is a caller-owned PTY fd.
        unsafe { libc::tcgetpgrp(master) }
    }

    fn dup2(&self, oldfd: RawFd, newfd: RawFd) -> io::Result<()> {
        // SAFETY: oldfd is owned; dup2 replaces newfd with a clone of oldfd.
        rc(unsafe { libc::dup2(oldfd, newfd) })
    }
}

pub fn open_pty() -> Result<Pty, PtyError> {
    open_pty_with(&RealPtySyscalls)
}

pub fn child_setup(slave: RawFd) -> Result<(), PtyError> {
    child_setup_with(&RealPtySyscalls, slave)
}

pub fn foreground_pgrp(master: RawFd) -> Option<libc::pid_t> {
    foreground_pgrp_with(&RealPtySyscalls, master)
}
