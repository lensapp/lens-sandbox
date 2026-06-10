use clap::{Parser, Subcommand, ValueEnum};
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

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    #[command(about = "Run an OCI image in a microVM.")]
    Run(RunArgs),
    #[command(about = "Open a new session (`docker exec`-style) against a running run.")]
    Exec(ExecArgs),
    #[command(about = "Send a signal to a running run (`docker kill`-style).")]
    Kill(KillArgs),
    #[command(about = "List active runs (`docker ps`-style).")]
    Ls,
    #[command(about = "Manage the named volumes used with `lns run -v` (`docker volume`-style).")]
    Volume(VolumeArgs),
    #[command(about = "Verify the audit chain of a completed run.")]
    Audit(AuditArgs),
    #[command(about = "Manage the Lens Sandbox background service.")]
    Service(ServiceArgs),
    #[command(about = "Update `lns` and `lns-service` to the latest release.")]
    Update(UpdateArgs),
    #[command(about = "Edit network rules in a policy file.")]
    Policy(PolicyArgs),
    #[command(about = "Manage the credential-integration catalog (connectable services).")]
    Integration(IntegrationArgs),
    #[command(
        about = "Get and set persistent defaults, applied to `lns run` when the matching flag is absent."
    )]
    Config(ConfigArgs),
}

#[derive(clap::Args)]
pub struct ConfigArgs {
    #[command(subcommand)]
    pub command: ConfigCommand,
}

#[derive(Subcommand)]
pub enum ConfigCommand {
    #[command(
        about = "Set a default; list keys (run.env, run.volume, run.publish) replace all previous values."
    )]
    Set(ConfigSetArgs),
    #[command(about = "Print a default's value(s); exits 1 when the key is not set.")]
    Get(ConfigKeyArgs),
    #[command(about = "Remove a default.")]
    Unset(ConfigKeyArgs),
    #[command(about = "List every configured default.")]
    List,
}

const CONFIG_KEY_HELP: &str = "Config key: run.cpus, run.mem, run.env, run.volume, or run.publish.";

#[derive(clap::Args)]
pub struct ConfigSetArgs {
    #[arg(value_parser = crate::config::ConfigKey::parse, help = CONFIG_KEY_HELP)]
    pub key: crate::config::ConfigKey,
    #[arg(
        required = true,
        help = "Value(s) to store; each is validated like the matching `lns run` flag."
    )]
    pub values: Vec<String>,
}

#[derive(clap::Args)]
pub struct ConfigKeyArgs {
    #[arg(value_parser = crate::config::ConfigKey::parse, help = CONFIG_KEY_HELP)]
    pub key: crate::config::ConfigKey,
}

#[derive(clap::Args)]
pub struct VolumeArgs {
    #[command(subcommand)]
    pub command: VolumeCommand,
}

#[derive(Subcommand)]
pub enum VolumeCommand {
    #[command(about = "List named volumes with their on-disk size, age, and holder.")]
    Ls,
    #[command(about = "Create a named volume ahead of its first `lns run -v` attach.")]
    Create(VolumeNameArg),
    #[command(about = "Show a volume's details as JSON.")]
    Inspect(VolumeNameArg),
    #[command(about = "Remove a named volume; refused while a run holds it.")]
    Rm(VolumeNameArg),
    #[command(about = "Remove every volume not attached to a running sandbox.")]
    Prune(VolumePruneArgs),
}

#[derive(clap::Args)]
pub struct VolumeNameArg {
    #[arg(
        value_parser = parse_volume_name,
        help = "Volume name, as used with `lns run -v name:/path`."
    )]
    pub name: String,
}

#[derive(clap::Args)]
pub struct VolumePruneArgs {
    #[arg(
        short = 'f',
        long,
        default_value_t = false,
        help = "Skip the confirmation prompt."
    )]
    pub force: bool,
}

fn parse_volume_name(s: &str) -> Result<String, String> {
    lns_ipc::validate_volume_name(s)?;
    Ok(s.to_string())
}

