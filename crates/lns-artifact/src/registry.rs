/// Whether a registry host is loopback, so a client should reach it over plaintext HTTP (as Docker/containerd treat `localhost:5000`) rather than HTTPS.
pub fn is_loopback_registry(registry: &str) -> bool {
    let host = match registry.rsplit_once(':') {
        Some((host, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => host,
        _ => registry,
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host == "localhost"
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_hosts_are_recognized_with_or_without_a_port() {
        assert!(is_loopback_registry("127.0.0.1:5000"));
        assert!(is_loopback_registry("127.0.0.1"));
        assert!(is_loopback_registry("127.9.9.9:8080"));
        assert!(is_loopback_registry("localhost:5000"));
        assert!(is_loopback_registry("localhost"));
        assert!(is_loopback_registry("[::1]:5000"));
    }

    #[test]
    fn remote_registries_are_not_loopback() {
        assert!(!is_loopback_registry("ghcr.io"));
        assert!(!is_loopback_registry("registry.example.test:443"));
        assert!(!is_loopback_registry("docker.io"));
        assert!(!is_loopback_registry("127registry.example"));
    }

    #[test]
    fn a_remote_host_masquerading_as_loopback_is_not_downgraded_to_http() {
        assert!(!is_loopback_registry("127.0.0.1.evil.com"));
        assert!(!is_loopback_registry("127.evil.com:443"));
        assert!(!is_loopback_registry("localhost.evil.com"));
        assert!(!is_loopback_registry("evil.com:127"));
    }
}
