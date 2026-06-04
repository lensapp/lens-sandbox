use std::os::fd::RawFd;
use std::sync::Arc;

mod real;
pub use real::{host_winsize, stdin_is_tty};

pub trait Tty: Send + Sync {
    fn isatty(&self, fd: RawFd) -> bool;
    fn tcgetattr(&self, fd: RawFd) -> Option<libc::termios>;
    fn tcsetattr(&self, fd: RawFd, termios: &libc::termios) -> bool;
    fn cfmakeraw(&self, termios: &mut libc::termios);
    fn winsize(&self, fd: RawFd) -> Option<(u16, u16)>;
}

pub struct RawModeGuard {
    fd: RawFd,
    prior: libc::termios,
    tty: Arc<dyn Tty>,
}

impl RawModeGuard {
    pub fn enable_if_tty_with(tty: Arc<dyn Tty>, fd: RawFd) -> Option<Self> {
        if !tty.isatty(fd) {
            return None;
        }
        let prior = tty.tcgetattr(fd)?;
        let mut raw = prior;
        tty.cfmakeraw(&mut raw);
        if !tty.tcsetattr(fd, &raw) {
            return None;
        }
        Some(Self { fd, prior, tty })
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = self.tty.tcsetattr(self.fd, &self.prior);
    }
}

pub fn host_winsize_with(tty: &dyn Tty, fd: RawFd) -> Option<(u16, u16)> {
    if !tty.isatty(fd) {
        return None;
    }
    tty.winsize(fd)
}

