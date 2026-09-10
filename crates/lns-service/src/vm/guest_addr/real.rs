use std::sync::{Arc, OnceLock};
use std::time::Duration;

use tracing::Instrument;

use anyhow::Result;
use lns_session::GuestNet;

use super::{Allocator, Conflict};
use crate::log;
use crate::vm::host_net::real::{RealHostFiles, RealHostNetwork, RealNeighbors};

fn allocator() -> &'static Arc<Allocator> {
    static ALLOCATOR: OnceLock<Arc<Allocator>> = OnceLock::new();
    ALLOCATOR.get_or_init(|| {
        Arc::new(Allocator::new(
            Arc::new(RealHostFiles),
            Arc::new(RealNeighbors),
            Arc::new(crate::clock::RealClock),
        ))
    })
}

/// How long the host waits for the shared bridge its own guest is bringing up, and for that guest to report the address it took.
const BRIDGE_ATTEMPTS: usize = 40;
const BRIDGE_RETRY: Duration = Duration::from_millis(250);
const REPLY_TIMEOUT: Duration = Duration::from_secs(20);

/// Only the macOS backend attaches a NAT interface, so a Linux guest keeps its no-interface behaviour whatever the environment says.
pub const SUPPORTED: bool = cfg!(target_os = "macos");

/// A reservation lives exactly as long as the guest that holds it: a failed launch gives the address back at once.
pub struct Lease {
    owner: String,
    pub net: GuestNet,
    pub address: std::net::Ipv4Addr,
}

impl Drop for Lease {
    fn drop(&mut self) {
        release(&self.owner);
    }
}

pub struct ConflictMonitor {
    stop: Option<tokio::sync::oneshot::Sender<()>>,
}

impl Drop for ConflictMonitor {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}

impl Lease {
    pub fn monitor(&self) -> ConflictMonitor {
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let report: Arc<dyn Fn(Conflict) + Send + Sync> = Arc::new(|conflict| {
            log::warn!(
                "the host has leased {} to {} while run {} holds it",
                conflict.address,
                conflict.holder.as_deref().unwrap_or("an unknown guest"),
                conflict.owner
            );
        });
        tokio::spawn(
            super::monitor_conflicts(
                allocator().clone(),
                self.owner.clone(),
                Duration::from_secs(30),
                stopped,
                report,
            )
            .instrument(tracing::Span::current()),
        );
        ConflictMonitor { stop: Some(stop) }
    }
}

/// The hardware address a run's guest will boot with, or `None` when this build boots on DHCP exactly as it did before. Known before the VMM starts; the addresses it will be offered are not.
pub fn planned_mac(vm_id: &str) -> Option<String> {
    (SUPPORTED && super::enabled(|k| std::env::var(k).ok())).then(|| super::mac_for(vm_id))
}

/// Address a guest that is already running: the bridge it is attached to only exists because it booted, so this is the first moment the host can see the network at all.
pub async fn address_guest(
    connector: &dyn crate::vm::GuestTransport,
    owner: &str,
    mac: &str,
) -> Result<Lease> {
    let addressed = super::address_guest(
        allocator(),
        &RealHostNetwork,
        || async {
            let fd = connector
                .connect(lns_session::BROKER_PORT, Duration::from_secs(10))
                .await
                .map_err(std::io::Error::other)?;
            bootstrap_stream(fd)
        },
        owner,
        mac,
        super::BootTiming {
            attempts: BRIDGE_ATTEMPTS,
            retry: BRIDGE_RETRY,
            reply: REPLY_TIMEOUT,
        },
    )
    .await?;
    Ok(Lease {
        owner: owner.to_string(),
        net: addressed.net,
        address: addressed.address,
    })
}

fn bootstrap_stream(fd: std::os::fd::RawFd) -> std::io::Result<tokio::net::UnixStream> {
    use std::os::fd::FromRawFd;

    // SAFETY: the transport hands ownership of fd over; OwnedFd closes it on drop.
    let owned = unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) };
    crate::vm::session_client::real::set_nonblocking(&owned)?;
    tokio::net::UnixStream::from_std(std::os::unix::net::UnixStream::from(owned))
}

fn release(owner: &str) {
    if super::enabled(|k| std::env::var(k).ok()) {
        allocator().release(owner);
    }
}
