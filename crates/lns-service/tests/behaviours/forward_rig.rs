use std::net::SocketAddr;
use std::sync::Mutex;

use lns_service::forward::{ForwardError, ForwardSpec, PortForwarder};

#[derive(Debug, Default)]
pub struct ForwardFake {
    binds: Mutex<Vec<(SocketAddr, u16)>>,
    unbinds: Mutex<Vec<SocketAddr>>,
    fail_on: Mutex<Option<SocketAddr>>,
}

impl ForwardFake {
    pub fn fail_on(&self, addr: SocketAddr) {
        *self.fail_on.lock().unwrap() = Some(addr);
    }

    pub fn was_bound(&self, addr: SocketAddr, container_port: u16) -> bool {
        self.binds
            .lock()
            .unwrap()
            .iter()
            .any(|(a, c)| *a == addr && *c == container_port)
    }

    pub fn bind_count(&self) -> usize {
        self.binds.lock().unwrap().len()
    }

    pub fn was_unbound(&self, addr: SocketAddr) -> bool {
        self.unbinds.lock().unwrap().contains(&addr)
    }
}

impl PortForwarder for ForwardFake {
    fn bind(&self, spec: &ForwardSpec) -> Result<(), ForwardError> {
        if *self.fail_on.lock().unwrap() == Some(spec.bind) {
            return Err(ForwardError::AddrInUse(spec.bind));
        }
        self.binds
            .lock()
            .unwrap()
            .push((spec.bind, spec.container_port));
        Ok(())
    }

    fn unbind(&self, bind: SocketAddr) {
        self.unbinds.lock().unwrap().push(bind);
    }
}
