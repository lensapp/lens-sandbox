use clap::{Parser, ValueEnum};
use lns_ipc::{PortPublish, Protocol};
use std::net::{IpAddr, Ipv4Addr};
use std::path::PathBuf;

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "lower")]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
}

#[derive(Parser)]
#[command(
    name = "lns",
    version,
    about = "Lens Sandbox — run OCI images in microVMs via Cloud Hypervisor (Linux) and Apple Vz (macOS)"
)]
pub struct Cli {
    #[arg(
        long,
        value_enum,
        default_value_t = LogLevel::Warn,
        global = true,
        help = "Log threshold: `warn` (default) shows warnings/errors; `info` adds progress lines; `debug` adds traces and the guest boot transcript; override with `LNS_LOG` or `RUST_LOG`."
    )]
    pub log_level: LogLevel,
}

#[derive(clap::Args)]
pub struct RunArgs {
    #[arg(
        help = "OCI image reference (e.g. alpine:3.20); omit for imageless mode, which requires a command after `--`."
    )]
    pub image: Option<String>,

    #[arg(
        long,
        help = "Name the run, addressable in place of its id by every `lns sandbox` verb. Auto-generated (adjective_noun) when omitted; must not be all digits."
    )]
    pub name: Option<String>,

    #[arg(
        long,
        help = "Registry to qualify a bare image reference (e.g. ghcr.io); falls back to the `run.registry` config default. A fully-qualified reference is used as-is."
    )]
    pub registry: Option<String>,

    #[arg(
        long,
        value_parser = clap::value_parser!(u8).range(1..),
        help = "Number of vCPUs; falls back to the `run.cpus` config default, then 1."
    )]
    pub cpus: Option<u8>,

    #[arg(
        short = 'm',
        long,
        visible_alias = "memory",
        value_name = "SIZE",
        value_parser = parse_mem_arg,
        help = "RAM in MiB, or with a unit suffix (`-m 2g`, `-m 512m`; b/k/m/g, rounded up to MiB); falls back to the `run.mem` config default, then 512."
    )]
    pub mem: Option<usize>,

    #[arg(
        long,
        help = "Policy file path; defaults to `lns-policy.yaml` in the current directory, auto-created with `defaultVerdict: ask` if absent."
    )]
    pub policy: Option<PathBuf>,

    #[arg(
        long,
        help = "Run-as user inside the sandbox. Defaults to the image's USER (root when the image sets none); imageless runs default to `sandbox`."
    )]
    pub sandbox_user: Option<String>,

    #[arg(
        long,
        help = "Run-as uid inside the sandbox. Defaults to the image's USER uid; imageless runs default to 65534."
    )]
    pub sandbox_uid: Option<u32>,

    #[arg(
        short = 'i',
        long = "interactive",
        default_value_t = true,
        help = "Keep stdin open (forward host stdin to the guest workload)."
    )]
    pub interactive: bool,

    #[arg(
        short = 't',
        long = "tty",
        default_value_t = true,
        help = "Allocate a PTY in the broker; pipe mode is selected automatically when stdin is not a TTY."
    )]
    pub tty: bool,

    #[arg(
        short = 'd',
        long,
        default_value_t = false,
        conflicts_with_all = ["interactive", "tty"],
        help = "Return immediately after starting; the run continues in the daemon and is reachable via `lns exec`, `lns kill`, `lns ls`."
    )]
    pub detach: bool,

    #[arg(
        long,
        default_value = "ctrl-p,ctrl-q",
        value_parser = parse_detach_keys_arg,
        help = "Comma-separated detach chord (single chars or `ctrl-X`); on match the CLI sends SIGHUP to the workload's foreground pgrp and reports its exit code. To leave a run running, start it with `-d` and re-join with `lns sandbox attach`."
    )]
    pub detach_keys: DetachChord,

    #[arg(
        short = 'w',
        long,
        value_name = "DIR",
        value_parser = parse_workdir_arg,
        help = "Working directory inside the sandbox (absolute path; created if missing). Defaults to the image's WORKDIR."
    )]
    pub workdir: Option<String>,

    #[arg(
        short = 'e',
        long = "env",
        value_name = "KEY=VALUE",
        value_parser = parse_env_kv,
        help = "Set a non-secret environment variable in the workload (repeatable). Secrets belong in the credential flow, not -e."
    )]
    pub env: Vec<String>,

    #[arg(
        long = "env-file",
        value_name = "FILE",
        help = "Read KEY=VALUE lines from a file into the workload env (repeatable; later files and -e win; `#` comments and blank lines are skipped)."
    )]
    pub env_file: Vec<PathBuf>,

    #[arg(
        short = 'p',
        long = "publish",
        value_parser = parse_publish_arg,
        help = "Publish a guest port to the host: `[host_ip:]hostport:containerport[/proto]`. Host bind defaults to loopback (127.0.0.1) — pass an explicit host_ip to expose wider."
    )]
    pub publish: Vec<PortPublish>,

    #[arg(
        short = 'v',
        long = "volume",
        value_parser = lns_ipc::VolumeMount::parse,
        help = "Attach a named volume as `name:/path[:ro]`; its contents persist across runs (Docker -v style)."
    )]
    pub volumes: Vec<lns_ipc::VolumeMount>,

    #[arg(
        last = true,
        help = "Override entrypoint+cmd. Everything after `--` is the command."
    )]
    pub cmd: Vec<String>,
}

