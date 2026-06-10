use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use lns_policy::{Policy, Verdict};

use crate::cli::RunArgs;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicySource {
    Explicit(PathBuf),
    FoundInCwd,
    AutoCreated,
}

const DEFAULT_POLICY_FILENAME: &str = "lns-policy.yaml";
const DEFAULT_POLICY_YAML: &str = "\
network:
  allowedRoutes: []
  defaultVerdict: ask
  defaultTransport: direct
";

pub fn policy_path(explicit: Option<&Path>, cwd: &Path) -> PathBuf {
    match explicit {
        Some(p) if p.is_absolute() => p.to_path_buf(),
        Some(p) => cwd.join(p),
        None => cwd.join(DEFAULT_POLICY_FILENAME),
    }
}

pub fn resolve_policy(explicit: Option<&Path>, cwd: &Path) -> Result<(PathBuf, PolicySource)> {
    if let Some(p) = explicit {
        return Ok((
            policy_path(Some(p), cwd),
            PolicySource::Explicit(p.to_path_buf()),
        ));
    }
    let path = policy_path(None, cwd);
    match std::fs::metadata(&path) {
        Ok(md) if md.is_file() => Ok((path, PolicySource::FoundInCwd)),
        Ok(_) => anyhow::bail!(
            "{} exists but is not a regular file; remove it or pass `--policy <path>`",
            path.display()
        ),
        Err(_) => {
            std::fs::write(&path, DEFAULT_POLICY_YAML)
                .with_context(|| format!("creating default policy at {}", path.display()))?;
            Ok((path, PolicySource::AutoCreated))
        }
    }
}

pub fn print_run_summary(
    args: &RunArgs,
    cwd: &Path,
    writer: &mut impl io::Write,
) -> Result<PathBuf> {
    let (path, source) = resolve_policy(args.policy.as_deref(), cwd)?;
    let policy = Policy::load_or_default(&path)
        .with_context(|| format!("loading policy from {}", path.display()))?;
    let body = format_summary(args, &policy, &path, &source);
    writer.write_all(body.as_bytes())?;
    Ok(path)
}

pub fn format_summary(
    args: &RunArgs,
    policy: &Policy,
    policy_path: &Path,
    source: &PolicySource,
) -> String {
    let mut s = String::with_capacity(512);
    s.push_str("lns run\n");
    writeln!(s, "  Image:     {}", image_line(args)).unwrap();
    for vol in &args.volumes {
        writeln!(s, "  Volume:    {}", volume_line(vol)).unwrap();
    }
    writeln!(s, "  Resources: {} vCPU · {} MiB", args.cpus, args.mem).unwrap();
    writeln!(s, "  Flags:     {}", flags_line(args)).unwrap();
    writeln!(s, "  Ports:     {}", ports_line(args)).unwrap();
    s.push_str("  Policy:\n");
    writeln!(s, "    file: {}", policy_path.display()).unwrap();
    writeln!(
        s,
        "    default verdict: {}",
        verdict_word(policy.network.default_verdict)
    )
    .unwrap();
    writeln!(s, "    rules: {}", rules_line(policy)).unwrap();
    writeln!(s, "    source: {}", source_line(source)).unwrap();
    s.push('\n');
    s
}

fn image_line(args: &RunArgs) -> String {
    match &args.image {
        Some(image) => format!("{image} (resolving…)"),
        None => "<imageless>".to_string(),
    }
}

fn volume_line(vol: &lns_ipc::VolumeMount) -> String {
    let mode = if vol.read_only { " (ro)" } else { "" };
    format!("{} → {}{mode}", vol.name, vol.target)
}

fn flags_line(args: &RunArgs) -> String {
    let mut flags: Vec<&str> = Vec::new();
    if args.interactive {
        flags.push("-i");
    }
    if args.tty {
        flags.push("-t");
    }
    if args.detach {
        flags.push("-d");
    }
    if flags.is_empty() {
        "(none)".to_string()
    } else {
        flags.join(" ")
    }
}

