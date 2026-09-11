use std::ffi::OsString;
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::Arc;

use anyhow::Result;

use super::engine::Running;
use super::policy::Boundary;
use super::resolver::SystemResolver;
use super::{Backend, GUEST_NETWORK, GUEST_PREFIX, NetAttachment, device_pair, serve};

const RESOLV_CONF: &str = "/etc/resolv.conf";

/// A run's network: the socket Vz is attached to and every task serving it, both held for exactly as long as the guest.
pub struct NetDevice {
    pub backend: Backend,
    socket: Option<OwnedFd>,
    _running: Option<Running>,
}

impl NetDevice {
    pub fn attachment(&self) -> NetAttachment {
        match &self.socket {
            Some(fd) => NetAttachment::DatagramFd(fd.as_raw_fd()),
            None => NetAttachment::Nat,
        }
    }
}

pub fn start(env_get: impl Fn(&str) -> Option<OsString>) -> Result<NetDevice> {
    let backend = Backend::from_env(&env_get)?;
    if backend == Backend::Vmnet {
        return Ok(NetDevice {
            backend,
            socket: None,
            _running: None,
        });
    }
    let pair = device_pair()?;
    let resolv_conf = std::fs::read_to_string(RESOLV_CONF).unwrap_or_default();
    let running = serve(
        pair.host,
        Arc::new(SystemResolver::from_resolv_conf(&resolv_conf)),
        Boundary::around(GUEST_NETWORK, GUEST_PREFIX),
    )?;
    Ok(NetDevice {
        backend,
        socket: Some(pair.vm),
        _running: Some(running),
    })
}