pub const DEFAULT_CPUS: u8 = 1;
pub const DEFAULT_MEM_MIB: usize = 512;

impl RunArgs {
    pub fn effective_cpus(&self) -> u8 {
        self.cpus.unwrap_or(DEFAULT_CPUS)
    }

    pub fn effective_mem(&self) -> usize {
        self.mem.unwrap_or(DEFAULT_MEM_MIB)
    }
}

#[derive(clap::Args)]
pub struct ExecArgs {
    #[arg(
        value_name = "RUN",
        help = "Target run id or name surfaced by `lns sandbox ls`."
    )]
    pub run: String,

    #[arg(
        short = 'i',
        long = "interactive",
        default_value_t = true,
        help = "`-i` — keep stdin open. Mirrors `docker exec -i`."
    )]
    pub interactive: bool,

    #[arg(
        short = 't',
        long = "tty",
        default_value_t = true,
        help = "Allocate a PTY for the exec session."
    )]
    pub tty: bool,

    #[arg(
        long,
        default_value = "ctrl-p,ctrl-q",
        value_parser = parse_detach_keys_arg,
        help = "Detach chord; on trigger closes only this exec session, leaving the primary session and VM running."
    )]
    pub detach_keys: DetachChord,

    #[arg(
        last = true,
        help = "Command to exec in the running workload. Everything after `--`."
    )]
    pub cmd: Vec<String>,
}

#[derive(clap::Args)]
pub struct KillArgs {
    #[arg(value_name = "RUN", help = "Target run id or name.")]
    pub run: String,

    #[arg(
        long,
        default_value = "TERM",
        help = "Signal name — bare or `SIG`-prefixed, case-insensitive; supported: TERM, INT, QUIT, HUP, WINCH, KILL."
    )]
    pub signal: String,
}

// Newtype: clap would otherwise treat Vec<u8> as a multi-value arg and downcast at runtime.
#[derive(Clone, Debug)]
pub struct DetachChord(pub Vec<u8>);

pub(crate) fn parse_detach_keys_arg(s: &str) -> Result<DetachChord, String> {
    crate::chord::parse_detach_keys(s)
        .map(DetachChord)
        .map_err(|e| e.to_string())
}

pub(crate) fn parse_env_kv(s: &str) -> Result<String, String> {
    let (key, _) = s
        .split_once('=')
        .ok_or_else(|| format!("expected KEY=VALUE form, got `{s}`"))?;
    if key.is_empty() {
        return Err(format!("empty variable name in `{s}`"));
    }
    Ok(s.to_string())
}

fn parse_workdir_arg(s: &str) -> Result<String, String> {
    if !s.starts_with('/') {
        return Err(format!(
            "workdir must be an absolute path inside the sandbox, got `{s}`"
        ));
    }
    Ok(s.to_string())
}

const MIB: u128 = 1024 * 1024;

pub(crate) fn parse_mem_arg(s: &str) -> Result<usize, String> {
    let lower = s.trim().to_ascii_lowercase();
    let digits_end = lower
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(lower.len());
    let (digits, suffix) = lower.split_at(digits_end);
    let value: u128 = digits
        .parse()
        .map_err(|_| format!("invalid memory size `{s}`: expected MiB, e.g. `512`, or `2g`"))?;
    let mib = match suffix {
        "" | "m" | "mb" | "mib" => value,
        "b" => value.div_ceil(MIB),
        "k" | "kb" | "kib" => value.div_ceil(1024),
        "g" | "gb" | "gib" => value
            .checked_mul(1024)
            .ok_or_else(|| format!("memory size `{s}` is out of range"))?,
        _ => {
            return Err(format!(
                "invalid memory size `{s}`: unknown unit `{suffix}` (use b, k, m, or g)"
            ));
        }
    };
    if mib == 0 {
        return Err(format!("invalid memory size `{s}`: must be at least 1 MiB"));
    }
    usize::try_from(mib).map_err(|_| format!("memory size `{s}` is out of range"))
}

