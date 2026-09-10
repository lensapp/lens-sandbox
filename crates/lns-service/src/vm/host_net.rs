use std::collections::BTreeSet;
use std::net::Ipv4Addr;

pub mod real;

/// Apple's files, read through a port so the whole allocator is host-tested without a shared network.
pub trait HostFiles: Send + Sync {
    fn read(&self, path: &str) -> std::io::Result<String>;
}

pub trait HostNetworkSource: Send + Sync {
    fn observe(&self) -> std::io::Result<String>;
}

/// Who answered ARP on the shared network recently; an address in use with no lease is still in use.
pub trait Neighbors: Send + Sync {
    fn observed(&self) -> std::io::Result<Vec<Ipv4Addr>>;
}

pub trait CommandOutput: Send + Sync {
    fn output(&self, program: &str, args: &[&str]) -> std::io::Result<std::process::Output>;
}

pub fn observe_neighbors(commands: &dyn CommandOutput) -> std::io::Result<Vec<Ipv4Addr>> {
    let output = commands.output("/usr/sbin/arp", &["-an"])?;
    if !output.status.success() {
        return Err(std::io::Error::other(format!(
            "arp -an exited with {}",
            output.status
        )));
    }
    let text = String::from_utf8(output.stdout)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    parse_arp_neighbors(&text)
}

/// Read, never written: lns assigns addresses beside Apple's DHCP server, it does not configure it.
pub const LEASES_PATH: &str = "/var/db/dhcpd_leases";

/// What vmnet builds the shared network from, so it answers before the first guest exists and bridge100 with it.
pub const VMNET_PLIST_PATH: &str = "/Library/Preferences/SystemConfiguration/com.apple.vmnet.plist";

const DEFAULT_NETWORK: Ipv4Addr = Ipv4Addr::new(192, 168, 64, 0);
const DEFAULT_PREFIX_LEN: u8 = 24;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostNetwork {
    pub network: Ipv4Addr,
    pub prefix_len: u8,
    pub gateway: Ipv4Addr,
}

impl Default for HostNetwork {
    /// What macOS gives a shared network out of the box: 192.168.64.0/24 with the host at .1.
    fn default() -> Self {
        Self {
            network: DEFAULT_NETWORK,
            prefix_len: DEFAULT_PREFIX_LEN,
            gateway: Ipv4Addr::new(192, 168, 64, 1),
        }
    }
}

impl HostNetwork {
    pub fn broadcast(&self) -> Ipv4Addr {
        Ipv4Addr::from(u32::from(self.network) | !mask_bits(self.prefix_len))
    }

    /// High addresses first: Apple's DHCP server hands out from the bottom of the range, so lns starts at the other end.
    pub fn usable_high_to_low(&self) -> impl Iterator<Item = Ipv4Addr> {
        let first = u32::from(self.network) + 1;
        let last = u32::from(self.broadcast()).saturating_sub(1);
        (first..=last).rev().map(Ipv4Addr::from)
    }

    pub fn contains(&self, addr: Ipv4Addr) -> bool {
        u32::from(addr) & mask_bits(self.prefix_len) == u32::from(self.network)
    }
}

fn mask_bits(prefix_len: u8) -> u32 {
    if prefix_len == 0 {
        0
    } else {
        u32::MAX << (32 - u32::from(prefix_len))
    }
}

/// When a lease record ends. A record lns cannot read holds its address: guessing would hand a live address to a second guest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LeaseExpiry {
    At(u64),
    Unreadable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lease {
    pub address: Ipv4Addr,
    pub hardware: Option<String>,
    pub expiry: LeaseExpiry,
}

impl Lease {
    pub fn holds_at(&self, now: u64) -> bool {
        match self.expiry {
            LeaseExpiry::At(when) => when > now,
            LeaseExpiry::Unreadable => true,
        }
    }
}

/// `/var/db/dhcpd_leases` is a list of brace-delimited records of `key=value` lines; a record with no address is not a lease.
pub fn parse_leases(text: &str) -> Vec<Lease> {
    let mut leases = Vec::new();
    let mut address = None;
    let mut hardware = None;
    let mut expiry = LeaseExpiry::Unreadable;
    for line in text.lines() {
        let line = line.trim();
        if line == "}" {
            if let Some(address) = address.take() {
                leases.push(Lease {
                    address,
                    hardware: hardware.take(),
                    expiry,
                });
            }
            hardware = None;
            expiry = LeaseExpiry::Unreadable;
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "ip_address" => address = value.trim().parse().ok(),
            "hw_address" => hardware = Some(normalise_hardware(value.trim())),
            "lease" => expiry = parse_expiry(value.trim()),
            _ => {}
        }
    }
    leases
}

