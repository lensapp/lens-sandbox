//! A sidecar is an auxiliary guest attached to a run: rootful, unsupervised, and with no network device, so any egress it does not route to the run's proxy reaches nothing.

use lns_ipc::VolumeMount;

pub mod bridge;

/// One service a sidecar publishes: `guest_port` inside the sidecar, surfaced as a unix socket in the workload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expose {
    pub guest_port: u16,
    pub socket_path: String,
    /// What the workload's uid needs to reach the socket.
    pub socket_mode: u32,
}

/// A sidecar declaration. Docker is the first of these, not the shape of it.
#[derive(Debug, Clone)]
pub struct Sidecar {
    /// Names the sidecar in logs, `ps`, and audit; also the identity policy will eventually scope rules to.
    pub id: String,
    /// Digest-pinned OCI reference — the sidecar's rootfs, resolved through the same ingest path as a workload image.
    pub image: String,
    pub argv: Vec<String>,
    pub env: Vec<String>,
    pub volumes: Vec<VolumeMount>,
    pub expose: Vec<Expose>,
    /// Inject `HTTP(S)_PROXY` and bridge the sidecar's local proxy port to the run's supervisor proxy.
    pub egress_via_proxy: bool,
    pub cpus: u8,
    pub memory_mib: usize,
}

/// The loopback port a sidecar's own egress dials; the guest-side `revforward` bridges it to the host, which lands it on the run's proxy.
pub const SIDECAR_PROXY_PORT: u16 = 3128;

impl Sidecar {
    /// The `revforward.<i>.*` kernel-cmdline keys the sidecar's broker needs to bridge its egress out to the host.
    pub fn revforward_cmdline(&self) -> Vec<(String, String)> {
        if !self.egress_via_proxy {
            return Vec::new();
        }
        vec![
            (
                "revforward.0.tcp".into(),
                format!("127.0.0.1:{SIDECAR_PROXY_PORT}"),
            ),
            (
                "revforward.0.hostport".into(),
                lns_session::SIDECAR_EGRESS_PORT.to_string(),
            ),
        ]
    }

    /// The `revforward.<i>.*` keys the *workload* guest needs so each exposed service surfaces at its socket there, one host-listening port per exposed service of this sidecar.
    pub fn workload_revforward_cmdline(&self) -> Vec<(String, String)> {
        let mut keys = Vec::new();
        for (index, expose) in self.expose.iter().enumerate() {
            keys.push((
                format!("revforward.{index}.unix"),
                expose.socket_path.clone(),
            ));
            keys.push((
                format!("revforward.{index}.mode"),
                format!("{:o}", expose.socket_mode),
            ));
            keys.push((
                format!("revforward.{index}.hostport"),
                lns_session::sidecar_service_port(index).to_string(),
            ));
        }
        keys
    }

    /// The env the sidecar's primary session is opened with; the broker installs the CA and strips it before exec, so the image needs no cooperation.
    pub fn session_env(&self, proxy_ca: Option<&str>) -> Vec<String> {
        let mut env = self.env.clone();
        env.extend(self.proxy_env());
        if let Some(pem) = proxy_ca.filter(|_| self.egress_via_proxy) {
            env.push(format!("{}={pem}", lns_session::PROXY_CA_ENV));
        }
        env
    }