pub(crate) fn parse_publish_arg(s: &str) -> Result<PortPublish, String> {
    let (addr_ports, protocol) = match s.rsplit_once('/') {
        Some((rest, proto)) => (rest, parse_protocol(proto, s)?),
        None => (s, Protocol::Tcp),
    };
    let (host_ip, host_port, container_port) = split_publish_addr(addr_ports).ok_or_else(|| {
        format!("invalid -p spec `{s}`: expected [host_ip:]hostport:containerport[/proto]")
    })?;
    Ok(PortPublish {
        host_ip,
        host_port: parse_port(host_port).ok_or_else(|| format!("invalid host port in `-p {s}`"))?,
        container_port: parse_port(container_port)
            .ok_or_else(|| format!("invalid container port in `-p {s}`"))?,
        protocol,
    })
}

fn parse_protocol(proto: &str, spec: &str) -> Result<Protocol, String> {
    match proto.to_ascii_lowercase().as_str() {
        "tcp" => Ok(Protocol::Tcp),
        "udp" => Err("udp publishing is not yet supported".to_string()),
        other => Err(format!("unknown protocol in `-p {spec}`: {other}")),
    }
}

fn split_publish_addr(s: &str) -> Option<(IpAddr, &str, &str)> {
    let (ip, host, container) = if let Some(rest) = s.strip_prefix('[') {
        let (ip, after) = rest.split_once(']')?;
        let (host, container) = after.strip_prefix(':')?.split_once(':')?;
        (ip.parse().ok()?, host, container)
    } else {
        match s.split(':').collect::<Vec<_>>().as_slice() {
            [host, container] => (IpAddr::V4(Ipv4Addr::LOCALHOST), *host, *container),
            [ip, host, container] => (ip.parse().ok()?, *host, *container),
            _ => return None,
        }
    };
    (!host.is_empty() && !container.is_empty()).then_some((ip, host, container))
}

