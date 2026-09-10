use std::collections::{HashMap, HashSet};
use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::host_net::{HostFiles, HostNetwork, HostNetworkSource, Neighbors, read_active_leases};
use super::net_bootstrap::{BootstrapError, Bootstrapped};
use crate::clock::Clock;
use lns_session::GuestNet;

pub mod real;

/// Off by default: an address lns picks can still be leased to someone else's guest later, and that race has no fix on macOS 15 (see `docs/running-workloads.md`).
pub const ENABLE_ENV: &str = "LNS_STATIC_GUEST_NET";

/// The guest picks from what it is sent without asking again, so it is sent spares.
pub const CANDIDATES_PER_GUEST: usize = 3;

/// A guest that keeps its hardware address keeps whatever the host has learned about it, so the address is derived from the run rather than drawn at random.
pub fn mac_for(vm_id: &str) -> String {
    use sha2::{Digest, Sha256};

    let digest = Sha256::digest(vm_id.as_bytes());
    format!(
        "02:00:{:02x}:{:02x}:{:02x}:{:02x}",
        digest[0], digest[1], digest[2], digest[3]
    )
}

pub fn enabled(get: impl Fn(&str) -> Option<String>) -> bool {
    get(ENABLE_ENV).as_deref() == Some("1")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reservation {
    pub owner: String,
    pub mac: String,
    pub candidates: Vec<Ipv4Addr>,
}

/// Apple's DHCP server leased an address lns had reserved: the run that holds it is no longer alone on that address.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Conflict {
    pub owner: String,
    pub address: Ipv4Addr,
    pub holder: Option<String>,
}

#[derive(Debug)]
pub enum AllocError {
    Exhausted { network: HostNetwork },
    Leases(std::io::Error),
    Neighbors(std::io::Error),
}

impl std::fmt::Display for AllocError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Exhausted { network } => write!(
                f,
                "no free address left on {}/{}: every one is leased, answered ARP or is reserved for another run",
                network.network, network.prefix_len
            ),
            Self::Leases(e) => write!(
                f,
                "the host lease file could not be read, so no address is known to be free: {e}"
            ),
            Self::Neighbors(e) => write!(
                f,
                "the host neighbor table could not be observed, so no address is known to be free: {e}"
            ),
        }
    }
}

impl std::error::Error for AllocError {}

/// Why no address could be reserved for a booting guest: the host network could not be discovered, or it holds nothing free. Either way the guest never started.
#[derive(Debug)]
pub enum ReserveError {
    Network(std::io::Error),
    Alloc(AllocError),
    Bootstrap(BootstrapError),
    Select(SelectError),
}

impl ReserveError {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Network(_) => "host_network_undiscoverable",
            Self::Alloc(AllocError::Exhausted { .. }) => "host_addresses_exhausted",
            Self::Alloc(_) => "host_address_state_unreadable",
            Self::Bootstrap(_) | Self::Select(_) => "guest_address_not_applied",
        }
    }

    /// The cause plus what the user can do about it, for the host's own error render.
    pub fn explain(&self) -> String {
        format!(
            "{self}\n  remedy: let every run exit, then start again, or unset {ENABLE_ENV} to boot on the host DHCP server"
        )
    }
}

impl std::fmt::Display for ReserveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Network(e) => write!(f, "the host shared network could not be discovered: {e}"),
            Self::Alloc(e) => write!(f, "{e}"),
            Self::Bootstrap(e) => write!(f, "{e}"),
            Self::Select(e) => write!(
                f,
                "the address the guest reported is not the one the host holds for it: {e}"
            ),
        }
    }
}

impl std::error::Error for ReserveError {}

/// The one reservation path both boot paths take: observe the shared network the running VMM brought up, then reserve on it. There is nothing to read before the VMM starts — `bridge100` is created by the first guest attached to it — so this is asked afterwards and never guessed at.
pub async fn reserve_on(
    allocator: &Allocator,
    live: &dyn HostNetworkSource,
    owner: &str,
    mac: &str,
    attempts: usize,
    retry: Duration,
) -> Result<GuestNet, ReserveError> {
    let network = crate::vm::host_net::observe_host_network(live, attempts, retry)
        .await
        .map_err(ReserveError::Network)?;
    allocator.set_network(network);
    allocator.reserve(owner, mac).map_err(ReserveError::Alloc)
}

/// How long the host waits for the shared network the VMM is bringing up, and for the guest to answer the plan it is sent.
#[derive(Debug, Clone, Copy)]
pub struct BootTiming {
    pub attempts: usize,
    pub retry: Duration,
    pub reply: Duration,
}

/// The address a guest is running on, and the reservation narrowed to it.
#[derive(Debug)]
pub struct Addressed {
    pub net: GuestNet,
    pub address: Ipv4Addr,
}

/// Why no workload started: the host could not address the guest, or the guest named its own reason for taking no address.
#[derive(Debug)]
pub enum AddressError {
    Reserve(ReserveError),
    Refused(lns_session::BrokerExitReason),
}

impl std::fmt::Display for AddressError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Reserve(e) => write!(f, "{e}"),
            Self::Refused(reason) => write!(f, "{}", reason.summary()),
        }
    }
}

impl std::error::Error for AddressError {}

