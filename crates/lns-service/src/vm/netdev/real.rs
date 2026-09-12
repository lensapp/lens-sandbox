use std::ffi::OsString;
use std::net::Ipv4Addr;
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;

use super::dns::{REFRESH_AFTER, Resolvers};
use super::engine::Running;
use super::resolver::{HostSources, HostUpstream};
use super::{Backend, NetAttachment, Subnet, device_pair, overlap_warning, serve};
use crate::log;

const RESOLV_CONF: &str = "/etc/resolv.conf";

const SCUTIL: &str = "/usr/sbin/scutil";

/// A run's network: the socket Vz is attached to and every task serving it, both held for exactly as long as the guest.
pub struct NetDevice {
    pub backend: Backend,
    pub detail: String,
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
            detail: "Apple NAT bridge".to_string(),
            socket: None,
            _running: None,
        });
    }
    let subnet = Subnet::from_env(&env_get)?;
    if let Some(warning) = overlap_warning(subnet, &host_addresses()) {
        log::warn!("{warning}");
    }
    let sources = Arc::new(HostSources::new(RESOLV_CONF, Some(Path::new(SCUTIL))));
    let pair = device_pair()?;
    let running = serve(
        pair.host,
        subnet,
        Arc::new(Resolvers::new(sources, REFRESH_AFTER)),
        Arc::new(HostUpstream),
    )?;
    Ok(NetDevice {
        backend,
        detail: subnet.cidr(),
        socket: Some(pair.vm),
        _running: Some(running),
    })
}

/// Every IPv4 address this host holds, so a guest subnet that shadows one can be named before the guest boots.
fn host_addresses() -> Vec<Ipv4Addr> {
    let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs writes one owned list head through the pointer and reports failure with a non-zero return.
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return Vec::new();
    }
    let mut found = Vec::new();
    let mut entry = head;
    while !entry.is_null() {
        // SAFETY: the list is owned by us until freeifaddrs below, and every node's `ifa_next` is either a node or null.
        let node = unsafe { &*entry };
        if let Some(address) = ipv4_of(node.ifa_addr) {
            found.push(address);
        }
        entry = node.ifa_next;
    }
    // SAFETY: `head` is the list getifaddrs just gave us and is freed exactly once.
    unsafe { libc::freeifaddrs(head) };
    found
}

fn ipv4_of(address: *const libc::sockaddr) -> Option<Ipv4Addr> {
    if address.is_null() {
        return None;
    }
    // SAFETY: the pointer comes from getifaddrs, so it points at a sockaddr whose family we read before casting further.
    let family = unsafe { (*address).sa_family };
    if i32::from(family) != libc::AF_INET {
        return None;
    }
    // SAFETY: AF_INET names a sockaddr_in, which is what this node's address is.
    let inet = unsafe { &*address.cast::<libc::sockaddr_in>() };
    Some(Ipv4Addr::from(u32::from_be(inet.sin_addr.s_addr)))
}