fn ports_line(args: &RunArgs) -> String {
    if args.publish.is_empty() {
        return "(none)".to_string();
    }
    args.publish
        .iter()
        .map(|p| {
            let bind = std::net::SocketAddr::new(p.host_ip, p.host_port);
            let mapping = format!("{bind} -> {}", p.container_port);
            if p.host_ip.is_loopback() {
                mapping
            } else {
                format!("{mapping} (exposed beyond this machine)")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn verdict_word(v: Verdict) -> &'static str {
    match v {
        Verdict::Allow => "allow",
        Verdict::Deny => "deny",
        Verdict::Ask => "ask",
    }
}

fn rules_line(policy: &Policy) -> String {
    let routes = &policy.network.allowed_routes;
    if routes.is_empty() {
        return "none defined; anything else asks".to_string();
    }
    let (allows, denies) = routes
        .iter()
        .fold((0u32, 0u32), |(a, d), r| match r.verdict {
            Verdict::Allow => (a + 1, d),
            Verdict::Deny => (a, d + 1),
            Verdict::Ask => (a, d),
        });
    format!("{} allow, {} deny, anything else asks", allows, denies)
}

fn source_line(source: &PolicySource) -> String {
    match source {
        PolicySource::FoundInCwd => "found in this directory".to_string(),
        PolicySource::AutoCreated => "auto-created (no policy in this directory)".to_string(),
        PolicySource::Explicit(p) => format!("--policy {}", p.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lns_policy::{RouteRule, Transport};

    fn run_args(image: Option<&str>) -> RunArgs {
        RunArgs {
            image: image.map(str::to_string),
            cpus: 1,
            mem: 512,
            policy: None,
            sandbox_user: None,
            sandbox_uid: None,
            interactive: true,
            tty: true,
            detach: false,
            detach_keys: crate::cli::DetachChord(vec![0x10, 0x11]),
            env: Vec::new(),
            publish: Vec::new(),
            volumes: Vec::new(),
            cmd: Vec::new(),
        }
    }

    fn publish(host_ip: &str, host_port: u16, container_port: u16) -> lns_ipc::PortPublish {
        lns_ipc::PortPublish {
            host_ip: host_ip.parse().unwrap(),
            host_port,
            container_port,
            protocol: lns_ipc::Protocol::Tcp,
        }
    }

    fn summary_with_publish(ports: Vec<lns_ipc::PortPublish>) -> String {
        let mut args = run_args(Some("prism"));
        args.publish = ports;
        format_summary(
            &args,
            &Policy::default(),
            Path::new("./lns-policy.yaml"),
            &PolicySource::FoundInCwd,
        )
    }

    #[test]
    fn ports_line_says_none_when_nothing_is_published() {
        assert!(summary_with_publish(Vec::new()).contains("Ports:     (none)"));
    }

    #[test]
    fn ports_line_renders_a_loopback_mapping_without_an_exposure_warning() {
        let s = summary_with_publish(vec![publish("127.0.0.1", 3003, 3003)]);
        assert!(s.contains("Ports:     127.0.0.1:3003 -> 3003"), "got: {s}");
        assert!(!s.contains("exposed beyond"), "loopback must not warn: {s}");
    }

    #[test]
    fn ports_line_flags_a_non_loopback_bind_as_exposed() {
        let s = summary_with_publish(vec![publish("0.0.0.0", 3003, 3003)]);
        assert!(
            s.contains("0.0.0.0:3003 -> 3003 (exposed beyond this machine)"),
            "got: {s}"
        );
    }

    #[test]
    fn ports_line_brackets_an_ipv6_host_bind() {
        let s = summary_with_publish(vec![publish("::1", 8080, 3003)]);
        assert!(s.contains("[::1]:8080 -> 3003"), "got: {s}");
    }

    #[test]
    fn ports_line_joins_multiple_mappings() {
        let s = summary_with_publish(vec![
            publish("127.0.0.1", 3003, 3003),
            publish("127.0.0.1", 9090, 9090),
        ]);
        assert!(
            s.contains("127.0.0.1:3003 -> 3003, 127.0.0.1:9090 -> 9090"),
            "got: {s}"
        );
    }

    fn allow(host: &str) -> RouteRule {
        RouteRule {
            match_pattern: host.to_string(),
            verdict: Verdict::Allow,
            transport: Transport::Direct,
            scheme: None,
            description: None,
            tls_terminate: false,
            rules: Vec::new(),
        }
    }

    fn deny(host: &str) -> RouteRule {
        RouteRule {
            match_pattern: host.to_string(),
            verdict: Verdict::Deny,
            transport: Transport::Direct,
            scheme: None,
            description: None,
            tls_terminate: false,
            rules: Vec::new(),
        }
    }

    #[test]
    fn summary_lists_image_resources_flags_and_policy_block() {
        let args = run_args(Some("ubuntu"));
        let s = format_summary(
            &args,
            &Policy::default(),
            Path::new("/home/dev/my-app/lns-policy.yaml"),
            &PolicySource::FoundInCwd,
        );
        assert!(s.contains("Image:"), "missing Image line: {s}");
        assert!(s.contains("Resources:"), "missing Resources line: {s}");
        assert!(s.contains("Flags:"), "missing Flags line: {s}");
        assert!(s.contains("Policy:"), "missing Policy block: {s}");
    }

    #[test]
    fn summary_lists_attached_volumes_with_target_and_ro_marker() {
        let mut args = run_args(Some("ubuntu"));
        args.volumes = vec![
            lns_ipc::VolumeMount {
                name: "prism-data".into(),
                target: "/data".into(),
                read_only: false,
            },
            lns_ipc::VolumeMount {
                name: "ro-cfg".into(),
                target: "/cfg".into(),
                read_only: true,
            },
        ];
        let s = format_summary(
            &args,
            &Policy::default(),
            Path::new("/x/lns-policy.yaml"),
            &PolicySource::FoundInCwd,
        );
        assert!(
            s.contains("Volume:    prism-data → /data"),
            "missing volume line: {s}"
        );
        assert!(
            s.contains("Volume:    ro-cfg → /cfg (ro)"),
            "missing ro volume line: {s}"
        );
    }

    #[test]
    fn summary_omits_volume_line_when_none_attached() {
        let s = format_summary(
            &run_args(Some("ubuntu")),
            &Policy::default(),
            Path::new("/x/lns-policy.yaml"),
            &PolicySource::FoundInCwd,
        );
        assert!(!s.contains("Volume:"), "no volume line expected: {s}");
    }

    #[test]
    fn image_field_shows_resolving_placeholder_until_service_confirms_digest() {
        let args = run_args(Some("ubuntu"));
        let s = format_summary(
            &args,
            &Policy::default(),
            Path::new("/x/lns-policy.yaml"),
            &PolicySource::FoundInCwd,
        );
        assert!(s.contains("ubuntu (resolving…)"), "no placeholder: {s}");
    }

    #[test]
    fn imageless_run_displays_imageless_marker_instead_of_image_placeholder() {
        let args = run_args(None);
        let s = format_summary(
            &args,
            &Policy::default(),
            Path::new("/x/lns-policy.yaml"),
            &PolicySource::FoundInCwd,
        );
        assert!(s.contains("<imageless>"), "no imageless marker: {s}");
        assert!(
            !s.contains("resolving"),
            "no resolving placeholder on imageless: {s}"
        );
    }

    #[test]
    fn resources_line_renders_cpu_and_memory_with_units() {
        let mut args = run_args(Some("ubuntu"));
        args.cpus = 4;
        args.mem = 2048;
        let s = format_summary(
            &args,
            &Policy::default(),
            Path::new("/x/lns-policy.yaml"),
            &PolicySource::FoundInCwd,
        );
        assert!(s.contains("4 vCPU · 2048 MiB"), "resources line wrong: {s}");
    }

    #[test]
    fn flags_line_lists_interactive_tty_and_detach_in_canonical_order() {
        let mut args = run_args(Some("ubuntu"));
        args.interactive = true;
        args.tty = true;
        args.detach = false;
        let s = format_summary(
            &args,
            &Policy::default(),
            Path::new("/x/lns-policy.yaml"),
            &PolicySource::FoundInCwd,
        );
        assert!(s.contains("Flags:     -i -t"), "flags line wrong: {s}");
    }

    #[test]
    fn flags_line_includes_detach_when_set() {
        let mut args = run_args(Some("ubuntu"));
        args.interactive = false;
        args.tty = false;
        args.detach = true;
        let s = format_summary(
            &args,
            &Policy::default(),
            Path::new("/x/lns-policy.yaml"),
            &PolicySource::FoundInCwd,
        );
        assert!(s.contains("Flags:     -d"), "flags line wrong: {s}");
    }

    #[test]
    fn rules_line_does_not_count_ask_verdict_rules_as_allow_or_deny() {
        let mut policy = Policy::default();
        policy.add_rule(RouteRule {
            match_pattern: "ambiguous.example".to_string(),
            verdict: Verdict::Ask,
            transport: Transport::Direct,
            scheme: None,
            description: None,
            tls_terminate: false,
            rules: Vec::new(),
        });
        let s = format_summary(
            &run_args(Some("ubuntu")),
            &policy,
            Path::new("./lns-policy.yaml"),
            &PolicySource::FoundInCwd,
        );
        assert!(s.contains("0 allow, 0 deny, anything else asks"));
    }

    #[test]
    fn flags_line_says_none_when_no_flags_are_set() {
        let mut args = run_args(Some("ubuntu"));
        args.interactive = false;
        args.tty = false;
        args.detach = false;
        let s = format_summary(
            &args,
            &Policy::default(),
            Path::new("/x/lns-policy.yaml"),
            &PolicySource::FoundInCwd,
        );
        assert!(s.contains("Flags:     (none)"), "flags line wrong: {s}");
    }

    #[test]
    fn policy_block_shows_file_path_default_verdict_and_rule_summary() {
        let mut policy = Policy::default();
        policy.add_rule(allow("api.linear.app"));
        policy.add_rule(allow("api.example.com"));
        policy.add_rule(allow("registry.npmjs.org"));
        policy.add_rule(deny("evil.example"));
        let s = format_summary(
            &run_args(Some("ubuntu")),
            &policy,
            Path::new("./lns-policy.yaml"),
            &PolicySource::FoundInCwd,
        );
        assert!(s.contains("file: ./lns-policy.yaml"));
        assert!(s.contains("default verdict: ask"));
        assert!(s.contains("3 allow, 1 deny, anything else asks"));
    }

    #[test]
    fn rules_line_says_none_defined_for_an_empty_route_list() {
        let s = format_summary(
            &run_args(Some("ubuntu")),
            &Policy::default(),
            Path::new("./lns-policy.yaml"),
            &PolicySource::FoundInCwd,
        );
        assert!(
            s.contains("rules: none defined; anything else asks"),
            "rules line wrong: {s}"
        );
    }

    #[test]
    fn rules_line_uses_singular_counts_at_one() {
        let mut policy = Policy::default();
        policy.add_rule(allow("api.linear.app"));
        policy.add_rule(deny("evil.example"));
        let s = format_summary(
            &run_args(Some("ubuntu")),
            &policy,
            Path::new("./lns-policy.yaml"),
            &PolicySource::FoundInCwd,
        );
        assert!(s.contains("1 allow, 1 deny, anything else asks"));
    }

    #[test]
    fn default_verdict_word_covers_each_variant() {
        assert_eq!(verdict_word(Verdict::Allow), "allow");
        assert_eq!(verdict_word(Verdict::Deny), "deny");
        assert_eq!(verdict_word(Verdict::Ask), "ask");
    }

    #[test]
    fn source_line_for_found_in_cwd_reads_found_in_this_directory() {
        let s = format_summary(
            &run_args(Some("ubuntu")),
            &Policy::default(),
            Path::new("./lns-policy.yaml"),
            &PolicySource::FoundInCwd,
        );
        assert!(s.contains("source: found in this directory"));
    }

    #[test]
    fn source_line_for_auto_created_calls_out_no_policy_in_directory() {
        let s = format_summary(
            &run_args(Some("ubuntu")),
            &Policy::default(),
            Path::new("./lns-policy.yaml"),
            &PolicySource::AutoCreated,
        );
        assert!(s.contains("source: auto-created (no policy in this directory)"));
    }

    #[test]
    fn source_line_for_explicit_flag_quotes_the_passed_path() {
        let s = format_summary(
            &run_args(Some("ubuntu")),
            &Policy::default(),
            Path::new("/home/ops/team-policy.yaml"),
            &PolicySource::Explicit(PathBuf::from("/home/ops/team-policy.yaml")),
        );
        assert!(s.contains("source: --policy /home/ops/team-policy.yaml"));
    }

    #[test]
    fn resolve_policy_explicit_passthrough_does_not_touch_disk() {
        let dir = tempfile::TempDir::new().unwrap();
        let explicit = dir.path().join("absent-but-named.yaml");
        let (resolved, source) = resolve_policy(Some(&explicit), dir.path()).unwrap();
        assert_eq!(resolved, explicit);
        assert_eq!(source, PolicySource::Explicit(explicit));
        assert!(!dir.path().join(DEFAULT_POLICY_FILENAME).exists());
    }

    #[test]
    fn resolve_policy_makes_relative_explicit_paths_absolute_against_cwd() {
        let dir = tempfile::TempDir::new().unwrap();
        let relative = Path::new("team-policy.yaml");
        let (resolved, source) = resolve_policy(Some(relative), dir.path()).unwrap();
        assert_eq!(
            resolved,
            dir.path().join("team-policy.yaml"),
            "daemon must receive an absolute path so its cwd doesn't change which file is loaded",
        );
        assert_eq!(
            source,
            PolicySource::Explicit(relative.to_path_buf()),
            "display source preserves what the user typed",
        );
    }

    #[test]
    fn resolve_policy_finds_existing_default_file_in_cwd() {
        let dir = tempfile::TempDir::new().unwrap();
        let preexisting = dir.path().join(DEFAULT_POLICY_FILENAME);
        std::fs::write(
            &preexisting,
            "network:\n  allowedRoutes: []\n  defaultVerdict: ask\n  defaultTransport: direct\n",
        )
        .unwrap();
        let (resolved, source) = resolve_policy(None, dir.path()).unwrap();
        assert_eq!(resolved, preexisting);
        assert_eq!(source, PolicySource::FoundInCwd);
    }

    #[test]
    fn resolve_policy_auto_creates_default_file_when_missing() {
        let dir = tempfile::TempDir::new().unwrap();
        let (resolved, source) = resolve_policy(None, dir.path()).unwrap();
        assert_eq!(resolved, dir.path().join(DEFAULT_POLICY_FILENAME));
        assert_eq!(source, PolicySource::AutoCreated);
        let body = std::fs::read_to_string(&resolved).unwrap();
        assert!(body.contains("defaultVerdict: ask"));
    }

    #[test]
    fn resolve_policy_errors_when_default_path_is_a_directory() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join(DEFAULT_POLICY_FILENAME)).unwrap();
        let err = resolve_policy(None, dir.path()).expect_err("must reject non-file");
        assert!(format!("{err:#}").contains("not a regular file"));
    }

    #[test]
    fn print_run_summary_writes_to_provided_writer_and_returns_resolved_path() {
        let dir = tempfile::TempDir::new().unwrap();
        let args = run_args(Some("ubuntu"));
        let mut buf = Vec::<u8>::new();
        let path = print_run_summary(&args, dir.path(), &mut buf).unwrap();
        assert_eq!(path, dir.path().join(DEFAULT_POLICY_FILENAME));
        let text = String::from_utf8(buf).unwrap();
        assert!(text.contains("lns run"));
        assert!(text.contains("Policy:"));
    }
}
