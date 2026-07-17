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
    about = "Lens Sandbox — run AI agents, commands, and OCI images in local microVMs. Control access into and out of the sandbox."
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
        value_name = "REF",
        help = "Sandbox reference: a registry coordinate (e.g. ghcr.io/team/hermes:1.4.0) or a path to a local definition (., lns.yaml, ./dir, /abs/path). Omit to run ./lns.yaml in the current directory."
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
        short = 'u',
        long = "user",
        value_name = "USER[:GROUP]",
        conflicts_with_all = ["sandbox_user", "sandbox_uid"],
        help = "Run-as user or uid inside the sandbox (`USER[:GROUP]`). Alias for `--sandbox-user` / `--sandbox-uid`."
    )]
    pub user: Option<String>,

    #[arg(
        long,
        help = "Run-as user inside the sandbox. Defaults to the image's USER; the unprivileged `sandbox` user when the image sets none."
    )]
    pub sandbox_user: Option<String>,

    #[arg(
        long,
        help = "Run-as uid inside the sandbox. Defaults to the image's USER uid; 65534 (the `sandbox` user) when the image sets none."
    )]
    pub sandbox_uid: Option<u32>,

    #[arg(
        long = "rm",
        default_value_t = false,
        help = "Automatically remove the run record after the workload exits."
    )]
    pub auto_remove: bool,

    #[arg(
        short = 'i',
        long = "interactive",
        action = clap::ArgAction::Set,
        num_args = 0..=1,
        require_equals = true,
        default_value_t = true,
        default_missing_value = "true",
        help = "Keep stdin open (forward host stdin to the guest workload). Pass `--interactive=false` (or `-i=false`) to disable."
    )]
    pub interactive: bool,

    #[arg(
        short = 't',
        long = "tty",
        action = clap::ArgAction::Set,
        num_args = 0..=1,
        require_equals = true,
        default_value_t = true,
        default_missing_value = "true",
        help = "Allocate a PTY in the broker; pipe mode is selected automatically when stdin is not a TTY. Pass `--tty=false` (or `-t=false`) to disable."
    )]
    pub tty: bool,

    #[arg(
        long,
        value_name = "COMMAND",
        help = "Override the image ENTRYPOINT while keeping command arguments after the image."
    )]
    pub entrypoint: Option<String>,

    #[arg(
        short = 'h',
        long = "hostname",
        value_name = "NAME",
        value_parser = parse_hostname_arg,
        help = "Set the guest hostname for this run."
    )]
    pub hostname: Option<String>,

    #[arg(
        short = 'd',
        long,
        default_value_t = false,
        conflicts_with_all = ["interactive", "tty"],
        help = "Return immediately after starting; the run continues in the daemon and is reachable via `lns exec`, `lns kill`, `lns ps`."
    )]
    pub detach: bool,

    #[arg(
        long,
        default_value = "ctrl-p,ctrl-q",
        value_parser = parse_detach_keys_arg,
        help = "Comma-separated detach chord (single chars or `ctrl-X`); on match the CLI returns 0 and leaves the run executing in the background — re-join it with `lns sandbox attach`. No signal is sent to the workload."
    )]
    pub detach_keys: DetachChord,

    #[arg(
        short = 'w',
        long,
        value_name = "DIR",
        value_parser = parse_workdir_arg,
        help = "Working directory inside the sandbox (absolute path; created if missing). Overrides spec.workdir and the image's WORKDIR."
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
        short = 'P',
        long = "publish-declared",
        default_value_t = false,
        help = "Publish the definition's declared spec.ports on loopback (host value when present, the container number otherwise). Automatic for a local ./lns.yaml run; opt-in for a pulled sandbox."
    )]
    pub publish_declared: bool,

    #[arg(skip)]
    pub declared_unpublished: Vec<u16>,

    #[arg(skip)]
    pub filesets: Vec<(String, String)>,

    #[arg(
        short = 'v',
        long = "volume",
        visible_alias = "mount",
        value_parser = parse_mount_arg,
        help = "Mount into the workload: `name:/path[:ro]`, `/host/path:/path[:ro]`, or `type=bind|volume,source=...,target=...[,readonly]`."
    )]
    pub mounts: Vec<lns_ipc::MountSpec>,

    #[arg(
        short = 'q',
        long = "quiet",
        default_value_t = false,
        help = "Suppress the launch banner and ✓ status lines; warnings, errors, and the workload's own output are still printed. Useful for scripted/programmatic callers."
    )]
    pub quiet: bool,

    #[arg(
        last = true,
        help = "Command to run in the workload; replaces the image CMD but keeps its ENTRYPOINT (use `--entrypoint` to override or clear that). Accepted after the image or after `--`."
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

    pub fn effective_sandbox_user(&self) -> Option<String> {
        self.user.clone().or_else(|| self.sandbox_user.clone())
    }

    pub fn effective_sandbox_uid(&self) -> Option<u32> {
        self.sandbox_uid
            .or_else(|| self.user.as_deref().and_then(user_spec_uid))
    }
}

