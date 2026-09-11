use std::ffi::OsString;
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::Path;

use anyhow::Result;

use super::super::cloud_hypervisor::process::{Child, RealChild, RealSpawner};
use super::{
    Backend, NetAttachment, NetLayout, RunningNetDev, START_TIMEOUT, connect_datagram,
    ensure_gvproxy, start_with,
};

/// A run's network backend: the gvproxy that serves its link and the socket Vz is attached to, both held for exactly as long as the guest.
pub struct NetDevice {
    pub backend: Backend,
    socket: Option<OwnedFd>,
    child: Option<RealChild>,
}

impl NetDevice {
    pub fn attachment(&self) -> NetAttachment {
        match &self.socket {
            Some(fd) => NetAttachment::DatagramFd(fd.as_raw_fd()),
            None => NetAttachment::Nat,
        }
    }
}

impl Drop for NetDevice {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.start_kill();
        }
    }
}

pub async fn start(
    run_dir: &Path,
    env_get: impl Fn(&str) -> Option<OsString>,
) -> Result<NetDevice> {
    let backend = Backend::from_env(&env_get)?;
    if backend == Backend::Vmnet {
        return Ok(NetDevice {
            backend,
            socket: None,
            child: None,
        });
    }
    let program = ensure_gvproxy(&crate::cache::root()?, &env_get).await?;
    let layout = NetLayout::for_run_dir(run_dir);
    let RunningNetDev { child, fd } = start_with(
        &RealSpawner,
        &program,
        &layout,
        START_TIMEOUT,
        connect_datagram,
    )
    .await?;
    Ok(NetDevice {
        backend,
        socket: Some(fd),
        child: Some(child),
    })
}
