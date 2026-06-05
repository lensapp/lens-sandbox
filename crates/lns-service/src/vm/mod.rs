use crate::log;
use anyhow::Result;
use std::path::PathBuf;

mod cloud_hypervisor;
#[cfg(target_os = "macos")]
pub mod diag_console;
#[cfg(target_os = "macos")]
pub mod session_client;
#[cfg(target_os = "macos")]
mod vz;

#[cfg(target_os = "macos")]
pub use vz::{VmStopGuard, VsockConnector};

#[cfg_attr(target_os = "macos", allow(dead_code))]
pub struct VmSpec {
    pub run_id: u32,
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
    #[cfg(target_os = "macos")]
    pub vsock: Option<VsockChannel>,
    #[cfg(target_os = "macos")]
    pub connector_tx: Option<tokio::sync::oneshot::Sender<VsockConnector>>,
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

#[cfg(target_os = "macos")]
pub struct VsockChannel {
    pub port: u32,
    pub fd_tx: tokio::sync::mpsc::UnboundedSender<std::os::fd::RawFd>,
}

#[cfg_attr(target_os = "macos", allow(dead_code))]
pub struct ExecSpec {
    pub kernel_env: Vec<(String, String)>,
    #[allow(dead_code)]
    pub workdir: Option<String>,
}

impl ExecSpec {
    pub fn server(
        image_config: Option<&oci_client::config::ConfigFile>,
        sandbox_user: &str,
        sandbox_uid: Option<u32>,
        ws_url: &str,
        token: &str,
        cmd: &[String],
    ) -> Self {
        let workdir = image_config_workdir(image_config);
        let agent_command = match image_config {
            Some(cfg) => crate::workload_argv::from_image_config(cfg, cmd),
            None => crate::workload_argv::shell_quote_argv(cmd),
        };

        let mut kernel_env = vec![
            ("LENS_SANDBOX_WS_URL".into(), ws_url.to_string()),
            ("LENS_SANDBOX_TOKEN".into(), token.to_string()),
            ("SANDBOX_USER".into(), sandbox_user.to_string()),
        ];
        if let Some(uid) = sandbox_uid {
            kernel_env.push(("SANDBOX_UID".into(), uid.to_string()));
        }
        kernel_env.push(("LENS_SANDBOX_USER".into(), sandbox_user.to_string()));
        kernel_env.push((
            "AGENT_COMMAND_B64".into(),
            crate::base64::encode(agent_command.as_bytes()),
        ));
        kernel_env.push((
            "PATH".into(),
            "/.lens/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".into(),
        ));

        ExecSpec {
            kernel_env,
            workdir,
        }
    }

    pub fn from_image_config(
        image_config: Option<&oci_client::config::ConfigFile>,
        override_cmd: &[String],
    ) -> Self {
        let argv = match image_config {
            Some(cfg) => crate::workload_argv::from_image_config(cfg, override_cmd),
            None => crate::workload_argv::shell_quote_argv(override_cmd),
        };
        let workdir = image_config_workdir(image_config);

        ExecSpec {
            kernel_env: vec![
                (
                    "AGENT_COMMAND_B64".into(),
                    crate::base64::encode(argv.as_bytes()),
                ),
                (
                    "PATH".into(),
                    "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".into(),
                ),
            ],
            workdir,
        }
    }