pub fn split_mounts(
    mounts: &[lns_ipc::MountSpec],
) -> (Vec<lns_ipc::VolumeMount>, Vec<lns_ipc::BindSpec>) {
    let mut volumes = Vec::new();
    let mut binds = Vec::new();
    for mount in mounts {
        match mount {
            lns_ipc::MountSpec::Named(v) => volumes.push(v.clone()),
            lns_ipc::MountSpec::Bind(b) => binds.push(b.clone()),
        }
    }
    (volumes, binds)
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
        action = clap::ArgAction::Set,
        num_args = 0..=1,
        require_equals = true,
        default_value_t = true,
        default_missing_value = "true",
        help = "`-i` — keep stdin open. Mirrors `docker exec -i`. Pass `--interactive=false` (or `-i=false`) for a non-interactive exec."
    )]
    pub interactive: bool,

    #[arg(
        short = 't',
        long = "tty",
        action = clap::ArgAction::Set,
        num_args = 0..=1,
        require_equals = true,
        default_value_t = true,
        default_missing_value = "true",
        help = "Allocate a PTY for the exec session. Pass `--tty=false` (or `-t=false`) for a non-interactive exec."
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
        short = 'q',
        long = "quiet",
        default_value_t = false,
        help = "Suppress ✓ status lines; warnings, errors, and the workload's own output are still printed."
    )]
    pub quiet: bool,

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

pub(crate) fn parse_hostname_arg(s: &str) -> Result<String, String> {
    if s.is_empty() {
        return Err("hostname must not be empty".to_string());
    }
    if s.len() > 63 {
        return Err(format!("hostname `{s}` is longer than 63 bytes"));
    }
    if s.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
    {
        Ok(s.to_string())
    } else {
        Err(format!(
            "hostname `{s}` may contain only ASCII letters, digits, hyphens, and dots"
        ))
    }
}

fn parse_workdir_arg(s: &str) -> Result<String, String> {
    if !s.starts_with('/') {
        return Err(format!(
            "workdir must be an absolute path inside the sandbox, got `{s}`"
        ));
    }
    Ok(s.to_string())
}

pub(crate) fn parse_mount_arg(s: &str) -> Result<lns_ipc::MountSpec, String> {
    if is_keyed_mount_spec(s) {
        parse_keyed_mount_arg(s)
    } else {
        lns_ipc::MountSpec::parse(s)
    }
}

fn is_keyed_mount_spec(s: &str) -> bool {
    s.split(',').any(|field| {
        let field = field.trim();
        matches!(
            field.split_once('='),
            Some((
                "type"
                    | "source"
                    | "src"
                    | "target"
                    | "destination"
                    | "dst"
                    | "readonly"
                    | "ro"
                    | "readwrite"
                    | "rw",
                _,
            ))
        ) || matches!(field, "readonly" | "ro" | "readwrite" | "rw")
    })
}

fn parse_keyed_mount_arg(s: &str) -> Result<lns_ipc::MountSpec, String> {
    let mut kind: Option<&str> = None;
    let mut source: Option<&str> = None;
    let mut target: Option<&str> = None;
    let mut read_only = false;
    for part in s.split(',') {
        let part = part.trim();
        match part.split_once('=') {
            Some(("type", value)) => kind = Some(value),
            Some(("source" | "src", value)) => source = Some(value),
            Some(("target" | "destination" | "dst", value)) => target = Some(value),
            Some(("readonly" | "ro", value)) => read_only = parse_mount_bool(value, s)?,
            Some(("readwrite" | "rw", value)) => read_only = !parse_mount_bool(value, s)?,
            None if matches!(part, "readonly" | "ro") => read_only = true,
            None if matches!(part, "readwrite" | "rw") => read_only = false,
            Some((key, _)) => {
                return Err(format!(
                    "invalid --mount spec `{s}`: unsupported field `{key}`"
                ));
            }
            None => {
                return Err(format!(
                    "invalid --mount spec `{s}`: unsupported field `{part}`"
                ));
            }
        }
    }
    let source = source.ok_or_else(|| format!("invalid --mount spec `{s}`: missing source"))?;
    let target = target.ok_or_else(|| format!("invalid --mount spec `{s}`: missing target"))?;
    let suffix = if read_only { ":ro" } else { "" };
    match kind {
        Some("bind") => lns_ipc::BindSpec::parse(&format!("{source}:{target}{suffix}"))
            .map(lns_ipc::MountSpec::Bind),
        Some("volume") => lns_ipc::VolumeMount::parse(&format!("{source}:{target}{suffix}"))
            .map(lns_ipc::MountSpec::Named),
        None if source.starts_with('/') => {
            lns_ipc::BindSpec::parse(&format!("{source}:{target}{suffix}"))
                .map(lns_ipc::MountSpec::Bind)
        }
        None => lns_ipc::VolumeMount::parse(&format!("{source}:{target}{suffix}"))
            .map(lns_ipc::MountSpec::Named),
        Some(other) => Err(format!(
            "invalid --mount spec `{s}`: type must be bind or volume, got `{other}`"
        )),
    }
}

