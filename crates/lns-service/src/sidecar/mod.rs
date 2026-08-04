//! A sidecar is an auxiliary guest attached to a run: rootful, unsupervised, and with no network device, so any egress it does not route to the run's proxy reaches nothing.

use anyhow::{Context, Result};
use lns_ipc::VolumeMount;

pub mod bridge;
pub(crate) mod launch;
pub mod ready;
pub(crate) mod supervise;

/// One service a sidecar publishes: `guest_port` inside the sidecar, surfaced as a unix socket in the workload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expose {
    pub guest_port: u16,
    pub socket_path: String,
    /// What the workload's uid needs to reach the socket.
    pub socket_mode: u32,
    /// The host-listening vsock port this service's workload-side bridge dials; unique across the whole run.
    pub host_port: u32,
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

/// One host-listening vsock channel per exposed service, with the receiving ends grouped per sidecar so each bring-up bridges only its own.
pub fn service_channels(
    sidecars: &[Sidecar],
) -> (
    Vec<crate::vm::VsockChannel>,
    Vec<Vec<tokio::sync::mpsc::UnboundedReceiver<std::os::fd::RawFd>>>,
) {
    let mut channels = Vec::new();
    let mut receivers = Vec::with_capacity(sidecars.len());
    for sidecar in sidecars {
        let mut mine = Vec::with_capacity(sidecar.expose.len());
        for expose in &sidecar.expose {
            let (fd_tx, fd_rx) = tokio::sync::mpsc::unbounded_channel();
            channels.push(crate::vm::VsockChannel {
                port: expose.host_port,
                fd_tx,
            });
            mine.push(fd_rx);
        }
        receivers.push(mine);
    }
    (channels, receivers)
}

/// A sidecar's session output belongs on the developer trace stream; the run's own output is the workload's.
pub fn session_output(frame: &lns_ipc::WireFrame) -> Option<String> {
    let bytes = match frame {
        lns_ipc::WireFrame::Stdout(bytes) | lns_ipc::WireFrame::Stderr(bytes) => bytes,
        lns_ipc::WireFrame::Json(_) => return None,
    };
    let text = String::from_utf8_lossy(bytes).trim_end().to_string();
    (!text.is_empty()).then_some(text)
}

/// The `revforward.<i>.*` keys the *workload* guest needs so every exposed service of every sidecar surfaces at its socket there. The broker stops parsing at the first gap, so the indices run contiguously across the whole run.
pub fn workload_revforward_cmdline(sidecars: &[Sidecar]) -> Vec<(String, String)> {
    let mut keys = Vec::new();
    for (index, expose) in sidecars.iter().flat_map(|s| &s.expose).enumerate() {
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
            expose.host_port.to_string(),
        ));
    }
    keys
}

/// What a sidecar gets when its declaration names no resources; a declaration that needs more says so.
const DEFAULT_CPUS: u8 = 2;
const DEFAULT_MEMORY_MIB: usize = 1024;

/// Map the declared sidecars onto the runtime shape, giving every exposed service in the run its own host-listening vsock port.
pub fn plan(
    declared: &[lns_artifact::sandbox::Sidecar],
    host: Option<lns_artifact::resources::HostCapacity>,
) -> Result<Vec<Sidecar>> {
    let mut planned = Vec::with_capacity(declared.len());
    let mut services = 0usize;
    for sidecar in declared {
        let mut expose = Vec::with_capacity(sidecar.expose.len());
        for service in &sidecar.expose {
            expose.push(
                exposed(service, services)
                    .with_context(|| format!("sidecar {:?}", sidecar.name))?,
            );
            services += 1;
        }
        let size = crate::artifact::resources::resolve_size(
            sidecar.resources.as_ref(),
            &lns_artifact::resources::ResourceOverrides::default(),
            lns_artifact::resources::VmSize {
                cpus: DEFAULT_CPUS,
                mem_mib: DEFAULT_MEMORY_MIB,
            },
            host,
        );
        planned.push(Sidecar {
            id: sidecar.name.clone(),
            image: sidecar.image.clone(),
            argv: sidecar
                .command
                .as_deref()
                .map(argv)
                .transpose()
                .with_context(|| format!("sidecar {:?}", sidecar.name))?
                .unwrap_or_default(),
            env: sidecar
                .env
                .iter()
                .map(|(key, value)| format!("{key}={value}"))
                .collect(),
            volumes: sidecar
                .volumes
                .iter()
                .map(|volume| VolumeMount {
                    name: volume.source().to_string(),
                    target: volume.target.clone(),
                    read_only: volume.read_only(),
                })
                .collect(),
            expose,
            egress_via_proxy: sidecar.egress == lns_artifact::sandbox::SidecarEgress::Proxy,
            cpus: size.cpus,
            memory_mib: size.mem_mib,
        });
    }
    Ok(planned)
}

