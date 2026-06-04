use std::mem::MaybeUninit;
use std::os::fd::RawFd;
use std::sync::Arc;

use super::{RawModeGuard, Tty, host_winsize_with, stdin_is_tty_with};

pub struct RealTty;

impl Tty for RealTty {
    fn isatty(&self, fd: RawFd) -> bool {
        // SAFETY: isatty is side-effect-free; out-of-range fds return 0.
        unsafe { libc::isatty(fd) != 0 }
    }

    fn tcgetattr(&self, fd: RawFd) -> Option<libc::termios> {
        let mut prior = MaybeUninit::<libc::termios>::uninit();
        // SAFETY: tcgetattr initialises *prior iff it returns 0.
        unsafe { (libc::tcgetattr(fd, prior.as_mut_ptr()) == 0).then(|| prior.assume_init()) }
    }

    fn tcsetattr(&self, fd: RawFd, termios: &libc::termios) -> bool {
        // SAFETY: tcsetattr only reads the termios pointee.
        unsafe { libc::tcsetattr(fd, libc::TCSANOW, termios) == 0 }
    }

    fn cfmakeraw(&self, termios: &mut libc::termios) {
        // SAFETY: cfmakeraw is a pure bit-tweak that never fails.
        unsafe { libc::cfmakeraw(termios) };
    }

    fn winsize(&self, fd: RawFd) -> Option<(u16, u16)> {
        let mut ws = libc::winsize {
            ws_row: 0,
            ws_col: 0,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: ioctl(TIOCGWINSZ) writes into ws on success; we check rc before reading.
        let rc = unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) };
        if rc != 0 {
            return None;
        }
        Some((ws.ws_row, ws.ws_col))
    }
}

fn real_tty() -> Arc<dyn Tty> {
    Arc::new(RealTty)
}

impl RawModeGuard {
    pub fn enable_if_tty() -> Option<Self> {
        Self::enable_if_tty_with(real_tty(), 0)
    }
}

pub fn host_winsize() -> Option<(u16, u16)> {
    host_winsize_with(real_tty().as_ref(), 0)
}

pub fn stdin_is_tty() -> bool {
    stdin_is_tty_with(real_tty().as_ref(), 0)
}
