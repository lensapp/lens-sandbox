use oci_client::client::ClientProtocol;

/// The only place a registry client's transport is chosen: plaintext HTTP for a loopback registry (as Docker/containerd treat `localhost:5000`), TLS for everything else.
pub fn client_protocol_for(registry: &str) -> ClientProtocol {
    if is_loopback_registry(registry) {
        ClientProtocol::Http
    } else {
        ClientProtocol::Https
    }
}

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
    use oci_client::client::ClientProtocol;

    fn is_plaintext(registry: &str) -> bool {
        matches!(client_protocol_for(registry), ClientProtocol::Http)
    }

    #[test]
    fn a_loopback_registry_is_reached_over_plaintext_http() {
        assert!(is_plaintext("localhost"));
        assert!(is_plaintext("localhost:5000"));
        assert!(is_plaintext("127.0.0.1"));
        assert!(is_plaintext("127.0.0.1:5000"));
        assert!(is_plaintext("127.9.9.9:8080"));
        assert!(is_plaintext("[::1]"));
        assert!(is_plaintext("[::1]:5000"));
    }

    #[test]
    fn every_other_registry_including_a_loopback_lookalike_stays_on_https() {
        assert!(!is_plaintext("ghcr.io"));
        assert!(!is_plaintext("docker.io"));
        assert!(!is_plaintext("registry.example.test:443"));
        assert!(!is_plaintext("127registry.example"));
        assert!(!is_plaintext("127.0.0.1.evil.com"));
        assert!(!is_plaintext("127.evil.com:443"));
        assert!(!is_plaintext("localhost.evil.com"));
        assert!(!is_plaintext("evil.com:127"));
        assert!(!is_plaintext("10.0.0.1:5000"));
        assert!(!is_plaintext(""));
    }

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