/// Everything between a started VMM and a started workload: observe the network the VMM brought up, reserve on it, tell the guest over the control channel, and narrow the reservation to the address the guest proved it could take. A guest that answers nothing usable holds nothing.
pub async fn address_guest<S, F, Fut>(
    allocator: &Allocator,
    live: &dyn HostNetworkSource,
    connect: F,
    owner: &str,
    mac: &str,
    timing: BootTiming,
) -> Result<Addressed, AddressError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    F: FnOnce() -> Fut,
    Fut: Future<Output = std::io::Result<S>>,
{
    let net = reserve_on(allocator, live, owner, mac, timing.attempts, timing.retry)
        .await
        .map_err(AddressError::Reserve)?;
    let outcome = bootstrap_guest(connect, &net, timing.reply).await;
    let address = match outcome {
        Ok(Bootstrapped::Applied(address)) => address,
        Ok(Bootstrapped::Refused(reason)) => {
            allocator.release(owner);
            return Err(AddressError::Refused(reason));
        }
        Err(e) => {
            allocator.release(owner);
            return Err(AddressError::Reserve(ReserveError::Bootstrap(e)));
        }
    };
    if let Err(e) = allocator.select(owner, address) {
        allocator.release(owner);
        return Err(AddressError::Reserve(ReserveError::Select(e)));
    }
    crate::log::info!("Address", "guest assigned {address}");
    Ok(Addressed { net, address })
}

async fn bootstrap_guest<S, F, Fut>(
    connect: F,
    net: &GuestNet,
    reply: Duration,
) -> Result<Bootstrapped, BootstrapError>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    F: FnOnce() -> Fut,
    Fut: Future<Output = std::io::Result<S>>,
{
    let stream = connect().await.map_err(BootstrapError::Io)?;
    crate::vm::net_bootstrap::configure(stream, net, reply).await
}

#[derive(Debug, PartialEq, Eq)]
pub enum SelectError {
    UnknownOwner,
    NotOffered(Ipv4Addr),
}

impl std::fmt::Display for SelectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownOwner => write!(f, "the guest has no address reservation"),
            Self::NotOffered(address) => {
                write!(f, "the guest selected unoffered address {address}")
            }
        }
    }
}

impl std::error::Error for SelectError {}

pub trait ConflictSource: Send + Sync {
    fn conflicts_for(&self, owner: &str) -> Vec<Conflict>;
}

pub async fn monitor_conflicts(
    source: Arc<dyn ConflictSource>,
    owner: String,
    every: Duration,
    mut stop: tokio::sync::oneshot::Receiver<()>,
    report: Arc<dyn Fn(Conflict) + Send + Sync>,
) {
    let mut interval = tokio::time::interval(every);
    let mut reported = HashSet::new();
    loop {
        tokio::select! {
            biased;
            _ = &mut stop => return,
            _ = interval.tick() => {
                for conflict in source.conflicts_for(&owner) {
                    if reported.insert(conflict.clone()) {
                        report(conflict);
                    }
                }
            }
        }
    }
}

pub struct Allocator {
    files: Arc<dyn HostFiles>,
    neighbors: Arc<dyn Neighbors>,
    clock: Arc<dyn Clock>,
    network: Mutex<Option<HostNetwork>>,
    held: Mutex<HashMap<String, Reservation>>,
}

impl Allocator {
    pub fn new(
        files: Arc<dyn HostFiles>,
        neighbors: Arc<dyn Neighbors>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        Self {
            files,
            neighbors,
            clock,
            network: Mutex::new(None),
            held: Mutex::new(HashMap::new()),
        }
    }

    pub fn set_network(&self, network: HostNetwork) {
        *self.network.lock().expect("host network poisoned") = Some(network);
    }

    /// One lock spans the read of every exclusion and the write of the new record, so two guests booting together cannot pick the same address.
    pub fn reserve(&self, owner: &str, mac: &str) -> Result<GuestNet, AllocError> {
        let network = self
            .network
            .lock()
            .expect("host network poisoned")
            .clone()
            .unwrap_or_default();
        let now = self.clock.now_unix();
        let leases = match read_active_leases(self.files.as_ref(), now) {
            Ok(leases) => leases,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(AllocError::Leases(e)),
        };
        let neighbours = self.neighbors.observed().map_err(AllocError::Neighbors)?;
        let mut held = self.held.lock().expect("reservation table poisoned");

        let mut taken: HashSet<Ipv4Addr> = HashSet::new();
        taken.insert(network.network);
        taken.insert(network.broadcast());
        taken.insert(network.gateway);
        taken.extend(leases.iter().map(|lease| lease.address));
        taken.extend(neighbours);
        for reservation in held.values() {
            if reservation.owner != owner {
                taken.extend(reservation.candidates.iter().copied());
            }
        }

        let candidates: Vec<Ipv4Addr> = network
            .usable_high_to_low()
            .filter(|addr| !taken.contains(addr))
            .take(CANDIDATES_PER_GUEST)
            .collect();
        if candidates.is_empty() {
            return Err(AllocError::Exhausted { network });
        }

        held.insert(
            owner.to_string(),
            Reservation {
                owner: owner.to_string(),
                mac: mac.to_string(),
                candidates: candidates.clone(),
            },
        );
        Ok(GuestNet {
            candidates,
            prefix_len: network.prefix_len,
            gateway: network.gateway,
            dns: vec![network.gateway],
        })
    }

    /// Called on teardown and on a failed launch alike: an address a guest never booted with is free at once.
    pub fn select(&self, owner: &str, address: Ipv4Addr) -> Result<(), SelectError> {
        let mut held = self.held.lock().expect("reservation table poisoned");
        let reservation = held.get_mut(owner).ok_or(SelectError::UnknownOwner)?;
        if !reservation.candidates.contains(&address) {
            return Err(SelectError::NotOffered(address));
        }
        reservation.candidates = vec![address];
        Ok(())
    }

    pub fn release(&self, owner: &str) {
        let mut held = self.held.lock().expect("reservation table poisoned");
        held.remove(owner);
    }

