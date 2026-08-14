#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use libc::c_int;
use std::io;
use std::os::fd::RawFd;

mod real;
pub use real::read_exact;
#[cfg(target_os = "linux")]
pub use real::{accept, connect_host, listen, write_all};

const AF_VSOCK: c_int = 40;
const VMADDR_CID_ANY: u32 = u32::MAX;
/// The well-known CID of the hypervisor host, as seen from inside a guest.
const VMADDR_CID_HOST: u32 = 2;

#[repr(C)]
pub(crate) struct SockaddrVm {
    svm_family: libc::sa_family_t,
    svm_reserved1: u16,
    svm_port: u32,
    svm_cid: u32,
    svm_zero: [u8; 4],
}

#[derive(Debug)]
pub enum VsockError {
    Syscall { op: String, err: io::Error },
}

impl std::fmt::Display for VsockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Syscall { op, err } => write!(f, "{op}: {err}"),
        }
    }
}

impl std::error::Error for VsockError {}

#[derive(Debug, PartialEq, Eq)]
pub enum ReadOutcome {
    Full,
    PeerClosed,
    Error,
}

pub(crate) trait VsockSyscalls {
    fn socket(&self) -> io::Result<RawFd>;
    fn bind(&self, fd: RawFd, addr: &SockaddrVm) -> io::Result<()>;
    fn connect(&self, fd: RawFd, addr: &SockaddrVm) -> io::Result<()>;
    fn listen(&self, fd: RawFd, backlog: c_int) -> io::Result<()>;
    fn accept_once(&self, listen_fd: RawFd) -> io::Result<RawFd>;
    fn close(&self, fd: RawFd);
    fn read(&self, fd: RawFd, buf: &mut [u8]) -> io::Result<usize>;
    fn write(&self, fd: RawFd, buf: &[u8]) -> io::Result<usize>;
}

fn vsock_listen_addr(port: u32) -> SockaddrVm {
    SockaddrVm {
        svm_family: AF_VSOCK as libc::sa_family_t,
        svm_reserved1: 0,
        svm_port: port,
        svm_cid: VMADDR_CID_ANY,
        svm_zero: [0; 4],
    }
}

fn vsock_host_addr(port: u32) -> SockaddrVm {
    SockaddrVm {
        svm_family: AF_VSOCK as libc::sa_family_t,
        svm_reserved1: 0,
        svm_port: port,
        svm_cid: VMADDR_CID_HOST,
        svm_zero: [0; 4],
    }
}

/// Dial a port the host is listening on. The mirror of `listen_with`: the guest is the client here, which is how a sidecar reaches the run's proxy and how a workload reaches a sidecar.
pub(crate) fn connect_host_with(sys: &dyn VsockSyscalls, port: u32) -> Result<RawFd, VsockError> {
    let fd = sys.socket().map_err(|err| VsockError::Syscall {
        op: format!("socket(AF_VSOCK) host-port={port}"),
        err,
    })?;
    let addr = vsock_host_addr(port);
    if let Err(err) = sys.connect(fd, &addr) {
        sys.close(fd);
        return Err(VsockError::Syscall {
            op: format!("connect(vsock host:{port})"),
            err,
        });
    }
    Ok(fd)
}

pub(crate) fn listen_with(sys: &dyn VsockSyscalls, port: u32) -> Result<RawFd, VsockError> {
    let fd = sys.socket().map_err(|err| VsockError::Syscall {
        op: format!("socket(AF_VSOCK) port={port}"),
        err,
    })?;
    let addr = vsock_listen_addr(port);
    if let Err(err) = sys.bind(fd, &addr) {
        sys.close(fd);
        return Err(VsockError::Syscall {
            op: format!("bind(vsock *:{port})"),
            err,
        });
    }
    if let Err(err) = sys.listen(fd, 4) {
        sys.close(fd);
        return Err(VsockError::Syscall {
            op: format!("listen(vsock *:{port})"),
            err,
        });
    }
    Ok(fd)
}

pub(crate) fn accept_with(sys: &dyn VsockSyscalls, listen_fd: RawFd) -> Result<RawFd, VsockError> {
    loop {
        match sys.accept_once(listen_fd) {
            Ok(fd) => return Ok(fd),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => {
                return Err(VsockError::Syscall {
                    op: "accept(vsock)".into(),
                    err,
                });
            }
        }
    }
}