    /// Proxy env for the sidecar's own processes.
    pub fn proxy_env(&self) -> Vec<String> {
        if !self.egress_via_proxy {
            return Vec::new();
        }
        let url = format!("http://127.0.0.1:{SIDECAR_PROXY_PORT}");
        vec![
            format!("HTTP_PROXY={url}"),
            format!("HTTPS_PROXY={url}"),
            format!("http_proxy={url}"),
            format!("https_proxy={url}"),
            "NO_PROXY=127.0.0.1,localhost".to_string(),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sidecar(egress_via_proxy: bool) -> Sidecar {
        Sidecar {
            id: "some-sidecar".into(),
            image: "example.test/some/image@sha256:abc".into(),
            argv: vec!["/bin/true".into()],
            env: Vec::new(),
            volumes: Vec::new(),
            expose: Vec::new(),
            egress_via_proxy,
            cpus: 2,
            memory_mib: 1024,
        }
    }

    fn expose(socket_path: &str, socket_mode: u32) -> Expose {
        Expose {
            guest_port: 2375,
            socket_path: socket_path.into(),
            socket_mode,
        }
    }

    #[test]
    fn a_proxied_sidecar_bridges_its_loopback_proxy_port_to_the_host_egress_port() {
        let keys = sidecar(true).revforward_cmdline();
        assert_eq!(
            keys,
            vec![
                ("revforward.0.tcp".to_string(), "127.0.0.1:3128".to_string()),
                (
                    "revforward.0.hostport".to_string(),
                    lns_session::SIDECAR_EGRESS_PORT.to_string()
                ),
            ]
        );
    }

    #[test]
    fn a_sidecar_that_wants_no_egress_gets_no_bridge_and_no_proxy_env() {
        // Without the bridge there is no route off the guest at all: no NIC, no vsock client.
        let s = sidecar(false);
        assert!(s.revforward_cmdline().is_empty());
        assert!(s.proxy_env().is_empty());
    }

    #[test]
    fn an_exposed_service_surfaces_in_the_workload_with_its_mode_in_octal() {
        // The cmdline carries octal; a decimal 438 would be parsed back as 0o438 and reject.
        let mut s = sidecar(false);
        s.expose = vec![expose("/var/run/some.sock", 0o666)];
        assert_eq!(
            s.workload_revforward_cmdline(),
            vec![
                (
                    "revforward.0.unix".to_string(),
                    "/var/run/some.sock".to_string()
                ),
                ("revforward.0.mode".to_string(), "666".to_string()),
                (
                    "revforward.0.hostport".to_string(),
                    lns_session::sidecar_service_port(0).to_string()
                ),
            ]
        );
    }

    #[test]
    fn each_exposed_service_gets_its_own_host_port() {
        // Two services sharing a channel would splice one's bytes into the other.
        let mut s = sidecar(false);
        s.expose = vec![
            expose("/var/run/one.sock", 0o600),
            expose("/var/run/two.sock", 0o600),
        ];
        let keys = s.workload_revforward_cmdline();
        let ports: Vec<&String> = keys
            .iter()
            .filter(|(k, _)| k.ends_with(".hostport"))
            .map(|(_, v)| v)
            .collect();
        assert_eq!(ports.len(), 2);
        assert_ne!(ports[0], ports[1]);
    }

    #[test]
    fn a_sidecar_exposing_nothing_asks_the_workload_for_no_bridges() {
        assert!(sidecar(true).workload_revforward_cmdline().is_empty());
    }

    #[test]
    fn proxy_env_points_every_common_variable_at_the_local_bridge() {
        let env = sidecar(true).proxy_env();
        for key in ["HTTP_PROXY", "HTTPS_PROXY", "http_proxy", "https_proxy"] {
            assert!(
                env.contains(&format!("{key}=http://127.0.0.1:3128")),
                "missing {key} in {env:?}"
            );
        }
    }

    #[test]
    fn loopback_is_excluded_from_the_proxy_so_the_bridge_itself_is_reachable() {
        // Without this a client that honours NO_PROXY would try to proxy its own proxy.
        let env = sidecar(true).proxy_env();
        assert!(env.iter().any(|e| e.starts_with("NO_PROXY=")
            && e.contains("127.0.0.1")
            && e.contains("localhost")));
    }

    #[test]
    fn the_session_env_carries_the_ca_for_the_broker_to_install() {
        let env = sidecar(true).session_env(Some("PEM-BODY"));
        assert!(
            env.contains(&format!("{}=PEM-BODY", lns_session::PROXY_CA_ENV)),
            "missing the CA in {env:?}"
        );
    }

    #[test]
    fn a_sidecar_that_does_not_use_the_proxy_is_told_nothing_about_its_ca() {
        // No bridge means no proxy to trust, so shipping the CA would widen what the guest trusts for nothing.
        let env = sidecar(false).session_env(Some("PEM-BODY"));
        assert!(
            !env.iter().any(|e| e.contains(lns_session::PROXY_CA_ENV)),
            "unexpected CA in {env:?}"
        );
    }

    #[test]
    fn a_run_with_no_ca_yet_still_gets_a_usable_session_env() {
        let env = sidecar(true).session_env(None);
        assert!(!env.iter().any(|e| e.contains(lns_session::PROXY_CA_ENV)));
        assert!(env.iter().any(|e| e.starts_with("HTTP_PROXY=")));
    }

    #[test]
    fn the_sidecars_own_declared_env_survives_alongside_the_injected_vars() {
        let mut s = sidecar(true);
        s.env = vec!["SOME_VAR=some-value".into()];
        let env = s.session_env(Some("PEM-BODY"));
        assert!(env.contains(&"SOME_VAR=some-value".to_string()), "{env:?}");
        assert!(env.iter().any(|e| e.starts_with("NO_PROXY=")), "{env:?}");
    }
}