fn parse_port(s: &str) -> Option<u16> {
    s.parse::<u16>().ok().filter(|&p| p != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_detach_keys_arg_returns_chord_bytes_on_valid_input() {
        let chord = parse_detach_keys_arg("ctrl-p,ctrl-q").unwrap();
        assert_eq!(chord.0, vec![0x10, 0x11]);
    }

    #[test]
    fn parse_detach_keys_arg_surfaces_parse_error_as_string() {
        let err = parse_detach_keys_arg("not-a-key").unwrap_err();
        assert!(!err.is_empty(), "expected non-empty error string");
    }

    #[test]
    fn parse_env_kv_accepts_a_plain_assignment() {
        assert_eq!(parse_env_kv("FOO=bar").unwrap(), "FOO=bar");
    }

    #[test]
    fn parse_env_kv_allows_an_empty_value() {
        assert_eq!(parse_env_kv("FOO=").unwrap(), "FOO=");
    }

    #[test]
    fn parse_env_kv_splits_on_the_first_equals_only() {
        assert_eq!(parse_env_kv("DSN=user=admin").unwrap(), "DSN=user=admin");
    }

    #[test]
    fn parse_env_kv_rejects_an_empty_key() {
        let err = parse_env_kv("=oops").unwrap_err();
        assert!(err.contains("empty variable name"), "got {err:?}");
    }

    #[test]
    fn parse_env_kv_rejects_a_bare_key_with_no_equals() {
        let err = parse_env_kv("HOME").unwrap_err();
        assert!(err.contains("KEY=VALUE"), "got {err:?}");
    }

    #[test]
    fn parse_workdir_arg_accepts_an_absolute_path() {
        assert_eq!(parse_workdir_arg("/app").unwrap(), "/app");
    }

    #[test]
    fn parse_workdir_arg_rejects_a_relative_path() {
        for spec in ["app", "./app", "../app", ""] {
            let err = parse_workdir_arg(spec).unwrap_err();
            assert!(err.contains("absolute path"), "spec {spec:?}: {err}");
        }
    }

    #[test]
    fn parse_mem_arg_bare_number_is_mib() {
        assert_eq!(parse_mem_arg("512").unwrap(), 512);
    }

    #[test]
    fn parse_mem_arg_m_suffixes_are_mib_passthrough() {
        for spec in ["512m", "512mb", "512mib", "512M", "512MiB"] {
            assert_eq!(parse_mem_arg(spec).unwrap(), 512, "spec: {spec}");
        }
    }

    #[test]
    fn parse_mem_arg_g_suffixes_scale_to_mib() {
        for spec in ["2g", "2gb", "2gib", "2G"] {
            assert_eq!(parse_mem_arg(spec).unwrap(), 2048, "spec: {spec}");
        }
    }

    #[test]
    fn parse_mem_arg_k_and_b_round_up_to_a_whole_mib() {
        assert_eq!(parse_mem_arg("1024k").unwrap(), 1);
        assert_eq!(parse_mem_arg("1500k").unwrap(), 2);
        assert_eq!(parse_mem_arg("1b").unwrap(), 1);
        assert_eq!(parse_mem_arg("1048577b").unwrap(), 2);
    }

    #[test]
    fn parse_mem_arg_rejects_zero() {
        for spec in ["0", "0g", "0b"] {
            let err = parse_mem_arg(spec).unwrap_err();
            assert!(err.contains("at least 1 MiB"), "spec {spec}: {err}");
        }
    }

    #[test]
    fn parse_mem_arg_rejects_an_unknown_unit() {
        let err = parse_mem_arg("12parsecs").unwrap_err();
        assert!(err.contains("unknown unit"), "got: {err}");
    }

    #[test]
    fn parse_mem_arg_rejects_a_unit_with_no_number() {
        let err = parse_mem_arg("g").unwrap_err();
        assert!(err.contains("expected MiB"), "got: {err}");
    }

    #[test]
    fn parse_mem_arg_rejects_a_size_beyond_usize() {
        let err = parse_mem_arg(&format!("{}g", u128::from(u64::MAX))).unwrap_err();
        assert!(err.contains("out of range"), "got: {err}");
    }

    #[test]
    fn parse_publish_arg_defaults_host_bind_to_loopback() {
        let p = parse_publish_arg("3003:3003").unwrap();
        assert_eq!(p.host_ip, IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(p.host_port, 3003);
        assert_eq!(p.container_port, 3003);
        assert_eq!(p.protocol, Protocol::Tcp);
    }

    #[test]
    fn parse_publish_arg_accepts_explicit_host_ip() {
        let p = parse_publish_arg("0.0.0.0:3003:3003").unwrap();
        assert_eq!(p.host_ip, IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    }

    #[test]
    fn parse_publish_arg_accepts_bracketed_ipv6_host_ip() {
        let p = parse_publish_arg("[::1]:8080:3003").unwrap();
        assert_eq!(p.host_ip, "::1".parse::<IpAddr>().unwrap());
        assert_eq!(p.host_port, 8080);
        assert_eq!(p.container_port, 3003);
    }

    #[test]
    fn parse_publish_arg_remaps_host_to_container_port() {
        let p = parse_publish_arg("8080:3003").unwrap();
        assert_eq!(p.host_port, 8080);
        assert_eq!(p.container_port, 3003);
    }

    #[test]
    fn parse_publish_arg_keeps_explicit_tcp_suffix() {
        assert_eq!(
            parse_publish_arg("3003:3003/tcp").unwrap().protocol,
            Protocol::Tcp
        );
    }

    #[test]
    fn parse_publish_arg_rejects_udp_as_not_yet_supported() {
        let err = parse_publish_arg("5353:5353/udp").unwrap_err();
        assert!(
            err.contains("udp publishing is not yet supported"),
            "got: {err}"
        );
    }

    #[test]
    fn parse_publish_arg_rejects_unknown_protocol() {
        let err = parse_publish_arg("3003:3003/sctp").unwrap_err();
        assert!(err.contains("unknown protocol"), "got: {err}");
    }

    #[test]
    fn parse_publish_arg_rejects_a_malformed_spec() {
        let err = parse_publish_arg("notaport").unwrap_err();
        assert!(err.contains("invalid -p spec"), "got: {err}");
    }

    #[test]
    fn parse_publish_arg_rejects_a_non_numeric_port() {
        assert!(
            parse_publish_arg("nope:3003")
                .unwrap_err()
                .contains("invalid host port")
        );
        assert!(
            parse_publish_arg("3003:nope")
                .unwrap_err()
                .contains("invalid container port")
        );
    }

    #[test]
    fn parse_publish_arg_rejects_port_zero() {
        assert!(parse_publish_arg("0:3003").is_err());
    }

    #[test]
    fn parse_publish_arg_rejects_a_bad_host_ip() {
        assert!(parse_publish_arg("999.999.999.999:3003:3003").is_err());
        assert!(parse_publish_arg("[notv6]:8080:3003").is_err());
    }

    #[test]
    fn parse_publish_arg_rejects_too_many_colon_segments() {
        assert!(parse_publish_arg("1:2:3:4").is_err());
    }

    #[test]
    fn parse_publish_arg_rejects_empty_segments() {
        for spec in ["3003:", ":3003", ":3003:3003", "3003:/tcp"] {
            let err = parse_publish_arg(spec).unwrap_err();
            assert!(err.contains("invalid -p spec"), "{spec:?} -> {err}");
        }
    }
}
