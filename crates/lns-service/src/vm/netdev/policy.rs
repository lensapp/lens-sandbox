use std::net::{IpAddr, Ipv4Addr, SocketAddr};

/// Why the host refused to carry a guest flow. The guest sees a reset or a dropped datagram; this is what the log says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    Loopback,
    Unspecified,
    LinkLocal,
    Multicast,
    Broadcast,
    GuestSubnet,
    NotIpv4,
}

impl Refusal {
    pub fn reason(self) -> &'static str {
        match self {
            Refusal::Loopback => "the host's loopback",
            Refusal::Unspecified => "an unspecified address",
            Refusal::LinkLocal => "link-local",
            Refusal::Multicast => "multicast",
            Refusal::Broadcast => "broadcast",
            Refusal::GuestSubnet => "the guest's own subnet",
            Refusal::NotIpv4 => "not IPv4",
        }
    }
}

/// Where a guest flow may be carried to on the host. The guest subnet is refused whole: the gateway serves DNS itself and hosts nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Boundary {
    Host {
        network: Ipv4Addr,
        prefix: u8,
    },
    /// Every destination allowed. Only a test uses this; a run is always served by [`Boundary::around`].
    Permissive,
}

impl Boundary {
    pub fn around(network: Ipv4Addr, prefix: u8) -> Self {
        Boundary::Host { network, prefix }
    }

    pub fn refusal(&self, destination: SocketAddr) -> Option<Refusal> {
        let Boundary::Host { network, prefix } = *self else {
            return None;
        };
        let IpAddr::V4(ip) = destination.ip() else {
            return Some(Refusal::NotIpv4);
        };
        let refusal = if ip.is_loopback() {
            Refusal::Loopback
        } else if ip.octets()[0] == 0 {
            Refusal::Unspecified
        } else if ip.is_link_local() {
            Refusal::LinkLocal
        } else if ip.is_multicast() {
            Refusal::Multicast
        } else if ip.is_broadcast() {
            Refusal::Broadcast
        } else if within(ip, network, prefix) {
            Refusal::GuestSubnet
        } else {
            return None;
        };
        Some(refusal)
    }
}

fn within(ip: Ipv4Addr, network: Ipv4Addr, prefix: u8) -> bool {
    let mask = u32::MAX.checked_shl(32 - u32::from(prefix)).unwrap_or(0);
    u32::from(ip) & mask == u32::from(network) & mask
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_boundary() -> Boundary {
        Boundary::around(Ipv4Addr::new(192, 168, 127, 0), 24)
    }

    fn refusal_for(address: &str) -> Option<Refusal> {
        default_boundary().refusal(SocketAddr::new(address.parse().unwrap(), 80))
    }

    #[test]
    fn the_default_boundary_refuses_every_range_a_guest_must_not_reach() {
        for (address, expected) in [
            ("127.0.0.1", Refusal::Loopback),
            ("127.4.5.6", Refusal::Loopback),
            ("0.0.0.0", Refusal::Unspecified),
            ("0.1.2.3", Refusal::Unspecified),
            ("169.254.1.1", Refusal::LinkLocal),
            ("224.0.0.1", Refusal::Multicast),
            ("239.255.255.250", Refusal::Multicast),
            ("255.255.255.255", Refusal::Broadcast),
            ("192.168.127.1", Refusal::GuestSubnet),
            ("192.168.127.254", Refusal::GuestSubnet),
            ("192.168.127.0", Refusal::GuestSubnet),
        ] {
            assert_eq!(
                refusal_for(address),
                Some(expected),
                "{address} must never be a destination the host opens for a guest"
            );
        }
    }

    #[test]
    fn the_gateway_itself_is_refused_on_every_port_it_does_not_serve() {
        let gateway: IpAddr = "192.168.127.1".parse().unwrap();
        for port in [22, 53, 80, 8080] {
            assert_eq!(
                default_boundary().refusal(SocketAddr::new(gateway, port)),
                Some(Refusal::GuestSubnet),
                "there is no control API on the gateway, whatever the port"
            );
        }
    }

    #[test]
    fn an_ordinary_destination_is_carried_exactly_as_it_was_on_the_bridge() {
        for address in ["93.184.216.34", "1.1.1.1", "10.0.0.5", "192.168.1.20"] {
            assert_eq!(
                refusal_for(address),
                None,
                "{address} reached the guest on vmnet and still does"
            );
        }
    }

    #[test]
    fn an_ipv6_destination_is_refused_because_the_link_is_ipv4_only() {
        assert_eq!(
            default_boundary().refusal("[::1]:80".parse().unwrap()),
            Some(Refusal::NotIpv4)
        );
    }

    #[test]
    fn a_permissive_boundary_carries_what_the_default_refuses() {
        for destination in ["127.0.0.1:80", "192.168.127.1:80", "[::1]:80"] {
            assert_eq!(
                Boundary::Permissive.refusal(destination.parse().unwrap()),
                None,
                "a test stands its listener wherever it likes"
            );
        }
    }

    #[test]
    fn every_refusal_says_which_range_it_was() {
        for refusal in [
            Refusal::Loopback,
            Refusal::Unspecified,
            Refusal::LinkLocal,
            Refusal::Multicast,
            Refusal::Broadcast,
            Refusal::GuestSubnet,
            Refusal::NotIpv4,
        ] {
            assert!(!refusal.reason().is_empty());
        }
    }

    #[test]
    fn a_boundary_around_the_whole_address_space_leaves_nothing_reachable() {
        let everything = Boundary::around(Ipv4Addr::UNSPECIFIED, 0);
        assert_eq!(
            everything.refusal("93.184.216.34:80".parse().unwrap()),
            Some(Refusal::GuestSubnet)
        );
    }
}
