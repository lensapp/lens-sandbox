#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

//! The mirror of `forward`: the guest listens locally and dials a port the host is listening on.

#[cfg(target_os = "linux")]
mod real;
#[cfg(target_os = "linux")]
pub use real::spawn_listeners;

/// Where a `revforward` listens inside the guest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Local {
    Tcp { addr: String },
    Unix { path: String, mode: u32 },
}

/// One local endpoint bridged to one host vsock port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spec {
    pub local: Local,
    pub host_port: u32,
}

/// Parse the `revforward.<i>.*` keys out of a kernel cmdline, dropping incomplete entries rather than failing the boot.
pub fn parse_cmdline(cmdline: &str) -> Vec<Spec> {
    let mut specs = Vec::new();
    for index in 0.. {
        let local = match (
            value(cmdline, &format!("revforward.{index}.tcp")),
            value(cmdline, &format!("revforward.{index}.unix")),
        ) {
            (Some(addr), _) => Local::Tcp {
                addr: addr.to_string(),
            },
            (None, Some(path)) => Local::Unix {
                path: path.to_string(),
                mode: value(cmdline, &format!("revforward.{index}.mode"))
                    .and_then(|m| u32::from_str_radix(m, 8).ok())
                    .unwrap_or(0o600),
            },
            (None, None) => break,
        };
        let Some(host_port) = value(cmdline, &format!("revforward.{index}.hostport"))
            .and_then(|p| p.parse::<u32>().ok())
        else {
            break;
        };
        specs.push(Spec { local, host_port });
    }
    specs
}

fn value<'a>(cmdline: &'a str, key: &str) -> Option<&'a str> {
    cmdline
        .split_whitespace()
        .find_map(|tok| tok.strip_prefix(key)?.strip_prefix('='))
        .filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_tcp_entry() {
        let specs =
            parse_cmdline("quiet revforward.0.tcp=127.0.0.1:3128 revforward.0.hostport=1032");
        assert_eq!(
            specs,
            vec![Spec {
                local: Local::Tcp {
                    addr: "127.0.0.1:3128".into()
                },
                host_port: 1032,
            }]
        );
    }

    #[test]
    fn parses_a_unix_entry_with_an_octal_mode() {
        let specs = parse_cmdline(
            "revforward.0.unix=/var/run/docker.sock revforward.0.mode=666 revforward.0.hostport=1031",
        );
        assert_eq!(
            specs,
            vec![Spec {
                local: Local::Unix {
                    path: "/var/run/docker.sock".into(),
                    mode: 0o666,
                },
                host_port: 1031,
            }]
        );
    }

    #[test]
    fn a_unix_entry_without_a_mode_is_owner_only() {
        let specs = parse_cmdline("revforward.0.unix=/run/x.sock revforward.0.hostport=1031");
        assert_eq!(
            specs[0].local,
            Local::Unix {
                path: "/run/x.sock".into(),
                mode: 0o600
            }
        );
    }

    #[test]
    fn parses_several_entries_in_order() {
        let specs = parse_cmdline(
            "revforward.0.tcp=127.0.0.1:3128 revforward.0.hostport=1032 \
             revforward.1.unix=/var/run/docker.sock revforward.1.hostport=1031",
        );
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].host_port, 1032);
        assert_eq!(specs[1].host_port, 1031);
    }

    #[test]
    fn an_entry_missing_its_host_port_stops_the_scan() {
        // Half-written config must not silently bridge to port 0.
        assert!(parse_cmdline("revforward.0.tcp=127.0.0.1:3128").is_empty());
    }

    #[test]
    fn no_keys_yields_no_specs() {
        assert!(parse_cmdline("quiet loglevel=3 upper.dev=/dev/vda").is_empty());
    }

    #[test]
    fn a_tcp_key_wins_over_a_unix_key_at_the_same_index() {
        let specs = parse_cmdline(
            "revforward.0.tcp=127.0.0.1:1 revforward.0.unix=/run/x revforward.0.hostport=9",
        );
        assert_eq!(
            specs[0].local,
            Local::Tcp {
                addr: "127.0.0.1:1".into()
            }
        );
    }

    #[test]
    fn an_empty_value_is_not_a_value() {
        assert!(parse_cmdline("revforward.0.tcp= revforward.0.hostport=1032").is_empty());
    }
}
