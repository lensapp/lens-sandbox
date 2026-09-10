use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;

/// The address plan the host reserved for one guest, sent over the session channel once the guest is up: the shared network does not exist until the VMM starts it, so nothing addressable can be known before then.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuestNet {
    pub candidates: Vec<Ipv4Addr>,
    pub prefix_len: u8,
    pub gateway: Ipv4Addr,
    pub dns: Vec<Ipv4Addr>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuestNetError {
    PrefixLength(u8),
    NoCandidate,
    OutsideSubnet(Ipv4Addr),
}

impl std::fmt::Display for GuestNetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PrefixLength(value) => {
                write!(f, "{value} is not a prefix length between 1 and 30")
            }
            Self::NoCandidate => write!(f, "the host offered no candidate address"),
            Self::OutsideSubnet(address) => {
                write!(f, "{address} is not on the gateway's own subnet")
            }
        }
    }
}

impl std::error::Error for GuestNetError {}

impl GuestNet {
    /// The guest checks the plan before it touches its interface: a plan it cannot use is a refusal, never a half-applied network.
    pub fn validate(&self) -> Result<(), GuestNetError> {
        if !(1..=30).contains(&self.prefix_len) {
            return Err(GuestNetError::PrefixLength(self.prefix_len));
        }
        if self.candidates.is_empty() {
            return Err(GuestNetError::NoCandidate);
        }
        let mask = u32::MAX << (32 - u32::from(self.prefix_len));
        let subnet = u32::from(self.gateway) & mask;
        for candidate in &self.candidates {
            if u32::from(*candidate) & mask != subnet {
                return Err(GuestNetError::OutsideSubnet(*candidate));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan() -> GuestNet {
        GuestNet {
            candidates: vec![
                Ipv4Addr::new(192, 168, 64, 254),
                Ipv4Addr::new(192, 168, 64, 253),
                Ipv4Addr::new(192, 168, 64, 252),
            ],
            prefix_len: 24,
            gateway: Ipv4Addr::new(192, 168, 64, 1),
            dns: vec![Ipv4Addr::new(192, 168, 64, 1)],
        }
    }

    #[test]
    fn a_plan_the_host_could_have_reserved_is_accepted() {
        assert_eq!(plan().validate(), Ok(()));
        let mut without_dns = plan();
        without_dns.dns.clear();
        assert_eq!(without_dns.validate(), Ok(()));
    }

    #[test]
    fn a_plan_with_no_candidate_leaves_the_guest_nothing_to_claim() {
        let mut empty = plan();
        empty.candidates.clear();
        assert_eq!(empty.validate(), Err(GuestNetError::NoCandidate));
    }

    #[test]
    fn a_prefix_that_leaves_no_usable_address_is_refused() {
        for prefix in [0, 31, 32, 33] {
            let mut wrong = plan();
            wrong.prefix_len = prefix;
            assert_eq!(
                wrong.validate(),
                Err(GuestNetError::PrefixLength(prefix)),
                "{prefix}"
            );
        }
    }

    #[test]
    fn a_candidate_outside_the_gateways_subnet_is_not_this_networks_address() {
        let mut foreign = plan();
        foreign.candidates.push(Ipv4Addr::new(10, 0, 0, 9));
        assert_eq!(
            foreign.validate(),
            Err(GuestNetError::OutsideSubnet(Ipv4Addr::new(10, 0, 0, 9)))
        );
    }

    #[test]
    fn each_refusal_says_what_was_wrong_with_the_plan() {
        assert!(
            GuestNetError::NoCandidate
                .to_string()
                .contains("no candidate address")
        );
        assert!(
            GuestNetError::PrefixLength(31)
                .to_string()
                .contains("between 1 and 30")
        );
        assert!(
            GuestNetError::OutsideSubnet(Ipv4Addr::new(10, 0, 0, 9))
                .to_string()
                .contains("10.0.0.9")
        );
    }
}
