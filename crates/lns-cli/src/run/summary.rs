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
  egress:
    http: []
  defaultVerdict: ask
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
    let (volumes, binds) = crate::cli::split_mounts(&args.mounts);
    for vol in &volumes {
        writeln!(s, "  Volume:    {}", volume_line(vol)).unwrap();
    }
    for bind in &binds {
        writeln!(s, "  Bind:      {}", bind_line(bind)).unwrap();
    }
    for (source, mount, owner) in &args.filesets {
        writeln!(s, "  Fileset:   {source} -> {mount} (owner: {owner})").unwrap();
    }
    if !args.tools.is_empty() {
        writeln!(s, "  Tools:     {}", args.tools.join(", ")).unwrap();
    }
    if let Some(dir) = &args.workdir {
        writeln!(s, "  Workdir:   {dir}").unwrap();
    }
    writeln!(
        s,
        "  Resources: {} vCPU · {} MiB",
        args.effective_cpus(),
        args.effective_mem()
    )
    .unwrap();
    writeln!(s, "  Flags:     {}", flags_line(args)).unwrap();
    writeln!(s, "  Ports:     {}", ports_line(args)).unwrap();
    if !args.declared_unpublished.is_empty() {
        writeln!(
            s,
            "  Declared:  {} (not published; opt in with -P)",
            args.declared_unpublished
                .iter()
                .map(u16::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        )
        .unwrap();
    }
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
    match (&args.image, &args.file) {
        (Some(image), _) => format!("{image} (resolving…)"),
        (None, Some(file)) => format!("{} (resolving…)", file.display()),
        (None, None) => "./lns.yaml (resolving…)".to_string(),
    }
}

fn volume_line(vol: &lns_ipc::VolumeMount) -> String {
    let mode = if vol.read_only { " (ro)" } else { "" };
    format!("{} → {}{mode}", vol.name, vol.target)
}

fn bind_line(bind: &lns_ipc::BindSpec) -> String {
    let mode = if bind.read_only {
        "read-only"
    } else {
        "read-write"
    };
    format!("{} → {} ({mode})", bind.host_source, bind.target)
}

/// Renders the post-scan secret disposition of each host bind (printed after KEEP/DROP resolution); empty when no bind exposed or dropped a secret.
pub fn format_bind_dispositions(binds: &[crate::run::host_bind::ResolvedBind]) -> String {
    let mut s = String::new();
    for bind in binds {
        for name in &bind.kept {
            writeln!(s, "  {name}: kept (exposed)").unwrap();
        }
        for name in &bind.dropped {
            writeln!(s, "  {name}: dropped").unwrap();
        }
    }
    s
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
    if args.auto_remove {
        flags.push("--rm");
    }
    if flags.is_empty() {
        "(none)".to_string()
    } else {
        flags.join(" ")
    }
}

/// The summary shows a path verbatim, shortens a ref digest to 12 characters, and names embedded files as inline.
pub fn fileset_source_display(fileset: &lns_artifact::sandbox::FilesetEntry) -> String {
    fileset_display(
        fileset.path.as_deref(),
        fileset.reference.as_deref(),
        fileset.inline.is_some(),
    )
}

pub fn fileset_owner_display(owner: lns_artifact::sandbox::FilesetOwner) -> &'static str {
    match owner {
        lns_artifact::sandbox::FilesetOwner::Workload => "workload",
        lns_artifact::sandbox::FilesetOwner::Root => "root",
    }
}

pub fn fileset_view_source_display(fileset: &lns_ipc::SandboxFileset) -> String {
    fileset_display(
        fileset.path.as_deref(),
        fileset.reference.as_deref(),
        fileset.inline,
    )
}

pub fn fileset_view_owner_display(owner: lns_ipc::SandboxFilesetOwner) -> &'static str {
    match owner {
        lns_ipc::SandboxFilesetOwner::Workload => "workload",
        lns_ipc::SandboxFilesetOwner::Root => "root",
    }
}

