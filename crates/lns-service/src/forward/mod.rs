use std::net::SocketAddr;
use std::sync::Arc;

use lns_ipc::PortPublish;

pub mod real;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForwardSpec {
    pub bind: SocketAddr,
    pub container_port: u16,
}

pub fn plan(published: &[PortPublish]) -> Vec<ForwardSpec> {
    published
        .iter()
        .map(|p| ForwardSpec {
            bind: SocketAddr::new(p.host_ip, p.host_port),
            container_port: p.container_port,
        })
        .collect()
}

#[derive(Debug)]
pub enum ForwardError {
    AddrInUse(SocketAddr),
    Other(String),
}

impl std::fmt::Display for ForwardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AddrInUse(addr) => write!(f, "address already in use: {addr}"),
            Self::Other(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for ForwardError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcceptAction {
    Stop,
    Retry,
    Backoff,
}

pub fn classify_accept_error(e: &std::io::Error) -> AcceptAction {
    if is_transient_resource_error(e) {
        AcceptAction::Backoff
    } else if is_recoverable_accept_error(e) {
        AcceptAction::Retry
    } else {
        AcceptAction::Stop
    }
}

fn is_recoverable_accept_error(e: &std::io::Error) -> bool {
    matches!(
        e.kind(),
        std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::Interrupted
    )
}

fn is_transient_resource_error(e: &std::io::Error) -> bool {
    matches!(
        e.raw_os_error(),
        Some(libc::EMFILE | libc::ENFILE | libc::ENOBUFS | libc::ENOMEM)
    )
}

pub trait PortForwarder: Send + Sync {
    fn bind(&self, spec: &ForwardSpec) -> Result<(), ForwardError>;
    fn unbind(&self, bind: SocketAddr);
}

#[must_use = "dropping the guard tears down the published ports"]
pub struct ForwardGuard {
    forwarder: Arc<dyn PortForwarder>,
    bound: Vec<SocketAddr>,
}

pub fn establish(
    forwarder: Arc<dyn PortForwarder>,
    specs: &[ForwardSpec],
) -> Result<ForwardGuard, ForwardError> {
    let mut bound = Vec::with_capacity(specs.len());
    for spec in specs {
        if let Err(e) = forwarder.bind(spec) {
            unbind_all(forwarder.as_ref(), &bound);
            return Err(e);
        }
        bound.push(spec.bind);
    }
    Ok(ForwardGuard { forwarder, bound })
}

fn unbind_all(forwarder: &dyn PortForwarder, addrs: &[SocketAddr]) {
    for addr in addrs {
        forwarder.unbind(*addr);
    }
}

impl ForwardGuard {
    pub fn bound_addrs(&self) -> &[SocketAddr] {
        &self.bound
    }
}

impl std::fmt::Debug for ForwardGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ForwardGuard")
            .field("bound", &self.bound)
            .finish()
    }
}

