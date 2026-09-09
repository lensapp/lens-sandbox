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

/// Only the macOS backend attaches a NAT interface, so a Linux guest keeps its no-interface behaviour whatever the environment says.
pub const SUPPORTED: bool = cfg!(target_os = "macos");

/// A reservation lives exactly as long as the guest that holds it: a failed launch gives the address back at once.
pub struct Lease {
    owner: String,
    pub mac: String,
    pub net: GuestNet,
}

impl Drop for Lease {
    fn drop(&mut self) {
        release(&self.owner);
    }
}

#[derive(Clone)]
pub struct AddressSelection {
    allocator: Arc<Allocator>,
    owner: String,
}

impl AddressSelection {
    pub fn new(allocator: Arc<Allocator>, owner: impl Into<String>) -> Self {
        Self {
            allocator,
            owner: owner.into(),
        }
    }

    pub fn confirm(&self, address: std::net::Ipv4Addr) -> Result<()> {
        self.allocator.select(&self.owner, address)?;
        Ok(())
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
    pub fn selection(&self) -> AddressSelection {
        AddressSelection::new(allocator().clone(), self.owner.clone())
    }

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

/// The only entry point a boot path uses: `None` means this build boots on DHCP, exactly as it did before.
pub async fn reserve(owner: &str, vm_id: &str) -> Result<Option<Lease>> {
    if !SUPPORTED || !super::enabled(|k| std::env::var(k).ok()) {
        return Ok(None);
    }
    let observed =
        crate::vm::host_net::observe_host_network(&RealHostNetwork, 20, Duration::from_millis(250))
            .await?;
    allocator().set_network(observed);
    let mac = super::mac_for(vm_id);
    let net = allocator().reserve(owner, &mac)?;
    log::info!(
        "Address",
        "{} via {} for {owner}",
        net.candidates
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(" or "),
        net.gateway
    );
    Ok(Some(Lease {
        owner: owner.to_string(),
        mac,
        net,
    }))
}

fn release(owner: &str) {
    if super::enabled(|k| std::env::var(k).ok()) {
        allocator().release(owner);
    }
}