pub fn active_leases(text: &str, now: u64) -> Vec<Lease> {
    parse_leases(text)
        .into_iter()
        .filter(|lease| lease.holds_at(now))
        .collect()
}

/// bootpd writes `hw_address=1,52:54:0:ab:cd:ef`: the leading type byte is not part of the address, and each octet is unpadded.
fn normalise_hardware(value: &str) -> String {
    let address = value.split_once(',').map_or(value, |(_, rest)| rest);
    address
        .split(':')
        .map(|octet| {
            u8::from_str_radix(octet, 16)
                .map_or_else(|_| octet.to_ascii_lowercase(), |byte| format!("{byte:02x}"))
        })
        .collect::<Vec<_>>()
        .join(":")
}

fn parse_expiry(value: &str) -> LeaseExpiry {
    let digits = value.strip_prefix("0x").unwrap_or(value);
    u64::from_str_radix(digits, 16).map_or(LeaseExpiry::Unreadable, LeaseExpiry::At)
}

pub fn parse_host_network(text: &str) -> Option<HostNetwork> {
    let inet = text.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        while let Some(field) = fields.next() {
            if field == "inet" {
                let gateway = fields.next()?;
                while let Some(field) = fields.next() {
                    if field == "netmask" {
                        return Some((gateway, fields.next()?));
                    }
                }
            }
        }
        None
    })?;
    let gateway: Ipv4Addr = inet.0.parse().ok()?;
    let mask: Ipv4Addr = parse_observed_mask(inet.1)?;
    let prefix_len = prefix_len_of(mask)?;
    Some(HostNetwork {
        network: Ipv4Addr::from(u32::from(gateway) & mask_bits(prefix_len)),
        prefix_len,
        gateway,
    })
}

fn parse_observed_mask(value: &str) -> Option<Ipv4Addr> {
    if let Some(hex) = value.strip_prefix("0x") {
        return u32::from_str_radix(hex, 16).ok().map(Ipv4Addr::from);
    }
    value.parse().ok()
}

/// Apple's plist is XML and lns carries no plist parser, so the two keys that define the shared network are read directly and anything ambiguous is no answer at all.
pub fn parse_declared_network(text: &str) -> Option<HostNetwork> {
    if text.matches("<key>Shared_Net_Address</key>").count() != 1
        || text.matches("<key>Shared_Net_Mask</key>").count() != 1
    {
        return None;
    }
    let gateway: Ipv4Addr = plist_string(text, "Shared_Net_Address")?.parse().ok()?;
    let prefix_len = plist_string(text, "Shared_Net_Mask")
        .and_then(|mask| mask.parse::<Ipv4Addr>().ok())
        .and_then(prefix_len_of)?;
    Some(HostNetwork {
        network: Ipv4Addr::from(u32::from(gateway) & mask_bits(prefix_len)),
        prefix_len,
        gateway,
    })
}

/// A guest with a shared-network interface is what creates `bridge100`, so every boot — including the ordinary DHCP ones — is a chance to learn what this host's shared network is before a later static guest has to ask.
pub async fn learn_host_network(
    source: &dyn HostNetworkSource,
    memory: &dyn NetworkMemory,
    attempts: usize,
    retry: std::time::Duration,
) {
    if let Ok(live) = observe_host_network(source, attempts, retry).await {
        memory.remember(&live);
    }
}

/// `Ok(None)` is a host that declares no shared network; `Err` is one whose declaration this user may not read, and the two are not the same answer.
pub fn declared_host_network(files: &dyn HostFiles) -> std::io::Result<Option<HostNetwork>> {
    files
        .read(VMNET_PLIST_PATH)
        .map(|text| parse_declared_network(&text))
}

/// The three ways a boot can ask this host what its shared network is, in the order it asks them.
pub struct HostView<'a> {
    pub live: &'a dyn HostNetworkSource,
    pub files: &'a dyn HostFiles,
    pub memory: &'a dyn NetworkMemory,
}

/// What this host's shared network was last observed to be, kept because the declaration vmnet builds it from is not readable by the user the service runs as.
pub trait NetworkMemory: Send + Sync {
    fn recall(&self) -> Option<HostNetwork>;
    fn remember(&self, network: &HostNetwork);
}

pub fn render_remembered(network: &HostNetwork) -> String {
    format!(
        "{}/{} {}\n",
        network.network, network.prefix_len, network.gateway
    )
}