#[derive(clap::Args)]
pub struct IntegrationArgs {
    #[command(subcommand)]
    pub command: IntegrationCommand,
}

#[derive(Subcommand)]
pub enum IntegrationCommand {
    #[command(about = "Declare a credential integration in your machine-global catalog.")]
    Add(IntegrationAddArgs),
    #[command(about = "List the bundled and user-declared integrations.")]
    List,
    #[command(about = "Remove a user-declared integration; bundled ones cannot be removed.")]
    Remove(IntegrationRemoveArgs),
    #[command(
        about = "Connect an integration to this directory's policy (oauth integrations sign in)."
    )]
    Connect(ConnectArgs),
    #[command(about = "Disconnect an integration from this directory's policy.")]
    Disconnect(DisconnectArgs),
}

#[derive(clap::Args)]
pub struct IntegrationAddArgs {
    #[arg(
        help = "New integration id; must not collide with a bundled or existing user integration."
    )]
    pub id: String,
    #[arg(long, help = "Environment variable the placeholder is seeded into.")]
    pub env_var: String,
    #[arg(
        long = "inject",
        required = true,
        value_parser = parse_injection,
        help = "Per-domain injection as KIND:DOMAIN (api_key_header needs KIND:DOMAIN:HEADER). Repeatable."
    )]
    pub inject: Vec<lns_policy::providers::InjectionDef>,
    #[arg(
        long = "route",
        help = "A host pattern the integration needs reachable. Repeatable."
    )]
    pub route: Vec<String>,
    #[arg(
        long,
        help = "Placeholder value; auto-generated (self-identifying) when omitted."
    )]
    pub placeholder: Option<String>,
}

#[derive(clap::Args)]
pub struct IntegrationRemoveArgs {
    #[arg(help = "User-declared integration id to remove.")]
    pub id: String,
}

#[derive(clap::Args)]
pub struct ConnectArgs {
    #[arg(help = "Integration id to connect (from `lns integration list`).")]
    pub id: String,
    #[arg(
        long,
        help = "Policy file path; defaults to `lns-policy.yaml` in the current directory."
    )]
    pub policy: Option<PathBuf>,
}

#[derive(clap::Args)]
pub struct DisconnectArgs {
    #[arg(help = "Integration id to disconnect.")]
    pub id: String,
    #[arg(
        long,
        help = "Policy file path; defaults to `lns-policy.yaml` in the current directory."
    )]
    pub policy: Option<PathBuf>,
}

fn parse_injection(s: &str) -> Result<lns_policy::providers::InjectionDef, String> {
    use lns_policy::providers::{InjectionDef, InjectionKind};
    let mut parts = s.splitn(3, ':');
    let kind_str = parts
        .next()
        .ok_or_else(|| format!("expected KIND:DOMAIN, got {s:?}"))?;
    let domain = parts
        .next()
        .ok_or_else(|| format!("expected KIND:DOMAIN, got {s:?}"))?;
    if domain.is_empty() {
        return Err(format!("injection {s:?} is missing a domain"));
    }
    let header_segment = parts.next();
    let kind = match kind_str {
        "bearer_header" => InjectionKind::BearerHeader,
        "uri_placeholder" => InjectionKind::UriPlaceholder,
        "token_header" => InjectionKind::TokenHeader,
        "basic_x_access_token" => InjectionKind::BasicXAccessToken,
        "api_key_header" => InjectionKind::ApiKeyHeader,
        "awsSigv4" | "aws_sigv4" => {
            return Err(
                "awsSigv4 carries real STS material and is not declarable from the CLI".to_string(),
            );
        }
        other => {
            return Err(format!(
                "unknown injection kind {other:?}; use bearer_header, uri_placeholder, token_header, basic_x_access_token, or api_key_header"
            ));
        }
    };
    let header = match (kind, header_segment) {
        (InjectionKind::ApiKeyHeader, Some(h)) if !h.is_empty() => Some(h.to_string()),
        (InjectionKind::ApiKeyHeader, _) => {
            return Err("api_key_header requires a header name (KIND:DOMAIN:HEADER)".to_string());
        }
        (_, Some(_)) => {
            return Err(format!(
                "kind {kind_str} does not take a header name; expected KIND:DOMAIN"
            ));
        }
        (_, None) => None,
    };
    Ok(InjectionDef {
        kind,
        domain: domain.to_string(),
        header,
    })
}

