use std::ffi::OsString;
use std::path::Path;

use anyhow::Result;

#[cfg(target_os = "macos")]
use std::os::fd::{AsRawFd, OwnedFd};

use super::super::cloud_hypervisor::process::{Child, RealChild, RealSpawner};
use super::{Backend, Host, NetLayout, START_TIMEOUT};
#[cfg(target_os = "macos")]
use super::{NetAttachment, RunningNetDev, connect_datagram, ensure_gvproxy, start_with};
#[cfg(target_os = "linux")]
use super::{VhostUserNet, located_passt, random_guest_mac, start_passt_with};

/// A run's network backend: the process that serves its link and what the VM is attached to, both held for exactly as long as the guest.
pub struct NetDevice {
    pub backend: Backend,
    #[cfg(target_os = "macos")]
    socket: Option<OwnedFd>,
    #[cfg(target_os = "linux")]
    link: Option<VhostUserNet>,
    child: Option<RealChild>,
}

#[cfg(target_os = "macos")]
impl NetDevice {
    pub fn attachment(&self) -> NetAttachment {
        match &self.socket {
            Some(fd) => NetAttachment::DatagramFd(fd.as_raw_fd()),
            None => NetAttachment::Nat,
        }
    }
}

#[cfg(target_os = "linux")]
impl NetDevice {
    pub fn attachment(&self) -> Option<VhostUserNet> {
        self.link.clone()
    }
}

impl Drop for NetDevice {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.start_kill();
        }
    }
}

#[cfg(target_os = "macos")]
pub async fn start(
    run_dir: &Path,
    env_get: impl Fn(&str) -> Option<OsString>,
) -> Result<NetDevice> {
    let backend = Backend::from_env(Host::THIS, &env_get)?;
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

#[cfg(target_os = "linux")]
pub async fn start(
    run_dir: &Path,
    env_get: impl Fn(&str) -> Option<OsString>,
) -> Result<NetDevice> {
    let backend = Backend::from_env(Host::THIS, &env_get)?;
    if backend == Backend::Off {
        return Ok(NetDevice {
            backend,
            link: None,
            child: None,
        });
    }
    let program = located_passt(&env_get)?;
    let layout = NetLayout::for_run_dir(run_dir);
    let child = start_passt_with(&RealSpawner, &program, &layout, START_TIMEOUT).await?;
    Ok(NetDevice {
        backend,
        link: Some(VhostUserNet {
            socket: layout.backend,
            mac: random_guest_mac(),
        }),
        child: Some(child),
    })
}
