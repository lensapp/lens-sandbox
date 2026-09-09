use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use anyhow::Result;
use lns_session::GuestNet;

use super::{Allocator, ReservationStore};
use crate::log;
use crate::vm::host_net::real::{RealHostFiles, RealNeighbors};

struct FileStore {
    path: PathBuf,
}

impl ReservationStore for FileStore {
    fn load(&self) -> std::io::Result<String> {
        std::fs::read_to_string(&self.path)
    }

    fn save(&self, contents: &str) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&self.path, contents)
    }
}

fn allocator() -> &'static Arc<Allocator> {
    static ALLOCATOR: OnceLock<Arc<Allocator>> = OnceLock::new();
    ALLOCATOR.get_or_init(|| {
        let path = crate::cache::root()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("guest-addresses.json");
        Arc::new(Allocator::new(
            Arc::new(RealHostFiles),
            Arc::new(RealNeighbors),
            Arc::new(crate::clock::RealClock),
            Arc::new(FileStore { path }),
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

/// The only entry point a boot path uses: `None` means this build boots on DHCP, exactly as it did before.
pub fn reserve(owner: &str, vm_id: &str) -> Result<Option<Lease>> {
    if !SUPPORTED || !super::enabled(|k| std::env::var(k).ok()) {
        return Ok(None);
    }
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
    for conflict in allocator().conflicts() {
        log::warn!(
            "the host has leased {} to {} while run {} holds it",
            conflict.address,
            conflict.holder.as_deref().unwrap_or("an unknown guest"),
            conflict.owner
        );
    }
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

/// A guest dies with the service that hosts it, so a reservation from a previous process names nobody and goes back at start.
pub fn reconcile_at_start() {
    if SUPPORTED && super::enabled(|k| std::env::var(k).ok()) {
        let live = crate::run_registry::snapshot()
            .into_iter()
            .map(|run| run.id)
            .collect();
        allocator().reconcile(&live);
    }
}