/// A record that does not describe one whole subnet with its gateway inside it is not an observation lns can boot a guest onto.
pub fn parse_remembered(text: &str) -> Option<HostNetwork> {
    let (cidr, gateway) = text.lines().next()?.split_once(char::is_whitespace)?;
    let (network, prefix_len) = cidr.split_once('/')?;
    let network = HostNetwork {
        network: network.trim().parse().ok()?,
        prefix_len: prefix_len.trim().parse().ok()?,
        gateway: gateway.trim().parse().ok()?,
    };
    let aligned = u32::from(network.network) & !mask_bits(network.prefix_len) == 0;
    ((1..=30).contains(&network.prefix_len) && aligned && network.contains(network.gateway))
        .then_some(network)
}

fn plist_string(text: &str, key: &str) -> Option<String> {
    let after_key = text.split_once(&format!("<key>{key}</key>"))?.1;
    let open = after_key.find("<string>")? + "<string>".len();
    let close = after_key[open..].find("</string>")? + open;
    Some(after_key[open..close].trim().to_string())
}

fn prefix_len_of(mask: Ipv4Addr) -> Option<u8> {
    let bits = u32::from(mask);
    let ones = bits.leading_ones();
    (bits.count_ones() == ones && (1..=30).contains(&ones)).then_some(ones as u8)
}

/// `arp -an` prints `? (192.168.64.7) at 52:54:0:ab:cd:ef on bridge100 ...`; only the address matters here.
pub fn parse_arp_neighbors(text: &str) -> std::io::Result<Vec<Ipv4Addr>> {
    let mut seen = BTreeSet::new();
    for line in text.lines() {
        let Some(open) = line.find('(') else { continue };
        let close = line[open..].find(')').ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "malformed arp output")
        })?;
        let addr = line[open + 1..open + close]
            .parse::<Ipv4Addr>()
            .map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "malformed arp address")
            })?;
        seen.insert(addr);
    }
    Ok(seen.into_iter().collect())
}

pub async fn observe_host_network(
    source: &dyn HostNetworkSource,
    attempts: usize,
    retry: std::time::Duration,
) -> std::io::Result<HostNetwork> {
    let mut last_error = None;
    for attempt in 0..attempts {
        match source.observe() {
            Ok(text) => match parse_host_network(&text) {
                Some(network) => return Ok(network),
                None => {
                    last_error = Some(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "the active VZ shared network observation was malformed",
                    ));
                }
            },
            Err(error) => last_error = Some(error),
        }
        if attempt + 1 < attempts {
            tokio::time::sleep(retry).await;
        }
    }
    Err(last_error.unwrap_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "the active VZ shared network did not appear",
        )
    }))
}

/// The network a guest boots onto: the live bridge when one exists, then the declaration vmnet will create it from, then what this host was last seen running — the first static guest is the one that creates the bridge, so waiting for it can only time out, and the declaration is root-only on a stock macOS host.
pub async fn host_network_for_boot(
    host: &HostView<'_>,
    attempts: usize,
    retry: std::time::Duration,
) -> std::io::Result<HostNetwork> {
    if let Ok(live) = observe_host_network(host.live, 1, retry).await {
        host.memory.remember(&live);
        return Ok(live);
    }
    let declaration = declared_host_network(host.files);
    if let Ok(Some(declared)) = declaration {
        return Ok(declared);
    }
    if let Some(remembered) = host.memory.recall() {
        return Ok(remembered);
    }
    match observe_host_network(host.live, attempts, retry).await {
        Ok(live) => {
            host.memory.remember(&live);
            Ok(live)
        }
        Err(error) => Err(std::io::Error::new(
            error.kind(),
            format!(
                "the shared network is not active ({error}), {}, and this host was never observed running one",
                declaration_note(&declaration)
            ),
        )),
    }
}

fn declaration_note(declaration: &std::io::Result<Option<HostNetwork>>) -> String {
    match declaration {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            format!("{VMNET_PLIST_PATH} does not exist")
        }
        Err(error) => format!("{VMNET_PLIST_PATH} is not readable by this user ({error})"),
        Ok(_) => format!("{VMNET_PLIST_PATH} declares no shared network"),
    }
}

