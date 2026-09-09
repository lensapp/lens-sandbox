use std::collections::{HashMap, HashSet};
use std::net::Ipv4Addr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::host_net::{HostFiles, HostNetwork, Neighbors, read_active_leases};
use crate::clock::Clock;
use lns_session::GuestNet;

pub mod real;

/// Off by default: an address lns picks can still be leased to someone else's guest later, and that race has no fix on macOS 15 (see `docs/running-workloads.md`).
pub const ENABLE_ENV: &str = "LNS_STATIC_GUEST_NET";

/// The guest cannot ask again once its kernel command line is built, so it carries spares.
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
mod tests {
    use super::*;
    use crate::vm::host_net::{BOOTPD_PLIST_PATH, LEASES_PATH, tests::FakeHostFiles};

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

    fn allocator_with(files: FakeHostFiles, neighbours: Vec<Ipv4Addr>, now: u64) -> Allocator {
        Allocator::new(
            Arc::new(files),
            Arc::new(FakeNeighbors(neighbours)),
            Arc::new(FixedClock(now)),
        )
    }

    fn empty_host() -> Allocator {
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
        let plist = "<key>net_address</key><string>192.168.64.0</string><key>net_mask</key><string>255.255.255.248</string>";
        let allocator = allocator_with(
            FakeHostFiles::new()
                .with(LEASES_PATH, "")
                .with(BOOTPD_PLIST_PATH, plist),
            Vec::new(),
            1_000,
        );
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
        let plist = "<key>net_address</key><string>10.0.0.8</string><key>net_mask</key><string>255.255.255.248</string>";
        let allocator = allocator_with(
            FakeHostFiles::new()
                .with(LEASES_PATH, "")
                .with(BOOTPD_PLIST_PATH, plist),
            (10..=14)
                .map(|last| Ipv4Addr::new(10, 0, 0, last))
                .collect(),
            1_000,
        );
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