    pub fn for_run(
        sandbox_user: &str,
        sandbox_uid: Option<u32>,
        cmd: &[String],
        image_config: Option<&oci_client::config::ConfigFile>,
        session: Option<&crate::supervisor::SupervisorSession>,
    ) -> Self {
        match session {
            Some(s) => Self::server(
                image_config,
                sandbox_user,
                sandbox_uid,
                &s.relay.url,
                &s.relay.token,
                cmd,
            ),
            None => Self::from_image_config(image_config, cmd),
        }
    }
}

fn image_config_workdir(image_config: Option<&oci_client::config::ConfigFile>) -> Option<String> {
    image_config
        .and_then(|c| c.config.as_ref())
        .and_then(|c| c.working_dir.clone())
        .filter(|s| !s.is_empty())
}

const DEFAULT_SANDBOX_USER: &str = "sandbox";
const DEFAULT_SANDBOX_UID: u32 = 65534;

#[cfg_attr(not(any(target_os = "linux", target_os = "macos")), allow(dead_code))]
pub fn resolve_run_as(
    explicit_user: Option<&str>,
    explicit_uid: Option<u32>,
    image_user: Option<&str>,
    imageless: bool,
) -> (String, Option<u32>) {
    if explicit_user.is_some() || explicit_uid.is_some() {
        return (
            explicit_user.unwrap_or(DEFAULT_SANDBOX_USER).to_string(),
            Some(explicit_uid.unwrap_or(DEFAULT_SANDBOX_UID)),
        );
    }
    if imageless {
        return (DEFAULT_SANDBOX_USER.to_string(), Some(DEFAULT_SANDBOX_UID));
    }
    match image_user.map(str::trim).filter(|u| !u.is_empty()) {
        None => (String::new(), Some(0)),
        Some(spec) => {
            let name = spec.split(':').next().unwrap_or(spec);
            (name.to_string(), name.parse::<u32>().ok())
        }
    }
}

#[cfg_attr(not(any(target_os = "linux", target_os = "macos")), allow(dead_code))]
pub fn build_kernel_cmdline(
    exec: &ExecSpec,
    console: &str,
    pci: bool,
    content_tag: &str,
    descriptor_sha256: Option<&str>,
    debug: bool,
    volumes: &[VolumeAttachment],
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
    fn base64_encode_output_has_no_whitespace() {
        let v = crate::base64::encode(b"echo hello world\tfoo\nbar");
        assert!(
            !v.chars().any(char::is_whitespace),
            "base64 output contained whitespace: {v:?}"
        );
    }

    #[test]
    fn from_image_config_emits_agent_command_b64_not_agent_command() {
        let spec = ExecSpec::from_image_config(None, &["echo".into(), "hello".into()]);
        let keys: Vec<&str> = spec.kernel_env.iter().map(|(k, _)| k.as_str()).collect();
        assert!(keys.contains(&"AGENT_COMMAND_B64"));
        assert!(!keys.contains(&"AGENT_COMMAND"));
    }

    #[test]
    fn from_image_config_b64_value_round_trips_to_echo_hello() {
        let spec = ExecSpec::from_image_config(None, &["echo".into(), "hello".into()]);
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
        let exec = ExecSpec::from_image_config(None, &["true".into()]);
        let quiet = build_kernel_cmdline(
            &exec,
            "hvc0",
            true,
            "lns-content",
            None,
            /*debug*/ false,
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
        let exec = ExecSpec::from_image_config(None, &["true".into()]);
        let with_pci = build_kernel_cmdline(
            &exec,
            "hvc0",
            /*pci*/ true,
            "lns-content",
            None,
            false,
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
        );
        let toks: Vec<&str> = no_pci.split_whitespace().collect();
        assert!(
            toks.contains(&"pci=off"),
            "pci=false must include `pci=off`: {toks:?}"
        );
    }

    #[test]
    fn build_kernel_cmdline_emits_agent_command_b64_intact() {
        let exec = ExecSpec::from_image_config(None, &["echo".into(), "hello".into()]);
        let cmdline = build_kernel_cmdline(
            &exec,
            "hvc0",
            true,
            "lns-content",
            Some("deadbeef"),
            false,
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
        let exec = ExecSpec::from_image_config(None, &["true".into()]);
        let cmdline = build_kernel_cmdline(&exec, "hvc0", true, "lns-content", None, false, &[]);
        assert!(
            !cmdline.contains("volume."),
            "no volume keys when none attached: {cmdline}"
        );
    }

    #[test]
    fn build_kernel_cmdline_emits_volume_dev_target_and_ro_per_volume() {
        let exec = ExecSpec::from_image_config(None, &["true".into()]);
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
        let cmdline =
            build_kernel_cmdline(&exec, "hvc0", true, "lns-content", None, false, &volumes);
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
        let exec = ExecSpec::from_image_config(None, &["true".into()]);
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
        let cmdline =
            build_kernel_cmdline(&exec, "hvc0", true, "lns-content", None, false, &volumes);
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
            "sandbox",
            Some(65534),
            "vsock://host:1024/v1/sandbox",
            "token",
            &["echo".into(), "hello".into()],
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
            credential_watcher: None,
        }
    }

    #[test]
    fn for_run_supervised_sets_ws_url_token_and_agent_command_b64() {
        use std::collections::HashMap;
        let session = fake_session("vsock://host:1024/v1/sandbox", "test-token-123");
        let cmd = vec!["echo".to_string(), "hi".to_string()];
        let exec = ExecSpec::for_run("sandbox", Some(65534), &cmd, None, Some(&session));
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
        let exec = ExecSpec::for_run("sandbox", Some(65534), &cmd, None, Some(&session));
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
        let exec = ExecSpec::for_run("sandbox", Some(65534), &cmd, None, None);
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

    fn config_with_workdir(workdir: &str) -> oci_client::config::ConfigFile {
        oci_client::config::ConfigFile {
            architecture: "arm64".into(),
            os: "linux".into(),
            config: Some(oci_client::config::Config {
                working_dir: Some(workdir.to_string()),
                entrypoint: Some(vec!["/entry".into()]),
                cmd: Some(vec!["arg".into()]),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn from_image_config_with_some_uses_image_entrypoint_and_workdir() {
        let cfg = config_with_workdir("/work");
        let spec = ExecSpec::from_image_config(Some(&cfg), &[]);
        assert_eq!(spec.workdir.as_deref(), Some("/work"));
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
    fn server_with_some_image_config_uses_image_entrypoint_and_workdir() {
        let cfg = config_with_workdir("/srv");
        let spec = ExecSpec::server(
            Some(&cfg),
            "sandbox",
            Some(65534),
            "vsock://host:1024/v1/sandbox",
            "token-xyz",
            &[],
        );
        assert_eq!(spec.workdir.as_deref(), Some("/srv"));
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
            run_id: 1,
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
            #[cfg(target_os = "macos")]
            vsock: None,
            #[cfg(target_os = "macos")]
            connector_tx: None,
            #[cfg(target_os = "macos")]
            console_fd: -1,
            debug: false,
            exec: ExecSpec::from_image_config(None, &["true".into()]),
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
    fn run_as_image_without_a_user_directive_runs_as_root() {
        assert_eq!(
            resolve_run_as(None, None, None, false),
            (String::new(), Some(0))
        );
        assert_eq!(
            resolve_run_as(None, None, Some(""), false),
            (String::new(), Some(0))
        );
        assert_eq!(
            resolve_run_as(None, None, Some("  "), false),
            (String::new(), Some(0))
        );
    }

    #[test]
    fn run_as_leaves_a_named_image_users_uid_for_the_guest_to_resolve_by_name() {
        assert_eq!(
            resolve_run_as(None, None, Some("www-data"), false),
            ("www-data".to_string(), None)
        );
        assert_eq!(
            resolve_run_as(None, None, Some("www-data:www-data"), false),
            ("www-data".to_string(), None)
        );
    }

    #[test]
    fn run_as_parses_a_numeric_image_user_into_its_uid() {
        assert_eq!(
            resolve_run_as(None, None, Some("1000"), false),
            ("1000".to_string(), Some(1000))
        );
        assert_eq!(
            resolve_run_as(None, None, Some("1000:1000"), false),
            ("1000".to_string(), Some(1000))
        );
    }

    #[test]
    fn run_as_explicit_cli_override_wins_over_the_image_user() {
        assert_eq!(
            resolve_run_as(Some("alice"), Some(1234), Some("www-data"), false),
            ("alice".to_string(), Some(1234))
        );
        assert_eq!(
            resolve_run_as(None, Some(1234), Some("www-data"), false),
            ("sandbox".to_string(), Some(1234))
        );
        assert_eq!(
            resolve_run_as(Some("alice"), None, None, true),
            ("alice".to_string(), Some(65534))
        );
    }

    #[test]
    fn run_as_imageless_runs_unprivileged_as_the_sandbox_user() {
        assert_eq!(
            resolve_run_as(None, None, None, true),
            ("sandbox".to_string(), Some(65534))
        );
        assert_eq!(
            resolve_run_as(None, None, Some("ignored-when-imageless"), true),
            ("sandbox".to_string(), Some(65534))
        );
    }

    #[test]
    fn server_omits_sandbox_uid_for_a_named_user_so_the_guest_resolves_it_by_name() {
        use std::collections::HashMap;
        let named = ExecSpec::server(
            None,
            "www-data",
            None,
            "vsock://host:1024/v1/sandbox",
            "token",
            &["true".into()],
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
            "1000",
            Some(1000),
            "vsock://host:1024/v1/sandbox",
            "token",
            &["true".into()],
        );
        let env: HashMap<_, _> = numeric.kernel_env.iter().cloned().collect();
        assert_eq!(env.get("SANDBOX_UID").map(String::as_str), Some("1000"));
    }
}