#[derive(clap::Args)]
pub struct PolicyArgs {
    #[command(subcommand)]
    pub command: PolicyCommand,
}

#[derive(Subcommand)]
pub enum PolicyCommand {
    #[command(about = "Add an allow rule for a destination pattern.")]
    Allow(PolicyRuleArgs),
    #[command(about = "Add a deny rule for a destination pattern.")]
    Deny(PolicyRuleArgs),
    #[command(about = "List the rules in the policy file.")]
    List(PolicyScopeArgs),
    #[command(about = "Remove the rule matching a destination pattern.")]
    Remove(PolicyRemoveArgs),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "lower")]
pub enum TransportArg {
    Direct,
    Upstream,
}

#[derive(clap::Args)]
pub struct PolicyRuleArgs {
    #[arg(help = "Destination pattern: host, wildcard (*.example.com), CIDR, or host:port.")]
    pub pattern: String,
    #[arg(long, help = "Human-readable note stored alongside the rule.")]
    pub description: Option<String>,
    #[arg(
        long,
        value_enum,
        default_value_t = TransportArg::Direct,
        help = "Transport for an allowed connection; ignored for deny rules."
    )]
    pub transport: TransportArg,
    #[arg(
        long,
        help = "Policy file path; defaults to `lns-policy.yaml` in the current directory."
    )]
    pub policy: Option<PathBuf>,
}

#[derive(clap::Args)]
pub struct PolicyScopeArgs {
    #[arg(
        long,
        help = "Policy file path; defaults to `lns-policy.yaml` in the current directory."
    )]
    pub policy: Option<PathBuf>,
}

#[derive(clap::Args)]
pub struct PolicyRemoveArgs {
    #[arg(help = "Destination pattern of the rule to remove.")]
    pub pattern: String,
    #[arg(
        long,
        help = "Policy file path; defaults to `lns-policy.yaml` in the current directory."
    )]
    pub policy: Option<PathBuf>,
}

#[derive(clap::Args)]
pub struct UpdateArgs {
    #[arg(
        long,
        default_value_t = false,
        help = "Re-install even if the running version matches, e.g. when the binary is corrupt or its codesign was invalidated."
    )]
    pub force: bool,

    #[arg(
        long,
        default_value_t = false,
        help = "Print the anonymous update-check payload that would be sent (install ID, version, OS/arch) and exit without contacting the network."
    )]
    pub dry_run: bool,
}

#[derive(clap::Args)]
pub struct ServiceArgs {
    #[command(subcommand)]
    pub command: ServiceCommand,
}

#[derive(Subcommand)]
pub enum ServiceCommand {
    #[command(about = "Start the Lens Sandbox background service.")]
    Start,
    #[command(about = "Stop the Lens Sandbox background service.")]
    Stop,
    #[command(about = "Show status of the Lens Sandbox background service.")]
    Status,
    #[command(
        about = "Register a per-user login agent and start the service now and on every login."
    )]
    Enable,
    #[command(about = "Stop the service and unregister the per-user login agent.")]
    Disable,
}

#[derive(clap::Args)]
pub struct AuditArgs {
    #[arg(help = "Run identifier surfaced by `lns run` as `✓ started run #<id>`.")]
    pub run_id: String,
}