    pub fn reserved(&self) -> Vec<Reservation> {
        let held = self.held.lock().expect("reservation table poisoned");
        let mut all: Vec<Reservation> = held.values().cloned().collect();
        all.sort_by(|a, b| a.owner.cmp(&b.owner));
        all
    }

    /// The coexistence check: ARP silence at boot proves nothing later, so every reserved address is re-read against Apple's live leases.
    pub fn conflicts(&self) -> Vec<Conflict> {
        let now = self.clock.now_unix();
        let Ok(leases) = read_active_leases(self.files.as_ref(), now) else {
            return Vec::new();
        };
        let held = self.held.lock().expect("reservation table poisoned");
        let mut found = Vec::new();
        for reservation in held.values() {
            for lease in &leases {
                let ours = lease
                    .hardware
                    .as_deref()
                    .is_some_and(|hw| hw.eq_ignore_ascii_case(&reservation.mac));
                if reservation.candidates.contains(&lease.address) && !ours {
                    found.push(Conflict {
                        owner: reservation.owner.clone(),
                        address: lease.address,
                        holder: lease.hardware.clone(),
                    });
                }
            }
        }
        found.sort_by(|a, b| (&a.owner, a.address).cmp(&(&b.owner, b.address)));
        found
    }
}

impl ConflictSource for Allocator {
    fn conflicts_for(&self, owner: &str) -> Vec<Conflict> {
        self.conflicts()
            .into_iter()
            .filter(|conflict| conflict.owner == owner)
            .collect()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::vm::host_net::{LEASES_PATH, tests::FakeHostFiles};

    struct FixedClock(u64);
    impl Clock for FixedClock {
        fn now_unix(&self) -> u64 {
            self.0
        }
    }

    struct FakeNeighbors(Vec<Ipv4Addr>);
    impl Neighbors for FakeNeighbors {
        fn observed(&self) -> std::io::Result<Vec<Ipv4Addr>> {
            Ok(self.0.clone())
        }
    }

    struct UnobservableNeighbors;
    impl Neighbors for UnobservableNeighbors {
        fn observed(&self) -> std::io::Result<Vec<Ipv4Addr>> {
            Err(std::io::Error::other("arp: no such file"))
        }
    }

    fn allocator_with(files: FakeHostFiles, neighbours: Vec<Ipv4Addr>, now: u64) -> Allocator {
        Allocator::new(
            Arc::new(files),
            Arc::new(FakeNeighbors(neighbours)),
            Arc::new(FixedClock(now)),
        )
    }

    pub(crate) fn empty_host() -> Allocator {
        allocator_with(
            FakeHostFiles::new().with(LEASES_PATH, ""),
            Vec::new(),
            1_000,
        )
    }

    fn copy_reservations(from: &Allocator, to: &Allocator) {
        *to.held.lock().expect("reservations poisoned") = from
            .reserved()
            .into_iter()
            .map(|reservation| (reservation.owner.clone(), reservation))
            .collect();
    }
    fn addr(last: u8) -> Ipv4Addr {
        Ipv4Addr::new(192, 168, 64, last)
    }

    #[test]
    fn a_booting_guest_gets_three_spare_addresses_from_the_top_of_the_range() {
        let plan = empty_host()
            .reserve("run-a", "52:54:00:00:00:01")
            .expect("free host");
        assert_eq!(plan.candidates, vec![addr(254), addr(253), addr(252)]);
        assert_eq!(plan.prefix_len, 24);
        assert_eq!(plan.gateway, addr(1));
        assert_eq!(
            plan.dns,
            vec![addr(1)],
            "the gateway serves DNS on Apple's shared network"
        );
    }

    #[test]
    fn two_guests_booting_together_never_share_a_candidate() {
        let allocator = empty_host();
        let first = allocator
            .reserve("run-a", "52:54:00:00:00:01")
            .expect("first");
        let second = allocator
            .reserve("run-b", "52:54:00:00:00:02")
            .expect("second");
        assert!(
            first
                .candidates
                .iter()
                .all(|a| !second.candidates.contains(a)),
            "{first:?} vs {second:?}",
        );
        assert_eq!(second.candidates, vec![addr(251), addr(250), addr(249)]);
    }

    #[test]
    fn a_provisioner_guest_is_reserved_apart_from_the_run_that_spawned_it() {
        let allocator = empty_host();
        let run = allocator
            .reserve("run-a", "52:54:00:00:00:01")
            .expect("run");
        let tools = allocator
            .reserve("run-a/tools", "52:54:00:00:00:11")
            .expect("provisioner");
        assert!(
            tools.candidates.iter().all(|a| !run.candidates.contains(a)),
            "a provisioner boots beside its run, not on its address"
        );
        assert_eq!(allocator.reserved().len(), 2);
    }

    struct Bridge(&'static str);
    impl crate::vm::host_net::HostNetworkSource for Bridge {
        fn observe(&self) -> std::io::Result<String> {
            Ok(self.0.to_string())
        }
    }

    struct BridgeAfter(std::sync::atomic::AtomicUsize, usize);
    impl crate::vm::host_net::HostNetworkSource for BridgeAfter {
        fn observe(&self) -> std::io::Result<String> {
            let seen = self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if seen < self.1 {
                return Err(std::io::Error::other("ifconfig bridge100 exited with 1"));
            }
            Ok("\tinet 192.168.66.1 netmask 0xffffff00 broadcast 192.168.66.255".to_string())
        }
    }

    const LIVE: &str = "\tinet 192.168.66.1 netmask 0xffffff00 broadcast 192.168.66.255";

    #[tokio::test]
    async fn a_workload_and_a_provisioner_are_both_addressed_on_the_network_the_vmm_brought_up() {
        let allocator = allocator_with(
            FakeHostFiles::new().with(LEASES_PATH, ""),
            Vec::new(),
            1_000,
        );
        let workload = reserve_on(
            &allocator,
            &Bridge(LIVE),
            "run-a",
            "52:54:00:00:00:01",
            20,
            Duration::from_secs(10),
        )
        .await
        .expect("the guest is addressed on the network its own boot brought up");
        let provisioner = reserve_on(
            &allocator,
            &Bridge(LIVE),
            "run-a/tools",
            "52:54:00:00:00:11",
            20,
            Duration::from_secs(10),
        )
        .await
        .expect("a sessionless provisioner is addressed the same way");

        assert_eq!(
            workload.candidates,
            vec![
                Ipv4Addr::new(192, 168, 66, 254),
                Ipv4Addr::new(192, 168, 66, 253),
                Ipv4Addr::new(192, 168, 66, 252)
            ]
        );
        assert_eq!(workload.gateway, Ipv4Addr::new(192, 168, 66, 1));
        assert!(
            provisioner
                .candidates
                .iter()
                .all(|a| !workload.candidates.contains(a)),
            "{provisioner:?} vs {workload:?}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_bridge_the_vmm_is_still_creating_is_waited_for_rather_than_guessed_at() {
        let live = BridgeAfter(std::sync::atomic::AtomicUsize::new(0), 3);
        let net = reserve_on(
            &allocator_with(
                FakeHostFiles::new().with(LEASES_PATH, ""),
                Vec::new(),
                1_000,
            ),
            &live,
            "run-a",
            "52:54:00:00:00:01",
            20,
            Duration::from_millis(250),
        )
        .await
        .expect("the bridge appears as the guest attaches to it");
        assert_eq!(net.gateway, Ipv4Addr::new(192, 168, 66, 1));
    }

    #[test]
    fn every_reason_a_host_cannot_address_a_guest_has_its_own_audit_name() {
        let network = ReserveError::Network(std::io::Error::other("no bridge, no declaration"));
        assert_eq!(network.as_str(), "host_network_undiscoverable");
        let exhausted = ReserveError::Alloc(AllocError::Exhausted {
            network: HostNetwork::default(),
        });
        assert_eq!(exhausted.as_str(), "host_addresses_exhausted");
        let unreadable = ReserveError::Alloc(AllocError::Neighbors(std::io::Error::other(
            "arp: no such file",
        )));
        assert_eq!(
            unreadable.as_str(),
            "host_address_state_unreadable",
            "an unobservable host is not an exhausted one, and the audit must not conflate them"
        );
        assert!(
            unreadable.to_string().contains("arp: no such file"),
            "{unreadable}"
        );
        let explained = unreadable.explain();
        assert!(
            explained.contains(ENABLE_ENV),
            "no remedy named: {explained}"
        );

        let unanswered = ReserveError::Bootstrap(
            crate::vm::net_bootstrap::BootstrapError::Timeout(Duration::from_secs(20)),
        );
        assert_eq!(
            unanswered.as_str(),
            "guest_address_not_applied",
            "a guest that never applied the plan is its own failure, not an exhausted host"
        );
        assert!(
            unanswered.to_string().contains("20s"),
            "the user is told how long the guest was waited for: {unanswered}"
        );
        let unheld = ReserveError::Select(SelectError::UnknownOwner);
        assert_eq!(unheld.as_str(), "guest_address_not_applied");
        assert!(
            unheld.to_string().contains("not the one the host holds"),
            "{unheld}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_reservation_on_an_undiscoverable_network_refuses_by_name() {
        use crate::vm::host_net::HostNetworkSource;

        struct AbsentBridge;
        impl HostNetworkSource for AbsentBridge {
            fn observe(&self) -> std::io::Result<String> {
                Err(std::io::Error::other("ifconfig bridge100 exited with 1"))
            }
        }

        let error = reserve_on(
            &empty_host(),
            &AbsentBridge,
            "run-a",
            "52:54:00:00:00:01",
            2,
            Duration::from_millis(250),
        )
        .await
        .expect_err("an unknown network is not the default network");
        assert!(matches!(error, ReserveError::Network(_)), "{error:?}");
        assert!(
            error.to_string().contains("shared network"),
            "the user is told which host state was missing: {error}"
        );
    }

    #[test]
    fn an_unexpired_lease_occupies_its_address_and_an_expired_one_does_not() {
        let leases = "{\n\tip_address=192.168.64.254\n\thw_address=1,52:54:0:aa:aa:aa\n\tlease=0x7d0\n}\n\
                      {\n\tip_address=192.168.64.253\n\thw_address=1,52:54:0:bb:bb:bb\n\tlease=0x1f4\n}\n";
        let allocator = allocator_with(
            FakeHostFiles::new().with(LEASES_PATH, leases),
            Vec::new(),
            1_000,
        );
        let plan = allocator
            .reserve("run-a", "52:54:00:00:00:01")
            .expect("free");
        assert!(!plan.candidates.contains(&addr(254)), "{plan:?}");
        assert!(
            plan.candidates.contains(&addr(253)),
            "an expired record must not remove an address forever: {plan:?}"
        );
    }

    #[test]
    fn a_lease_file_of_expired_history_still_leaves_the_range_usable() {
        let mut leases = String::new();
        for last in 2..=254u8 {
            leases.push_str(&format!(
                "{{\n\tip_address=192.168.64.{last}\n\thw_address=1,52:54:0:0:0:1\n\tlease=0x1\n}}\n"
            ));
        }
        let allocator = allocator_with(
            FakeHostFiles::new().with(LEASES_PATH, &leases),
            Vec::new(),
            1_000,
        );
        let plan = allocator
            .reserve("run-a", "52:54:00:00:00:01")
            .expect("history is not occupancy");
        assert_eq!(plan.candidates.first(), Some(&addr(254)));
    }

    #[test]
    fn a_record_lns_cannot_read_keeps_its_address_out_of_the_offer() {
        let leases = "{\n\tip_address=192.168.64.254\n\tlease=nonsense\n}\n";
        let allocator = allocator_with(
            FakeHostFiles::new().with(LEASES_PATH, leases),
            Vec::new(),
            1_000,
        );
        let plan = allocator
            .reserve("run-a", "52:54:00:00:00:01")
            .expect("free");
        assert!(!plan.candidates.contains(&addr(254)), "{plan:?}");
    }

    #[test]
    fn a_lease_file_that_cannot_be_read_stops_allocation_instead_of_guessing() {
        let allocator = allocator_with(
            FakeHostFiles::new().failing(
                LEASES_PATH,
                std::io::Error::from(std::io::ErrorKind::PermissionDenied),
            ),
            Vec::new(),
            1_000,
        );
        let error = allocator
            .reserve("run-a", "52:54:00:00:00:01")
            .expect_err("nothing is known to be free");
        assert!(matches!(error, AllocError::Leases(_)), "{error:?}");
        assert!(error.to_string().contains("lease file"), "{error}");
    }

    #[test]
    fn a_host_that_never_started_its_shared_network_has_no_lease_file_and_that_is_fine() {
        let allocator = allocator_with(FakeHostFiles::new(), Vec::new(), 1_000);
        assert_eq!(
            allocator
                .reserve("run-a", "52:54:00:00:00:01")
                .expect("an absent lease file is an empty one")
                .candidates
                .first(),
            Some(&addr(254))
        );
    }

    #[test]
    fn the_gateway_the_network_and_the_broadcast_address_are_never_offered() {
        let allocator = allocator_with(
            FakeHostFiles::new().with(LEASES_PATH, ""),
            Vec::new(),
            1_000,
        );
        allocator.set_network(HostNetwork {
            network: addr(0),
            prefix_len: 29,
            gateway: addr(1),
        });
        let plan = allocator
            .reserve("run-a", "52:54:00:00:00:01")
            .expect("free");
        assert_eq!(plan.prefix_len, 29);
        assert_eq!(plan.candidates, vec![addr(6), addr(5), addr(4)]);
        assert!(
            !plan.candidates.contains(&addr(1)),
            "the gateway is the host"
        );
        assert!(!plan.candidates.contains(&addr(7)));
        assert!(!plan.candidates.contains(&addr(0)));
    }

    #[test]
    fn an_address_that_answered_arp_is_in_use_even_with_no_lease() {
        let allocator = allocator_with(
            FakeHostFiles::new().with(LEASES_PATH, ""),
            vec![addr(254), addr(253)],
            1_000,
        );
        let plan = allocator
            .reserve("run-a", "52:54:00:00:00:01")
            .expect("free");
        assert_eq!(plan.candidates, vec![addr(252), addr(251), addr(250)]);
    }

    #[test]
    fn a_full_network_refuses_the_boot_and_names_the_network_it_ran_out_of() {
        let allocator = allocator_with(
            FakeHostFiles::new().with(LEASES_PATH, ""),
            (10..=14)
                .map(|last| Ipv4Addr::new(10, 0, 0, last))
                .collect(),
            1_000,
        );
        allocator.set_network(HostNetwork {
            network: Ipv4Addr::new(10, 0, 0, 8),
            prefix_len: 29,
            gateway: Ipv4Addr::new(10, 0, 0, 9),
        });
        let error = allocator
            .reserve("run-a", "52:54:00:00:00:01")
            .expect_err("nothing left");
        assert!(matches!(error, AllocError::Exhausted { .. }), "{error:?}");
        assert!(error.to_string().contains("10.0.0.8/29"), "{error}");
    }

    #[test]
    fn a_confirmed_guest_address_releases_spares_and_rejects_unoffered_reports() {
        let allocator = empty_host();
        let plan = allocator
            .reserve("run-a", "52:54:00:00:00:01")
            .expect("reserve");
        assert_eq!(
            allocator.select("run-a", addr(200)),
            Err(SelectError::NotOffered(addr(200)))
        );
        allocator
            .select("run-a", plan.candidates[1])
            .expect("offered address");
        assert_eq!(
            allocator.reserved()[0].candidates,
            vec![plan.candidates[1]],
            "only the address the guest proved remains reserved"
        );
        let next = allocator
            .reserve("run-b", "52:54:00:00:00:02")
            .expect("spares released");
        assert!(next.candidates.contains(&plan.candidates[0]));
        assert_eq!(
            allocator.select("missing", plan.candidates[0]),
            Err(SelectError::UnknownOwner)
        );
    }

    #[test]
    fn a_released_address_is_offered_again_to_the_next_guest() {
        let allocator = empty_host();
        let first = allocator
            .reserve("run-a", "52:54:00:00:00:01")
            .expect("first");
        allocator.release("run-a");
        assert!(allocator.reserved().is_empty());
        let second = allocator
            .reserve("run-b", "52:54:00:00:00:02")
            .expect("second");
        assert_eq!(second.candidates, first.candidates);
    }

    #[test]
    fn releasing_a_run_that_never_reserved_changes_nothing() {
        let allocator = empty_host();
        allocator
            .reserve("run-a", "52:54:00:00:00:01")
            .expect("first");
        allocator.release("run-z");
        assert_eq!(allocator.reserved().len(), 1);
    }

    #[test]
    fn a_new_service_process_starts_with_no_reservations() {
        let before = empty_host();
        before.reserve("run-a", "52:54:00:00:00:01").expect("first");
        assert_eq!(before.reserved().len(), 1);
        assert!(
            empty_host().reserved().is_empty(),
            "the service owns every VM process, so process exit tears down every reservation holder"
        );
    }

    #[test]
    fn an_external_lease_on_a_reserved_address_is_reported_as_a_conflict() {
        let files = FakeHostFiles::new().with(LEASES_PATH, "");
        let allocator = allocator_with(files, Vec::new(), 1_000);
        allocator
            .reserve("run-a", "52:54:00:00:00:01")
            .expect("free host");
        assert!(
            allocator.conflicts().is_empty(),
            "nothing has taken the address yet"
        );

        let stolen =
            "{\n\tip_address=192.168.64.254\n\thw_address=1,52:54:0:99:99:99\n\tlease=0x7d0\n}\n";
        let after = allocator_with(
            FakeHostFiles::new().with(LEASES_PATH, stolen),
            Vec::new(),
            1_000,
        );
        copy_reservations(&allocator, &after);
        assert_eq!(
            after.conflicts(),
            vec![Conflict {
                owner: "run-a".into(),
                address: addr(254),
                holder: Some("52:54:00:99:99:99".into()),
            }],
            "ARP silence at boot does not stop Apple's DHCP server from leasing the same address later"
        );
    }

    #[test]
    fn a_lease_to_the_guests_own_hardware_is_not_a_conflict() {
        let allocator = allocator_with(
            FakeHostFiles::new().with(LEASES_PATH, ""),
            Vec::new(),
            1_000,
        );
        allocator
            .reserve("run-a", "52:54:00:AB:CD:EF")
            .expect("free host");
        let ours =
            "{\n\tip_address=192.168.64.254\n\thw_address=1,52:54:0:ab:cd:ef\n\tlease=0x7d0\n}\n";
        let after = allocator_with(
            FakeHostFiles::new().with(LEASES_PATH, ours),
            Vec::new(),
            1_000,
        );
        copy_reservations(&allocator, &after);
        assert!(after.conflicts().is_empty(), "{:?}", after.conflicts());
    }

    #[test]
    fn an_expired_or_unreadable_lease_file_reports_no_conflict() {
        let allocator = allocator_with(
            FakeHostFiles::new().with(LEASES_PATH, ""),
            Vec::new(),
            1_000,
        );
        allocator
            .reserve("run-a", "52:54:00:00:00:01")
            .expect("free host");

        let expired =
            "{\n\tip_address=192.168.64.254\n\thw_address=1,52:54:0:99:99:99\n\tlease=0x1\n}\n";
        let stale = allocator_with(
            FakeHostFiles::new().with(LEASES_PATH, expired),
            Vec::new(),
            1_000,
        );
        copy_reservations(&allocator, &stale);
        assert!(
            stale.conflicts().is_empty(),
            "an expired lease holds nobody"
        );

        let unreadable = allocator_with(
            FakeHostFiles::new().failing(
                LEASES_PATH,
                std::io::Error::from(std::io::ErrorKind::PermissionDenied),
            ),
            Vec::new(),
            1_000,
        );
        copy_reservations(&allocator, &unreadable);
        assert!(unreadable.conflicts().is_empty());
    }

    #[test]
    fn concurrent_reservations_are_atomic() {
        let allocator = Arc::new(empty_host());
        let plans: Vec<Vec<Ipv4Addr>> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|i| {
                    let allocator = allocator.clone();
                    scope.spawn(move || {
                        allocator
                            .reserve(&format!("run-{i}"), &format!("52:54:00:00:00:{i:02x}"))
                            .expect("free host")
                            .candidates
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().expect("thread"))
                .collect()
        });
        let mut seen = HashSet::new();
        for plan in &plans {
            for address in plan {
                assert!(
                    seen.insert(*address),
                    "{address} was offered twice: {plans:?}"
                );
            }
        }
        assert_eq!(seen.len(), 24);
    }

    #[test]
    fn re_reserving_the_same_run_replaces_its_own_record_rather_than_avoiding_it() {
        let allocator = empty_host();
        let first = allocator
            .reserve("run-a", "52:54:00:00:00:01")
            .expect("first");
        let again = allocator
            .reserve("run-a", "52:54:00:00:00:01")
            .expect("again");
        assert_eq!(first.candidates, again.candidates);
        assert_eq!(allocator.reserved().len(), 1);
    }

    #[test]
    fn a_neighbor_table_that_cannot_be_observed_stops_allocation_instead_of_guessing() {
        let allocator = Allocator::new(
            Arc::new(FakeHostFiles::new().with(LEASES_PATH, "")),
            Arc::new(UnobservableNeighbors),
            Arc::new(FixedClock(1_000)),
        );
        let error = allocator
            .reserve("run-a", "52:54:00:00:00:01")
            .expect_err("an unobserved ARP table removes an exclusion");
        assert!(matches!(error, AllocError::Neighbors(_)), "{error:?}");
        let rendered = error.to_string();
        assert!(rendered.contains("neighbor table"), "{rendered}");
        assert!(
            rendered.contains("arp: no such file"),
            "the cause the host reported must survive: {rendered}"
        );
    }

    #[test]
    fn a_refused_confirmation_says_whether_the_address_or_the_reservation_was_wrong() {
        assert_eq!(
            SelectError::NotOffered(addr(200)).to_string(),
            "the guest selected unoffered address 192.168.64.200"
        );
        assert_eq!(
            SelectError::UnknownOwner.to_string(),
            "the guest has no address reservation"
        );
    }

    #[test]
    fn a_runs_monitor_is_told_about_its_own_conflict_and_no_one_elses() {
        let allocator = empty_host();
        allocator
            .reserve("run-a", "52:54:00:00:00:01")
            .expect("first");
        allocator
            .reserve("run-b", "52:54:00:00:00:02")
            .expect("second");
        let stolen = "{\n\tip_address=192.168.64.254\n\thw_address=1,52:54:0:99:99:99\n\tlease=0x7d0\n}\n\
             {\n\tip_address=192.168.64.251\n\thw_address=1,52:54:0:88:88:88\n\tlease=0x7d0\n}\n";
        let after = allocator_with(
            FakeHostFiles::new().with(LEASES_PATH, stolen),
            Vec::new(),
            1_000,
        );
        copy_reservations(&allocator, &after);
        assert_eq!(
            after
                .conflicts_for("run-a")
                .into_iter()
                .map(|conflict| conflict.address)
                .collect::<Vec<_>>(),
            vec![addr(254)],
            "a run's monitor warns about the address that run holds"
        );
        assert_eq!(
            after
                .conflicts_for("run-b")
                .into_iter()
                .map(|conflict| conflict.address)
                .collect::<Vec<_>>(),
            vec![addr(251)]
        );
        assert!(after.conflicts_for("run-z").is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn conflict_monitor_checks_throughout_one_guests_lifecycle_and_routes_its_warning() {
        #[derive(Default)]
        struct Source(Mutex<Vec<Conflict>>);
        impl ConflictSource for Source {
            fn conflicts_for(&self, owner: &str) -> Vec<Conflict> {
                self.0
                    .lock()
                    .expect("source poisoned")
                    .iter()
                    .filter(|conflict| conflict.owner == owner)
                    .cloned()
                    .collect()
            }
        }

        let source = Arc::new(Source::default());
        let reported = Arc::new(Mutex::new(Vec::new()));
        let reported_by_monitor = reported.clone();
        let report: Arc<dyn Fn(Conflict) + Send + Sync> = Arc::new(move |conflict| {
            reported_by_monitor
                .lock()
                .expect("reports poisoned")
                .push(conflict);
        });
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(monitor_conflicts(
            source.clone(),
            "run-a".into(),
            Duration::from_secs(30),
            stop_rx,
            report,
        ));
        tokio::task::yield_now().await;
        source.0.lock().expect("source poisoned").extend([
            Conflict {
                owner: "run-a".into(),
                address: addr(254),
                holder: Some("52:54:00:99:99:99".into()),
            },
            Conflict {
                owner: "run-b".into(),
                address: addr(253),
                holder: None,
            },
        ]);

        tokio::time::advance(Duration::from_secs(30)).await;
        tokio::task::yield_now().await;
        assert_eq!(
            *reported.lock().expect("reports poisoned"),
            vec![Conflict {
                owner: "run-a".into(),
                address: addr(254),
                holder: Some("52:54:00:99:99:99".into()),
            }],
            "the monitor must warn the run whose live reservation acquired the conflict"
        );

        stop_tx.send(()).expect("monitor alive");
        task.await.expect("monitor task");
        tokio::time::advance(Duration::from_secs(30)).await;
        assert_eq!(reported.lock().expect("reports poisoned").len(), 1);
    }

    #[test]
    fn static_addressing_stays_off_until_the_host_asks_for_it() {
        assert!(!enabled(|_| None));
        assert!(!enabled(|_| Some("0".into())));
        assert!(enabled(|key| (key == ENABLE_ENV).then(|| "1".to_string())));
    }

    /// A guest that answers the plan it is sent with `reply`.
    fn scripted_guest(reply: Option<lns_session::ServerFrame>) -> tokio::io::DuplexStream {
        let (host, mut server) = tokio::io::duplex(4096);
        tokio::spawn(async move {
            let mut len = [0u8; 4];
            tokio::io::AsyncReadExt::read_exact(&mut server, &mut len)
                .await
                .expect("the host sends the plan first");
            let mut body = vec![0u8; lns_session::decode_length_prefix(&len).expect("length")];
            tokio::io::AsyncReadExt::read_exact(&mut server, &mut body)
                .await
                .expect("the plan body");
            if let Some(frame) = reply {
                let bytes = lns_session::encode_frame(&frame).expect("encode");
                tokio::io::AsyncWriteExt::write_all(&mut server, &bytes)
                    .await
                    .expect("answer the host");
                tokio::io::AsyncWriteExt::flush(&mut server)
                    .await
                    .expect("flush");
                std::future::pending::<()>().await;
            }
        });
        host
    }

    fn timing() -> BootTiming {
        BootTiming {
            attempts: 2,
            retry: Duration::from_millis(250),
            reply: Duration::from_secs(20),
        }
    }

    #[tokio::test]
    async fn the_guest_is_told_the_plan_and_the_reservation_narrows_to_the_address_it_took() {
        let allocator = allocator_with(
            FakeHostFiles::new().with(LEASES_PATH, ""),
            Vec::new(),
            1_000,
        );
        let taken = Ipv4Addr::new(192, 168, 66, 253);
        let addressed = address_guest(
            &allocator,
            &Bridge(LIVE),
            || async {
                Ok(scripted_guest(Some(
                    lns_session::ServerFrame::NetworkApplied {
                        address: taken.to_string(),
                    },
                )))
            },
            "run-a",
            "52:54:00:00:00:01",
            timing(),
        )
        .await
        .expect("the guest took an address the host reserved for it");
        assert_eq!(addressed.address, taken);
        assert_eq!(addressed.net.gateway, Ipv4Addr::new(192, 168, 66, 1));
        assert_eq!(
            allocator.reserved(),
            vec![Reservation {
                owner: "run-a".into(),
                mac: "52:54:00:00:00:01".into(),
                candidates: vec![taken],
            }],
            "the spares the guest did not take go back for the next guest"
        );
    }

    #[tokio::test]
    async fn a_guest_that_never_answers_gives_the_address_back_instead_of_holding_it() {
        let allocator = allocator_with(
            FakeHostFiles::new().with(LEASES_PATH, ""),
            Vec::new(),
            1_000,
        );
        let error = address_guest(
            &allocator,
            &Bridge(LIVE),
            || async { Ok(scripted_guest(None)) },
            "run-a",
            "52:54:00:00:00:01",
            BootTiming {
                reply: Duration::from_millis(50),
                ..timing()
            },
        )
        .await
        .expect_err("a guest that reports nothing has taken nothing");
        assert!(
            matches!(
                error,
                AddressError::Reserve(ReserveError::Bootstrap(
                    crate::vm::net_bootstrap::BootstrapError::Disconnected
                ))
            ),
            "{error:?}"
        );
        assert!(
            allocator.reserved().is_empty(),
            "an address no guest took must not be held against the next run"
        );
    }

    #[tokio::test]
    async fn a_guest_that_refuses_the_plan_stops_the_run_with_its_own_reason() {
        let allocator = allocator_with(
            FakeHostFiles::new().with(LEASES_PATH, ""),
            Vec::new(),
            1_000,
        );
        let reason = lns_session::BrokerExitReason::NoStaticAddress {
            offered: vec!["192.168.66.254".into()],
        };
        let error = address_guest(
            &allocator,
            &Bridge(LIVE),
            || async {
                Ok(scripted_guest(Some(lns_session::ServerFrame::Refused(
                    reason.clone(),
                ))))
            },
            "run-a",
            "52:54:00:00:00:01",
            timing(),
        )
        .await
        .expect_err("a refusal is not a booted guest");
        assert!(
            matches!(&error, AddressError::Refused(got) if got.as_str() == "no_static_address"),
            "{error:?}"
        );
        assert_eq!(
            error.to_string(),
            reason.summary(),
            "the guest's own words are what the run reports"
        );
        assert!(
            allocator.reserved().is_empty(),
            "{:?}",
            allocator.reserved()
        );
    }

    #[tokio::test]
    async fn a_guest_the_host_cannot_reach_holds_no_address_and_starts_no_workload() {
        let allocator = allocator_with(
            FakeHostFiles::new().with(LEASES_PATH, ""),
            Vec::new(),
            1_000,
        );
        let error = address_guest(
            &allocator,
            &Bridge(LIVE),
            || async {
                Err::<tokio::io::DuplexStream, _>(std::io::Error::other("vsock connect refused"))
            },
            "run-a",
            "52:54:00:00:00:01",
            timing(),
        )
        .await
        .expect_err("an unreachable guest is not an addressed one");
        assert!(
            error.to_string().contains("vsock connect refused"),
            "{error}"
        );
        assert!(allocator.reserved().is_empty());
    }

    #[tokio::test(start_paused = true)]
    async fn a_host_that_cannot_discover_its_network_never_opens_a_control_channel() {
        struct AbsentBridge;
        impl crate::vm::host_net::HostNetworkSource for AbsentBridge {
            fn observe(&self) -> std::io::Result<String> {
                Err(std::io::Error::other("ifconfig bridge100 exited with 1"))
            }
        }
        let error = address_guest(
            &empty_host(),
            &AbsentBridge,
            || async {
                panic!("nothing is sent to a guest before the host knows what to send it");
                #[allow(unreachable_code)]
                Ok::<tokio::io::DuplexStream, std::io::Error>(unreachable!())
            },
            "run-a",
            "52:54:00:00:00:01",
            timing(),
        )
        .await
        .expect_err("an undiscoverable network addresses nothing");
        assert!(
            matches!(error, AddressError::Reserve(ReserveError::Network(_))),
            "{error:?}"
        );
    }

    #[test]
    fn the_address_the_guest_took_is_reported_into_its_own_runs_log() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("current-thread runtime");
        let frames = crate::log::testing::capture_run_frames(|| {
            runtime.block_on(async {
                address_guest(
                    &allocator_with(
                        FakeHostFiles::new().with(LEASES_PATH, ""),
                        Vec::new(),
                        1_000,
                    ),
                    &Bridge(LIVE),
                    || async {
                        Ok(scripted_guest(Some(
                            lns_session::ServerFrame::NetworkApplied {
                                address: "192.168.66.254".into(),
                            },
                        )))
                    },
                    "run-a",
                    "52:54:00:00:00:01",
                    timing(),
                )
                .await
                .expect("the guest reported its address");
            });
        });
        let rendered = format!("{frames:?}");
        assert!(
            rendered.contains("guest assigned 192.168.66.254"),
            "the user reads the address in their own run's log: {rendered}"
        );
    }
}

#[cfg(test)]
mod mac_tests {
    use super::*;

    #[test]
    fn a_vm_keeps_one_mac_across_restarts_and_no_two_vms_share_one() {
        let first = mac_for("run-abc");
        assert_eq!(first, mac_for("run-abc"));
        assert_ne!(first, mac_for("run-abd"));
    }

    #[test]
    fn the_derived_mac_is_a_locally_administered_unicast_address() {
        let mac = mac_for("run-abc");
        let octets: Vec<u8> = mac
            .split(':')
            .map(|o| u8::from_str_radix(o, 16).expect("hex octet"))
            .collect();
        assert_eq!(octets.len(), 6, "{mac}");
        assert_eq!(octets[0] & 0x01, 0, "a unicast address has a clear I/G bit");
        assert_eq!(
            octets[0] & 0x02,
            0x02,
            "a locally administered address has the U/L bit set"
        );
        assert!(
            mac.chars().all(|c| c.is_ascii_hexdigit() || c == ':'),
            "the address goes on a kernel command line: {mac}"
        );
    }
}
