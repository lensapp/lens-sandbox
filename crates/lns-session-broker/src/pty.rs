#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::ffi::{CStr, CString};
use std::io;
use std::os::fd::RawFd;

#[cfg(target_os = "linux")]
mod real;
#[cfg(target_os = "linux")]
pub use real::{child_setup, foreground_pgrp, open_pty};

#[derive(Debug)]
pub enum PtyError {
    Syscall { op: String, err: io::Error },
    InteriorNul(&'static str),
}

impl std::fmt::Display for PtyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Syscall { op, err } => write!(f, "{op}: {err}"),
            Self::InteriorNul(field) => {
                write!(f, "interior NUL in {field}")
            }
        }
    }
}

impl std::error::Error for PtyError {}

#[derive(Debug)]
pub struct Pty {
    pub master: RawFd,
    pub slave: RawFd,
}

pub(crate) trait PtySyscalls {
    fn posix_openpt(&self) -> io::Result<RawFd>;
    fn grantpt(&self, master: RawFd) -> io::Result<()>;
    fn unlockpt(&self, master: RawFd) -> io::Result<()>;
    fn ptsname(&self, master: RawFd) -> io::Result<String>;
    fn open_slave(&self, path: &CStr) -> io::Result<RawFd>;
    fn close(&self, fd: RawFd);
    fn setsid(&self) -> io::Result<()>;
    fn ioctl_set_controlling_tty(&self, slave: RawFd) -> io::Result<()>;
    fn getpgrp(&self) -> libc::pid_t;
    fn tcsetpgrp(&self, slave: RawFd, pgrp: libc::pid_t) -> io::Result<()>;
    fn tcgetpgrp(&self, master: RawFd) -> libc::pid_t;
    fn dup2(&self, oldfd: RawFd, newfd: RawFd) -> io::Result<()>;
}

fn syscall_err(op: &str, err: io::Error) -> PtyError {
    PtyError::Syscall {
        op: op.to_string(),
        err,
    }
}

pub(crate) fn open_pty_with(sys: &dyn PtySyscalls) -> Result<Pty, PtyError> {
    let master = sys
        .posix_openpt()
        .map_err(|err| syscall_err("posix_openpt", err))?;
    if let Err(err) = sys.grantpt(master) {
        sys.close(master);
        return Err(syscall_err("grantpt", err));
    }
    if let Err(err) = sys.unlockpt(master) {
        sys.close(master);
        return Err(syscall_err("unlockpt", err));
    }
    let slave_path = match sys.ptsname(master) {
        Ok(p) => p,
        Err(err) => {
            sys.close(master);
            return Err(syscall_err("ptsname", err));
        }
    };
    let c_slave = match CString::new(slave_path) {
        Ok(c) => c,
        Err(_) => {
            sys.close(master);
            return Err(PtyError::InteriorNul("pts-slave-path"));
        }
    };
    let slave = match sys.open_slave(&c_slave) {
        Ok(s) => s,
        Err(err) => {
            sys.close(master);
            return Err(syscall_err("open(pts slave)", err));
        }
    };
    Ok(Pty { master, slave })
}

pub(crate) fn child_setup_with(sys: &dyn PtySyscalls, slave: RawFd) -> Result<(), PtyError> {
    sys.setsid().map_err(|err| syscall_err("setsid", err))?;
    sys.ioctl_set_controlling_tty(slave)
        .map_err(|err| syscall_err("ioctl(TIOCSCTTY)", err))?;
    let pgrp = sys.getpgrp();
    sys.tcsetpgrp(slave, pgrp)
        .map_err(|err| syscall_err("tcsetpgrp(pts slave)", err))?;
    for fd in 0..=2 {
        sys.dup2(slave, fd)
            .map_err(|err| syscall_err(&format!("dup2(pts slave -> {fd})"), err))?;
    }
    if slave > 2 {
        sys.close(slave);
    }
    Ok(())
}