#[derive(clap::Args)]
pub struct RunArgs {
    #[arg(
        help = "OCI image reference (e.g. alpine:3.20); omit for imageless mode, which requires a command after `--`."
    )]
    pub image: Option<String>,

    #[arg(
        long,
        help = "Number of vCPUs; falls back to the `run.cpus` config default, then 1."
    )]
    pub cpus: Option<u8>,

    #[arg(
        long,
        help = "RAM in MiB; falls back to the `run.mem` config default, then 512."
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
        help = "Comma-separated detach chord (single chars or `ctrl-X`); on match the CLI sends SIGHUP to the workload's foreground pgrp."
    )]
    pub detach_keys: DetachChord,

    #[arg(
        short = 'e',
        long = "env",
        value_name = "KEY=VALUE",
        value_parser = parse_env_kv,
        help = "Set a non-secret environment variable in the workload (repeatable). Secrets belong in the credential flow, not -e."
    )]
    pub env: Vec<String>,

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
    #[arg(help = "Target run id surfaced by `lns ls` or `lns run -d`.")]
    pub run_id: u32,

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
    #[arg(help = "Target run id.")]
    pub run_id: u32,

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

fn parse_detach_keys_arg(s: &str) -> Result<DetachChord, String> {
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
    fn parse_injection_accepts_the_two_declarable_kinds() {
        use lns_policy::providers::InjectionKind;
        let bearer = parse_injection("bearer_header:api.acme.corp").unwrap();
        assert_eq!(bearer.kind, InjectionKind::BearerHeader);
        assert_eq!(bearer.domain, "api.acme.corp");
        let uri = parse_injection("uri_placeholder:api.rocket.example").unwrap();
        assert_eq!(uri.kind, InjectionKind::UriPlaceholder);
    }

    #[test]
    fn parse_injection_rejects_awssigv4_with_a_clear_reason() {
        let err = parse_injection("awsSigv4:*.amazonaws.com").unwrap_err();
        assert!(err.contains("awsSigv4"), "got: {err}");
        assert!(parse_injection("aws_sigv4:x").is_err());
    }

    #[test]
    fn parse_injection_rejects_an_unknown_kind() {
        let err = parse_injection("basic_auth:api.acme.corp").unwrap_err();
        assert!(err.contains("unknown injection kind"), "got: {err}");
    }

    #[test]
    fn parse_injection_requires_a_kind_and_domain() {
        assert!(
            parse_injection("bearer_header")
                .unwrap_err()
                .contains("KIND:DOMAIN")
        );
        assert!(
            parse_injection("bearer_header:")
                .unwrap_err()
                .contains("missing a domain")
        );
    }

    #[test]
    fn parse_injection_accepts_token_header() {
        use lns_policy::providers::InjectionKind;
        let inj = parse_injection("token_header:api.example.com").unwrap();
        assert_eq!(inj.kind, InjectionKind::TokenHeader);
        assert_eq!(inj.domain, "api.example.com");
        assert_eq!(inj.header, None);
    }

    #[test]
    fn parse_injection_accepts_basic_x_access_token() {
        use lns_policy::providers::InjectionKind;
        let inj = parse_injection("basic_x_access_token:example.com").unwrap();
        assert_eq!(inj.kind, InjectionKind::BasicXAccessToken);
        assert_eq!(inj.domain, "example.com");
        assert_eq!(inj.header, None);
    }

    #[test]
    fn parse_injection_accepts_api_key_header_with_header_name() {
        use lns_policy::providers::InjectionKind;
        let inj = parse_injection("api_key_header:api.example.test:x-api-key").unwrap();
        assert_eq!(inj.kind, InjectionKind::ApiKeyHeader);
        assert_eq!(inj.domain, "api.example.test");
        assert_eq!(inj.header.as_deref(), Some("x-api-key"));
    }

    #[test]
    fn parse_injection_rejects_api_key_header_without_a_header_name() {
        let err = parse_injection("api_key_header:api.example.test").unwrap_err();
        assert!(
            err.contains("api_key_header") && err.contains("header name"),
            "got: {err}"
        );
        let err = parse_injection("api_key_header:api.example.test:").unwrap_err();
        assert!(
            err.contains("api_key_header") && err.contains("header name"),
            "got: {err}"
        );
    }

    #[test]
    fn parse_injection_rejects_a_header_segment_on_kinds_that_do_not_use_one() {
        let err = parse_injection("bearer_header:api.acme.corp:x-api-key").unwrap_err();
        assert!(err.contains("does not take a header name"), "got: {err}");
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