/// A pulled sandbox disclosed the same way at launch as a local one: its preflight view's filesets become summary lines.
pub fn fileset_summaries_from_view(view: &lns_ipc::SandboxView) -> Vec<(String, String, String)> {
    view.filesets
        .iter()
        .map(|fileset| {
            (
                fileset_view_source_display(fileset),
                fileset.mount_path.clone(),
                fileset_view_owner_display(fileset.owner).to_string(),
            )
        })
        .collect()
}

/// The declared tools a published sandbox discloses at launch; the local-definition case reads them off the definition instead.
pub fn tools_from_view(view: &lns_ipc::SandboxView) -> Vec<String> {
    view.tools.clone()
}

fn fileset_display(path: Option<&str>, reference: Option<&str>, inline: bool) -> String {
    if let Some(path) = path {
        return path.to_string();
    }
    if inline {
        return "inline".to_string();
    }
    let reference = reference.unwrap_or_default();
    match reference.split_once("@sha256:") {
        Some((repo, digest)) if digest.chars().count() > 12 => {
            let short: String = digest.chars().take(12).collect();
            format!("{repo}@sha256:{short}…")
        }
        _ => reference.to_string(),
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
    let routes = &policy.network.egress.http;
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
            file: None,
            name: None,
            registry: None,
            cpus: None,
            mem: None,
            policy: None,
            user: None,
            sandbox_user: None,
            sandbox_uid: None,
            auto_remove: false,
            interactive: true,
            tty: true,
            entrypoint: None,
            hostname: None,
            detach: false,
            detach_keys: crate::cli::DetachChord(vec![0x10, 0x11]),
            workdir: None,
            env: Vec::new(),
            env_file: Vec::new(),
            publish: Vec::new(),
            publish_declared: false,
            declared_unpublished: Vec::new(),
            filesets: Vec::new(),
            tools: Vec::new(),
            mounts: Vec::new(),
            assume_yes: false,
            quiet: false,
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

    fn fileset_entry(
        path: Option<&str>,
        reference: Option<&str>,
    ) -> lns_artifact::sandbox::FilesetEntry {
        lns_artifact::sandbox::FilesetEntry {
            path: path.map(str::to_string),
            reference: reference.map(str::to_string),
            inline: None,
            mount_path: "/s".into(),
            owner: lns_artifact::sandbox::FilesetOwner::default(),
        }
    }

    #[test]
    fn fileset_display_keeps_a_path_verbatim_and_shortens_a_pinned_digest() {
        assert_eq!(
            fileset_source_display(&fileset_entry(Some("./skills"), None)),
            "./skills"
        );
        let long = format!("reg/skills@sha256:{}", "a".repeat(64));
        assert_eq!(
            fileset_source_display(&fileset_entry(None, Some(&long))),
            format!("reg/skills@sha256:{}…", "a".repeat(12))
        );
    }

    #[test]
    fn fileset_display_shows_an_already_short_ref_verbatim() {
        assert_eq!(
            fileset_source_display(&fileset_entry(None, Some("reg/skills@sha256:abc"))),
            "reg/skills@sha256:abc"
        );
    }

    #[test]
    fn fileset_display_discloses_inline_source_and_owners_without_content() {
        let mut fileset = fileset_entry(None, None);
        fileset.inline = Some(std::collections::BTreeMap::from([(
            "settings.json".to_string(),
            "do-not-print".to_string(),
        )]));

        assert_eq!(fileset_source_display(&fileset), "inline");
        assert_eq!(
            fileset_owner_display(lns_artifact::sandbox::FilesetOwner::Workload),
            "workload"
        );
        assert_eq!(
            fileset_owner_display(lns_artifact::sandbox::FilesetOwner::Root),
            "root"
        );
    }

    #[test]
    fn pulled_fileset_summaries_disclose_inline_source_and_root_owner() {
        let view = lns_ipc::SandboxView {
            reference: "registry.example.test/team/sandbox:latest".into(),
            digest: "sha256:abc".into(),
            image: "registry.example.test/runtime:1".into(),
            workdir: None,
            mounts: Vec::new(),
            ports: Vec::new(),
            filesets: vec![lns_ipc::SandboxFileset {
                path: None,
                reference: None,
                inline: true,
                mount_path: "/etc/agent".into(),
                owner: lns_ipc::SandboxFilesetOwner::Root,
            }],
            connectors: Vec::new(),
            env: Vec::new(),
            credentials: Vec::new(),
            tools: Vec::new(),
            policy_flags: Vec::new(),
        };

        assert_eq!(
            fileset_summaries_from_view(&view),
            vec![(
                "inline".to_string(),
                "/etc/agent".to_string(),
                "root".to_string()
            )]
        );
    }

    #[test]
    fn fileset_display_shortens_a_multibyte_digest_by_chars_not_bytes() {
        let reference = format!("reg/skills@sha256:{}{}", "a".repeat(11), "é".repeat(5));
        assert_eq!(
            fileset_source_display(&fileset_entry(None, Some(&reference))),
            format!("reg/skills@sha256:{}é…", "a".repeat(11)),
            "a crafted pulled fileset digest must truncate by chars, never panic on a byte boundary"
        );
    }

    #[test]
    fn tools_line_lists_declared_tools_and_is_absent_without_them() {
        let mut args = run_args(Some("prism"));
        args.tools = vec!["node@22.11.0".into(), "python@3.12.6".into()];
        let s = format_summary(
            &args,
            &Policy::default(),
            Path::new("./lns-policy.yaml"),
            &PolicySource::FoundInCwd,
        );
        assert!(
            s.contains("Tools:     node@22.11.0, python@3.12.6"),
            "got: {s}"
        );
        let none = summary_with_publish(Vec::new());
        assert!(!none.contains("Tools:"), "got: {none}");
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
        args.mounts = vec![
            lns_ipc::MountSpec::Named(lns_ipc::VolumeMount {
                name: "prism-data".into(),
                target: "/data".into(),
                read_only: false,
            }),
            lns_ipc::MountSpec::Named(lns_ipc::VolumeMount {
                name: "ro-cfg".into(),
                target: "/cfg".into(),
                read_only: true,
            }),
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
    fn summary_lists_a_host_bind_with_its_mode() {
        let mut args = run_args(Some("ubuntu"));
        args.mounts = vec![lns_ipc::MountSpec::Bind(lns_ipc::BindSpec {
            host_source: "/Users/me/proj".into(),
            target: "/work".into(),
            read_only: false,
        })];
        let s = format_summary(
            &args,
            &Policy::default(),
            Path::new("/x/lns-policy.yaml"),
            &PolicySource::FoundInCwd,
        );
        assert!(
            s.contains("Bind:      /Users/me/proj → /work (read-write)"),
            "missing bind line: {s}"
        );
    }

    #[test]
    fn summary_marks_a_read_only_host_bind() {
        let mut args = run_args(Some("ubuntu"));
        args.mounts = vec![lns_ipc::MountSpec::Bind(lns_ipc::BindSpec {
            host_source: "/Users/me/cfg".into(),
            target: "/cfg".into(),
            read_only: true,
        })];
        let s = format_summary(
            &args,
            &Policy::default(),
            Path::new("/x/lns-policy.yaml"),
            &PolicySource::FoundInCwd,
        );
        assert!(
            s.contains("Bind:      /Users/me/cfg → /cfg (read-only)"),
            "missing read-only bind line: {s}"
        );
    }

    #[test]
    fn bind_dispositions_render_kept_and_dropped_secrets() {
        let binds = vec![crate::run::host_bind::ResolvedBind {
            host_source: "/Users/me/proj".into(),
            target: "/work".into(),
            read_only: false,
            kept: vec![".env".into()],
            dropped: vec![".npmrc".into()],
        }];
        let s = format_bind_dispositions(&binds);
        assert!(s.contains(".env: kept (exposed)"), "missing kept line: {s}");
        assert!(s.contains(".npmrc: dropped"), "missing dropped line: {s}");
    }

    #[test]
    fn bind_dispositions_are_empty_without_detected_secrets() {
        let binds = vec![crate::run::host_bind::ResolvedBind {
            host_source: "/Users/me/proj".into(),
            target: "/work".into(),
            read_only: false,
            kept: vec![],
            dropped: vec![],
        }];
        assert!(format_bind_dispositions(&binds).is_empty());
    }

    #[test]
    fn summary_lists_the_requested_workdir() {
        let mut args = run_args(Some("ubuntu"));
        args.workdir = Some("/app".into());
        let s = format_summary(
            &args,
            &Policy::default(),
            Path::new("/x/lns-policy.yaml"),
            &PolicySource::FoundInCwd,
        );
        assert!(s.contains("Workdir:   /app"), "missing workdir line: {s}");
    }

    #[test]
    fn summary_omits_workdir_line_when_unset() {
        let s = format_summary(
            &run_args(Some("ubuntu")),
            &Policy::default(),
            Path::new("/x/lns-policy.yaml"),
            &PolicySource::FoundInCwd,
        );
        assert!(!s.contains("Workdir:"), "no workdir line expected: {s}");
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
    fn no_reference_run_shows_the_local_definition_as_the_image() {
        let args = run_args(None);
        let s = format_summary(
            &args,
            &Policy::default(),
            Path::new("/x/lns-policy.yaml"),
            &PolicySource::FoundInCwd,
        );
        assert!(s.contains("./lns.yaml (resolving…)"), "got: {s}");
        assert!(
            !s.contains("imageless"),
            "the imageless marker is retired: {s}"
        );
    }

    #[test]
    fn a_file_selector_run_shows_the_selected_definition_as_the_image() {
        let mut args = run_args(None);
        args.file = Some(std::path::PathBuf::from("lns.dev.yaml"));
        let s = format_summary(
            &args,
            &Policy::default(),
            Path::new("/x/lns-policy.yaml"),
            &PolicySource::FoundInCwd,
        );
        assert!(s.contains("lns.dev.yaml (resolving…)"), "got: {s}");
        assert!(!s.contains("./lns.yaml"), "got: {s}");
    }

    #[test]
    fn resources_line_falls_back_to_one_vcpu_and_512_mib_when_nothing_is_requested() {
        let s = format_summary(
            &run_args(Some("ubuntu")),
            &Policy::default(),
            Path::new("/x/lns-policy.yaml"),
            &PolicySource::FoundInCwd,
        );
        assert!(s.contains("1 vCPU · 512 MiB"), "resources line wrong: {s}");
    }

    #[test]
    fn resources_line_renders_cpu_and_memory_with_units() {
        let mut args = run_args(Some("ubuntu"));
        args.cpus = Some(4);
        args.mem = Some(2048);
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
    fn flags_line_includes_auto_remove_when_set() {
        let mut args = run_args(Some("ubuntu"));
        args.auto_remove = true;
        let s = format_summary(
            &args,
            &Policy::default(),
            Path::new("/x/lns-policy.yaml"),
            &PolicySource::FoundInCwd,
        );
        assert!(s.contains("Flags:     -i -t --rm"), "flags line wrong: {s}");
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
            binaries: None,
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
        policy.add_rule(RouteRule::allow_host("api.linear.app"));
        policy.add_rule(RouteRule::allow_host("api.example.com"));
        policy.add_rule(RouteRule::allow_host("registry.npmjs.org"));
        policy.add_rule(RouteRule::deny_host("evil.example"));
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
        policy.add_rule(RouteRule::allow_host("api.linear.app"));
        policy.add_rule(RouteRule::deny_host("evil.example"));
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
            "network:\n  egress:\n    http: []\n  defaultVerdict: ask\n",
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
        assert!(
            body.contains("egress:") && body.contains("http: []"),
            "the scaffold must name the table the guest reads, not the deprecated one:\n{body}"
        );
        assert!(!body.contains("allowedRoutes"), "got:\n{body}");
        assert!(!body.contains("defaultTransport"), "got:\n{body}");
        assert!(!body.contains("transport:"), "got:\n{body}");
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