pub fn stdin_is_tty_with(tty: &dyn Tty, fd: RawFd) -> bool {
    tty.isatty(fd)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum TtyCall {
        Isatty {
            fd: RawFd,
            ret: bool,
        },
        Tcgetattr {
            fd: RawFd,
            ok: bool,
        },
        Tcsetattr {
            fd: RawFd,
            kind: TcsetattrKind,
            ret: bool,
        },
        Cfmakeraw,
        Winsize {
            fd: RawFd,
            ret: Option<(u16, u16)>,
        },
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TcsetattrKind {
        Raw,
        Prior,
    }

    struct FakeTty {
        isatty_rets: Mutex<VecDeque<bool>>,
        tcgetattr_rets: Mutex<VecDeque<Option<libc::termios>>>,
        tcsetattr_rets: Mutex<VecDeque<bool>>,
        winsize_rets: Mutex<VecDeque<Option<(u16, u16)>>>,
        calls: Mutex<Vec<TtyCall>>,
        raw_marker: Mutex<bool>,
    }

    impl FakeTty {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                isatty_rets: Mutex::new(VecDeque::new()),
                tcgetattr_rets: Mutex::new(VecDeque::new()),
                tcsetattr_rets: Mutex::new(VecDeque::new()),
                winsize_rets: Mutex::new(VecDeque::new()),
                calls: Mutex::new(Vec::new()),
                raw_marker: Mutex::new(false),
            })
        }

        fn push_isatty(&self, ret: bool) {
            self.isatty_rets.lock().unwrap().push_back(ret);
        }
        fn push_tcgetattr(&self, ret: Option<libc::termios>) {
            self.tcgetattr_rets.lock().unwrap().push_back(ret);
        }
        fn push_tcsetattr(&self, ret: bool) {
            self.tcsetattr_rets.lock().unwrap().push_back(ret);
        }
        fn push_winsize(&self, ret: Option<(u16, u16)>) {
            self.winsize_rets.lock().unwrap().push_back(ret);
        }
        fn calls(&self) -> Vec<TtyCall> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Tty for FakeTty {
        fn isatty(&self, fd: RawFd) -> bool {
            let ret = self
                .isatty_rets
                .lock()
                .unwrap()
                .pop_front()
                .expect("FakeTty: unexpected isatty call (no scripted return)");
            self.calls.lock().unwrap().push(TtyCall::Isatty { fd, ret });
            ret
        }

        fn tcgetattr(&self, fd: RawFd) -> Option<libc::termios> {
            let ret = self
                .tcgetattr_rets
                .lock()
                .unwrap()
                .pop_front()
                .expect("FakeTty: unexpected tcgetattr call");
            *self.raw_marker.lock().unwrap() = false;
            self.calls.lock().unwrap().push(TtyCall::Tcgetattr {
                fd,
                ok: ret.is_some(),
            });
            ret
        }

        fn tcsetattr(&self, fd: RawFd, _termios: &libc::termios) -> bool {
            let ret = self
                .tcsetattr_rets
                .lock()
                .unwrap()
                .pop_front()
                .expect("FakeTty: unexpected tcsetattr call");
            let kind = if *self.raw_marker.lock().unwrap() {
                *self.raw_marker.lock().unwrap() = false;
                TcsetattrKind::Raw
            } else {
                TcsetattrKind::Prior
            };
            self.calls
                .lock()
                .unwrap()
                .push(TtyCall::Tcsetattr { fd, kind, ret });
            ret
        }

        fn cfmakeraw(&self, _termios: &mut libc::termios) {
            *self.raw_marker.lock().unwrap() = true;
            self.calls.lock().unwrap().push(TtyCall::Cfmakeraw);
        }

        fn winsize(&self, fd: RawFd) -> Option<(u16, u16)> {
            let ret = self
                .winsize_rets
                .lock()
                .unwrap()
                .pop_front()
                .expect("FakeTty: unexpected winsize call");
            self.calls
                .lock()
                .unwrap()
                .push(TtyCall::Winsize { fd, ret });
            ret
        }
    }

    fn zeroed_termios() -> libc::termios {
        // SAFETY: all-zero is a valid repr(C) termios value; used only as a test fixture.
        unsafe { std::mem::zeroed() }
    }

    #[test]
    fn enable_if_tty_returns_none_when_not_a_tty() {
        let fake = FakeTty::new();
        fake.push_isatty(false);

        let guard = RawModeGuard::enable_if_tty_with(fake.clone(), 0);

        assert!(guard.is_none());
        assert_eq!(fake.calls(), vec![TtyCall::Isatty { fd: 0, ret: false }],);
    }

    #[test]
    fn enable_if_tty_returns_none_when_tcgetattr_fails() {
        let fake = FakeTty::new();
        fake.push_isatty(true);
        fake.push_tcgetattr(None);

        let guard = RawModeGuard::enable_if_tty_with(fake.clone(), 0);

        assert!(guard.is_none());
        assert_eq!(
            fake.calls(),
            vec![
                TtyCall::Isatty { fd: 0, ret: true },
                TtyCall::Tcgetattr { fd: 0, ok: false },
            ],
        );
    }

    #[test]
    fn enable_if_tty_returns_none_when_tcsetattr_fails() {
        let fake = FakeTty::new();
        fake.push_isatty(true);
        fake.push_tcgetattr(Some(zeroed_termios()));
        fake.push_tcsetattr(false);

        let guard = RawModeGuard::enable_if_tty_with(fake.clone(), 0);

        assert!(guard.is_none());
        assert_eq!(
            fake.calls(),
            vec![
                TtyCall::Isatty { fd: 0, ret: true },
                TtyCall::Tcgetattr { fd: 0, ok: true },
                TtyCall::Cfmakeraw,
                TtyCall::Tcsetattr {
                    fd: 0,
                    kind: TcsetattrKind::Raw,
                    ret: false,
                },
            ],
        );
    }

    #[test]
    fn enable_if_tty_happy_path_returns_guard_and_drop_restores_prior() {
        let fake = FakeTty::new();
        fake.push_isatty(true);
        fake.push_tcgetattr(Some(zeroed_termios()));
        fake.push_tcsetattr(true);
        fake.push_tcsetattr(true);

        {
            let guard = RawModeGuard::enable_if_tty_with(fake.clone(), 7);
            assert!(guard.is_some());
            assert_eq!(fake.calls().len(), 4);
        }
        assert_eq!(
            fake.calls(),
            vec![
                TtyCall::Isatty { fd: 7, ret: true },
                TtyCall::Tcgetattr { fd: 7, ok: true },
                TtyCall::Cfmakeraw,
                TtyCall::Tcsetattr {
                    fd: 7,
                    kind: TcsetattrKind::Raw,
                    ret: true,
                },
                TtyCall::Tcsetattr {
                    fd: 7,
                    kind: TcsetattrKind::Prior,
                    ret: true,
                },
            ],
        );
    }

    #[test]
    fn drop_swallows_tcsetattr_failure() {
        let fake = FakeTty::new();
        fake.push_isatty(true);
        fake.push_tcgetattr(Some(zeroed_termios()));
        fake.push_tcsetattr(true);
        fake.push_tcsetattr(false);

        {
            let _guard = RawModeGuard::enable_if_tty_with(fake.clone(), 0).expect("happy path");
        }
        assert_eq!(fake.calls().len(), 5);
    }

    #[test]
    fn host_winsize_returns_none_when_not_a_tty() {
        let fake = FakeTty::new();
        fake.push_isatty(false);

        let result = host_winsize_with(fake.as_ref(), 0);

        assert_eq!(result, None);
        assert_eq!(fake.calls(), vec![TtyCall::Isatty { fd: 0, ret: false }],);
    }

    #[test]
    fn host_winsize_returns_none_when_ioctl_fails() {
        let fake = FakeTty::new();
        fake.push_isatty(true);
        fake.push_winsize(None);

        let result = host_winsize_with(fake.as_ref(), 0);

        assert_eq!(result, None);
        assert_eq!(
            fake.calls(),
            vec![
                TtyCall::Isatty { fd: 0, ret: true },
                TtyCall::Winsize { fd: 0, ret: None },
            ],
        );
    }

    #[test]
    fn host_winsize_returns_some_on_success() {
        let fake = FakeTty::new();
        fake.push_isatty(true);
        fake.push_winsize(Some((24, 80)));

        let result = host_winsize_with(fake.as_ref(), 0);

        assert_eq!(result, Some((24, 80)));
    }

    #[test]
    fn host_winsize_uses_the_provided_fd() {
        let fake = FakeTty::new();
        fake.push_isatty(true);
        fake.push_winsize(Some((10, 20)));

        let _ = host_winsize_with(fake.as_ref(), 42);

        assert_eq!(
            fake.calls(),
            vec![
                TtyCall::Isatty { fd: 42, ret: true },
                TtyCall::Winsize {
                    fd: 42,
                    ret: Some((10, 20))
                },
            ],
        );
    }

    #[test]
    fn stdin_is_tty_returns_true_from_fake() {
        let fake = FakeTty::new();
        fake.push_isatty(true);

        assert!(stdin_is_tty_with(fake.as_ref(), 0));
        assert_eq!(fake.calls(), vec![TtyCall::Isatty { fd: 0, ret: true }],);
    }

    #[test]
    fn stdin_is_tty_returns_false_from_fake() {
        let fake = FakeTty::new();
        fake.push_isatty(false);

        assert!(!stdin_is_tty_with(fake.as_ref(), 0));
    }

    #[test]
    fn host_winsize_smoke_returns_some_or_none_consistently() {
        let _ = host_winsize();
    }

    #[test]
    fn stdin_is_tty_smoke_returns_bool() {
        let _ = stdin_is_tty();
    }
}