fn exposed(service: &lns_artifact::sandbox::SidecarExpose, index: usize) -> Result<Expose> {
    Ok(Expose {
        guest_port: u16::try_from(service.guest_port)
            .with_context(|| format!("guest port {}", service.guest_port))?,
        socket_path: service.socket.clone(),
        socket_mode: service.mode_bits()?,
        host_port: lns_session::sidecar_service_port(index),
    })
}

/// A declared command is one string; splitting it with shell quoting keeps `--flag "two words"` one argument.
fn argv(command: &str) -> Result<Vec<String>> {
    shlex::split(command)
        .ok_or_else(|| anyhow::anyhow!("command {command:?} has an unterminated quote"))
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

    fn expose_at(socket_path: &str, host_port: u32) -> Expose {
        Expose {
            guest_port: 2375,
            socket_path: socket_path.into(),
            socket_mode: 0o600,
            host_port,
        }
    }

    fn expose(socket_path: &str, socket_mode: u32) -> Expose {
        Expose {
            guest_port: 2375,
            socket_path: socket_path.into(),
            socket_mode,
            host_port: lns_session::sidecar_service_port(0),
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
            workload_revforward_cmdline(&[s]),
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
    fn a_sidecar_exposing_nothing_asks_the_workload_for_no_bridges() {
        assert!(workload_revforward_cmdline(&[sidecar(true)]).is_empty());
    }

    #[test]
    fn the_workload_bridges_run_contiguously_from_zero_across_every_sidecar() {
        // The broker stops parsing revforward keys at the first gap, so a second sidecar's services must continue the numbering.
        let mut first = sidecar(false);
        first.expose = vec![expose("/var/run/one.sock", 0o600)];
        let mut second = sidecar(false);
        second.expose = vec![
            expose("/var/run/two.sock", 0o600),
            expose("/var/run/three.sock", 0o600),
        ];
        let keys = workload_revforward_cmdline(&[first, second]);
        let sockets: Vec<(&str, &str)> = keys
            .iter()
            .filter(|(k, _)| k.ends_with(".unix"))
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        assert_eq!(
            sockets,
            vec![
                ("revforward.0.unix", "/var/run/one.sock"),
                ("revforward.1.unix", "/var/run/two.sock"),
                ("revforward.2.unix", "/var/run/three.sock"),
            ]
        );
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
    fn declared(name: &str) -> lns_artifact::sandbox::Sidecar {
        lns_artifact::sandbox::Sidecar {
            name: name.into(),
            image: "example.test/some/image@sha256:abc".into(),
            command: None,
            env: std::collections::BTreeMap::new(),
            egress: lns_artifact::sandbox::SidecarEgress::None,
            resources: None,
            volumes: Vec::new(),
            expose: Vec::new(),
        }
    }

    fn declared_expose(guest_port: i64, socket: &str) -> lns_artifact::sandbox::SidecarExpose {
        lns_artifact::sandbox::SidecarExpose {
            guest_port,
            socket: socket.into(),
            mode: None,
        }
    }

    #[test]
    fn planning_names_the_sidecar_whose_command_never_closes_its_quote() {
        let mut d = declared("some-sidecar");
        d.command = Some(r#"/usr/bin/aux --flag "two words"#.into());

        let err = format!(
            "{:#}",
            plan(&[d], None).expect_err("an argv nobody can read")
        );

        assert!(err.contains("some-sidecar"), "got: {err}");
        assert!(err.contains("unterminated quote"), "got: {err}");
    }

    #[test]
    fn planning_carries_the_declaration_onto_the_runtime_shape() {
        let mut d = declared("some-sidecar");
        d.command = Some(r#"/usr/bin/aux --flag "two words""#.into());
        d.env = [("AUX_ROOT".to_string(), "/data".to_string())]
            .into_iter()
            .collect();
        d.egress = lns_artifact::sandbox::SidecarEgress::Proxy;
        d.expose = vec![declared_expose(2375, "/var/run/aux.sock")];

        let planned = plan(&[d], None).expect("plan");

        assert_eq!(planned[0].id, "some-sidecar");
        assert_eq!(
            planned[0].argv,
            vec!["/usr/bin/aux", "--flag", "two words"],
            "a quoted argument must stay one argv entry"
        );
        assert_eq!(planned[0].env, vec!["AUX_ROOT=/data".to_string()]);
        assert!(planned[0].egress_via_proxy);
        assert_eq!(planned[0].expose[0].guest_port, 2375);
        assert_eq!(planned[0].expose[0].socket_path, "/var/run/aux.sock");
        assert_eq!(
            planned[0].expose[0].socket_mode,
            lns_artifact::sandbox::DEFAULT_SOCKET_MODE
        );
    }

    #[test]
    fn every_exposed_service_in_the_run_gets_its_own_host_port() {
        // Two services sharing a host port would splice one's bytes into the other.
        let mut first = declared("some-sidecar");
        first.expose = vec![declared_expose(2375, "/var/run/one.sock")];
        let mut second = declared("other-sidecar");
        second.expose = vec![declared_expose(2375, "/var/run/two.sock")];

        let planned = plan(&[first, second], None).expect("plan");

        assert_ne!(
            planned[0].expose[0].host_port, planned[1].expose[0].host_port,
            "numbering is per run, not per sidecar"
        );
    }

    #[test]
    fn a_declared_volume_becomes_a_named_attachment() {
        let mut d = declared("some-sidecar");
        d.volumes = vec![
            serde_json::from_str(r#"{"name":"aux-state","target":"/data","readOnly":true}"#)
                .expect("volume"),
        ];

        let planned = plan(&[d], None).expect("plan");

        assert_eq!(planned[0].volumes[0].name, "aux-state");
        assert_eq!(planned[0].volumes[0].target, "/data");
        assert!(planned[0].volumes[0].read_only);
    }

    #[test]
    fn a_sidecar_that_declares_no_size_gets_the_default_one() {
        let planned = plan(&[declared("some-sidecar")], None).expect("plan");
        assert_eq!(planned[0].cpus, DEFAULT_CPUS);
        assert_eq!(planned[0].memory_mib, DEFAULT_MEMORY_MIB);
    }

    #[test]
    fn a_declared_size_wins_over_the_default() {
        let mut d = declared("some-sidecar");
        d.resources = Some(serde_json::from_str(r#"{"cpu":4,"memory":"4Gi"}"#).expect("resources"));
        let planned = plan(&[d], None).expect("plan");
        assert_eq!(planned[0].cpus, 4);
        assert_eq!(planned[0].memory_mib, 4096);
    }

    #[test]
    fn a_share_of_the_host_is_read_against_the_host_the_run_boots_on() {
        // A share means nothing without the host's own size, so a planner that dropped it would silently hand every sidecar the default.
        let mut d = declared("some-sidecar");
        d.resources =
            Some(serde_json::from_str(r#"{"cpu":"50%","memory":"50%"}"#).expect("resources"));
        let host = lns_artifact::resources::HostCapacity {
            cpus: 8,
            mem_mib: 16384,
        };

        let planned = plan(&[d], Some(host)).expect("plan");

        assert_eq!(planned[0].cpus, 4);
        assert_eq!(planned[0].memory_mib, 8192);
    }

    #[test]
    fn a_share_without_a_host_to_read_it_against_falls_back_to_the_default() {
        // Probing the host can fail, and a sidecar that cannot boot is worse than one that boots small.
        let mut d = declared("some-sidecar");
        d.resources =
            Some(serde_json::from_str(r#"{"cpu":"50%","memory":"50%"}"#).expect("resources"));

        let planned = plan(&[d], None).expect("plan");

        assert_eq!(planned[0].cpus, DEFAULT_CPUS);
        assert_eq!(planned[0].memory_mib, DEFAULT_MEMORY_MIB);
    }

    #[test]
    fn planning_names_the_sidecar_whose_port_does_not_fit_a_port() {
        // parse() already rejects this; a declaration reaching the planner another way must not wrap into a valid port.
        let mut d = declared("some-sidecar");
        d.expose = vec![declared_expose(99999, "/var/run/aux.sock")];
        let err = format!("{:#}", plan(&[d], None).expect_err("99999 is not a port"));
        assert!(err.contains("some-sidecar"), "got: {err}");
    }

    #[test]
    fn planning_surfaces_a_socket_mode_that_is_not_octal() {
        let mut d = declared("some-sidecar");
        d.expose = vec![lns_artifact::sandbox::SidecarExpose {
            guest_port: 2375,
            socket: "/var/run/aux.sock".into(),
            mode: Some("0999".into()),
        }];
        let err = format!("{:#}", plan(&[d], None).expect_err("0999 is not octal"));
        assert!(err.contains("must be octal"), "got: {err}");
    }

    #[test]
    fn every_exposed_service_gets_a_channel_grouped_under_its_own_sidecar() {
        let mut first = sidecar(false);
        first.expose = vec![expose("/var/run/one.sock", 0o600)];
        let mut second = sidecar(false);
        second.expose = vec![
            expose_at("/var/run/two.sock", lns_session::sidecar_service_port(1)),
            expose_at("/var/run/three.sock", lns_session::sidecar_service_port(2)),
        ];

        let (channels, receivers) = service_channels(&[first, second]);

        assert_eq!(
            channels.iter().map(|c| c.port).collect::<Vec<_>>(),
            vec![
                lns_session::sidecar_service_port(0),
                lns_session::sidecar_service_port(1),
                lns_session::sidecar_service_port(2),
            ],
            "each channel listens on the port its service was planned onto"
        );
        assert_eq!(
            receivers.iter().map(Vec::len).collect::<Vec<_>>(),
            vec![1, 2],
            "a bring-up must receive only its own sidecar's connections"
        );
    }

    #[test]
    fn a_sidecar_exposing_nothing_needs_no_channel() {
        let (channels, receivers) = service_channels(&[sidecar(true)]);
        assert!(channels.is_empty());
        assert_eq!(receivers.iter().map(Vec::len).collect::<Vec<_>>(), vec![0]);
    }

    #[test]
    fn stdout_and_stderr_from_a_sidecar_reach_the_trace_stream() {
        assert_eq!(
            session_output(&lns_ipc::WireFrame::Stdout(b"dockerd ready\n".to_vec())).as_deref(),
            Some("dockerd ready")
        );
        assert_eq!(
            session_output(&lns_ipc::WireFrame::Stderr(b"warning\n".to_vec())).as_deref(),
            Some("warning")
        );
    }

    #[test]
    fn a_blank_frame_says_nothing_worth_logging() {
        assert!(session_output(&lns_ipc::WireFrame::Stdout(b"\n".to_vec())).is_none());
        assert!(
            session_output(&lns_ipc::WireFrame::Json(lns_ipc::Response::Pong)).is_none(),
            "a control frame is not sidecar output"
        );
    }
}