impl Drop for ForwardGuard {
    fn drop(&mut self) {
        unbind_all(self.forwarder.as_ref(), &self.bound);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lns_ipc::Protocol;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeForwarder {
        bound: Mutex<Vec<SocketAddr>>,
        unbound: Mutex<Vec<SocketAddr>>,
        fail_on: Option<SocketAddr>,
    }

    impl FakeForwarder {
        fn failing(addr: SocketAddr) -> Self {
            Self {
                fail_on: Some(addr),
                ..Self::default()
            }
        }
        fn bound(&self) -> Vec<SocketAddr> {
            self.bound.lock().unwrap().clone()
        }
        fn unbound(&self) -> Vec<SocketAddr> {
            self.unbound.lock().unwrap().clone()
        }
    }

    impl PortForwarder for FakeForwarder {
        fn bind(&self, spec: &ForwardSpec) -> Result<(), ForwardError> {
            if self.fail_on == Some(spec.bind) {
                return Err(ForwardError::AddrInUse(spec.bind));
            }
            self.bound.lock().unwrap().push(spec.bind);
            Ok(())
        }
        fn unbind(&self, bind: SocketAddr) {
            self.unbound.lock().unwrap().push(bind);
        }
    }

    fn pp(ip: &str, host: u16, container: u16) -> PortPublish {
        PortPublish {
            host_ip: ip.parse().unwrap(),
            host_port: host,
            container_port: container,
            protocol: Protocol::Tcp,
        }
    }

    fn addr(ip: &str, port: u16) -> SocketAddr {
        SocketAddr::new(ip.parse().unwrap(), port)
    }

    #[test]
    fn plan_maps_each_publish_to_a_bind_addr_and_container_port() {
        let specs = plan(&[pp("127.0.0.1", 8080, 3003)]);
        assert_eq!(
            specs,
            vec![ForwardSpec {
                bind: addr("127.0.0.1", 8080),
                container_port: 3003,
            }]
        );
    }

    #[test]
    fn establish_binds_every_spec() {
        let fake = Arc::new(FakeForwarder::default());
        let specs = plan(&[pp("127.0.0.1", 3003, 3003), pp("127.0.0.1", 9090, 9090)]);
        let guard = establish(fake.clone(), &specs).expect("establish");
        assert_eq!(
            guard.bound_addrs(),
            &[addr("127.0.0.1", 3003), addr("127.0.0.1", 9090)]
        );
        assert_eq!(
            fake.bound(),
            vec![addr("127.0.0.1", 3003), addr("127.0.0.1", 9090)]
        );
    }

    #[test]
    fn establish_rolls_back_already_bound_ports_when_one_fails() {
        let fake = Arc::new(FakeForwarder::failing(addr("127.0.0.1", 9090)));
        let specs = plan(&[pp("127.0.0.1", 3003, 3003), pp("127.0.0.1", 9090, 9090)]);
        let err = establish(fake.clone(), &specs).expect_err("second bind must fail");
        assert!(matches!(err, ForwardError::AddrInUse(a) if a == addr("127.0.0.1", 9090)));
        assert_eq!(
            fake.unbound(),
            vec![addr("127.0.0.1", 3003)],
            "the first bind must be rolled back on failure"
        );
    }

    #[test]
    fn dropping_the_guard_unbinds_every_port() {
        let fake = Arc::new(FakeForwarder::default());
        let specs = plan(&[pp("127.0.0.1", 3003, 3003)]);
        let guard = establish(fake.clone(), &specs).expect("establish");
        assert!(fake.unbound().is_empty());
        drop(guard);
        assert_eq!(fake.unbound(), vec![addr("127.0.0.1", 3003)]);
    }

    #[test]
    fn a_held_guard_keeps_ports_bound_until_it_is_dropped() {
        let fake = Arc::new(FakeForwarder::default());
        let specs = plan(&[pp("127.0.0.1", 3003, 3003)]);
        let guard = establish(fake.clone(), &specs).expect("establish");
        assert!(
            fake.unbound().is_empty(),
            "a detached run keeps its ports up"
        );
        drop(guard);
        assert_eq!(fake.unbound(), vec![addr("127.0.0.1", 3003)]);
    }

    #[test]
    fn establish_with_no_specs_binds_nothing() {
        let fake = Arc::new(FakeForwarder::default());
        let guard = establish(fake.clone(), &[]).expect("establish");
        assert!(guard.bound_addrs().is_empty());
        assert!(fake.bound().is_empty());
    }

    #[test]
    fn guard_debug_lists_bound_addrs() {
        let fake = Arc::new(FakeForwarder::default());
        let guard = establish(fake, &plan(&[pp("127.0.0.1", 3003, 3003)])).expect("establish");
        let dbg = format!("{guard:?}");
        assert!(dbg.contains("ForwardGuard"), "got: {dbg}");
        assert!(dbg.contains("3003"), "got: {dbg}");
    }

    #[test]
    fn forward_error_display_renders_both_variants() {
        assert!(
            ForwardError::AddrInUse(addr("127.0.0.1", 3003))
                .to_string()
                .contains("address already in use")
        );
        assert_eq!(ForwardError::Other("boom".into()).to_string(), "boom");
    }

    #[test]
    fn classify_accept_error_backs_off_on_resource_exhaustion() {
        for code in [libc::EMFILE, libc::ENFILE, libc::ENOBUFS, libc::ENOMEM] {
            let e = std::io::Error::from_raw_os_error(code);
            assert_eq!(
                classify_accept_error(&e),
                AcceptAction::Backoff,
                "code {code}"
            );
        }
    }

    #[test]
    fn classify_accept_error_retries_dropped_connections() {
        for kind in [
            std::io::ErrorKind::ConnectionAborted,
            std::io::ErrorKind::ConnectionReset,
            std::io::ErrorKind::Interrupted,
        ] {
            assert_eq!(
                classify_accept_error(&std::io::Error::from(kind)),
                AcceptAction::Retry,
                "kind {kind:?}"
            );
        }
    }

    #[test]
    fn classify_accept_error_stops_on_unexpected_errors() {
        assert_eq!(
            classify_accept_error(&std::io::Error::from(std::io::ErrorKind::PermissionDenied)),
            AcceptAction::Stop
        );
        assert_eq!(
            classify_accept_error(&std::io::Error::from_raw_os_error(libc::EBADF)),
            AcceptAction::Stop
        );
    }
}