fn parse_mount_bool(value: &str, spec: &str) -> Result<bool, String> {
    match value.to_ascii_lowercase().as_str() {
        "1" | "true" => Ok(true),
        "0" | "false" => Ok(false),
        _ => Err(format!(
            "invalid --mount spec `{spec}`: boolean value `{value}`"
        )),
    }
}

fn user_spec_uid(spec: &str) -> Option<u32> {
    spec.split_once(':')
        .map_or(spec, |(user, _)| user)
        .parse::<u32>()
        .ok()
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
    use clap::Parser;

    #[derive(Parser)]
    #[command(disable_help_flag = true)]
    struct RunHarness {
        #[command(flatten)]
        args: RunArgs,
    }

    #[derive(Parser)]
    struct ExecHarness {
        #[command(flatten)]
        args: ExecArgs,
    }

    fn parse_run(argv: &[&str]) -> Result<RunArgs, clap::Error> {
        let mut full = vec!["run"];
        full.extend_from_slice(argv);
        RunHarness::try_parse_from(full).map(|h| h.args)
    }

    fn parse_exec(argv: &[&str]) -> Result<ExecArgs, clap::Error> {
        let mut full = vec!["exec"];
        full.extend_from_slice(argv);
        ExecHarness::try_parse_from(full).map(|h| h.args)
    }

    #[test]
    fn run_interactive_and_tty_default_to_true() {
        let args = parse_run(&["alpine:3.20"]).expect("defaults parse");
        assert!(args.interactive, "interactive should default to true");
        assert!(args.tty, "tty should default to true");
    }

    #[test]
    fn run_interactive_and_tty_accept_explicit_false() {
        let args = parse_run(&["--interactive=false", "--tty=false", "alpine:3.20"])
            .expect("explicit false should parse");
        assert!(!args.interactive, "interactive should be false");
        assert!(!args.tty, "tty should be false");
    }

    #[test]
    fn run_short_flags_accept_explicit_false() {
        let args =
            parse_run(&["-i=false", "-t=false", "alpine:3.20"]).expect("short =false should parse");
        assert!(!args.interactive);
        assert!(!args.tty);
    }

    #[test]
    fn run_interactive_and_tty_accept_explicit_true() {
        let args = parse_run(&["--interactive=true", "--tty=true", "alpine:3.20"])
            .expect("explicit true should parse");
        assert!(args.interactive);
        assert!(args.tty);
    }

    #[test]
    fn run_bare_flags_followed_by_positional_stay_true() {
        // `-i` / `-t` with no value must use default_missing_value and NOT swallow
        // the following positional image arg (require_equals guards this).
        let args = parse_run(&["-i", "-t", "alpine:3.20"]).expect("bare flags parse");
        assert!(args.interactive);
        assert!(args.tty);
        assert_eq!(args.image.as_deref(), Some("alpine:3.20"));
    }

    #[test]
    fn run_detach_still_works_with_default_flag_values() {
        // -d conflicts_with_all [interactive, tty]; defaults must not trip the conflict.
        let args = parse_run(&["-d", "alpine:3.20"]).expect("detach with defaults should parse");
        assert!(args.detach);
    }

    #[test]
    fn run_detach_conflicts_with_explicitly_provided_interactive() {
        let err = parse_run(&["-d", "--interactive=true", "alpine:3.20"])
            .err()
            .expect("explicit -i with -d should conflict");
        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn run_accepts_compat_flags_that_map_to_launch_config() {
        let args = parse_run(&[
            "--rm",
            "-u",
            "1000:1000",
            "--entrypoint",
            "/bin/sh",
            "-h",
            "demo-host",
            "alpine:3.20",
        ])
        .expect("compat flags parse");
        assert!(args.auto_remove);
        assert_eq!(args.effective_sandbox_user().as_deref(), Some("1000:1000"));
        assert_eq!(args.effective_sandbox_uid(), Some(1000));
        assert_eq!(args.entrypoint.as_deref(), Some("/bin/sh"));
        assert_eq!(args.hostname.as_deref(), Some("demo-host"));
    }

    #[test]
    fn run_mount_accepts_keyed_bind_syntax() {
        let args = parse_run(&[
            "--mount",
            "type=bind,src=/Users/me/project,target=/work,readonly",
            "alpine:3.20",
        ])
        .expect("keyed --mount syntax parses");
        let (volumes, binds) = split_mounts(&args.mounts);
        assert!(volumes.is_empty());
        assert_eq!(binds.len(), 1);
        assert_eq!(binds[0].host_source, "/Users/me/project");
        assert_eq!(binds[0].target, "/work");
        assert!(binds[0].read_only);
    }

    #[test]
    fn run_mount_accepts_keyed_volume_syntax() {
        let args = parse_run(&[
            "--mount",
            "type=volume,source=cache,target=/cache",
            "alpine:3.20",
        ])
        .expect("keyed volume --mount syntax parses");
        let (volumes, binds) = split_mounts(&args.mounts);
        assert!(binds.is_empty());
        assert_eq!(volumes.len(), 1);
        assert_eq!(volumes[0].name, "cache");
        assert_eq!(volumes[0].target, "/cache");
        assert!(!volumes[0].read_only);
    }

    #[test]
    fn parse_hostname_arg_rejects_empty_long_and_invalid_names() {
        assert_eq!(
            parse_hostname_arg("").expect_err("empty hostname must fail"),
            "hostname must not be empty"
        );
        let long = "a".repeat(64);
        assert!(
            parse_hostname_arg(&long)
                .expect_err("long hostname must fail")
                .contains("longer than 63 bytes")
        );
        assert!(
            parse_hostname_arg("bad_host")
                .expect_err("underscore hostname must fail")
                .contains("may contain only ASCII letters")
        );
    }

    #[test]
    fn parse_mount_arg_infers_bind_and_accepts_readonly_key() {
        let mount = parse_mount_arg("source=/Users/me/project,target=/work,readonly=true")
            .expect("inferred bind mount parses");
        assert_eq!(
            mount,
            lns_ipc::MountSpec::Bind(lns_ipc::BindSpec {
                host_source: "/Users/me/project".into(),
                target: "/work".into(),
                read_only: true,
            })
        );
    }

    #[test]
    fn parse_mount_arg_infers_volume_and_accepts_readwrite_controls() {
        let writable = parse_mount_arg("source=cache,target=/cache,readwrite=true")
            .expect("inferred writable volume parses");
        assert_eq!(
            writable,
            lns_ipc::MountSpec::Named(lns_ipc::VolumeMount {
                name: "cache".into(),
                target: "/cache".into(),
                read_only: false,
            })
        );

        let readonly = parse_mount_arg("source=cache,target=/cache,readwrite=false")
            .expect("inferred readonly volume parses");
        assert_eq!(
            readonly,
            lns_ipc::MountSpec::Named(lns_ipc::VolumeMount {
                name: "cache".into(),
                target: "/cache".into(),
                read_only: true,
            })
        );
    }

    #[test]
    fn parse_mount_arg_accepts_bare_readwrite_marker() {
        let mount =
            parse_mount_arg("type=volume,source=cache,target=/cache,rw").expect("rw parses");
        assert_eq!(
            mount,
            lns_ipc::MountSpec::Named(lns_ipc::VolumeMount {
                name: "cache".into(),
                target: "/cache".into(),
                read_only: false,
            })
        );
    }

    #[test]
    fn parse_mount_arg_rejects_unsupported_mount_fields() {
        for spec in [
            "type=bind,source=/host,target=/work,unknown=value",
            "type=bind,source=/host,target=/work,mystery",
            "type=tmpfs,source=tmp,target=/tmp",
            "type=bind,source=/host,target=/work,readonly=maybe",
        ] {
            assert!(parse_mount_arg(spec).is_err(), "{spec} should be rejected");
        }
    }

    #[test]
    fn parse_mount_arg_keeps_a_colon_spec_whose_host_path_contains_equals() {
        let mount = parse_mount_arg("/tmp/a=b:/work").expect("bind path with '=' parses");
        assert_eq!(
            mount,
            lns_ipc::MountSpec::Bind(lns_ipc::BindSpec {
                host_source: "/tmp/a=b".into(),
                target: "/work".into(),
                read_only: false,
            })
        );
    }

    #[test]
    fn exec_interactive_and_tty_default_to_true() {
        let args = parse_exec(&["demo", "--", "echo", "hi"]).expect("defaults parse");
        assert!(args.interactive);
        assert!(args.tty);
    }

    #[test]
    fn exec_interactive_and_tty_accept_explicit_false() {
        let args = parse_exec(&[
            "--tty=false",
            "--interactive=false",
            "demo",
            "--",
            "echo",
            "hi",
        ])
        .expect("explicit false should parse");
        assert!(!args.interactive, "interactive should be false");
        assert!(!args.tty, "tty should be false");
    }

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
