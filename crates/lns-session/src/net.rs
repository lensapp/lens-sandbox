use std::net::Ipv4Addr;

/// The address plan the host reserved for one guest, carried on the kernel command line because the session channel arrives after the network step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuestNet {
    pub candidates: Vec<Ipv4Addr>,
    pub prefix_len: u8,
    pub gateway: Ipv4Addr,
    pub dns: Vec<Ipv4Addr>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuestNetError {
    Shape(String),
    Address(String),
    PrefixLength(String),
    NoCandidate,
}

impl std::fmt::Display for GuestNetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Shape(value) => write!(
                f,
                "expected <addrs>/<prefix>|<gateway>|<dns>, got {value:?}"
            ),
            Self::Address(value) => write!(f, "{value:?} is not an IPv4 address"),
            Self::PrefixLength(value) => {
                write!(f, "{value:?} is not a prefix length between 1 and 30")
            }
            Self::NoCandidate => write!(f, "the host offered no candidate address"),
        }
    }
}

impl std::error::Error for GuestNetError {}

impl GuestNet {
    pub fn to_cmdline_value(&self) -> String {
        let addrs: Vec<String> = self.candidates.iter().map(Ipv4Addr::to_string).collect();
        let dns: Vec<String> = self.dns.iter().map(Ipv4Addr::to_string).collect();
        format!(
            "{}/{}|{}|{}",
            addrs.join(","),
            self.prefix_len,
            self.gateway,
            dns.join(",")
        )
    }

    pub fn parse(value: &str) -> Result<Self, GuestNetError> {
        let mut sections = value.split('|');
        let (Some(addrs), Some(gateway), Some(dns), None) = (
            sections.next(),
            sections.next(),
            sections.next(),
            sections.next(),
        ) else {
            return Err(GuestNetError::Shape(value.to_string()));
        };
        let (addrs, prefix) = addrs
            .split_once('/')
            .ok_or_else(|| GuestNetError::Shape(value.to_string()))?;
        let candidates = parse_addrs(addrs)?;
        if candidates.is_empty() {
            return Err(GuestNetError::NoCandidate);
        }
        let prefix_len: u8 = prefix
            .parse()
            .ok()
            .filter(|n| (1..=30).contains(n))
            .ok_or_else(|| GuestNetError::PrefixLength(prefix.to_string()))?;
        Ok(Self {
            candidates,
            prefix_len,
            gateway: parse_addr(gateway)?,
            dns: parse_addrs(dns)?,
        })
    }
}

fn parse_addrs(list: &str) -> Result<Vec<Ipv4Addr>, GuestNetError> {
    if list.is_empty() {
        return Ok(Vec::new());
    }
    list.split(',').map(parse_addr).collect()
}

fn parse_addr(value: &str) -> Result<Ipv4Addr, GuestNetError> {
    value
        .parse()
        .map_err(|_| GuestNetError::Address(value.to_string()))
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
    fn the_plan_survives_the_kernel_command_line_unchanged() {
        let value = plan().to_cmdline_value();
        assert_eq!(
            value,
            "192.168.64.254,192.168.64.253,192.168.64.252/24|192.168.64.1|192.168.64.1"
        );
        assert!(
            !value.chars().any(char::is_whitespace),
            "the kernel splits a value on whitespace and the guest would read half a plan: {value}"
        );
        assert_eq!(GuestNet::parse(&value), Ok(plan()));
    }

    #[test]
    fn a_plan_with_no_dns_server_leaves_the_guest_its_own_fallback() {
        let mut without = plan();
        without.dns.clear();
        let value = without.to_cmdline_value();
        assert!(value.ends_with('|'), "{value}");
        assert_eq!(GuestNet::parse(&value), Ok(without));
    }

    #[test]
    fn a_truncated_or_overlong_plan_is_refused_rather_than_half_applied() {
        for value in [
            "192.168.64.254/24|192.168.64.1",
            "192.168.64.254/24|192.168.64.1|1.1.1.1|extra",
            "192.168.64.254|192.168.64.1|",
        ] {
            assert_eq!(
                GuestNet::parse(value),
                Err(GuestNetError::Shape(value.to_string())),
                "{value}"
            );
        }
    }

    #[test]
    fn every_address_in_the_plan_must_be_ipv4() {
        assert_eq!(
            GuestNet::parse("192.168.64.254,nonsense/24|192.168.64.1|"),
            Err(GuestNetError::Address("nonsense".into()))
        );
        assert_eq!(
            GuestNet::parse("192.168.64.254/24|fe80::1|"),
            Err(GuestNetError::Address("fe80::1".into()))
        );
        assert_eq!(
            GuestNet::parse("192.168.64.254/24|192.168.64.1|1.1.1.1,x"),
            Err(GuestNetError::Address("x".into()))
        );
    }

    #[test]
    fn a_prefix_that_leaves_no_usable_address_is_refused() {
        for prefix in ["0", "31", "32", "24x", ""] {
            let value = format!("192.168.64.254/{prefix}|192.168.64.1|");
            assert_eq!(
                GuestNet::parse(&value),
                Err(GuestNetError::PrefixLength(prefix.to_string())),
                "{value}"
            );
        }
    }

    #[test]
    fn an_empty_candidate_list_names_itself() {
        assert_eq!(
            GuestNet::parse("/24|192.168.64.1|"),
            Err(GuestNetError::NoCandidate)
        );
    }

    #[test]
    fn each_refusal_says_what_was_wrong_with_the_value() {
        assert!(
            GuestNetError::Shape("x".into())
                .to_string()
                .contains("<addrs>/<prefix>|<gateway>|<dns>")
        );
        assert!(
            GuestNetError::Address("x".into())
                .to_string()
                .contains("not an IPv4 address")
        );
        assert!(
            GuestNetError::PrefixLength("31".into())
                .to_string()
                .contains("between 1 and 30")
        );
        assert!(
            GuestNetError::NoCandidate
                .to_string()
                .contains("no candidate address")
        );
    }
}
