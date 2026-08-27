use std::fs::File;
use std::io::{BufRead, BufReader};
use std::os::fd::AsRawFd;

use super::Terminal;

pub struct RealTerminal {
    tty: Option<BufReader<File>>,
}

impl RealTerminal {
    pub fn open() -> Self {
        Self {
            tty: File::open("/dev/tty").ok().map(BufReader::new),
        }
    }
}

impl Terminal for RealTerminal {
    fn is_available(&self) -> bool {
        self.tty.is_some()
    }

    fn read_answer(&mut self) -> std::io::Result<String> {
        let Some(tty) = self.tty.as_mut() else {
            return Err(std::io::Error::other(
                "there is no controlling terminal to read an answer from",
            ));
        };
        let mut line = String::new();
        tty.read_line(&mut line)?;
        Ok(line)
    }

    /// Clears `ECHO` for the read and restores it after, so a pasted token never reaches the screen. A terminal whose attributes cannot be read fails the read rather than falling back to echoing it.
    fn read_secret(&mut self) -> std::io::Result<String> {
        let Some(tty) = self.tty.as_mut() else {
            return Err(std::io::Error::other(
                "there is no controlling terminal to read a secret from",
            ));
        };
        let fd = tty.get_ref().as_raw_fd();
        let _hidden = EchoOff::on(fd)?;
        let mut line = String::new();
        tty.read_line(&mut line)?;
        Ok(line)
    }
}

/// Holds `ECHO` off for as long as it lives, so a read that fails still restores the terminal.
struct EchoOff {
    fd: std::os::fd::RawFd,
    prior: libc::termios,
}

impl EchoOff {
    fn on(fd: std::os::fd::RawFd) -> std::io::Result<Self> {
        let mut prior = std::mem::MaybeUninit::<libc::termios>::uninit();
        // SAFETY: tcgetattr initialises *prior iff it returns 0, which is the only path that reads it.
        let prior = unsafe {
            if libc::tcgetattr(fd, prior.as_mut_ptr()) != 0 {
                return Err(cannot_hide());
            }
            prior.assume_init()
        };
        let mut hidden = prior;
        hidden.c_lflag &= !libc::ECHO;
        // SAFETY: `hidden` is a copy of the attributes tcgetattr just filled in, with one flag cleared.
        if unsafe { libc::tcsetattr(fd, libc::TCSAFLUSH, &hidden) } != 0 {
            return Err(cannot_hide());
        }
        Ok(Self { fd, prior })
    }
}

/// Carries the OS error, so a failure to hide a secret is diagnosable rather than just refused.
fn cannot_hide() -> std::io::Error {
    std::io::Error::other(format!(
        "cannot turn off echo on this terminal, so refusing to read a secret that would be visible: {}",
        std::io::Error::last_os_error()
    ))
}

impl Drop for EchoOff {
    fn drop(&mut self) {
        // SAFETY: restoring the attributes tcgetattr filled in for this same fd.
        unsafe {
            libc::tcsetattr(self.fd, libc::TCSAFLUSH, &self.prior);
        }
    }
}