pub(crate) fn read_exact_with(sys: &dyn VsockSyscalls, fd: RawFd, buf: &mut [u8]) -> ReadOutcome {
    let mut off = 0;
    while off < buf.len() {
        match sys.read(fd, &mut buf[off..]) {
            Ok(0) => return ReadOutcome::PeerClosed,
            Ok(n) => off += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return ReadOutcome::Error,
        }
    }
    ReadOutcome::Full
}

pub(crate) fn write_all_with(sys: &dyn VsockSyscalls, fd: RawFd, buf: &[u8]) -> bool {
    let mut off = 0;
    while off < buf.len() {
        match sys.write(fd, &buf[off..]) {
            Ok(0) => return false,
            Ok(n) => off += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return false,
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;
    use std::io::ErrorKind;

    #[derive(Debug, Clone, PartialEq)]
    enum Call {
        Socket,
        Bind { fd: RawFd, port: u32, cid: u32 },
        Connect { fd: RawFd, port: u32, cid: u32 },
        Listen { fd: RawFd, backlog: c_int },
        AcceptOnce(RawFd),
        Close(RawFd),
    }

    enum ReadStep {
        Data(Vec<u8>),
        Eof,
        Interrupted,
        Fail(ErrorKind),
    }

    enum WriteStep {
        Wrote(usize),
        Interrupted,
        Fail(ErrorKind),
    }

    #[derive(Default)]
    struct FakeVsock {
        calls: RefCell<Vec<Call>>,
        socket_ret: Option<io::Result<RawFd>>,
        bind_fail: Option<ErrorKind>,
        connect_fail: Option<ErrorKind>,
        listen_fail: Option<ErrorKind>,
        accept_script: RefCell<VecDeque<io::Result<RawFd>>>,
        reads: RefCell<VecDeque<ReadStep>>,
        writes: RefCell<VecDeque<WriteStep>>,
    }

    impl FakeVsock {
        fn calls(&self) -> Vec<Call> {
            self.calls.borrow().clone()
        }
    }

    impl VsockSyscalls for FakeVsock {
        fn socket(&self) -> io::Result<RawFd> {
            self.calls.borrow_mut().push(Call::Socket);
            match &self.socket_ret {
                Some(Ok(fd)) => Ok(*fd),
                Some(Err(e)) => Err(io::Error::new(e.kind(), "scripted")),
                None => Ok(7),
            }
        }
        fn bind(&self, fd: RawFd, addr: &SockaddrVm) -> io::Result<()> {
            self.calls.borrow_mut().push(Call::Bind {
                fd,
                port: addr.svm_port,
                cid: addr.svm_cid,
            });
            match self.bind_fail {
                Some(k) => Err(io::Error::new(k, "scripted")),
                None => Ok(()),
            }
        }
        fn connect(&self, fd: RawFd, addr: &SockaddrVm) -> io::Result<()> {
            self.calls.borrow_mut().push(Call::Connect {
                fd,
                port: addr.svm_port,
                cid: addr.svm_cid,
            });
            match self.connect_fail {
                Some(k) => Err(io::Error::new(k, "scripted")),
                None => Ok(()),
            }
        }
        fn listen(&self, fd: RawFd, backlog: c_int) -> io::Result<()> {
            self.calls.borrow_mut().push(Call::Listen { fd, backlog });
            match self.listen_fail {
                Some(k) => Err(io::Error::new(k, "scripted")),
                None => Ok(()),
            }
        }
        fn accept_once(&self, listen_fd: RawFd) -> io::Result<RawFd> {
            self.calls.borrow_mut().push(Call::AcceptOnce(listen_fd));
            self.accept_script
                .borrow_mut()
                .pop_front()
                .unwrap_or(Ok(99))
        }
        fn close(&self, fd: RawFd) {
            self.calls.borrow_mut().push(Call::Close(fd));
        }
        fn read(&self, _fd: RawFd, buf: &mut [u8]) -> io::Result<usize> {
            match self
                .reads
                .borrow_mut()
                .pop_front()
                .expect("FakeVsock: no scripted read")
            {
                ReadStep::Data(d) => {
                    let n = d.len().min(buf.len());
                    buf[..n].copy_from_slice(&d[..n]);
                    Ok(n)
                }
                ReadStep::Eof => Ok(0),
                ReadStep::Interrupted => Err(io::Error::from(ErrorKind::Interrupted)),
                ReadStep::Fail(k) => Err(io::Error::from(k)),
            }
        }
        fn write(&self, _fd: RawFd, buf: &[u8]) -> io::Result<usize> {
            match self
                .writes
                .borrow_mut()
                .pop_front()
                .expect("FakeVsock: no scripted write")
            {
                WriteStep::Wrote(n) => Ok(n.min(buf.len())),
                WriteStep::Interrupted => Err(io::Error::from(ErrorKind::Interrupted)),
                WriteStep::Fail(k) => Err(io::Error::from(k)),
            }
        }
    }

    #[test]
    fn connect_host_dials_the_host_cid_not_the_wildcard() {
        // A guest reaching out must address CID 2; VMADDR_CID_ANY is a listen-side
        // wildcard and connecting to it would never find the host.
        let fake = FakeVsock::default();
        let fd = connect_host_with(&fake, 1032).expect("connect");
        assert_eq!(fd, 7);
        assert_eq!(
            fake.calls(),
            vec![
                Call::Socket,
                Call::Connect {
                    fd: 7,
                    port: 1032,
                    cid: VMADDR_CID_HOST
                }
            ]
        );
    }

    #[test]
    fn connect_host_closes_the_socket_when_the_dial_fails() {
        // Otherwise every refused bridge attempt leaks an fd until the guest hits EMFILE.
        let fake = FakeVsock {
            connect_fail: Some(ErrorKind::ConnectionRefused),
            ..FakeVsock::default()
        };
        let err = connect_host_with(&fake, 1032).expect_err("refused");
        assert!(
            err.to_string().contains("connect(vsock host:1032)"),
            "got: {err}"
        );
        let calls = fake.calls();
        assert!(
            calls.contains(&Call::Close(7)),
            "the socket must be closed on failure: {calls:?}"
        );
    }

    #[test]
    fn connect_host_surfaces_a_socket_failure_without_dialling() {
        let fake = FakeVsock {
            socket_ret: Some(Err(io::Error::from(ErrorKind::PermissionDenied))),
            ..FakeVsock::default()
        };
        let err = connect_host_with(&fake, 1032).expect_err("no socket");
        assert!(
            err.to_string().contains("socket(AF_VSOCK) host-port=1032"),
            "got: {err}"
        );
        assert_eq!(fake.calls(), vec![Call::Socket]);
    }

    #[test]
    fn vsock_error_display() {
        let e = VsockError::Syscall {
            op: "bind(vsock *:9)".into(),
            err: io::Error::other("nope"),
        };
        assert_eq!(format!("{e}"), "bind(vsock *:9): nope");
    }

    #[test]
    fn listen_addr_targets_any_cid_on_requested_port() {
        let addr = vsock_listen_addr(1234);
        assert_eq!(addr.svm_family, AF_VSOCK as libc::sa_family_t);
        assert_eq!(addr.svm_cid, VMADDR_CID_ANY);
        assert_eq!(addr.svm_port, 1234);
        assert_eq!(addr.svm_zero, [0; 4]);
        assert_eq!(addr.svm_reserved1, 0);
    }

    #[test]
    fn listen_binds_then_listens_in_order() {
        let sys = FakeVsock {
            socket_ret: Some(Ok(5)),
            ..Default::default()
        };
        let fd = listen_with(&sys, 77).expect("listen");
        assert_eq!(fd, 5);
        assert_eq!(
            sys.calls(),
            vec![
                Call::Socket,
                Call::Bind {
                    fd: 5,
                    port: 77,
                    cid: VMADDR_CID_ANY,
                },
                Call::Listen { fd: 5, backlog: 4 },
            ]
        );
    }

    #[test]
    fn listen_reports_socket_failure_without_closing() {
        let sys = FakeVsock {
            socket_ret: Some(Err(io::Error::from(ErrorKind::PermissionDenied))),
            ..Default::default()
        };
        let err = listen_with(&sys, 77).unwrap_err();
        assert!(
            matches!(&err, VsockError::Syscall { op, .. } if op.contains("socket(AF_VSOCK) port=77"))
        );
        assert!(!sys.calls().iter().any(|c| matches!(c, Call::Close(_))));
    }

    #[test]
    fn listen_closes_fd_when_bind_fails() {
        let sys = FakeVsock {
            socket_ret: Some(Ok(5)),
            bind_fail: Some(ErrorKind::AddrInUse),
            ..Default::default()
        };
        let err = listen_with(&sys, 77).unwrap_err();
        assert!(matches!(&err, VsockError::Syscall { op, .. } if op.contains("bind(vsock *:77)")));
        assert!(sys.calls().contains(&Call::Close(5)));
    }

    #[test]
    fn listen_closes_fd_when_listen_fails() {
        let sys = FakeVsock {
            socket_ret: Some(Ok(5)),
            listen_fail: Some(ErrorKind::Other),
            ..Default::default()
        };
        let err = listen_with(&sys, 77).unwrap_err();
        assert!(
            matches!(&err, VsockError::Syscall { op, .. } if op.contains("listen(vsock *:77)"))
        );
        assert!(sys.calls().contains(&Call::Close(5)));
    }

    #[test]
    fn listen_uses_the_default_socket_fd_when_unset() {
        let sys = FakeVsock::default();
        let fd = listen_with(&sys, 9).expect("listen");
        assert_eq!(fd, 7);
    }

    #[test]
    fn accept_returns_first_connection() {
        let sys = FakeVsock::default();
        sys.accept_script.borrow_mut().push_back(Ok(42));
        assert_eq!(accept_with(&sys, 5).unwrap(), 42);
    }

    #[test]
    fn accept_retries_on_interrupt_then_succeeds() {
        let sys = FakeVsock::default();
        sys.accept_script
            .borrow_mut()
            .push_back(Err(io::Error::from(ErrorKind::Interrupted)));
        sys.accept_script.borrow_mut().push_back(Ok(43));
        assert_eq!(accept_with(&sys, 5).unwrap(), 43);
        assert_eq!(
            sys.calls()
                .iter()
                .filter(|c| matches!(c, Call::AcceptOnce(_)))
                .count(),
            2
        );
    }

    #[test]
    fn accept_reports_terminal_error() {
        let sys = FakeVsock::default();
        sys.accept_script
            .borrow_mut()
            .push_back(Err(io::Error::from(ErrorKind::ConnectionAborted)));
        let err = accept_with(&sys, 5).unwrap_err();
        assert!(matches!(&err, VsockError::Syscall { op, .. } if op == "accept(vsock)"));
    }

    #[test]
    fn read_exact_accumulates_until_full() {
        let sys = FakeVsock::default();
        sys.reads
            .borrow_mut()
            .push_back(ReadStep::Data(b"he".to_vec()));
        sys.reads
            .borrow_mut()
            .push_back(ReadStep::Data(b"llo".to_vec()));
        let mut buf = [0u8; 5];
        assert_eq!(read_exact_with(&sys, 3, &mut buf), ReadOutcome::Full);
        assert_eq!(&buf, b"hello");
    }

    #[test]
    fn read_exact_reports_peer_closed_on_eof() {
        let sys = FakeVsock::default();
        sys.reads.borrow_mut().push_back(ReadStep::Eof);
        let mut buf = [0u8; 4];
        assert_eq!(read_exact_with(&sys, 3, &mut buf), ReadOutcome::PeerClosed);
    }

    #[test]
    fn read_exact_retries_on_interrupt() {
        let sys = FakeVsock::default();
        sys.reads.borrow_mut().push_back(ReadStep::Interrupted);
        sys.reads
            .borrow_mut()
            .push_back(ReadStep::Data(b"ok".to_vec()));
        let mut buf = [0u8; 2];
        assert_eq!(read_exact_with(&sys, 3, &mut buf), ReadOutcome::Full);
        assert_eq!(&buf, b"ok");
    }

    #[test]
    fn read_exact_reports_error_on_failure() {
        let sys = FakeVsock::default();
        sys.reads
            .borrow_mut()
            .push_back(ReadStep::Fail(ErrorKind::ConnectionReset));
        let mut buf = [0u8; 2];
        assert_eq!(read_exact_with(&sys, 3, &mut buf), ReadOutcome::Error);
    }

    #[test]
    fn write_all_accumulates_partial_writes() {
        let sys = FakeVsock::default();
        sys.writes.borrow_mut().push_back(WriteStep::Wrote(1));
        sys.writes.borrow_mut().push_back(WriteStep::Wrote(2));
        assert!(write_all_with(&sys, 3, b"abc"));
    }

    #[test]
    fn write_all_retries_on_interrupt() {
        let sys = FakeVsock::default();
        sys.writes.borrow_mut().push_back(WriteStep::Interrupted);
        sys.writes.borrow_mut().push_back(WriteStep::Wrote(3));
        assert!(write_all_with(&sys, 3, b"abc"));
    }

    #[test]
    fn write_all_fails_on_zero_or_error() {
        let sys = FakeVsock::default();
        sys.writes.borrow_mut().push_back(WriteStep::Wrote(0));
        assert!(!write_all_with(&sys, 3, b"abc"));

        let sys = FakeVsock::default();
        sys.writes
            .borrow_mut()
            .push_back(WriteStep::Fail(ErrorKind::BrokenPipe));
        assert!(!write_all_with(&sys, 3, b"abc"));
    }
}
