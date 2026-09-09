use std::collections::BTreeSet;
use std::net::Ipv4Addr;

pub mod real;

/// Apple's files, read through a port so the whole allocator is host-tested without a shared network.
pub trait HostFiles: Send + Sync {
    fn read(&self, path: &str) -> std::io::Result<String>;
}

/// Who answered ARP on the shared network recently; an address in use with no lease is still in use.
pub trait Neighbors: Send + Sync {
    fn observed(&self) -> Vec<Ipv4Addr>;
}

/// Read, never written: lns assigns addresses beside Apple's DHCP server, it does not configure it.
pub const LEASES_PATH: &str = "/var/db/dhcpd_leases";
pub const BOOTPD_PLIST_PATH: &str = "/etc/bootpd.plist";

const DEFAULT_NETWORK: Ipv4Addr = Ipv4Addr::new(192, 168, 64, 0);
const DEFAULT_PREFIX_LEN: u8 = 24;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostNetwork {
    pub network: Ipv4Addr,
    pub prefix_len: u8,
    pub gateway: Ipv4Addr,
}

impl Default for HostNetwork {
    /// What macOS gives a shared network when `/etc/bootpd.plist` says nothing: 192.168.64.0/24 with the host at .1.
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
    pub fn usable_high_to_low(&self) -> Vec<Ipv4Addr> {
        let first = u32::from(self.network) + 1;
        let last = u32::from(self.broadcast()).saturating_sub(1);
        (first..=last).rev().map(Ipv4Addr::from).collect()
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

/// Apple's plist is XML and lns carries no plist parser, so the two keys it needs are read directly and anything else falls back.
pub fn parse_bootpd_network(text: &str) -> Option<HostNetwork> {
    let network: Ipv4Addr = plist_string(text, "net_address")?.parse().ok()?;
    let prefix_len = plist_string(text, "net_mask")
        .and_then(|mask| mask.parse::<Ipv4Addr>().ok())
        .and_then(prefix_len_of)?;
    Some(HostNetwork {
        network: Ipv4Addr::from(u32::from(network) & mask_bits(prefix_len)),
        prefix_len,
        gateway: Ipv4Addr::from((u32::from(network) & mask_bits(prefix_len)) + 1),
    })
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
pub fn parse_arp_neighbors(text: &str) -> Vec<Ipv4Addr> {
    let mut seen = BTreeSet::new();
    for line in text.lines() {
        let Some(open) = line.find('(') else { continue };
        let Some(close) = line[open..].find(')') else {
            continue;
        };
        if let Ok(addr) = line[open + 1..open + close].parse::<Ipv4Addr>() {
            seen.insert(addr);
        }
    }
    seen.into_iter().collect()
}

/// The parameters lns allocates within. A missing or unreadable plist is the ordinary case, not a failure.
pub fn read_host_network(files: &dyn HostFiles) -> HostNetwork {
    files
        .read(BOOTPD_PLIST_PATH)
        .ok()
        .and_then(|text| parse_bootpd_network(&text))
        .unwrap_or_default()
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

    #[test]
    fn the_apple_default_network_is_used_when_the_plist_says_nothing() {
        let files = FakeHostFiles::new();
        assert_eq!(read_host_network(&files), HostNetwork::default());
        assert_eq!(
            HostNetwork::default().gateway,
            Ipv4Addr::new(192, 168, 64, 1)
        );
    }

    #[test]
    fn the_plist_moves_the_network_when_the_host_has_one_of_its_own() {
        let plist = "<plist><dict><key>Subnets</key><array><dict>
            <key>net_address</key><string>10.37.129.0</string>
            <key>net_mask</key><string>255.255.255.0</string>
            </dict></array></dict></plist>";
        let files = FakeHostFiles::new().with(BOOTPD_PLIST_PATH, plist);
        assert_eq!(
            read_host_network(&files),
            HostNetwork {
                network: Ipv4Addr::new(10, 37, 129, 0),
                prefix_len: 24,
                gateway: Ipv4Addr::new(10, 37, 129, 1),
            }
        );
    }

    #[test]
    fn a_plist_lns_cannot_read_falls_back_instead_of_inventing_a_network() {
        for text in [
            "<plist><dict></dict></plist>",
            "<plist><key>net_address</key><string>nonsense</string><key>net_mask</key><string>255.255.255.0</string></plist>",
            "<plist><key>net_address</key><string>10.0.0.0</string></plist>",
            "<plist><key>net_address</key><string>10.0.0.0</string><key>net_mask</key><string>255.0.255.0</string></plist>",
            "<plist><key>net_address</key><string>10.0.0.0</string><key>net_mask</key><string>255.255.255.254</string></plist>",
            "<plist><key>net_address</key><string>10.0.0.0</string><key>net_mask</key>",
            "<plist><key>net_address</key><string>10.0.0.0</string><key>net_mask</key><string>255.255.255.0",
        ] {
            let files = FakeHostFiles::new().with(BOOTPD_PLIST_PATH, text);
            assert_eq!(read_host_network(&files), HostNetwork::default(), "{text}");
        }
    }

    #[test]
    fn a_host_address_inside_the_subnet_still_names_the_subnet_it_belongs_to() {
        let plist = "<key>net_address</key><string>192.168.64.1</string><key>net_mask</key><string>255.255.255.0</string>";
        assert_eq!(
            parse_bootpd_network(plist),
            Some(HostNetwork::default()),
            "bootpd names the router's own address in some configurations"
        );
    }

    #[test]
    fn the_usable_range_runs_from_the_top_and_excludes_network_and_broadcast() {
        let net = HostNetwork::default();
        let usable = net.usable_high_to_low();
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
        assert_eq!(net.usable_high_to_low().len(), 6);
        assert_eq!(mask_bits(0), 0);
    }

    #[test]
    fn arp_output_names_the_neighbours_that_answered() {
        let text = "? (192.168.64.1) at 5a:df:1:2:3:4 on bridge100 ifscope [ethernet]\n\
                    ? (192.168.64.7) at 52:54:0:ab:cd:ef on bridge100 ifscope [ethernet]\n\
                    ? (192.168.64.7) at 52:54:0:ab:cd:ef on bridge100 ifscope permanent [ethernet]\n\
                    nothing useful here\n\
                    ? (not-an-address) at incomplete on bridge100\n\
                    ? (10.0.0.1 at broken\n";
        assert_eq!(
            parse_arp_neighbors(text),
            vec![
                Ipv4Addr::new(192, 168, 64, 1),
                Ipv4Addr::new(192, 168, 64, 7)
            ]
        );
    }
}