pub(crate) fn foreground_pgrp_with(sys: &dyn PtySyscalls, master: RawFd) -> Option<libc::pid_t> {
    let pgrp = sys.tcgetpgrp(master);
    (pgrp > 0).then_some(pgrp)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::io::ErrorKind;

    #[derive(Debug, Clone, PartialEq)]
    enum Call {
        PosixOpenpt,
        Grantpt(RawFd),
        Unlockpt(RawFd),
        Ptsname(RawFd),
        OpenSlave(String),
        Close(RawFd),
        Setsid,
        IoctlSctty(RawFd),
        Getpgrp,
        Tcsetpgrp { slave: RawFd, pgrp: libc::pid_t },
        Tcgetpgrp(RawFd),
        Dup2 { oldfd: RawFd, newfd: RawFd },
    }

    type FailWhen = Box<dyn Fn(&Call) -> Option<ErrorKind>>;

    struct FakePty {
        calls: RefCell<Vec<Call>>,
        fail_when: FailWhen,
        ptsname_ret: String,
        tcgetpgrp_ret: libc::pid_t,
        master_fd: RawFd,
        slave_fd: RawFd,
        pgrp: libc::pid_t,
    }

    impl FakePty {
        fn new() -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                fail_when: Box::new(|_| None),
                ptsname_ret: "/dev/pts/3".to_string(),
                tcgetpgrp_ret: 4242,
                master_fd: 10,
                slave_fd: 11,
                pgrp: 99,
            }
        }
        fn fail_when(mut self, f: impl Fn(&Call) -> Option<ErrorKind> + 'static) -> Self {
            self.fail_when = Box::new(f);
            self
        }
        fn ptsname_ret(mut self, s: &str) -> Self {
            self.ptsname_ret = s.to_string();
            self
        }
        fn tcgetpgrp_ret(mut self, p: libc::pid_t) -> Self {
            self.tcgetpgrp_ret = p;
            self
        }
        fn calls(&self) -> Vec<Call> {
            self.calls.borrow().clone()
        }
        fn check(&self, call: Call) -> io::Result<()> {
            let outcome = (self.fail_when)(&call);
            self.calls.borrow_mut().push(call);
            match outcome {
                Some(kind) => Err(io::Error::new(kind, "scripted failure")),
                None => Ok(()),
            }
        }
    }

    impl PtySyscalls for FakePty {
        fn posix_openpt(&self) -> io::Result<RawFd> {
            self.check(Call::PosixOpenpt)?;
            Ok(self.master_fd)
        }
        fn grantpt(&self, master: RawFd) -> io::Result<()> {
            self.check(Call::Grantpt(master))
        }
        fn unlockpt(&self, master: RawFd) -> io::Result<()> {
            self.check(Call::Unlockpt(master))
        }
        fn ptsname(&self, master: RawFd) -> io::Result<String> {
            self.check(Call::Ptsname(master))?;
            Ok(self.ptsname_ret.clone())
        }
        fn open_slave(&self, path: &CStr) -> io::Result<RawFd> {
            self.check(Call::OpenSlave(path.to_str().unwrap().to_string()))?;
            Ok(self.slave_fd)
        }
        fn close(&self, fd: RawFd) {
            self.calls.borrow_mut().push(Call::Close(fd));
        }
        fn setsid(&self) -> io::Result<()> {
            self.check(Call::Setsid)
        }
        fn ioctl_set_controlling_tty(&self, slave: RawFd) -> io::Result<()> {
            self.check(Call::IoctlSctty(slave))
        }
        fn getpgrp(&self) -> libc::pid_t {
            self.calls.borrow_mut().push(Call::Getpgrp);
            self.pgrp
        }
        fn tcsetpgrp(&self, slave: RawFd, pgrp: libc::pid_t) -> io::Result<()> {
            self.check(Call::Tcsetpgrp { slave, pgrp })
        }
        fn tcgetpgrp(&self, master: RawFd) -> libc::pid_t {
            self.calls.borrow_mut().push(Call::Tcgetpgrp(master));
            self.tcgetpgrp_ret
        }
        fn dup2(&self, oldfd: RawFd, newfd: RawFd) -> io::Result<()> {
            self.check(Call::Dup2 { oldfd, newfd })
        }
    }

    #[test]
    fn pty_error_display_covers_both_variants() {
        let s = PtyError::Syscall {
            op: "grantpt".into(),
            err: io::Error::other("boom"),
        };
        assert_eq!(format!("{s}"), "grantpt: boom");
        let n = PtyError::InteriorNul("pts-slave-path");
        assert!(format!("{n}").contains("interior NUL in pts-slave-path"));
    }

    #[test]
    fn open_pty_allocates_master_then_slave_in_order() {
        let sys = FakePty::new();
        let pty = open_pty_with(&sys).expect("open");
        assert_eq!(pty.master, 10);
        assert_eq!(pty.slave, 11);
        assert_eq!(
            sys.calls(),
            vec![
                Call::PosixOpenpt,
                Call::Grantpt(10),
                Call::Unlockpt(10),
                Call::Ptsname(10),
                Call::OpenSlave("/dev/pts/3".into()),
            ]
        );
    }

    #[test]
    fn open_pty_reports_posix_openpt_failure_without_closing() {
        let sys = FakePty::new()
            .fail_when(|c| matches!(c, Call::PosixOpenpt).then_some(ErrorKind::PermissionDenied));
        let err = open_pty_with(&sys).unwrap_err();
        assert!(matches!(&err, PtyError::Syscall { op, .. } if op == "posix_openpt"));
        assert!(!sys.calls().iter().any(|c| matches!(c, Call::Close(_))));
    }

    #[test]
    fn open_pty_closes_master_when_grantpt_fails() {
        let sys =
            FakePty::new().fail_when(|c| matches!(c, Call::Grantpt(_)).then_some(ErrorKind::Other));
        let err = open_pty_with(&sys).unwrap_err();
        assert!(matches!(&err, PtyError::Syscall { op, .. } if op == "grantpt"));
        assert!(
            sys.calls().contains(&Call::Close(10)),
            "master closed on grantpt failure"
        );
    }

    #[test]
    fn open_pty_closes_master_when_unlockpt_fails() {
        let sys = FakePty::new()
            .fail_when(|c| matches!(c, Call::Unlockpt(_)).then_some(ErrorKind::Other));
        let err = open_pty_with(&sys).unwrap_err();
        assert!(matches!(&err, PtyError::Syscall { op, .. } if op == "unlockpt"));
        assert!(sys.calls().contains(&Call::Close(10)));
    }

    #[test]
    fn open_pty_closes_master_when_ptsname_fails() {
        let sys =
            FakePty::new().fail_when(|c| matches!(c, Call::Ptsname(_)).then_some(ErrorKind::Other));
        let err = open_pty_with(&sys).unwrap_err();
        assert!(matches!(&err, PtyError::Syscall { op, .. } if op == "ptsname"));
        assert!(sys.calls().contains(&Call::Close(10)));
    }

    #[test]
    fn open_pty_closes_master_when_slave_path_has_interior_nul() {
        let sys = FakePty::new().ptsname_ret("/dev/pts/\0bad");
        let err = open_pty_with(&sys).unwrap_err();
        assert!(matches!(err, PtyError::InteriorNul("pts-slave-path")));
        assert!(sys.calls().contains(&Call::Close(10)));
        // open_slave is never attempted once the path can't be turned into a CString.
        assert!(!sys.calls().iter().any(|c| matches!(c, Call::OpenSlave(_))));
    }

    #[test]
    fn open_pty_closes_master_when_open_slave_fails() {
        let sys = FakePty::new()
            .fail_when(|c| matches!(c, Call::OpenSlave(_)).then_some(ErrorKind::NotFound));
        let err = open_pty_with(&sys).unwrap_err();
        assert!(matches!(&err, PtyError::Syscall { op, .. } if op == "open(pts slave)"));
        assert!(sys.calls().contains(&Call::Close(10)));
    }

    #[test]
    fn child_setup_runs_full_sequence_and_closes_high_slave() {
        let sys = FakePty::new();
        child_setup_with(&sys, 5).expect("child setup");
        assert_eq!(
            sys.calls(),
            vec![
                Call::Setsid,
                Call::IoctlSctty(5),
                Call::Getpgrp,
                Call::Tcsetpgrp { slave: 5, pgrp: 99 },
                Call::Dup2 { oldfd: 5, newfd: 0 },
                Call::Dup2 { oldfd: 5, newfd: 1 },
                Call::Dup2 { oldfd: 5, newfd: 2 },
                Call::Close(5),
            ]
        );
    }

    #[test]
    fn child_setup_keeps_low_slave_fd_open() {
        let sys = FakePty::new();
        child_setup_with(&sys, 1).expect("child setup");
        // slave fd 1 was dup2'd onto stdout; closing it would close the freshly-installed fd.
        assert!(!sys.calls().contains(&Call::Close(1)));
    }

    #[test]
    fn child_setup_reports_setsid_failure() {
        let sys =
            FakePty::new().fail_when(|c| matches!(c, Call::Setsid).then_some(ErrorKind::Other));
        let err = child_setup_with(&sys, 5).unwrap_err();
        assert!(matches!(&err, PtyError::Syscall { op, .. } if op == "setsid"));
    }

    #[test]
    fn child_setup_reports_ioctl_failure() {
        let sys = FakePty::new()
            .fail_when(|c| matches!(c, Call::IoctlSctty(_)).then_some(ErrorKind::Other));
        let err = child_setup_with(&sys, 5).unwrap_err();
        assert!(matches!(&err, PtyError::Syscall { op, .. } if op.contains("TIOCSCTTY")));
    }

    #[test]
    fn child_setup_reports_tcsetpgrp_failure() {
        let sys = FakePty::new()
            .fail_when(|c| matches!(c, Call::Tcsetpgrp { .. }).then_some(ErrorKind::Other));
        let err = child_setup_with(&sys, 5).unwrap_err();
        assert!(matches!(&err, PtyError::Syscall { op, .. } if op.contains("tcsetpgrp")));
    }

    #[test]
    fn child_setup_reports_dup2_failure_with_target_fd() {
        let sys = FakePty::new()
            .fail_when(|c| matches!(c, Call::Dup2 { newfd: 0, .. }).then_some(ErrorKind::Other));
        let err = child_setup_with(&sys, 5).unwrap_err();
        assert!(matches!(&err, PtyError::Syscall { op, .. } if op == "dup2(pts slave -> 0)"));
    }

    #[test]
    fn foreground_pgrp_is_some_for_positive_and_none_otherwise() {
        let sys = FakePty::new().tcgetpgrp_ret(7);
        assert_eq!(foreground_pgrp_with(&sys, 10), Some(7));
        let sys = FakePty::new().tcgetpgrp_ret(0);
        assert_eq!(foreground_pgrp_with(&sys, 10), None);
        let sys = FakePty::new().tcgetpgrp_ret(-1);
        assert_eq!(foreground_pgrp_with(&sys, 10), None);
    }
}