/// An unreadable lease file is not an empty one: with no record to read, every candidate has to come from ARP and the reservation table.
pub fn read_active_leases(files: &dyn HostFiles, now: u64) -> Result<Vec<Lease>, std::io::Error> {
    files
        .read(LEASES_PATH)
        .map(|text| active_leases(&text, now))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    pub(crate) struct FakeHostFiles {
        files: HashMap<String, std::io::Result<String>>,
    }

    impl FakeHostFiles {
        pub(crate) fn new() -> Self {
            Self {
                files: HashMap::new(),
            }
        }
        pub(crate) fn with(mut self, path: &str, text: &str) -> Self {
            self.files.insert(path.into(), Ok(text.to_string()));
            self
        }
        pub(crate) fn failing(mut self, path: &str, error: std::io::Error) -> Self {
            self.files.insert(path.into(), Err(error));
            self
        }
    }

    impl HostFiles for FakeHostFiles {
        fn read(&self, path: &str) -> std::io::Result<String> {
            match self.files.get(path) {
                Some(Ok(text)) => Ok(text.clone()),
                Some(Err(e)) => Err(std::io::Error::new(e.kind(), e.to_string())),
                None => Err(std::io::Error::from(std::io::ErrorKind::NotFound)),
            }
        }
    }

    pub(crate) struct FakeNetworkMemory(Mutex<Option<HostNetwork>>);

    impl FakeNetworkMemory {
        pub(crate) fn empty() -> Self {
            Self(Mutex::new(None))
        }
        pub(crate) fn holding(network: HostNetwork) -> Self {
            Self(Mutex::new(Some(network)))
        }
    }

    impl NetworkMemory for FakeNetworkMemory {
        fn recall(&self) -> Option<HostNetwork> {
            self.0.lock().expect("memory poisoned").clone()
        }
        fn remember(&self, network: &HostNetwork) {
            *self.0.lock().expect("memory poisoned") = Some(network.clone());
        }
    }

    const LEASES: &str = "{
\tname=alpha
\tip_address=192.168.64.2
\thw_address=1,52:54:0:12:34:56
\tidentifier=1,52:54:0:12:34:56
\tlease=0x66c0f0a1
}
{
\tname=beta
\tip_address=192.168.64.3
\thw_address=1,52:54:0:aa:b:cd
\tlease=0x5f000000
}
";

    #[test]
    fn a_lease_record_yields_its_address_its_hardware_and_its_expiry() {
        let leases = parse_leases(LEASES);
        assert_eq!(leases.len(), 2);
        assert_eq!(leases[0].address, Ipv4Addr::new(192, 168, 64, 2));
        assert_eq!(leases[0].hardware.as_deref(), Some("52:54:00:12:34:56"));
        assert_eq!(leases[0].expiry, LeaseExpiry::At(0x66c0_f0a1));
        assert_eq!(leases[1].hardware.as_deref(), Some("52:54:00:aa:0b:cd"));
    }

    #[test]
    fn a_full_history_of_expired_records_does_not_occupy_the_whole_range() {
        let now = 0x66c0_f0a0;
        let active = active_leases(LEASES, now);
        assert_eq!(
            active.iter().map(|l| l.address).collect::<Vec<_>>(),
            vec![Ipv4Addr::new(192, 168, 64, 2)],
            "an expired record is history, not an occupant"
        );
        assert!(
            active_leases(LEASES, 0x66c0_f0a1).is_empty(),
            "a lease that ends exactly now is over"
        );
    }

    #[test]
    fn a_record_whose_expiry_cannot_be_read_holds_its_address() {
        let text = "{\n\tip_address=192.168.64.9\n\tlease=nonsense\n}\n";
        let leases = parse_leases(text);
        assert_eq!(leases[0].expiry, LeaseExpiry::Unreadable);
        assert!(
            leases[0].holds_at(u64::MAX),
            "lns must not hand out an address whose record it failed to read"
        );
    }

    #[test]
    fn malformed_records_are_skipped_without_losing_the_readable_ones() {
        let text = "garbage\n{\n\tname=nothing\n}\n{\n\tip_address=not-an-address\n}\n{\n\tip_address=192.168.64.4\n\tlease=0x10\n}\n";
        let leases = parse_leases(text);
        assert_eq!(leases.len(), 1);
        assert_eq!(leases[0].address, Ipv4Addr::new(192, 168, 64, 4));
    }

    #[test]
    fn an_unreadable_lease_file_is_reported_and_never_read_as_an_empty_one() {
        let files = FakeHostFiles::new().failing(
            LEASES_PATH,
            std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        );
        let error = read_active_leases(&files, 0).expect_err("the caller decides what to do");
        assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);

        let files = FakeHostFiles::new().with(LEASES_PATH, LEASES);
        assert_eq!(read_active_leases(&files, 0).expect("readable").len(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn active_vz_network_observation_retries_until_the_interface_appears() {
        struct Source(Mutex<Vec<std::io::Result<String>>>);
        impl HostNetworkSource for Source {
            fn observe(&self) -> std::io::Result<String> {
                self.0.lock().expect("source poisoned").remove(0)
            }
        }
        let source = Source(Mutex::new(vec![
            Err(std::io::Error::from(std::io::ErrorKind::NotFound)),
            Ok("bridge100: flags=8863\n\tinet 10.37.129.7 netmask 0xffffff00 broadcast 10.37.129.255\n".into()),
        ]));
        let observed = observe_host_network(&source, 2, std::time::Duration::from_secs(1));
        tokio::pin!(observed);
        tokio::time::advance(std::time::Duration::from_secs(1)).await;
        assert_eq!(
            observed.await.expect("network appeared"),
            HostNetwork {
                network: Ipv4Addr::new(10, 37, 129, 0),
                prefix_len: 24,
                gateway: Ipv4Addr::new(10, 37, 129, 7),
            },
            "the observed interface address is the gateway; it is not inferred as network + 1"
        );
    }

    #[tokio::test]
    async fn missing_or_malformed_active_vz_network_is_an_error_not_a_default() {
        struct Source(&'static str);
        impl HostNetworkSource for Source {
            fn observe(&self) -> std::io::Result<String> {
                Ok(self.0.into())
            }
        }
        for text in [
            "",
            "inet 192.168.64.1",
            "inet 192.168.64.1 netmask nonsense",
            "inet 192.168.64.1 netmask 255.0.255.0",
        ] {
            assert!(
                observe_host_network(&Source(text), 1, std::time::Duration::ZERO)
                    .await
                    .is_err(),
                "{text:?}"
            );
        }
        assert!(
            observe_host_network(
                &Source("inet 192.168.64.1 netmask 255.255.255.0"),
                0,
                std::time::Duration::ZERO,
            )
            .await
            .is_err()
        );
    }

    struct AbsentBridge;
    impl HostNetworkSource for AbsentBridge {
        fn observe(&self) -> std::io::Result<String> {
            Err(std::io::Error::other("ifconfig bridge100 exited with 1"))
        }
    }

    const DECLARED: &str = "<plist><dict>
            <key>Shared_Net_Address</key><string>192.168.66.1</string>
            <key>Shared_Net_Mask</key><string>255.255.255.0</string>
            </dict></plist>";

    #[tokio::test]
    async fn the_first_static_boot_reads_the_declared_network_the_bridge_has_not_been_created_from_yet()
     {
        let files = FakeHostFiles::new().with(VMNET_PLIST_PATH, DECLARED);
        let started = std::time::Instant::now();
        let network = host_network_for_boot(
            &HostView {
                live: &AbsentBridge,
                files: &files,
                memory: &FakeNetworkMemory::empty(),
            },
            20,
            std::time::Duration::from_secs(10),
        )
        .await
        .expect("the shared network is declared even before vmnet creates bridge100");
        assert_eq!(
            network,
            HostNetwork {
                network: Ipv4Addr::new(192, 168, 66, 0),
                prefix_len: 24,
                gateway: Ipv4Addr::new(192, 168, 66, 1),
            },
            "the declared host address is the gateway, and it is read, not inferred"
        );
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "the first guest is the one that creates bridge100, so waiting for it can only time out"
        );
    }

    #[tokio::test]
    async fn a_live_bridge_outranks_the_declaration_it_was_created_from() {
        struct Live;
        impl HostNetworkSource for Live {
            fn observe(&self) -> std::io::Result<String> {
                Ok("bridge100: flags=8863\n\tinet 10.37.129.7 netmask 0xffffff00\n".into())
            }
        }
        let files = FakeHostFiles::new().with(VMNET_PLIST_PATH, DECLARED);
        assert_eq!(
            host_network_for_boot(
                &HostView {
                    live: &Live,
                    files: &files,
                    memory: &FakeNetworkMemory::empty(),
                },
                1,
                std::time::Duration::ZERO
            )
            .await
            .expect("an active network is observable")
            .gateway,
            Ipv4Addr::new(10, 37, 129, 7),
            "a declaration edited after the network came up is not what the guest would reach"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_host_that_neither_runs_nor_declares_a_shared_network_says_so() {
        let error = host_network_for_boot(
            &HostView {
                live: &AbsentBridge,
                files: &FakeHostFiles::new(),
                memory: &FakeNetworkMemory::empty(),
            },
            2,
            std::time::Duration::from_millis(250),
        )
        .await
        .expect_err("no network is not the default network");
        let rendered = error.to_string();
        assert!(rendered.contains(VMNET_PLIST_PATH), "{rendered}");
        assert!(rendered.contains("ifconfig bridge100"), "{rendered}");
    }

    #[tokio::test]
    async fn a_declaration_this_user_may_not_read_falls_back_to_the_network_this_host_was_seen_running()
     {
        let files = FakeHostFiles::new().failing(
            VMNET_PLIST_PATH,
            std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        );
        let remembered = HostNetwork {
            network: Ipv4Addr::new(192, 168, 66, 0),
            prefix_len: 24,
            gateway: Ipv4Addr::new(192, 168, 66, 1),
        };
        let memory = FakeNetworkMemory::holding(remembered.clone());
        let started = std::time::Instant::now();
        let network = host_network_for_boot(
            &HostView {
                live: &AbsentBridge,
                files: &files,
                memory: &memory,
            },
            3,
            std::time::Duration::from_secs(2),
        )
        .await
        .expect("the plist is root-only, so the last observation of this host is what is left");
        assert_eq!(network, remembered);
        assert!(
            started.elapsed() < std::time::Duration::from_secs(1),
            "a bridge no guest is bringing up is not worth waiting for"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_declaration_this_user_may_not_read_is_named_as_that_and_not_as_an_absent_one() {
        let files = FakeHostFiles::new().failing(
            VMNET_PLIST_PATH,
            std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        );
        let rendered = host_network_for_boot(
            &HostView {
                live: &AbsentBridge,
                files: &files,
                memory: &FakeNetworkMemory::empty(),
            },
            2,
            std::time::Duration::from_millis(250),
        )
        .await
        .expect_err("nothing readable is left to discover")
        .to_string();
        assert!(rendered.contains(VMNET_PLIST_PATH), "{rendered}");
        assert!(rendered.contains("permission denied"), "{rendered}");
        assert!(
            !rendered.contains("declares no shared network"),
            "an unreadable declaration must not be reported as an absent one: {rendered}"
        );
        assert!(rendered.contains("ifconfig bridge100"), "{rendered}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_host_that_declares_nothing_is_told_apart_from_one_whose_declaration_is_unreadable() {
        let files = FakeHostFiles::new().with(VMNET_PLIST_PATH, "<plist><dict></dict></plist>");
        let rendered = host_network_for_boot(
            &HostView {
                live: &AbsentBridge,
                files: &files,
                memory: &FakeNetworkMemory::empty(),
            },
            2,
            std::time::Duration::from_millis(250),
        )
        .await
        .expect_err("a readable declaration naming no network is still no network")
        .to_string();
        assert!(
            rendered.contains("declares no shared network"),
            "{rendered}"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn any_boot_teaches_the_host_what_its_shared_network_is() {
        struct Late(Mutex<usize>);
        impl HostNetworkSource for Late {
            fn observe(&self) -> std::io::Result<String> {
                let mut seen = self.0.lock().expect("source poisoned");
                *seen += 1;
                if *seen < 2 {
                    return Err(std::io::Error::other("ifconfig bridge100 exited with 1"));
                }
                Ok("bridge100: flags=8863\n\tinet 10.37.129.7 netmask 0xffffff00\n".into())
            }
        }
        let memory = FakeNetworkMemory::empty();
        learn_host_network(
            &Late(Mutex::new(0)),
            &memory,
            3,
            std::time::Duration::from_millis(250),
        )
        .await;
        assert_eq!(
            memory.recall().map(|network| network.gateway),
            Some(Ipv4Addr::new(10, 37, 129, 7)),
            "a guest booting on DHCP is what creates the bridge a later static guest needs to know about"
        );
    }

    #[tokio::test(start_paused = true)]
    async fn a_boot_that_brings_no_shared_network_up_teaches_nothing() {
        let memory = FakeNetworkMemory::holding(HostNetwork::default());
        learn_host_network(
            &AbsentBridge,
            &memory,
            2,
            std::time::Duration::from_millis(250),
        )
        .await;
        assert_eq!(
            memory.recall(),
            Some(HostNetwork::default()),
            "a boot with no shared network is no observation, not an erasure of the last one"
        );
    }

    #[tokio::test]
    async fn a_live_observation_is_remembered_so_the_next_cold_start_is_not_a_guess() {
        struct Live;
        impl HostNetworkSource for Live {
            fn observe(&self) -> std::io::Result<String> {
                Ok("bridge100: flags=8863\n\tinet 10.37.129.7 netmask 0xffffff00\n".into())
            }
        }
        let memory = FakeNetworkMemory::empty();
        let network = host_network_for_boot(
            &HostView {
                live: &Live,
                files: &FakeHostFiles::new(),
                memory: &memory,
            },
            1,
            std::time::Duration::ZERO,
        )
        .await
        .expect("an active network is observable");
        assert_eq!(memory.recall(), Some(network));
    }

    #[tokio::test(start_paused = true)]
    async fn a_bridge_that_appears_while_the_host_waits_is_remembered_too() {
        struct Late(Mutex<usize>);
        impl HostNetworkSource for Late {
            fn observe(&self) -> std::io::Result<String> {
                let mut seen = self.0.lock().expect("source poisoned");
                *seen += 1;
                if *seen < 3 {
                    return Err(std::io::Error::other("ifconfig bridge100 exited with 1"));
                }
                Ok("bridge100: flags=8863\n\tinet 10.37.129.7 netmask 0xffffff00\n".into())
            }
        }
        let memory = FakeNetworkMemory::empty();
        let network = host_network_for_boot(
            &HostView {
                live: &Late(Mutex::new(0)),
                files: &FakeHostFiles::new(),
                memory: &memory,
            },
            3,
            std::time::Duration::from_millis(250),
        )
        .await
        .expect("another guest brought the bridge up while this one waited");
        assert_eq!(memory.recall(), Some(network));
    }

    #[test]
    fn a_remembered_network_survives_a_round_trip_and_a_damaged_record_is_no_answer() {
        let network = HostNetwork {
            network: Ipv4Addr::new(10, 37, 129, 0),
            prefix_len: 24,
            gateway: Ipv4Addr::new(10, 37, 129, 7),
        };
        assert_eq!(
            parse_remembered(&render_remembered(&network)),
            Some(network)
        );
        for text in [
            "",
            "10.37.129.0/24",
            "nonsense/24 10.37.129.7",
            "10.37.129.0/nonsense 10.37.129.7",
            "10.37.129.0/24 nonsense",
            "10.37.129.0/31 10.37.129.7",
            "10.37.129.5/24 10.37.129.7",
            "10.37.129.0/24 192.168.64.1",
        ] {
            assert_eq!(
                parse_remembered(text),
                None,
                "a record lns cannot trust is not a network: {text:?}"
            );
        }
    }

    #[test]
    fn a_declaration_that_cannot_be_read_whole_names_no_network() {
        for text in [
            "<plist><dict></dict></plist>",
            "<plist><key>Shared_Net_Address</key><string>nonsense</string><key>Shared_Net_Mask</key><string>255.255.255.0</string></plist>",
            "<plist><key>Shared_Net_Address</key><string>10.0.0.1</string></plist>",
            "<plist><key>Shared_Net_Address</key><string>10.0.0.1</string><key>Shared_Net_Mask</key><string>255.0.255.0</string></plist>",
            "<plist><key>Shared_Net_Address</key><string>10.0.0.1</string><key>Shared_Net_Mask</key><string>255.255.255.254</string></plist>",
            "<plist><key>Shared_Net_Address</key><string>10.0.0.1</string><key>Shared_Net_Mask</key>",
            "<plist><key>Shared_Net_Address</key><string>10.0.0.1</string><key>Shared_Net_Mask</key><string>255.255.255.0",
            "<key>Shared_Net_Address</key><string>10.0.0.1</string><key>Shared_Net_Mask</key><string>255.255.255.0</string><key>Shared_Net_Address</key><string>172.16.0.1</string><key>Shared_Net_Mask</key><string>255.255.0.0</string>",
        ] {
            assert_eq!(parse_declared_network(text), None, "{text}");
        }
    }

    #[test]
    fn the_declared_network_moves_when_the_host_moves_its_shared_subnet() {
        let plist = "<plist><dict>
            <key>Shared_Net_Address</key><string>10.37.129.1</string>
            <key>Shared_Net_Mask</key><string>255.255.255.0</string>
            </dict></plist>";
        assert_eq!(
            parse_declared_network(plist).expect("valid declaration"),
            HostNetwork {
                network: Ipv4Addr::new(10, 37, 129, 0),
                prefix_len: 24,
                gateway: Ipv4Addr::new(10, 37, 129, 1),
            }
        );
    }

    #[test]
    fn the_usable_range_runs_from_the_top_and_excludes_network_and_broadcast() {
        let net = HostNetwork::default();
        let usable: Vec<_> = net.usable_high_to_low().collect();
        assert_eq!(usable.first(), Some(&Ipv4Addr::new(192, 168, 64, 254)));
        assert_eq!(usable.last(), Some(&Ipv4Addr::new(192, 168, 64, 1)));
        assert_eq!(usable.len(), 254);
        assert_eq!(net.broadcast(), Ipv4Addr::new(192, 168, 64, 255));
        assert!(!usable.contains(&net.broadcast()));
        assert!(!usable.contains(&net.network));
    }

    #[test]
    fn an_address_outside_the_subnet_is_not_this_networks_business() {
        let net = HostNetwork::default();
        assert!(net.contains(Ipv4Addr::new(192, 168, 64, 7)));
        assert!(!net.contains(Ipv4Addr::new(192, 168, 65, 7)));
    }

    #[test]
    fn a_smaller_subnet_narrows_the_usable_range_the_same_way() {
        let net = HostNetwork {
            network: Ipv4Addr::new(10, 0, 0, 8),
            prefix_len: 29,
            gateway: Ipv4Addr::new(10, 0, 0, 9),
        };
        assert_eq!(net.broadcast(), Ipv4Addr::new(10, 0, 0, 15));
        assert_eq!(net.usable_high_to_low().count(), 6);
        assert_eq!(mask_bits(0), 0);
    }

    #[test]
    fn a_broad_subnet_yields_its_first_candidates_without_materialising_the_range() {
        let net = HostNetwork {
            network: Ipv4Addr::UNSPECIFIED,
            prefix_len: 1,
            gateway: Ipv4Addr::new(1, 0, 0, 1),
        };
        assert_eq!(
            net.usable_high_to_low().take(3).collect::<Vec<_>>(),
            vec![
                Ipv4Addr::new(127, 255, 255, 254),
                Ipv4Addr::new(127, 255, 255, 253),
                Ipv4Addr::new(127, 255, 255, 252),
            ]
        );
    }

    #[test]
    fn arp_output_names_the_neighbours_that_answered() {
        let text = "? (192.168.64.1) at 5a:df:1:2:3:4 on bridge100 ifscope [ethernet]\n\
                    ? (192.168.64.7) at 52:54:0:ab:cd:ef on bridge100 ifscope [ethernet]\n\
                    ? (192.168.64.7) at 52:54:0:ab:cd:ef on bridge100 ifscope permanent [ethernet]\n\
                    nothing useful here\n";
        assert_eq!(
            parse_arp_neighbors(text).expect("valid arp output"),
            vec![
                Ipv4Addr::new(192, 168, 64, 1),
                Ipv4Addr::new(192, 168, 64, 7)
            ]
        );
        for malformed in [
            "? (not-an-address) at incomplete on bridge100\n",
            "? (10.0.0.1 at broken\n",
        ] {
            assert!(parse_arp_neighbors(malformed).is_err(), "{malformed}");
        }
    }

    struct FakeCommand(std::io::Result<std::process::Output>);
    impl CommandOutput for FakeCommand {
        fn output(&self, _program: &str, _args: &[&str]) -> std::io::Result<std::process::Output> {
            match &self.0 {
                Ok(output) => Ok(std::process::Output {
                    status: output.status,
                    stdout: output.stdout.clone(),
                    stderr: output.stderr.clone(),
                }),
                Err(error) => Err(std::io::Error::new(error.kind(), error.to_string())),
            }
        }
    }

    #[test]
    fn neighbor_observation_propagates_spawn_exit_and_parse_failures() {
        use std::os::unix::process::ExitStatusExt;

        let spawn = FakeCommand(Err(std::io::Error::from(
            std::io::ErrorKind::PermissionDenied,
        )));
        assert_eq!(
            observe_neighbors(&spawn).expect_err("spawn failure").kind(),
            std::io::ErrorKind::PermissionDenied
        );

        let exited = FakeCommand(Ok(std::process::Output {
            status: std::process::ExitStatus::from_raw(1 << 8),
            stdout: Vec::new(),
            stderr: b"arp failed".to_vec(),
        }));
        assert!(observe_neighbors(&exited).is_err());

        let malformed = FakeCommand(Ok(std::process::Output {
            status: std::process::ExitStatus::from_raw(0),
            stdout: b"? (not-an-address) at incomplete\n".to_vec(),
            stderr: Vec::new(),
        }));
        assert_eq!(
            observe_neighbors(&malformed)
                .expect_err("parse failure")
                .kind(),
            std::io::ErrorKind::InvalidData
        );
    }
}
