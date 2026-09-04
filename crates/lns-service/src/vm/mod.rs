use crate::log;
use anyhow::Result;
use std::path::PathBuf;

mod cloud_hypervisor;
mod connect;
#[cfg(target_os = "macos")]
pub mod diag_console;
pub mod session_client;
mod transport;
#[cfg(target_os = "macos")]
mod vz;

pub use transport::{GuestTransport, VmStopGuard};
#[cfg(target_os = "macos")]
pub use vz::VsockConnector;

#[cfg_attr(target_os = "macos", allow(dead_code))]
pub struct VmSpec {
    pub run_id: String,
    pub cpus: u8,
    pub memory_mib: usize,
    pub kernel: PathBuf,
    pub initrd: PathBuf,
    pub composefs_descriptor: PathBuf,
    pub content_share: PathBuf,
    pub content_tag: String,
    pub descriptor_sha256: Option<String>,
    pub upper_disk: PathBuf,
    pub volumes: Vec<VolumeAttachment>,
    pub binds: Vec<BindAttachment>,
    pub workload_uid: Option<u32>,
    pub workload_gid: Option<u32>,
    pub vsock: Option<VsockChannel>,
    pub connector_tx: Option<tokio::sync::oneshot::Sender<std::sync::Arc<dyn GuestTransport>>>,
    #[cfg(target_os = "macos")]
    pub console_fd: std::os::fd::RawFd,
    pub debug: bool,
    pub exec: ExecSpec,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeAttachment {
    pub host_image: PathBuf,
    pub target: String,
    pub read_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindAttachment {
    pub host_source: PathBuf,
    pub target: String,
    pub read_only: bool,
    pub dropped_paths: Vec<String>,
    /// Every path the definition excluded, so the guest can decide which of them a connector's claim needs left unmounted.
    pub excluded_paths: Vec<String>,
    /// Excluded paths a fileset writes into, so the guest leaves each one for the fileset instead of mounting the share over it.
    pub seeded_paths: Vec<String>,
}

/// The virtio-fs tag the host shares a bind under and the guest mounts it by; one per bind, distinct from the content share.
pub fn bind_share_tag(index: usize) -> String {
    format!("lns-bind-{index}")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeDisk {
    pub host_image: PathBuf,
}

pub fn guest_block_dev(index: usize) -> String {
    let mut suffix = Vec::new();
    let mut n = index;
    loop {
        suffix.push((b'a' + (n % 26) as u8) as char);
        if n < 26 {
            break;
        }
        n = n / 26 - 1;
    }
    let suffix: String = suffix.into_iter().rev().collect();
    format!("/dev/vd{suffix}")
}

/// Volume disks are always attached writable so ext4 can replay its journal and seed a fresh volume; read-only is enforced per-mount inside the guest.
pub fn volume_disks(volumes: &[VolumeAttachment]) -> Vec<VolumeDisk> {
    let mut disks: Vec<VolumeDisk> = Vec::new();
    for v in volumes {
        if !disks.iter().any(|d| d.host_image == v.host_image) {
            disks.push(VolumeDisk {
                host_image: v.host_image.clone(),
            });
        }
    }
    disks
}

fn volume_disk_index(disks: &[VolumeDisk], host_image: &std::path::Path) -> usize {
    disks
        .iter()
        .position(|d| d.host_image == host_image)
        .expect("every attachment's image is present in volume_disks")
}

pub struct VsockChannel {
    pub port: u32,
    pub fd_tx: tokio::sync::mpsc::UnboundedSender<std::os::fd::RawFd>,
}

#[cfg_attr(target_os = "macos", allow(dead_code))]
pub struct ExecSpec {
    pub kernel_env: Vec<(String, String)>,
}

impl ExecSpec {
    fn egress_marker(allowed: bool) -> (String, String) {
        (
            lns_session::EGRESS_ALLOWED_ENV.into(),
            u8::from(allowed).to_string(),
        )
    }
    pub fn server(
        image_config: Option<&oci_client::config::ConfigFile>,
        run_as: &RunAs,
        ws_url: &str,
        token: &str,
        entrypoint: Option<&str>,
        cmd: &[String],
        egress_allowed: bool,
    ) -> Self {
        let agent_command = match image_config {
            Some(cfg) => crate::workload_argv::from_image_config(cfg, entrypoint, cmd),
            None => crate::workload_argv::from_imageless(entrypoint, cmd),
        };

        let mut kernel_env = vec![
            ("LENS_SANDBOX_WS_URL".into(), ws_url.to_string()),
            ("LENS_SANDBOX_TOKEN".into(), token.to_string()),
            ("SANDBOX_USER".into(), run_as.user.clone()),
        ];
        if let Some(uid) = run_as.uid {
            kernel_env.push(("SANDBOX_UID".into(), uid.to_string()));
        }
        if let Some(group) = &run_as.group {
            kernel_env.push(("SANDBOX_GROUP".into(), group.clone()));
        }
        kernel_env.push(("LENS_SANDBOX_USER".into(), run_as.user.clone()));
        kernel_env.push((
            "AGENT_COMMAND_B64".into(),
            crate::base64::encode(agent_command.as_bytes()),
        ));
        kernel_env.push(Self::egress_marker(egress_allowed));
        kernel_env.push((
            "PATH".into(),
            crate::workload_env::GUEST_DEFAULT_PATH.into(),
        ));

        ExecSpec { kernel_env }
    }

    pub fn from_image_config(
        image_config: Option<&oci_client::config::ConfigFile>,
        entrypoint: Option<&str>,
        override_cmd: &[String],
    ) -> Self {
        let argv = match image_config {
            Some(cfg) => crate::workload_argv::from_image_config(cfg, entrypoint, override_cmd),
            None => crate::workload_argv::from_imageless(entrypoint, override_cmd),
        };

        ExecSpec {
            kernel_env: vec![
                (
                    "AGENT_COMMAND_B64".into(),
                    crate::base64::encode(argv.as_bytes()),
                ),
                // The tool provisioner boots without a session and fetches over the network, so it refuses without a lease too.
                Self::egress_marker(true),
                (
                    "PATH".into(),
                    "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".into(),
                ),
            ],
        }
    }

    pub fn for_run(
        run_as: &RunAs,
        entrypoint: Option<&str>,
        cmd: &[String],
        image_config: Option<&oci_client::config::ConfigFile>,
        session: Option<&crate::supervisor::SupervisorSession>,
    ) -> Self {
        match session {
            Some(s) => Self::server(
                image_config,
                run_as,
                &s.relay.url,
                &s.relay.token,
                entrypoint,
                cmd,
                s.egress_allowed,
            ),
            None => Self::from_image_config(image_config, entrypoint, cmd),
        }
    }
}

const DEFAULT_SANDBOX_USER: &str = "sandbox";
const DEFAULT_SANDBOX_UID: u32 = 65534;
const DEFAULT_SANDBOX_GID: u32 = 65534;

#[cfg_attr(not(any(target_os = "linux", target_os = "macos")), allow(dead_code))]
#[derive(Debug, PartialEq, Eq)]
pub struct RunAs {
    pub user: String,
    pub uid: Option<u32>,
    pub group: Option<String>,
}

#[cfg_attr(not(any(target_os = "linux", target_os = "macos")), allow(dead_code))]
pub fn resolve_run_as(
    explicit_user: Option<&str>,
    explicit_uid: Option<u32>,
    spec_user: Option<&str>,
    image_user: Option<&str>,
) -> RunAs {
    if explicit_user.is_some() || explicit_uid.is_some() {
        let (user, group) = explicit_user
            .map(split_user_group)
            .unwrap_or((DEFAULT_SANDBOX_USER, None));
        return RunAs {
            user: user.to_string(),
            uid: Some(
                explicit_uid
                    .or_else(|| user.parse::<u32>().ok())
                    .unwrap_or(DEFAULT_SANDBOX_UID),
            ),
            group: group.map(str::to_string),
        };
    }
    // A declared user takes this branch's semantics, not the flag's: a name keeps `uid: None` so the guest's own passwd resolves it, where the flag branch would hand it the nobody uid.
    match spec_user
        .or(image_user)
        .map(str::trim)
        .filter(|u| !u.is_empty())
    {
        None => RunAs {
            user: DEFAULT_SANDBOX_USER.to_string(),
            uid: Some(DEFAULT_SANDBOX_UID),
            group: None,
        },
        Some(spec) => {
            let (user, group) = match spec.split_once(':') {
                Some((user, group)) => (user, (!group.is_empty()).then(|| group.to_string())),
                None => (spec, None),
            };
            RunAs {
                user: user.to_string(),
                uid: user.parse::<u32>().ok(),
                group,
            }
        }
    }
}

fn split_user_group(spec: &str) -> (&str, Option<&str>) {
    let (user, group) = match spec.split_once(':') {
        Some((user, group)) => (user, (!group.is_empty()).then_some(group)),
        None => (spec, None),
    };
    let user = if user.is_empty() {
        DEFAULT_SANDBOX_USER
    } else {
        user
    };
    (user, group)
}

#[cfg_attr(not(any(target_os = "linux", target_os = "macos")), allow(dead_code))]
pub fn host_known_workload_gid(run_as: &RunAs) -> Option<u32> {
    (run_as.user == DEFAULT_SANDBOX_USER
        && run_as.uid == Some(DEFAULT_SANDBOX_UID)
        && run_as.group.is_none())
    .then_some(DEFAULT_SANDBOX_GID)
}

#[cfg_attr(not(any(target_os = "linux", target_os = "macos")), allow(dead_code))]
#[allow(clippy::too_many_arguments)] // a flat list of independent cmdline inputs reads clearer than a one-shot params struct
pub fn build_kernel_cmdline(
    exec: &ExecSpec,
    console: &str,
    pci: bool,
    content_tag: &str,
    descriptor_sha256: Option<&str>,
    debug: bool,
    volumes: &[VolumeAttachment],
    binds: &[BindAttachment],
) -> String {
    let mut parts = vec![format!("console={console}"), "rw".to_string()];
    if !debug {
        parts.push("quiet".to_string());
        parts.push("loglevel=3".to_string());
    }
    if !pci {
        parts.push("pci=off".to_string());
    }
    parts.push("upper.dev=/dev/vda".to_string());
    parts.push("composefs.descriptor.dev=/dev/vdb".to_string());
    parts.push(format!("content.tag={content_tag}"));
    if let Some(hex) = descriptor_sha256 {
        let bare = hex.strip_prefix("sha256:").unwrap_or(hex);
        parts.push(format!("composefs.descriptor.sha256={bare}"));
    }
    let disks = volume_disks(volumes);
    for (i, v) in volumes.iter().enumerate() {
        let dev = guest_block_dev(2 + volume_disk_index(&disks, &v.host_image));
        parts.push(format!("volume.{i}.dev={dev}"));
        parts.push(format!("volume.{i}.target={}", v.target));
        parts.push(format!("volume.{i}.ro={}", u8::from(v.read_only)));
    }
    for (i, b) in binds.iter().enumerate() {
        parts.push(format!("bind.{i}.tag={}", bind_share_tag(i)));
        parts.push(format!("bind.{i}.target={}", b.target));
        parts.push(format!("bind.{i}.ro={}", u8::from(b.read_only)));
        parts.push(format!("bind.{i}.drops={}", b.dropped_paths.len()));
        for (j, dropped) in b.dropped_paths.iter().enumerate() {
            parts.push(format!("bind.{i}.drop.{j}={dropped}"));
        }
        parts.push(format!("bind.{i}.seeds={}", b.seeded_paths.len()));
        for (j, seeded) in b.seeded_paths.iter().enumerate() {
            parts.push(format!("bind.{i}.seed.{j}={seeded}"));
        }
        parts.push(format!("bind.{i}.excludes={}", b.excluded_paths.len()));
        for (j, excluded) in b.excluded_paths.iter().enumerate() {
            parts.push(format!("bind.{i}.exclude.{j}={excluded}"));
        }
    }
    for (k, v) in &exec.kernel_env {
        debug_assert!(
            !v.contains(char::is_whitespace),
            "kernel cmdline value for {k} contains whitespace ({v:?}); \
             this will be tokenised by the kernel and lose data. \
             Encode the value upstream (e.g. base64) before emitting it"
        );
        parts.push(format!("{k}={v}"));
    }
    parts.join(" ")
}

pub trait VmmBackend: Send {
    fn run(&self, spec: VmSpec) -> Result<()>;
    fn name(&self) -> &'static str;
}

pub fn detect_backend() -> Box<dyn VmmBackend> {
    #[cfg(target_os = "linux")]
    {
        Box::new(cloud_hypervisor::CloudHypervisor)
    }
    #[cfg(target_os = "macos")]
    {
        Box::new(vz::VzBackend)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        compile_error!("lns only supports linux (Cloud Hypervisor) and macos (Vz)")
    }
}

pub async fn boot(spec: VmSpec, backend: Option<Box<dyn VmmBackend>>) -> Result<()> {
    let backend = backend.unwrap_or_else(detect_backend);
    log::debug!("starting microVM via {} backend", backend.name());
    let handle = tokio::task::spawn_blocking(move || backend.run(spec));
    handle.await??;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kernel_environment_tells_the_broker_if_egress_is_allowed() {
        assert_eq!(
            ExecSpec::egress_marker(true),
            (lns_session::EGRESS_ALLOWED_ENV.into(), "1".into())
        );
        assert_eq!(
            ExecSpec::egress_marker(false),
            (lns_session::EGRESS_ALLOWED_ENV.into(), "0".into())
        );
    }

    #[test]
    fn base64_encode_output_has_no_whitespace() {
        let v = crate::base64::encode(b"echo hello world\tfoo\nbar");
        assert!(
            !v.chars().any(char::is_whitespace),
            "base64 output contained whitespace: {v:?}"
        );
    }

    #[test]
    fn from_image_config_emits_agent_command_b64_not_agent_command() {
        let spec = ExecSpec::from_image_config(None, None, &["echo".into(), "hello".into()]);
        let keys: Vec<&str> = spec.kernel_env.iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys.contains(&"AGENT_COMMAND_B64"));
        assert!(!keys.contains(&"AGENT_COMMAND"));
    }

    #[test]
    fn a_session_less_boot_declares_that_it_needs_egress() {
        let spec = ExecSpec::for_run(
            &resolve_run_as(Some("0"), Some(0), None, None),
            None,
            &["/bin/sh".into()],
            None,
            None,
        );
        let env: std::collections::HashMap<_, _> = spec.kernel_env.iter().cloned().collect();
        assert_eq!(
            env.get(lns_session::EGRESS_ALLOWED_ENV).map(String::as_str),
            Some("1"),
            "the tool provisioner fetches tools over the network, so a missing lease is fatal for it too"
        );
    }

    #[test]
    fn from_image_config_b64_value_round_trips_to_echo_hello() {
        let spec = ExecSpec::from_image_config(None, None, &["echo".into(), "hello".into()]);
        let (_, b64) = spec
            .kernel_env
            .iter()
            .find(|(k, _)| k == "AGENT_COMMAND_B64")
            .expect("AGENT_COMMAND_B64 in kernel_env");
        let decoded = base64_decode_for_test(b64);
        assert_eq!(decoded.as_deref(), Some("echo hello"));
    }

    #[test]
    fn from_image_config_b64_value_carries_quoted_args() {
        let spec = ExecSpec::from_image_config(
            None,
            None,
            &["sh".into(), "-c".into(), "echo \"hi there\"".into()],
        );
        let (_, b64) = spec
            .kernel_env
            .iter()
            .find(|(k, _)| k == "AGENT_COMMAND_B64")
            .unwrap();
        let decoded = base64_decode_for_test(b64).expect("decode");
        assert_eq!(decoded, "sh -c 'echo \"hi there\"'");
    }

    #[test]
    fn build_kernel_cmdline_quiet_tokens_gated_on_debug() {
        let exec = ExecSpec::from_image_config(None, None, &["true".into()]);
        let quiet = build_kernel_cmdline(
            &exec,
            "hvc0",
            true,
            "lns-content",
            None,
            /*debug*/ false,
            &[],
            &[],
        );
        let toks: Vec<&str> = quiet.split_whitespace().collect();
        assert!(toks.contains(&"quiet"), "expected `quiet` at debug=false");
        assert!(
            toks.contains(&"loglevel=3"),
            "expected `loglevel=3` at debug=false"
        );

        let loud = build_kernel_cmdline(
            &exec,
            "hvc0",
            true,
            "lns-content",
            None,
            /*debug*/ true,
            &[],
            &[],
        );
        let toks: Vec<&str> = loud.split_whitespace().collect();
        assert!(
            !toks.contains(&"quiet"),
            "debug=true must omit `quiet`: {toks:?}"
        );
        assert!(
            !toks.contains(&"loglevel=3"),
            "debug=true must omit `loglevel=3`: {toks:?}"
        );
    }

    #[test]
    fn build_kernel_cmdline_pci_off_gated_on_pci_flag() {
        let exec = ExecSpec::from_image_config(None, None, &["true".into()]);
        let with_pci = build_kernel_cmdline(
            &exec,
            "hvc0",
            /*pci*/ true,
            "lns-content",
            None,
            false,
            &[],
            &[],
        );
        let toks: Vec<&str> = with_pci.split_whitespace().collect();
        assert!(
            !toks.contains(&"pci=off"),
            "pci=true must omit `pci=off`: {toks:?}"
        );

        let no_pci = build_kernel_cmdline(
            &exec,
            "hvc0",
            /*pci*/ false,
            "lns-content",
            None,
            false,
            &[],
            &[],
        );
        let toks: Vec<&str> = no_pci.split_whitespace().collect();
        assert!(
            toks.contains(&"pci=off"),
            "pci=false must include `pci=off`: {toks:?}"
        );
    }

    #[test]
    fn build_kernel_cmdline_emits_agent_command_b64_intact() {
        let exec = ExecSpec::from_image_config(None, None, &["echo".into(), "hello".into()]);
        let cmdline = build_kernel_cmdline(
            &exec,
            "hvc0",
            true,
            "lns-content",
            Some("deadbeef"),
            false,
            &[],
            &[],
        );
        let toks: Vec<&str> = cmdline.split_whitespace().collect();
        let b64_tok = toks
            .iter()
            .find(|t| t.starts_with("AGENT_COMMAND_B64="))
            .expect("AGENT_COMMAND_B64 token present");
        let value = b64_tok.strip_prefix("AGENT_COMMAND_B64=").unwrap();
        assert_eq!(
            base64_decode_for_test(value).as_deref(),
            Some("echo hello"),
            "cmdline token's value must decode to the original agent command"
        );
        assert!(
            !toks.contains(&"hello"),
            "orphan `hello` token present in cmdline: {toks:?}"
        );
    }

    #[test]
    fn guest_block_dev_maps_index_to_letter() {
        assert_eq!(guest_block_dev(0), "/dev/vda");
        assert_eq!(guest_block_dev(1), "/dev/vdb");
        assert_eq!(guest_block_dev(2), "/dev/vdc");
    }

    #[test]
    fn guest_block_dev_uses_linux_base26_naming_past_the_first_26_disks() {
        assert_eq!(guest_block_dev(25), "/dev/vdz");
        assert_eq!(guest_block_dev(26), "/dev/vdaa");
        assert_eq!(guest_block_dev(27), "/dev/vdab");
        assert_eq!(guest_block_dev(51), "/dev/vdaz");
        assert_eq!(guest_block_dev(52), "/dev/vdba");
        assert_eq!(guest_block_dev(701), "/dev/vdzz");
        assert_eq!(guest_block_dev(702), "/dev/vdaaa");
    }

    #[test]
    fn build_kernel_cmdline_no_volumes_emits_no_volume_keys() {
        let exec = ExecSpec::from_image_config(None, None, &["true".into()]);
        let cmdline =
            build_kernel_cmdline(&exec, "hvc0", true, "lns-content", None, false, &[], &[]);
        assert!(
            !cmdline.contains("volume."),
            "no volume keys when none attached: {cmdline}"
        );
        assert!(
            !cmdline.contains("bind."),
            "no bind keys when none attached: {cmdline}"
        );
    }

    #[test]
    fn bind_share_tag_is_distinct_per_index() {
        assert_eq!(bind_share_tag(0), "lns-bind-0");
        assert_eq!(bind_share_tag(1), "lns-bind-1");
        assert_ne!(bind_share_tag(0), bind_share_tag(1));
    }

    #[test]
    fn build_kernel_cmdline_emits_tag_target_ro_and_drops_per_bind() {
        let exec = ExecSpec::from_image_config(None, None, &["true".into()]);
        let binds = [
            BindAttachment {
                host_source: "/Users/me/proj".into(),
                target: "/work".into(),
                read_only: false,
                dropped_paths: vec![".env".into(), ".npmrc".into()],
                excluded_paths: Vec::new(),
                seeded_paths: Vec::new(),
            },
            BindAttachment {
                host_source: "/Users/me/cfg".into(),
                target: "/cfg".into(),
                read_only: true,
                dropped_paths: vec![],
                excluded_paths: Vec::new(),
                seeded_paths: Vec::new(),
            },
        ];
        let cmdline =
            build_kernel_cmdline(&exec, "hvc0", true, "lns-content", None, false, &[], &binds);
        let toks: Vec<&str> = cmdline.split_whitespace().collect();
        for expected in [
            "bind.0.tag=lns-bind-0",
            "bind.0.target=/work",
            "bind.0.ro=0",
            "bind.0.drops=2",
            "bind.0.drop.0=.env",
            "bind.0.drop.1=.npmrc",
            "bind.1.tag=lns-bind-1",
            "bind.1.target=/cfg",
            "bind.1.ro=1",
            "bind.1.drops=0",
        ] {
            assert!(toks.contains(&expected), "missing {expected}: {toks:?}");
        }
        assert!(
            toks.contains(&"content.tag=lns-content"),
            "content share tag must be left untouched: {toks:?}"
        );
    }

    #[test]
    fn build_kernel_cmdline_emits_volume_dev_target_and_ro_per_volume() {
        let exec = ExecSpec::from_image_config(None, None, &["true".into()]);
        let volumes = [
            VolumeAttachment {
                host_image: "/store/a.img".into(),
                target: "/data".into(),
                read_only: false,
            },
            VolumeAttachment {
                host_image: "/store/b.img".into(),
                target: "/srv/state".into(),
                read_only: true,
            },
        ];
        let cmdline = build_kernel_cmdline(
            &exec,
            "hvc0",
            true,
            "lns-content",
            None,
            false,
            &volumes,
            &[],
        );
        let toks: Vec<&str> = cmdline.split_whitespace().collect();
        for expected in [
            "volume.0.dev=/dev/vdc",
            "volume.0.target=/data",
            "volume.0.ro=0",
            "volume.1.dev=/dev/vdd",
            "volume.1.target=/srv/state",
            "volume.1.ro=1",
        ] {
            assert!(toks.contains(&expected), "missing {expected}: {toks:?}");
        }
    }

    #[test]
    fn volume_disks_dedupes_to_one_writable_device_per_image_regardless_of_mount_mode() {
        let volumes = [
            VolumeAttachment {
                host_image: "/store/a.img".into(),
                target: "/data".into(),
                read_only: false,
            },
            VolumeAttachment {
                host_image: "/store/a.img".into(),
                target: "/srv/state".into(),
                read_only: true,
            },
            VolumeAttachment {
                host_image: "/store/b.img".into(),
                target: "/ro".into(),
                read_only: true,
            },
        ];
        let disks = volume_disks(&volumes);
        assert_eq!(
            disks,
            vec![
                VolumeDisk {
                    host_image: "/store/a.img".into(),
                },
                VolumeDisk {
                    host_image: "/store/b.img".into(),
                },
            ]
        );
    }

    #[test]
    fn build_kernel_cmdline_points_repeated_volume_at_one_shared_device() {
        let exec = ExecSpec::from_image_config(None, None, &["true".into()]);
        let volumes = [
            VolumeAttachment {
                host_image: "/store/a.img".into(),
                target: "/data".into(),
                read_only: false,
            },
            VolumeAttachment {
                host_image: "/store/a.img".into(),
                target: "/srv/state".into(),
                read_only: true,
            },
        ];
        let cmdline = build_kernel_cmdline(
            &exec,
            "hvc0",
            true,
            "lns-content",
            None,
            false,
            &volumes,
            &[],
        );
        let toks: Vec<&str> = cmdline.split_whitespace().collect();
        for expected in [
            "volume.0.dev=/dev/vdc",
            "volume.0.target=/data",
            "volume.0.ro=0",
            "volume.1.dev=/dev/vdc",
            "volume.1.target=/srv/state",
            "volume.1.ro=1",
        ] {
            assert!(toks.contains(&expected), "missing {expected}: {toks:?}");
        }
    }

    #[test]
    fn server_mode_emits_agent_command_b64_not_raw_agent_command() {
        let exec = ExecSpec::server(
            None,
            &run_as("sandbox", Some(65534)),
            "vsock://host:1024/v1/sandbox",
            "token",
            None,
            &["echo".into(), "hello".into()],
            true,
        );
        let keys: Vec<&str> = exec.kernel_env.iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys.contains(&"AGENT_COMMAND_B64"));
        assert!(!keys.contains(&"AGENT_COMMAND"));

        let (_, b64) = exec
            .kernel_env
            .iter()
            .find(|(k, _)| k == "AGENT_COMMAND_B64")
            .expect("AGENT_COMMAND_B64 emitted");
        assert_eq!(
            base64_decode_for_test(b64).as_deref(),
            Some("echo hello"),
            "supervised path must encode the user's command verbatim"
        );
    }

    fn run_as(user: &str, uid: Option<u32>) -> RunAs {
        RunAs {
            user: user.to_string(),
            uid,
            group: None,
        }
    }

    #[test]
    fn host_knows_the_workload_gid_only_for_the_default_sandbox() {
        assert_eq!(
            host_known_workload_gid(&run_as("sandbox", Some(65534))),
            Some(65534),
            "the default sandbox is nobody:nogroup, which the host pins"
        );
        assert_eq!(
            host_known_workload_gid(&run_as("sandbox", Some(1000))),
            None,
            "a non-default uid's gid is resolved in-guest"
        );
        assert_eq!(
            host_known_workload_gid(&RunAs {
                user: "app".into(),
                uid: Some(65534),
                group: Some("staff".into()),
            }),
            None,
            "an explicit group is resolved in-guest"
        );
    }

    fn fake_session(url: &str, token: &str) -> crate::supervisor::SupervisorSession {
        let (fd_tx, _fd_rx) = tokio::sync::mpsc::unbounded_channel();
        crate::supervisor::SupervisorSession {
            assets: crate::supervisor::SupervisorAssets {
                supervisor_bin: std::path::PathBuf::from("/tmp/fake-supervisor"),
                guest_tools_root: std::path::PathBuf::from("/tmp/fake-tools"),
            },
            relay: crate::relay::Relay {
                url: url.to_string(),
                token: token.to_string(),
                audit_path: std::path::PathBuf::from("/tmp/fake-audit.jsonl"),
                fd_tx,
            },
            watcher: None,
            egress_allowed: true,
        }
    }

    #[test]
    fn for_run_supervised_sets_ws_url_token_and_agent_command_b64() {
        use std::collections::HashMap;
        let session = fake_session("vsock://host:1024/v1/sandbox", "test-token-123");
        let cmd = vec!["echo".to_string(), "hi".to_string()];
        let exec = ExecSpec::for_run(
            &run_as("sandbox", Some(65534)),
            None,
            &cmd,
            None,
            Some(&session),
        );
        let env: HashMap<_, _> = exec.kernel_env.iter().cloned().collect();
        assert_eq!(
            env.get("LENS_SANDBOX_WS_URL").map(String::as_str),
            Some("vsock://host:1024/v1/sandbox")
        );
        assert_eq!(
            env.get("LENS_SANDBOX_TOKEN").map(String::as_str),
            Some("test-token-123")
        );
        assert_eq!(
            env.get("LENS_SANDBOX_USER").map(String::as_str),
            Some("sandbox"),
            "supervisor's LENS_SANDBOX_USER must mirror lns-init's SANDBOX_USER"
        );
        assert!(
            env.contains_key("AGENT_COMMAND_B64"),
            "supervised path must put the agent command on the cmdline"
        );
        assert!(
            !env.contains_key("AGENT_COMMAND"),
            "raw AGENT_COMMAND would be shredded by the kernel's whitespace tokenizer"
        );
        assert_eq!(
            base64_decode_for_test(env.get("AGENT_COMMAND_B64").unwrap()).as_deref(),
            Some("echo hi"),
        );
    }

    #[test]
    fn for_run_supervised_does_not_force_rust_log() {
        use std::collections::HashMap;
        let session = fake_session("vsock://host:1024/v1/sandbox", "test-token-123");
        let cmd = vec!["sh".to_string()];
        let exec = ExecSpec::for_run(
            &run_as("sandbox", Some(65534)),
            None,
            &cmd,
            None,
            Some(&session),
        );
        let env: HashMap<_, _> = exec.kernel_env.iter().cloned().collect();
        assert!(
            !env.contains_key("RUST_LOG"),
            "supervised kernel_env must not pin RUST_LOG — the supervisor's \
             own is_terminal default keeps tty interactive shells quiet and \
             enables tracing on pipe-stdin (detached) sessions; got: {env:?}"
        );
    }

    #[test]
    fn for_run_unsupervised_sets_agent_command_b64() {
        use std::collections::HashMap;
        let cmd = vec!["echo".to_string(), "hi".to_string()];
        let exec = ExecSpec::for_run(&run_as("sandbox", Some(65534)), None, &cmd, None, None);
        let env: HashMap<_, _> = exec.kernel_env.iter().cloned().collect();
        assert!(
            env.contains_key("AGENT_COMMAND_B64"),
            "unsupervised path must emit AGENT_COMMAND_B64"
        );
        assert!(
            !env.contains_key("LENS_SANDBOX_WS_URL"),
            "unsupervised path must not set server-mode env"
        );
    }

    fn base64_decode_for_test(s: &str) -> Option<String> {
        let mut out = Vec::with_capacity(s.len() * 3 / 4);
        let bytes = s.as_bytes();
        let mut buf = [0u8; 4];
        let mut len = 0;
        for &c in bytes {
            if c.is_ascii_whitespace() {
                continue;
            }
            let v = match c {
                b'A'..=b'Z' => c - b'A',
                b'a'..=b'z' => c - b'a' + 26,
                b'0'..=b'9' => c - b'0' + 52,
                b'+' => 62,
                b'/' => 63,
                b'=' => 0x80,
                _ => return None,
            };
            buf[len] = v;
            len += 1;
            if len == 4 {
                let pads = buf.iter().filter(|&&b| b == 0x80).count();
                let b0 = buf[0] as u32;
                let b1 = buf[1] as u32;
                let b2 = if buf[2] == 0x80 { 0 } else { buf[2] as u32 };
                let b3 = if buf[3] == 0x80 { 0 } else { buf[3] as u32 };
                let v = (b0 << 18) | (b1 << 12) | (b2 << 6) | b3;
                out.push((v >> 16) as u8);
                if pads < 2 {
                    out.push((v >> 8) as u8);
                }
                if pads < 1 {
                    out.push(v as u8);
                }
                len = 0;
            }
        }
        if len != 0 {
            return None;
        }
        String::from_utf8(out).ok()
    }

    fn config_with_entrypoint() -> oci_client::config::ConfigFile {
        oci_client::config::ConfigFile {
            architecture: "arm64".into(),
            os: "linux".into(),
            config: Some(oci_client::config::Config {
                entrypoint: Some(vec!["/entry".into()]),
                cmd: Some(vec!["arg".into()]),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn from_image_config_with_some_uses_image_entrypoint() {
        let cfg = config_with_entrypoint();
        let spec = ExecSpec::from_image_config(Some(&cfg), None, &[]);
        let b64 = spec
            .kernel_env
            .iter()
            .find(|(k, _)| k == "AGENT_COMMAND_B64")
            .map(|(_, v)| v.clone())
            .unwrap();
        let decoded = base64_decode_for_test(&b64).unwrap();
        assert_eq!(decoded, "/entry arg");
    }

    #[test]
    fn server_with_some_image_config_uses_image_entrypoint() {
        let cfg = config_with_entrypoint();
        let spec = ExecSpec::server(
            Some(&cfg),
            &run_as("sandbox", Some(65534)),
            "vsock://host:1024/v1/sandbox",
            "token-xyz",
            None,
            &[],
            true,
        );
        let b64 = spec
            .kernel_env
            .iter()
            .find(|(k, _)| k == "AGENT_COMMAND_B64")
            .map(|(_, v)| v.clone())
            .unwrap();
        let decoded = base64_decode_for_test(&b64).unwrap();
        assert_eq!(decoded, "/entry arg");
    }

    #[test]
    fn detect_backend_returns_host_default_backend() {
        let backend = detect_backend();
        assert!(!backend.name().is_empty());
    }

    struct FakeBackend {
        outcome: std::sync::Mutex<Option<Result<()>>>,
    }
    impl VmmBackend for FakeBackend {
        fn run(&self, _spec: VmSpec) -> Result<()> {
            self.outcome
                .lock()
                .unwrap()
                .take()
                .unwrap_or_else(|| Ok(()))
        }
        fn name(&self) -> &'static str {
            "fake"
        }
    }

    fn dummy_vmspec() -> VmSpec {
        VmSpec {
            run_id: "aa01".to_string(),
            cpus: 1,
            memory_mib: 256,
            kernel: PathBuf::from("/dev/null"),
            initrd: PathBuf::from("/dev/null"),
            composefs_descriptor: PathBuf::from("/dev/null"),
            content_share: PathBuf::from("/dev/null"),
            content_tag: "test".into(),
            descriptor_sha256: None,
            upper_disk: PathBuf::from("/dev/null"),
            volumes: vec![],
            binds: vec![],
            workload_uid: None,
            workload_gid: None,
            vsock: None,
            connector_tx: None,
            #[cfg(target_os = "macos")]
            console_fd: -1,
            debug: false,
            exec: ExecSpec::from_image_config(None, None, &["true".into()]),
        }
    }

    #[tokio::test]
    async fn boot_with_propagates_backend_success() {
        let backend = Box::new(FakeBackend {
            outcome: std::sync::Mutex::new(Some(Ok(()))),
        }) as Box<dyn VmmBackend>;
        boot(dummy_vmspec(), Some(backend)).await.unwrap();
    }

    #[tokio::test]
    async fn boot_with_propagates_backend_error_through_join() {
        let backend = Box::new(FakeBackend {
            outcome: std::sync::Mutex::new(Some(Err(anyhow::anyhow!("simulated VMM crash")))),
        }) as Box<dyn VmmBackend>;
        let err = boot(dummy_vmspec(), Some(backend)).await.unwrap_err();
        assert!(format!("{err}").contains("simulated VMM crash"));
    }

    #[test]
    fn base64_decode_helper_skips_ascii_whitespace() {
        assert_eq!(
            base64_decode_for_test("Zm9v\nYg==").as_deref(),
            Some("foob")
        );
    }

    #[test]
    fn base64_decode_helper_rejects_invalid_alphabet_chars() {
        assert!(base64_decode_for_test("Zm!9v").is_none());
    }

    #[test]
    fn base64_decode_helper_rejects_trailing_partial_block() {
        assert!(base64_decode_for_test("Zm9").is_none());
    }

    #[test]
    fn base64_decode_helper_round_trips_plus_and_slash_alphabet() {
        let s = crate::base64::encode(b"???>>>");
        assert!(s.contains('/'));
        assert!(s.contains('+'));
        assert_eq!(base64_decode_for_test(&s).as_deref(), Some("???>>>"));
    }

    #[test]
    fn run_as_image_without_a_user_directive_drops_to_the_sandbox_user() {
        for image_user in [None, Some(""), Some("  ")] {
            assert_eq!(
                resolve_run_as(None, None, None, image_user),
                RunAs {
                    user: "sandbox".to_string(),
                    uid: Some(65534),
                    group: None,
                },
                "a sandbox run must stay unprivileged unless the image or the user asks for a specific identity"
            );
        }
    }

    #[test]
    fn run_as_leaves_a_named_image_users_uid_for_the_guest_to_resolve_by_name() {
        assert_eq!(
            resolve_run_as(None, None, None, Some("www-data"),),
            RunAs {
                user: "www-data".to_string(),
                uid: None,
                group: None,
            }
        );
    }

    #[test]
    fn run_as_parses_a_numeric_image_user_into_its_uid() {
        assert_eq!(
            resolve_run_as(None, None, None, Some("1000"),),
            RunAs {
                user: "1000".to_string(),
                uid: Some(1000),
                group: None,
            }
        );
    }

    #[test]
    fn run_as_keeps_the_group_component_of_the_image_user() {
        assert_eq!(
            resolve_run_as(None, None, None, Some("www-data:www-data"),),
            RunAs {
                user: "www-data".to_string(),
                uid: None,
                group: Some("www-data".to_string()),
            },
            "a named user:group must carry its group on so the guest can resolve it"
        );
        assert_eq!(
            resolve_run_as(None, None, None, Some("app:root"),),
            RunAs {
                user: "app".to_string(),
                uid: None,
                group: Some("root".to_string()),
            },
            "an explicitly requested group must not be replaced by the user's primary group"
        );
        assert_eq!(
            resolve_run_as(None, None, None, Some("1001:0"),),
            RunAs {
                user: "1001".to_string(),
                uid: Some(1001),
                group: Some("0".to_string()),
            },
            "a numeric uid:gid must keep its gid, not fall back to gid == uid"
        );
    }

    #[test]
    fn run_as_treats_an_empty_group_component_as_no_group() {
        assert_eq!(
            resolve_run_as(None, None, None, Some("app:"),),
            RunAs {
                user: "app".to_string(),
                uid: None,
                group: None,
            }
        );
    }

    #[test]
    fn run_as_explicit_cli_override_wins_over_the_image_user_and_carries_no_group() {
        assert_eq!(
            resolve_run_as(Some("alice"), Some(1234), None, Some("www-data:www-data"),),
            RunAs {
                user: "alice".to_string(),
                uid: Some(1234),
                group: None,
            }
        );
        assert_eq!(
            resolve_run_as(None, Some(1234), None, Some("www-data"),),
            RunAs {
                user: "sandbox".to_string(),
                uid: Some(1234),
                group: None,
            }
        );
        assert_eq!(
            resolve_run_as(Some("alice"), None, None, None,),
            RunAs {
                user: "alice".to_string(),
                uid: Some(65534),
                group: None,
            }
        );
    }

    #[test]
    fn run_as_explicit_user_parses_numeric_uid_and_group() {
        assert_eq!(
            resolve_run_as(Some("1000:1001"), Some(1000), None, None,),
            RunAs {
                user: "1000".to_string(),
                uid: Some(1000),
                group: Some("1001".to_string()),
            }
        );
        assert_eq!(
            resolve_run_as(Some("node:staff"), None, None, None,),
            RunAs {
                user: "node".to_string(),
                uid: Some(65534),
                group: Some("staff".to_string()),
            }
        );
    }

    #[test]
    fn run_as_empty_user_segment_falls_back_to_the_sandbox_user() {
        assert_eq!(
            resolve_run_as(Some(":1000"), None, None, None,),
            RunAs {
                user: "sandbox".to_string(),
                uid: Some(65534),
                group: Some("1000".to_string()),
            }
        );
        assert_eq!(
            resolve_run_as(Some(""), Some(1000), None, None,),
            RunAs {
                user: "sandbox".to_string(),
                uid: Some(1000),
                group: None,
            }
        );
    }

    #[test]
    fn server_omits_sandbox_uid_for_a_named_user_so_the_guest_resolves_it_by_name() {
        use std::collections::HashMap;
        let named = ExecSpec::server(
            None,
            &run_as("www-data", None),
            "vsock://host:1024/v1/sandbox",
            "token",
            None,
            &["true".into()],
            true,
        );
        let env: HashMap<_, _> = named.kernel_env.iter().cloned().collect();
        assert_eq!(
            env.get("SANDBOX_USER").map(String::as_str),
            Some("www-data")
        );
        assert!(
            !env.contains_key("SANDBOX_UID"),
            "a named user must not pin a placeholder SANDBOX_UID: {env:?}"
        );

        let numeric = ExecSpec::server(
            None,
            &run_as("1000", Some(1000)),
            "vsock://host:1024/v1/sandbox",
            "token",
            None,
            &["true".into()],
            true,
        );
        let env: HashMap<_, _> = numeric.kernel_env.iter().cloned().collect();
        assert_eq!(env.get("SANDBOX_UID").map(String::as_str), Some("1000"));
    }

    #[test]
    fn server_emits_sandbox_group_only_when_the_image_user_requested_one() {
        use std::collections::HashMap;
        let with_group = ExecSpec::server(
            None,
            &RunAs {
                user: "app".to_string(),
                uid: None,
                group: Some("root".to_string()),
            },
            "vsock://host:1024/v1/sandbox",
            "token",
            None,
            &["true".into()],
            true,
        );
        let env: HashMap<_, _> = with_group.kernel_env.iter().cloned().collect();
        assert_eq!(
            env.get("SANDBOX_GROUP").map(String::as_str),
            Some("root"),
            "the guest needs the requested group to resolve its gid: {env:?}"
        );

        let without_group = ExecSpec::server(
            None,
            &run_as("sandbox", Some(65534)),
            "vsock://host:1024/v1/sandbox",
            "token",
            None,
            &["true".into()],
            true,
        );
        let env: HashMap<_, _> = without_group.kernel_env.iter().cloned().collect();
        assert!(
            !env.contains_key("SANDBOX_GROUP"),
            "no group means the guest falls back to the user's primary group: {env:?}"
        );
    }
}
