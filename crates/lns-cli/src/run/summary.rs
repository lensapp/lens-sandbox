use std::fmt::Write as _;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use lns_policy::{Policy, Verdict};

use crate::cli::RunArgs;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicySource {
    Found,
    AutoCreated,
}

pub const DEFAULT_POLICY_FILENAME: &str = "lns-local-mixin.yaml";

/// The project's own mixin is found beside the document and never named, so a definition run from another directory is governed by that directory's decisions rather than the one you typed in.
pub fn policy_path(project: &Path) -> PathBuf {
    project.join(DEFAULT_POLICY_FILENAME)
}

pub fn resolve_policy(project: &Path) -> Result<(PathBuf, PolicySource)> {
    let path = policy_path(project);
    match std::fs::metadata(&path) {
        Ok(md) if md.is_file() => Ok((path, PolicySource::Found)),
        Ok(_) => anyhow::bail!(
            "{} exists but is not a regular file; remove it so the run can record what you decide",
            path.display()
        ),
        Err(_) => {
            lns_policy::Policy::default()
                .save_atomic(&path)
                .with_context(|| format!("creating default policy at {}", path.display()))?;
            Ok((path, PolicySource::AutoCreated))
        }
    }
}

/// The size the run will boot with: an explicit flag outranks what the definition declared, which outranks this machine's config defaults, which outrank the built-in size.
pub fn resolved_size(
    declared: lns_artifact::resources::DeclaredSize,
    args: &RunArgs,
) -> lns_artifact::resources::VmSize {
    let configured = lns_artifact::resources::ConfiguredDefaults {
        cpus: args.cpus_config,
        mem_mib: args.mem_config,
    };
    lns_artifact::resources::resolve_declared(
        declared,
        &lns_artifact::resources::ResourceOverrides {
            cpus: args.cpus,
            mem_mib: args.mem,
        },
        configured.over(lns_artifact::resources::DEFAULT_VM_SIZE),
    )
}

/// How the disclosure names each source: a reference the user typed as a tag reads as `typed -> pinned`, so what they approve names bytes without losing what they asked for.
pub fn mixin_display(resolved: &[String], typed: &[String], pinned: &[String]) -> Vec<String> {
    resolved
        .iter()
        .map(|entry| {
            match typed
                .iter()
                .zip(pinned)
                .find(|(typed, pin)| *pin == entry && *typed != entry)
            {
                Some((typed, _)) => format!("{typed} \u{2192} {entry}"),
                None => entry.clone(),
            }
        })
        .collect()
}

/// What the mixin flags become once the preflight has answered: the run carries the digests it pinned, and the summary shows each beside the reference the user typed. A typed reference goes no further than this, so the boot can only merge bytes the disclosure named.
pub fn adopt_pinned_mixins(
    args: &mut RunArgs,
    resolved: &[String],
    pinned: &[String],
    contributions: &[lns_ipc::SourceContribution],
) {
    args.resolved_mixins = mixin_display(resolved, &args.mixins, pinned);
    args.mixins = pinned.to_vec();
    args.contributions = contributions.to_vec();
}

/// A target the user claimed with `-v` has no author but them, so the declared mount it displaced must not be named as the author of the one that boots.
pub fn drop_overridden_mounts(args: &mut RunArgs, overridden: &[String]) {
    args.contributions
        .retain(|c| c.block != lns_ipc::ContributionBlock::Mount || !overridden.contains(&c.key));
}

pub fn print_run_summary(
    args: &RunArgs,
    size: lns_artifact::resources::VmSize,
    project: &Path,
    writer: &mut impl io::Write,
) -> Result<PathBuf> {
    let (path, source) = resolve_policy(project)?;
    let policy = Policy::load_or_default(&path)
        .with_context(|| format!("loading policy from {}", path.display()))?;
    let body = format_summary(args, size, &policy, &path, &source);
    writer.write_all(body.as_bytes())?;
    Ok(path)
}

/// How an entry names the source that decided it, empty for a run that resolved no mixin; only for a block whose keys are unique, never for egress, where two sources may key alike.
fn attribution(args: &RunArgs, block: lns_ipc::ContributionBlock, key: &str) -> String {
    if args.resolved_mixins.is_empty() {
        return String::new();
    }
    contribution_attribution(&args.contributions, block, key)
}

/// One author is not an attribution question, so a caller rendering an uncomposed document passes no contributions.
pub(crate) fn contribution_attribution(
    contributions: &[lns_ipc::SourceContribution],
    block: lns_ipc::ContributionBlock,
    key: &str,
) -> String {
    contributions
        .iter()
        .find(|c| c.block == block && c.key == key)
        .map(attribution_of)
        .unwrap_or_default()
}

/// The suffix one contribution renders as, taken from the entry rather than found again by key, since a key names one entry only outside egress.
fn attribution_of(found: &lns_ipc::SourceContribution) -> String {
    from_suffix(
        &found.source,
        found
            .displaced
            .iter()
            .map(|d| (d.summary.as_str(), d.source.as_str())),
    )
}

/// The same suffix for a merge the CLI ran itself, so an offline render attributes an entry the way a run summary does.
pub(crate) fn merged_attribution(found: &lns_artifact::merge::Contribution) -> String {
    from_suffix(
        &found.source,
        found
            .displaced
            .iter()
            .map(|d| (d.summary.as_str(), d.source.as_str())),
    )
}

fn from_suffix<'a>(source: &str, displaced: impl Iterator<Item = (&'a str, &'a str)>) -> String {
    let replaced: String = displaced
        .map(|(summary, from)| format!(", replaced {summary} from {}", short_source(from)))
        .collect();
    format!("  [from {}{replaced}]", short_source(source))
}

/// A source as the disclosure names it, with a digest shortened the way every other reference in this summary is.
fn short_source(source: &str) -> String {
    match source.split_once("@sha256:") {
        Some((repo, digest)) if digest.chars().count() > 12 => {
            let short: String = digest.chars().take(12).collect();
            format!("{repo}@sha256:{short}…")
        }
        _ => source.to_string(),
    }
}

/// Where every value in this summary starts, so a label too long to fit still lines its entries up under one another.
const COLUMN: usize = 13;

/// How much of a script's first line the summary shows before it elides; the hash names the exact bytes, and `lns sandbox inspect` prints the body whole.
const SCRIPT_HEAD_CHARS: usize = 44;

/// One `pre-start` script as the approval reads it: what it does, who runs it, and enough of an identity to tell two apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptSummary {
    pub head: String,
    pub user: String,
    pub lines: usize,
    pub digest: String,
    /// What the author wrote to justify the script; §3.1.13 shows it here, the way an egress rule's own description is shown.
    pub description: Option<String>,
}

/// A local definition's own scripts in the shape the view carries, so one summarizer answers for both run paths.
pub fn scripts_of(spec: &lns_artifact::sandbox::SandboxSpec) -> Vec<lns_ipc::SandboxScript> {
    spec.scripts
        .iter()
        .map(|script| lns_ipc::SandboxScript {
            when: script.when.as_str().to_string(),
            run: script.run.clone(),
            user: script.user.clone(),
            description: script.description.clone(),
        })
        .collect()
}

/// Summarize a resolved sandbox's scripts for the approval; the full body stays on the view, since a long script would bury everything else the developer is approving.
pub fn script_summaries(scripts: &[lns_ipc::SandboxScript]) -> Vec<ScriptSummary> {
    scripts
        .iter()
        .map(|script| ScriptSummary {
            head: head_of(&script.run),
            // A script naming no user runs as whatever the workload does, and saying so beats an empty column.
            user: script
                .user
                .clone()
                .unwrap_or_else(|| "workload".to_string()),
            lines: script.run.lines().filter(|l| !l.trim().is_empty()).count(),
            digest: short_digest(&script.run),
            description: script.description.clone(),
        })
        .collect()
}

fn head_of(body: &str) -> String {
    let first = body
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    match first.char_indices().nth(SCRIPT_HEAD_CHARS) {
        Some((cut, _)) => format!("{}…", &first[..cut]),
        None => first.to_string(),
    }
}

fn short_digest(body: &str) -> String {
    let full = <sha2::Sha256 as sha2::Digest>::digest(body.as_bytes());
    let hex: String = full.iter().map(|b| format!("{b:02x}")).collect();
    format!("sha256:{}…", &hex[..12])
}

/// Attribution here is by position, not by key: two scripts can read alike, and neither overrides the other.
fn write_scripts(s: &mut String, args: &RunArgs) {
    let entries: Vec<&lns_ipc::SourceContribution> = args
        .contributions
        .iter()
        .filter(|c| c.block == lns_ipc::ContributionBlock::Script)
        .collect();
    let composed = !args.resolved_mixins.is_empty();
    let heading = "  Scripts:";
    let pad = COLUMN.saturating_sub(heading.len()).max(1);
    let value_column = heading.len() + pad;
    for (index, script) in args.scripts.iter().enumerate() {
        let from = entries
            .get(index)
            .filter(|_| composed)
            .map(|entry| format!("  [from {}]", short_source(&entry.source)))
            .unwrap_or_default();
        let note = script
            .description
            .as_deref()
            .map(|note| format!("  {note}"))
            .unwrap_or_default();
        let line = format!(
            "{}  runs as {}  ({} line{}, {}){from}{note}",
            script.head,
            script.user,
            script.lines,
            if script.lines == 1 { "" } else { "s" },
            script.digest
        );
        if index == 0 {
            writeln!(s, "{heading}{:pad$}{line}", "").unwrap();
        } else {
            writeln!(s, "{:value_column$}{line}", "").unwrap();
        }
    }
}

/// The blocks §1.5 names. A run that resolved no mixin still lists its rules — §4.2 lets a pulled entry outrank this directory only because the merged table is disclosed first — but names no source for them, since one author is not an attribution question.
fn write_disclosed_blocks(s: &mut String, args: &RunArgs) {
    let listed = |block| -> Vec<&lns_ipc::SourceContribution> {
        args.contributions
            .iter()
            .filter(|c| c.block == block)
            .collect()
    };
    let composed = !args.resolved_mixins.is_empty();
    write_block(
        s,
        "Rules",
        &listed(lns_ipc::ContributionBlock::Egress),
        composed,
    );
    write_block(
        s,
        "Credentials",
        &listed(lns_ipc::ContributionBlock::Credential),
        composed,
    );
}

fn write_block(
    s: &mut String,
    label: &str,
    entries: &[&lns_ipc::SourceContribution],
    attributed: bool,
) {
    let Some((first, rest)) = entries.split_first() else {
        return;
    };
    let heading = format!("  {label}:");
    let pad = COLUMN.saturating_sub(heading.len()).max(1);
    let value_column = heading.len() + pad;
    writeln!(s, "{heading}{:pad$}{}", "", entry_line(first, attributed)).unwrap();
    for entry in rest {
        writeln!(s, "{:value_column$}{}", "", entry_line(entry, attributed)).unwrap();
    }
}

/// One entry as the disclosure reads it: what it decided, the source that decided it, and whatever the entry says about itself.
fn entry_line(entry: &lns_ipc::SourceContribution, attributed: bool) -> String {
    let attribution = if attributed {
        attribution_of(entry)
    } else {
        String::new()
    };
    let note = entry
        .note
        .as_deref()
        .map(|note| format!("  {note}"))
        .unwrap_or_default();
    format!("{}{attribution}{note}", entry.key)
}

pub fn format_summary(
    args: &RunArgs,
    size: lns_artifact::resources::VmSize,
    policy: &Policy,
    policy_path: &Path,
    source: &PolicySource,
) -> String {
    let mut s = String::with_capacity(512);
    s.push_str("lns run\n");
    writeln!(s, "  Image:     {}", image_line(args)).unwrap();
    let (volumes, binds) = crate::cli::split_mounts(&args.mounts);
    let mount = lns_ipc::ContributionBlock::Mount;
    for vol in &volumes {
        let from = attribution(args, mount, &vol.target);
        writeln!(s, "  Volume:    {}{from}", volume_line(vol)).unwrap();
    }
    for bind in &binds {
        let from = attribution(args, mount, &bind.target);
        writeln!(s, "  Bind:      {}{from}", bind_line(bind)).unwrap();
    }
    for fileset in &args.filesets {
        writeln!(
            s,
            "  Fileset:   {} -> {} (owner: {}){}",
            fileset.source,
            fileset.guest_path,
            fileset.owner,
            attribution(args, mount, &fileset.guest_path)
        )
        .unwrap();
    }
    if !args.tools.is_empty() {
        let tools: Vec<String> = args
            .tools
            .iter()
            .map(|tool| {
                format!(
                    "{tool}{}",
                    attribution(
                        args,
                        lns_ipc::ContributionBlock::Tool,
                        lns_artifact::merge::tool_name(tool)
                    )
                )
            })
            .collect();
        writeln!(s, "  Tools:     {}", tools.join(", ")).unwrap();
    }
    write_scripts(&mut s, args);
    if !args.resolved_mixins.is_empty() {
        writeln!(s, "  Mixins:    {}", args.resolved_mixins.join(", ")).unwrap();
    }
    write_disclosed_blocks(&mut s, args);
    if let Some(dir) = &args.workdir {
        writeln!(s, "  Workdir:   {dir}").unwrap();
    }
    writeln!(
        s,
        "  Resources: {} vCPU · {} MiB · {} disk",
        size.cpus,
        size.mem_mib,
        crate::volume::format_size(size.disk_bytes)
    )
    .unwrap();
    writeln!(s, "  Flags:     {}", flags_line(args)).unwrap();
    writeln!(s, "  Ports:     {}", ports_line(args)).unwrap();
    s.push_str("  Policy:\n");
    writeln!(s, "    file: {}", policy_path.display()).unwrap();
    writeln!(s, "    unmatched destinations: {}", unmatched_line(policy)).unwrap();
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

/// How the summary names one fileset's source, so the operator sees which of their files a sandbox reads.
pub fn fileset_source_display(fileset: &lns_artifact::sandbox::FilesetEntry) -> String {
    fileset_display(
        fileset.path.as_deref(),
        fileset.inline.is_some(),
        fileset.host_path.as_deref(),
        fileset.optional,
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
        fileset.inline,
        fileset.host_path.as_deref(),
        fileset.optional,
    )
}

pub fn fileset_view_owner_display(owner: lns_ipc::SandboxFilesetOwner) -> &'static str {
    match owner {
        lns_ipc::SandboxFilesetOwner::Workload => "workload",
        lns_ipc::SandboxFilesetOwner::Root => "root",
    }
}

/// One disclosed fileset. `host_path` separates a file read off the machine running the sandbox from the files the document itself ships, and carries the path a pulled sandbox must be granted before it is read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilesetSummary {
    pub source: String,
    pub guest_path: String,
    pub owner: String,
    pub host_path: Option<String>,
    pub optional: bool,
    /// The mixin that contributed this entry, absent when the document being run declared it itself — a merged document cannot otherwise say which of its filesets came from a registry.
    pub declared_by: Option<String>,
}

impl FilesetSummary {
    pub fn from_host(&self) -> bool {
        self.host_path.is_some()
    }
}

/// Which registry artifact contributed the entry at this guest path, absent when something on the developer's own machine did — a merged document reads as one file, so only the merge's own attribution can tell a pulled mixin's entry from one the developer wrote.
///
/// The preflight digest-pins every reference it fetches, so being pinned is what separates a registry source from the root document, this directory's decisions file, and a directory mixin the developer pointed at — all three of which are their own consent.
pub fn fileset_declared_by(
    contributions: &[lns_ipc::SourceContribution],
    guest_path: &str,
) -> Option<String> {
    contributions
        .iter()
        .find(|c| c.block == lns_ipc::ContributionBlock::Mount && c.key == guest_path)
        .map(|c| c.source.clone())
        .filter(|source| lns_artifact::spec::is_digest_pinned_image(source))
}

/// A pulled sandbox disclosed the same way at launch as a local one: its preflight view's filesets become summary lines.
pub fn fileset_summaries_from_view(view: &lns_ipc::SandboxView) -> Vec<FilesetSummary> {
    view.filesets
        .iter()
        .map(|fileset| FilesetSummary {
            source: fileset_view_source_display(fileset),
            guest_path: fileset.guest_path.clone(),
            owner: fileset_view_owner_display(fileset.owner).to_string(),
            host_path: fileset.host_path.clone(),
            optional: fileset.optional,
            declared_by: None,
        })
        .collect()
}

/// The declared tools a published sandbox discloses at launch; the local-definition case reads them off the definition instead.
pub fn tools_from_view(view: &lns_ipc::SandboxView) -> Vec<String> {
    view.tools.clone()
}

/// The summary shows a path verbatim, names embedded files as inline, and names a host file as one — the three sources a fileset entry can have.
fn fileset_display(
    path: Option<&str>,
    inline: bool,
    host_path: Option<&str>,
    optional: bool,
) -> String {
    if let Some(host_path) = host_path {
        let suffix = if optional { " (optional)" } else { "" };
        return format!("host file {host_path}{suffix}");
    }
    if inline {
        return "inline".to_string();
    }
    path.unwrap_or_default().to_string()
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
            let from = attribution(
                args,
                lns_ipc::ContributionBlock::Port,
                &p.container_port.to_string(),
            );
            if p.host_ip.is_loopback() {
                format!("{mapping}{from}")
            } else {
                format!("{mapping} (exposed beyond this machine){from}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn unmatched_line(policy: &Policy) -> &'static str {
    if policy.network.is_closed() {
        "denied by the catch-all rule"
    } else {
        "ask"
    }
}

fn rules_line(policy: &Policy) -> String {
    let egress = &policy.network.egress;
    if egress.http.is_empty() && egress.tcp.is_empty() {
        return "none defined; anything else asks".to_string();
    }
    let (allows, denies) = tally(egress.http.iter().map(|r| r.verdict));
    let raw = raw_counts(&egress.tcp);
    let tail = if policy.network.is_closed() {
        "anything else denied"
    } else {
        "anything else asks"
    };
    format!("{allows} allow, {denies} deny{raw}, {tail}")
}

/// Raw splices counted apart from the routes, since nothing inspects one and folded into the route count a splice would read as one more gated host. Empty when the table is, so a policy with no raw rules carries no zeros.
fn raw_counts(tcp: &[lns_policy::TcpEgressRule]) -> String {
    if tcp.is_empty() {
        return String::new();
    }
    let (allows, denies) = tally(tcp.iter().map(|r| r.verdict));
    format!(", {allows} raw allow, {denies} raw deny")
}

fn tally(verdicts: impl Iterator<Item = Verdict>) -> (u32, u32) {
    verdicts.fold((0u32, 0u32), |(a, d), verdict| match verdict {
        Verdict::Allow => (a + 1, d),
        Verdict::Deny => (a, d + 1),
    })
}

fn source_line(source: &PolicySource) -> String {
    match source {
        PolicySource::Found => "found in the project directory".to_string(),
        PolicySource::AutoCreated => {
            "auto-created (no policy in the project directory)".to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lns_artifact::resources::{DEFAULT_VM_SIZE, VmSize};
    use lns_policy::{RouteRule, TcpEgressRule};

    /// Most lines are indifferent to the size, so they read the built-in default; the resources line has its own tests.
    fn summary_of(
        args: &RunArgs,
        policy: &Policy,
        policy_path: &Path,
        source: &PolicySource,
    ) -> String {
        format_summary(args, DEFAULT_VM_SIZE, policy, policy_path, source)
    }

    fn spec_with_scripts(entries: &str) -> lns_artifact::sandbox::SandboxSpec {
        serde_json::from_str(&format!(r#"{{"image":"x:1","scripts":[{entries}]}}"#))
            .expect("a valid fixture")
    }

    #[test]
    fn a_local_definitions_scripts_summarize_the_way_a_pulled_ones_do() {
        let spec = spec_with_scripts(
            r#"{"when":"pre-start","user":"root","run":"apt-get install -y psql","description":"the psql the prompts assume"}"#,
        );
        let summarized = script_summaries(&scripts_of(&spec));
        assert_eq!(
            summarized,
            vec![ScriptSummary {
                head: "apt-get install -y psql".into(),
                user: "root".into(),
                lines: 1,
                digest: summarized[0].digest.clone(),
                description: Some("the psql the prompts assume".into()),
            }],
            "one summarizer answers for both run paths, so a local run discloses exactly what a pulled one does"
        );
    }

    #[test]
    fn a_script_too_long_to_show_is_elided_rather_than_wrapped() {
        let body = format!("echo {}", "x".repeat(200));
        let spec = spec_with_scripts(&format!(
            r#"{{"when":"pre-start","run":{}}}"#,
            serde_json::to_string(&body).expect("a str encodes")
        ));
        let summarized = script_summaries(&scripts_of(&spec));
        let head = summarized[0].head.clone();
        assert!(
            head.ends_with('…') && head.chars().count() < 60,
            "a long first line would push the rest of the approval off the screen; got {head:?}"
        );
        assert_eq!(
            summarized[0].user, "workload",
            "a script naming no user runs as the workload does, and saying so beats an empty column"
        );
    }

    #[test]
    fn two_scripts_with_the_same_body_summarize_alike_and_are_both_listed() {
        let spec = spec_with_scripts(
            r#"{"when":"pre-start","run":"npm ci"},{"when":"pre-start","run":"npm ci"}"#,
        );
        let summarized = script_summaries(&scripts_of(&spec));
        assert_eq!(summarized.len(), 2);
        assert_eq!(
            summarized[0], summarized[1],
            "the block has no key, so identical entries are two real scripts rather than one duplicated"
        );
    }

    #[test]
    fn the_summary_lists_each_script_in_run_order_with_its_user_and_hash() {
        let mut args = run_args(Some("ghcr.io/acme/reviewer:1"));
        let spec = spec_with_scripts(
            r#"{"when":"pre-start","user":"root","run":"apt-get install -y psql"},{"when":"pre-start","run":"npm ci"}"#,
        );
        args.scripts = script_summaries(&scripts_of(&spec));
        let mut rendered = String::new();
        write_scripts(&mut rendered, &args);
        let lines: Vec<&str> = rendered.lines().collect();
        assert_eq!(lines.len(), 2, "got:\n{rendered}");
        let first = lines[0].to_string();
        assert!(
            first.contains("Scripts:")
                && first.contains("apt-get install -y psql")
                && first.contains("runs as root")
                && first.contains("sha256:"),
            "a pulled script asking for root is why this line exists, so it names the body, the user and the exact bytes; got: {first}"
        );
        let second = lines[1].to_string();
        assert!(
            second.contains("npm ci") && second.contains("runs as workload"),
            "the second script lines up under the first without repeating the label; got: {second}"
        );
    }

    #[test]
    fn a_script_a_mixin_contributed_names_that_mixin() {
        let mut args = run_args(Some("ghcr.io/acme/reviewer:1"));
        args.scripts = script_summaries(&scripts_of(&spec_with_scripts(
            r#"{"when":"pre-start","run":"apt-get install -y psql"}"#,
        )));
        args.resolved_mixins = vec!["ghcr.io/acme/postgres-tools@sha256:c41e8b7d20a9".into()];
        args.contributions = vec![lns_ipc::SourceContribution {
            block: lns_ipc::ContributionBlock::Script,
            key: "apt-get install -y psql".into(),
            source: "ghcr.io/acme/postgres-tools@sha256:c41e8b7d20a9".into(),
            note: None,
            displaced: Vec::new(),
        }];
        let mut rendered = String::new();
        write_scripts(&mut rendered, &args);
        assert!(
            rendered.contains("postgres-tools"),
            "a script nobody can trace to a document is one nobody can refuse; got:\n{rendered}"
        );
    }

    #[test]
    fn a_run_that_layered_on_nothing_names_no_source_for_its_own_scripts() {
        let mut args = run_args(Some("ghcr.io/acme/reviewer:1"));
        args.scripts = script_summaries(&scripts_of(&spec_with_scripts(
            r#"{"when":"pre-start","run":"npm ci"}"#,
        )));
        let mut rendered = String::new();
        write_scripts(&mut rendered, &args);
        assert!(
            !rendered.contains("[from"),
            "one author is not an attribution question, exactly as the other disclosed blocks treat it; got:\n{rendered}"
        );
    }

    #[test]
    fn a_scripts_own_description_is_what_the_approval_reads() {
        let mut args = run_args(Some("ghcr.io/acme/reviewer:1"));
        args.scripts = script_summaries(&scripts_of(&spec_with_scripts(
            r#"{"when":"pre-start","user":"root","run":"apt-get install -y psql","description":"the psql the prompts assume"}"#,
        )));
        let mut rendered = String::new();
        write_scripts(&mut rendered, &args);
        assert!(
            rendered.contains("the psql the prompts assume"),
            "§3.1.13 shows a script's description here the way an egress rule's is shown — it is the author's own justification, on the one screen the developer approves; got:\n{rendered}"
        );
    }

    #[test]
    fn the_digest_identifies_the_body_rather_than_the_line_it_starts_with() {
        let one = script_summaries(&scripts_of(&spec_with_scripts(
            r#"{"when":"pre-start","run":"apt-get update"}"#,
        )));
        let two = script_summaries(&scripts_of(&spec_with_scripts(
            r#"{"when":"pre-start","run":"apt-get update\nrm -rf /"}"#,
        )));
        assert_eq!(
            one[0].head, two[0].head,
            "the fixture is only meaningful while both share a first line"
        );
        assert_ne!(
            one[0].digest, two[0].digest,
            "the hash is what tells an approver that the elided remainder is the same bytes they saw last time, so it has to cover the whole body rather than the line on screen"
        );
    }

    #[test]
    fn a_run_declaring_no_scripts_says_nothing_about_them() {
        let args = run_args(Some("ghcr.io/acme/reviewer:1"));
        let mut rendered = String::new();
        write_scripts(&mut rendered, &args);
        assert!(
            rendered.is_empty(),
            "a heading with nothing under it is noise in the one summary the developer has to read; got:\n{rendered}"
        );
    }

    fn run_args(image: Option<&str>) -> RunArgs {
        RunArgs {
            scripts: Vec::new(),
            mixins: Vec::new(),
            resolved_mixins: Vec::new(),
            contributions: Vec::new(),
            image: image.map(str::to_string),
            file: None,
            name: None,
            registry: None,
            cpus: None,
            mem: None,
            cpus_config: None,
            mem_config: None,
            user: None,
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
        summary_of(
            &args,
            &Policy::default(),
            Path::new("./lns-local-mixin.yaml"),
            &PolicySource::Found,
        )
    }

    fn fileset_entry(path: Option<&str>) -> lns_artifact::sandbox::FilesetEntry {
        lns_artifact::sandbox::FilesetEntry {
            path: path.map(str::to_string),
            inline: None,
            host_path: None,
            guest_path: "/s".into(),
            owner: lns_artifact::sandbox::FilesetOwner::default(),
            optional: false,
        }
    }

    #[test]
    fn fileset_display_keeps_a_path_verbatim() {
        assert_eq!(
            fileset_source_display(&fileset_entry(Some("./skills"))),
            "./skills",
            "a published entry keeps its path (docs/sandbox-spec.md §6), so the disclosure names the directory the author shipped"
        );
    }

    #[test]
    fn fileset_display_names_a_host_file_and_marks_it_optional() {
        let mut fileset = fileset_entry(None);
        fileset.host_path = Some("~/.gitconfig".into());

        assert_eq!(
            fileset_source_display(&fileset),
            "host file ~/.gitconfig",
            "the tilde stays verbatim so the operator recognizes the file the sandbox asked for"
        );

        fileset.optional = true;
        assert_eq!(
            fileset_source_display(&fileset),
            "host file ~/.gitconfig (optional)"
        );
    }

    #[test]
    fn fileset_display_discloses_inline_source_and_owners_without_content() {
        let mut fileset = fileset_entry(None);
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
            mixins: Vec::new(),
            pinned_mixins: Vec::new(),
            contributions: Vec::new(),
            reference: "registry.example.test/team/sandbox:latest".into(),
            digest: "sha256:abc".into(),
            image: "registry.example.test/runtime:1".into(),
            workdir: None,
            user: None,
            mounts: Vec::new(),
            ports: Vec::new(),
            filesets: vec![lns_ipc::SandboxFileset {
                path: None,
                inline: true,
                host_path: None,
                guest_path: "/etc/agent".into(),
                owner: lns_ipc::SandboxFilesetOwner::Root,
                optional: false,
            }],
            env: Vec::new(),
            credentials: Vec::new(),
            tools: Vec::new(),
            scripts: Vec::new(),
            policy_flags: Vec::new(),
            cpus: None,
            mem_mib: None,
            disk_bytes: None,
        };

        assert_eq!(
            fileset_summaries_from_view(&view),
            vec![FilesetSummary {
                source: "inline".to_string(),
                guest_path: "/etc/agent".to_string(),
                owner: "root".to_string(),
                host_path: None,
                optional: false,
                declared_by: None,
            }]
        );
    }

    #[test]
    fn a_digest_the_user_typed_reads_once_rather_than_pointing_at_itself() {
        let pinned = format!("ghcr.io/acme/obs@sha256:{}", "c".repeat(64));
        assert_eq!(
            mixin_display(
                std::slice::from_ref(&pinned),
                std::slice::from_ref(&pinned),
                std::slice::from_ref(&pinned),
            ),
            [pinned],
            "a user who typed the digest already sees the bytes, so repeating it either side of an arrow says nothing"
        );
    }

    #[test]
    fn a_tag_the_user_named_reads_as_the_tag_and_the_digest_it_pinned_to() {
        let pinned = format!("ghcr.io/acme/obs@sha256:{}", "c".repeat(64));
        let declared = format!("ghcr.io/acme/base@sha256:{}", "a".repeat(64));
        let shown = mixin_display(
            &[declared.clone(), pinned.clone()],
            &["obs-tools:2".to_string()],
            std::slice::from_ref(&pinned),
        );
        assert_eq!(
            shown,
            [declared, format!("obs-tools:2 \u{2192} {pinned}")],
            "the user approves bytes, but a line that dropped the tag they typed would not tell them which flag produced it"
        );
    }

    #[test]
    fn a_tag_the_user_typed_never_travels_further_than_the_preflight() {
        let pinned = format!("ghcr.io/acme/obs@sha256:{}", "c".repeat(64));
        let mut args = run_args(Some("prism"));
        args.mixins = vec!["obs-tools:2".to_string()];
        adopt_pinned_mixins(
            &mut args,
            std::slice::from_ref(&pinned),
            std::slice::from_ref(&pinned),
            &[],
        );
        assert_eq!(
            args.mixins,
            std::slice::from_ref(&pinned),
            "the run merges the digest the preflight showed; a tag reaching the service could move between the disclosure and the merge, and the service refuses one outright"
        );
        assert_eq!(
            args.resolved_mixins,
            [format!("obs-tools:2 \u{2192} {pinned}")],
            "the tag survives in the disclosure alone, so the user still sees which flag produced this digest"
        );
    }

    #[test]
    fn a_run_that_named_no_mixin_carries_none_after_the_preflight() {
        let mut args = run_args(Some("prism"));
        adopt_pinned_mixins(&mut args, &[], &[], &[]);
        assert!(args.mixins.is_empty());
        assert!(
            args.resolved_mixins.is_empty(),
            "a sandbox that resolved nothing must not read as a composed one"
        );
    }

    #[test]
    fn a_mixin_only_the_document_declared_is_disclosed_without_being_carried() {
        let declared = format!("ghcr.io/acme/base@sha256:{}", "a".repeat(64));
        let mut args = run_args(Some("prism"));
        adopt_pinned_mixins(&mut args, std::slice::from_ref(&declared), &[], &[]);
        assert_eq!(
            args.resolved_mixins,
            [declared],
            "the document's own mixins are what the summary names"
        );
        assert!(
            args.mixins.is_empty(),
            "the service resolves a document's own mixins from the document, so sending them back would merge each source twice"
        );
    }

    #[test]
    fn the_mixins_line_names_what_a_composed_sandbox_resolved_into() {
        let mut args = run_args(Some("prism"));
        args.resolved_mixins = vec!["ghcr.io/acme/postgres-tools@sha256:c41e8b7d".into()];
        let s = summary_of(
            &args,
            &Policy::default(),
            Path::new("./lns-local-mixin.yaml"),
            &PolicySource::Found,
        );
        assert!(
            s.contains("Mixins:    ghcr.io/acme/postgres-tools@sha256:c41e8b7d"),
            "the resolved document declares no mixins of its own, so without this line a composed sandbox reads as an authored one and a tool the user did not expect has nowhere to be traced to; got: {s}"
        );
        let authored = summary_of(
            &run_args(Some("prism")),
            &Policy::default(),
            Path::new("./lns-local-mixin.yaml"),
            &PolicySource::Found,
        );
        assert!(!authored.contains("Mixins:"), "got: {authored}");
    }

    fn contributed(
        block: lns_ipc::ContributionBlock,
        key: &str,
        source: &str,
        displaced: &[(&str, &str)],
    ) -> lns_ipc::SourceContribution {
        lns_ipc::SourceContribution {
            block,
            key: key.to_string(),
            source: source.to_string(),
            note: None,
            displaced: displaced
                .iter()
                .map(|(source, summary)| lns_ipc::DisplacedEntry {
                    source: (*source).to_string(),
                    summary: (*summary).to_string(),
                })
                .collect(),
        }
    }

    fn composed(args: &mut RunArgs, contributions: Vec<lns_ipc::SourceContribution>) {
        args.resolved_mixins = vec![format!("ghcr.io/acme/obs@sha256:{}", "c".repeat(64))];
        args.contributions = contributions;
    }

    fn pinned_mixin() -> String {
        format!("ghcr.io/someone/toolkit@sha256:{}", "a".repeat(64))
    }

    #[test]
    fn a_registry_mixins_fileset_names_the_mixin_that_declared_it() {
        let mixin = pinned_mixin();
        let contributions = vec![contributed(
            lns_ipc::ContributionBlock::Mount,
            "/home/agent/.gitconfig",
            &mixin,
            &[],
        )];
        assert_eq!(
            fileset_declared_by(&contributions, "/home/agent/.gitconfig"),
            Some(mixin),
            "a merged document reads as one file, so only the attribution can say a mixin declared this"
        );
    }

    #[test]
    fn the_running_documents_own_fileset_owes_no_decision() {
        let contributions = vec![contributed(
            lns_ipc::ContributionBlock::Mount,
            "/home/agent/.gitconfig",
            lns_artifact::merge::ROOT_LABEL,
            &[],
        )];
        assert_eq!(
            fileset_declared_by(&contributions, "/home/agent/.gitconfig"),
            None
        );
    }

    #[test]
    fn this_directorys_own_decisions_file_owes_no_decision() {
        let contributions = vec![contributed(
            lns_ipc::ContributionBlock::Mount,
            "/home/agent/.gitconfig",
            "lns-local-mixin.yaml",
            &[],
        )];
        assert_eq!(
            fileset_declared_by(&contributions, "/home/agent/.gitconfig"),
            None,
            "the local mixin is the developer's own file; asking them to grant it would refuse a non-interactive run over their own decision"
        );
    }

    #[test]
    fn a_directory_mixin_the_developer_named_owes_no_decision() {
        let contributions = vec![contributed(
            lns_ipc::ContributionBlock::Mount,
            "/home/agent/.gitconfig",
            "/work/mixins/pg/lns.yaml",
            &[],
        )];
        assert_eq!(
            fileset_declared_by(&contributions, "/home/agent/.gitconfig"),
            None,
            "a directory the developer pointed at is on their machine and is their own consent, exactly as their lns.yaml is"
        );
    }

    #[test]
    fn a_contribution_of_another_block_never_supplies_a_decider() {
        let contributions = vec![contributed(
            lns_ipc::ContributionBlock::Tool,
            "/home/agent/.gitconfig",
            &pinned_mixin(),
            &[],
        )];
        assert_eq!(
            fileset_declared_by(&contributions, "/home/agent/.gitconfig"),
            None,
            "a tool keyed alike must not decide a fileset"
        );
    }

    #[test]
    fn a_guest_path_no_contribution_claims_owes_no_decision() {
        let contributions = vec![contributed(
            lns_ipc::ContributionBlock::Mount,
            "/home/agent/.vimrc",
            &pinned_mixin(),
            &[],
        )];
        assert_eq!(
            fileset_declared_by(&contributions, "/home/agent/.gitconfig"),
            None
        );
    }

    #[test]
    fn a_tool_a_mixin_replaced_names_the_mixin_and_the_version_it_replaced() {
        let mut args = run_args(Some("prism"));
        args.tools = vec!["node@22".into()];
        composed(
            &mut args,
            vec![contributed(
                lns_ipc::ContributionBlock::Tool,
                "node",
                &format!("ghcr.io/acme/obs@sha256:{}", "c".repeat(64)),
                &[("the sandbox", "node@20")],
            )],
        );
        let s = summary_of(
            &args,
            &Policy::default(),
            Path::new("./lns-local-mixin.yaml"),
            &PolicySource::Found,
        );
        assert!(
            s.contains("node@22  [from ghcr.io/acme/obs@sha256:cccccccccccc…, replaced node@20 from the sandbox]"),
            "a developer reading `node@22` has to be able to see that a mixin put it there over the version their own document asked for; got: {s}"
        );
    }

    #[test]
    fn a_composed_run_discloses_the_credentials_the_merge_produced() {
        // §1.5: a credential the sandbox never asked for has to be traceable to the source that asked.
        let mut args = run_args(Some("prism"));
        let obs = format!("ghcr.io/acme/obs@sha256:{}", "c".repeat(64));
        composed(
            &mut args,
            vec![contributed(
                lns_ipc::ContributionBlock::Credential,
                "SOME_TOKEN",
                &obs,
                &[],
            )],
        );
        let s = summary_of(
            &args,
            &Policy::default(),
            Path::new("./lns-local-mixin.yaml"),
            &PolicySource::Found,
        );
        assert!(
            s.contains("Credentials: SOME_TOKEN  [from ghcr.io/acme/obs@sha256:cccccccccccc…]"),
            "got: {s}"
        );
    }

    #[test]
    fn a_second_credential_lines_up_under_the_first() {
        let mut args = run_args(Some("prism"));
        let obs = format!("ghcr.io/acme/obs@sha256:{}", "c".repeat(64));
        composed(
            &mut args,
            vec![
                contributed(
                    lns_ipc::ContributionBlock::Credential,
                    "SOME_TOKEN",
                    &obs,
                    &[],
                ),
                contributed(
                    lns_ipc::ContributionBlock::Credential,
                    "SOME_OTHER_TOKEN",
                    &obs,
                    &[],
                ),
            ],
        );
        let s = summary_of(
            &args,
            &Policy::default(),
            Path::new("./lns-local-mixin.yaml"),
            &PolicySource::Found,
        );
        let first = s
            .lines()
            .find(|l| l.contains("SOME_TOKEN"))
            .expect("the first credential renders");
        let second = s
            .lines()
            .find(|l| l.contains("SOME_OTHER_TOKEN"))
            .expect("the second credential renders");
        assert_eq!(
            first.find("SOME_TOKEN"),
            second.find("SOME_OTHER_TOKEN"),
            "a label wider than the column still has to line its own entries up, or the block stops reading as one list"
        );
    }

    #[test]
    fn a_second_rule_lines_up_under_the_first_without_repeating_the_label() {
        let mut args = run_args(Some("prism"));
        let obs = format!("ghcr.io/acme/obs@sha256:{}", "c".repeat(64));
        composed(
            &mut args,
            vec![
                contributed(
                    lns_ipc::ContributionBlock::Egress,
                    "api.vendor.example",
                    &obs,
                    &[],
                ),
                contributed(
                    lns_ipc::ContributionBlock::Egress,
                    "proxy.vendor.example",
                    "the sandbox",
                    &[],
                ),
            ],
        );
        let s = summary_of(
            &args,
            &Policy::default(),
            Path::new("./lns-local-mixin.yaml"),
            &PolicySource::Found,
        );
        assert!(
            s.contains("\n             proxy.vendor.example  [from the sandbox]\n"),
            "a merged table is read as a table, so the second entry has to sit under the first rather than restate the label; got: {s}"
        );
    }

    #[test]
    fn a_mount_the_user_added_on_the_command_line_is_attributed_to_nobody() {
        let mut args = run_args(Some("prism"));
        args.mounts = vec![lns_ipc::MountSpec::Named(lns_ipc::VolumeMount {
            name: "scratch".into(),
            target: "/scratch".into(),
            read_only: false,
            size_bytes: None,
        })];
        composed(&mut args, Vec::new());
        let s = summary_of(
            &args,
            &Policy::default(),
            Path::new("./lns-local-mixin.yaml"),
            &PolicySource::Found,
        );
        assert!(
            s.contains("Volume:    scratch \u{2192} /scratch\n"),
            "the user typed this one, so naming a source for it would invent an author; got: {s}"
        );
    }

    #[test]
    fn a_mount_the_user_overrode_loses_the_declared_sources_attribution() {
        let mut args = run_args(Some("prism"));
        let obs = format!("ghcr.io/acme/obs@sha256:{}", "c".repeat(64));
        args.mounts = vec![lns_ipc::MountSpec::Named(lns_ipc::VolumeMount {
            name: "mine".into(),
            target: "/scratch".into(),
            read_only: false,
            size_bytes: None,
        })];
        composed(
            &mut args,
            vec![contributed(
                lns_ipc::ContributionBlock::Mount,
                "/scratch",
                &obs,
                &[],
            )],
        );
        drop_overridden_mounts(&mut args, &["/scratch".to_string()]);
        let s = summary_of(
            &args,
            &Policy::default(),
            Path::new("./lns-local-mixin.yaml"),
            &PolicySource::Found,
        );
        assert!(
            s.contains("Volume:    mine \u{2192} /scratch\n"),
            "the mixin's mount is not what boots, so naming it as the author of the user's own mount inverts who decided this; got: {s}"
        );
    }

    #[test]
    fn two_sources_declaring_one_rule_each_keep_their_own_source() {
        let mut args = run_args(Some("prism"));
        let obs = format!("ghcr.io/acme/obs@sha256:{}", "c".repeat(64));
        composed(
            &mut args,
            vec![
                contributed(
                    lns_ipc::ContributionBlock::Egress,
                    "allow api.vendor.example",
                    &obs,
                    &[],
                ),
                contributed(
                    lns_ipc::ContributionBlock::Egress,
                    "allow api.vendor.example",
                    "the sandbox",
                    &[],
                ),
            ],
        );
        let s = summary_of(
            &args,
            &Policy::default(),
            Path::new("./lns-local-mixin.yaml"),
            &PolicySource::Found,
        );
        assert!(
            s.contains(
                "Rules:     allow api.vendor.example  [from ghcr.io/acme/obs@sha256:cccccccccccc…]"
            ),
            "got: {s}"
        );
        assert!(
            s.contains("\n             allow api.vendor.example  [from the sandbox]\n"),
            "egress is the one block two sources can key alike, so looking each line up by its key would name the first source twice and hide that the sandbox said this too; got: {s}"
        );
    }

    #[test]
    fn a_port_a_mixin_publishes_names_the_mixin() {
        let mut args = run_args(Some("prism"));
        let obs = format!("ghcr.io/acme/obs@sha256:{}", "c".repeat(64));
        args.publish = vec![lns_ipc::PortPublish {
            host_ip: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            host_port: 8080,
            container_port: 8080,
            protocol: lns_ipc::Protocol::Tcp,
        }];
        composed(
            &mut args,
            vec![contributed(
                lns_ipc::ContributionBlock::Port,
                "8080",
                &obs,
                &[],
            )],
        );
        let s = summary_of(
            &args,
            &Policy::default(),
            Path::new("./lns-local-mixin.yaml"),
            &PolicySource::Found,
        );
        assert!(
            s.contains("[from ghcr.io/acme/obs@sha256:cccccccccccc…]"),
            "a mixin opening a host socket is a thing the developer is approving, so it has to name who asked; got: {s}"
        );
    }

    #[test]
    fn a_source_whose_digest_is_not_ascii_truncates_on_a_character() {
        let mut args = run_args(Some("prism"));
        composed(
            &mut args,
            vec![contributed(
                lns_ipc::ContributionBlock::Tool,
                "node",
                "some/dir@sha256:a\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}",
                &[],
            )],
        );
        args.tools = vec!["node@22".into()];
        let s = summary_of(
            &args,
            &Policy::default(),
            Path::new("./lns-local-mixin.yaml"),
            &PolicySource::Found,
        );
        assert!(
            s.contains("some/dir@sha256:a\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{2026}"),
            "a directory the user named is theirs to spell, so shortening it by bytes would panic on a run that is otherwise fine; got: {s}"
        );
    }

    #[test]
    fn an_uncomposed_run_prints_exactly_what_it_printed_before_attribution_existed() {
        let mut args = run_args(Some("prism"));
        args.tools = vec!["node@22".into()];
        let s = summary_of(
            &args,
            &Policy::default(),
            Path::new("./lns-local-mixin.yaml"),
            &PolicySource::Found,
        );
        assert!(s.contains("Tools:     node@22\n"), "got: {s}");
        assert!(
            !s.contains("[from") && !s.contains("Rules:") && !s.contains("Credentials:"),
            "a run that resolved no mixin has one author, so attributing every line to them is noise; got: {s}"
        );
    }

    #[test]
    fn a_run_that_resolved_no_mixin_still_names_the_rules_the_sandbox_ships() {
        let mut args = run_args(Some("prism"));
        args.contributions = ["allow db.vendor.example:5432", "allow api.vendor.example"]
            .into_iter()
            .map(|key| lns_ipc::SourceContribution {
                block: lns_ipc::ContributionBlock::Egress,
                key: key.into(),
                source: "the sandbox".into(),
                note: None,
                displaced: Vec::new(),
            })
            .collect();
        let mut closed = Policy::default();
        closed.add_rule(RouteRule::deny_host("*"));

        let s = summary_of(
            &args,
            &closed,
            Path::new("./lns-policy.yaml"),
            &PolicySource::Found,
        );

        assert!(
            s.contains("Rules:")
                && s.contains("allow db.vendor.example:5432")
                && s.contains("allow api.vendor.example"),
            "§4.2 lets a pulled rule outrank this directory only because the merged table is disclosed first, and a raw allow this file cannot deny is exactly the entry that has to be named; got: {s}"
        );
        assert!(
            !s.contains("[from"),
            "one author is not an attribution question, so naming them on every line is noise; got: {s}"
        );
    }

    #[test]
    fn tools_line_lists_declared_tools_and_is_absent_without_them() {
        let mut args = run_args(Some("prism"));
        args.tools = vec!["node@22.11.0".into(), "python@3.12.6".into()];
        let s = summary_of(
            &args,
            &Policy::default(),
            Path::new("./lns-local-mixin.yaml"),
            &PolicySource::Found,
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
        let s = summary_of(
            &args,
            &Policy::default(),
            Path::new("/home/dev/my-app/lns-local-mixin.yaml"),
            &PolicySource::Found,
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
                size_bytes: None,
            }),
            lns_ipc::MountSpec::Named(lns_ipc::VolumeMount {
                name: "ro-cfg".into(),
                target: "/cfg".into(),
                read_only: true,
                size_bytes: None,
            }),
        ];
        let s = summary_of(
            &args,
            &Policy::default(),
            Path::new("/x/lns-local-mixin.yaml"),
            &PolicySource::Found,
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
            exclude: Vec::new(),
            optional: false,
        })];
        let s = summary_of(
            &args,
            &Policy::default(),
            Path::new("/x/lns-local-mixin.yaml"),
            &PolicySource::Found,
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
            exclude: Vec::new(),
            optional: false,
        })];
        let s = summary_of(
            &args,
            &Policy::default(),
            Path::new("/x/lns-local-mixin.yaml"),
            &PolicySource::Found,
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
        let s = summary_of(
            &args,
            &Policy::default(),
            Path::new("/x/lns-local-mixin.yaml"),
            &PolicySource::Found,
        );
        assert!(s.contains("Workdir:   /app"), "missing workdir line: {s}");
    }

    #[test]
    fn summary_omits_workdir_line_when_unset() {
        let s = summary_of(
            &run_args(Some("ubuntu")),
            &Policy::default(),
            Path::new("/x/lns-local-mixin.yaml"),
            &PolicySource::Found,
        );
        assert!(!s.contains("Workdir:"), "no workdir line expected: {s}");
    }

    #[test]
    fn summary_omits_volume_line_when_none_attached() {
        let s = summary_of(
            &run_args(Some("ubuntu")),
            &Policy::default(),
            Path::new("/x/lns-local-mixin.yaml"),
            &PolicySource::Found,
        );
        assert!(!s.contains("Volume:"), "no volume line expected: {s}");
    }

    #[test]
    fn image_field_shows_resolving_placeholder_until_service_confirms_digest() {
        let args = run_args(Some("ubuntu"));
        let s = summary_of(
            &args,
            &Policy::default(),
            Path::new("/x/lns-local-mixin.yaml"),
            &PolicySource::Found,
        );
        assert!(s.contains("ubuntu (resolving…)"), "no placeholder: {s}");
    }

    #[test]
    fn no_reference_run_shows_the_local_definition_as_the_image() {
        let args = run_args(None);
        let s = summary_of(
            &args,
            &Policy::default(),
            Path::new("/x/lns-local-mixin.yaml"),
            &PolicySource::Found,
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
        let s = summary_of(
            &args,
            &Policy::default(),
            Path::new("/x/lns-local-mixin.yaml"),
            &PolicySource::Found,
        );
        assert!(s.contains("lns.dev.yaml (resolving…)"), "got: {s}");
        assert!(!s.contains("./lns.yaml"), "got: {s}");
    }

    #[test]
    fn resources_line_renders_the_size_the_run_will_boot_with() {
        let s = summary_of(
            &run_args(Some("ubuntu")),
            &Policy::default(),
            Path::new("/x/lns-local-mixin.yaml"),
            &PolicySource::Found,
        );
        assert!(s.contains("1 vCPU · 512 MiB"), "resources line wrong: {s}");
    }

    #[test]
    fn resources_line_reports_the_resolved_size_not_the_flags() {
        let args = run_args(Some("ubuntu"));
        assert!(
            args.cpus.is_none() && args.mem.is_none(),
            "the point of this test is that no flag was passed"
        );
        let s = format_summary(
            &args,
            VmSize {
                cpus: 3,
                mem_mib: 6144,
                disk_bytes: 40 << 30,
            },
            &Policy::default(),
            Path::new("/x/lns-local-mixin.yaml"),
            &PolicySource::Found,
        );
        assert!(s.contains("3 vCPU · 6144 MiB"), "resources line wrong: {s}");
    }

    #[test]
    fn resolved_size_ranks_the_flag_over_the_document_over_the_config_default() {
        let declared = lns_artifact::resources::DeclaredSize {
            cpus: Some(2),
            mem_mib: Some(1024),
            disk_bytes: None,
        };
        let mut args = run_args(Some("ubuntu"));
        args.cpus_config = Some(4);
        args.mem_config = Some(4096);
        let size = resolved_size(declared, &args);
        assert_eq!(
            (size.cpus, size.mem_mib),
            (2, 1024),
            "a sandbox that declares its own resources must not be resized by whoever cloned it"
        );

        args.cpus = Some(8);
        args.mem = Some(8192);
        let size = resolved_size(declared, &args);
        assert_eq!((size.cpus, size.mem_mib), (8, 8192));
    }

    #[test]
    fn resolved_size_falls_back_to_a_config_default_when_the_document_is_silent() {
        let mut args = run_args(Some("ubuntu"));
        args.cpus_config = Some(4);
        args.mem_config = Some(4096);
        let size = resolved_size(Default::default(), &args);
        assert_eq!((size.cpus, size.mem_mib), (4, 4096));
    }

    #[test]
    fn resources_line_discloses_the_disk_the_run_gets() {
        let s = format_summary(
            &run_args(Some("ubuntu")),
            VmSize {
                cpus: 1,
                mem_mib: 512,
                disk_bytes: 40 << 30,
            },
            &Policy::default(),
            Path::new("/x/lns-local-mixin.yaml"),
            &PolicySource::Found,
        );
        assert!(s.contains("40 GiB disk"), "resources line wrong: {s}");
    }

    #[test]
    fn flags_line_lists_interactive_tty_and_detach_in_canonical_order() {
        let mut args = run_args(Some("ubuntu"));
        args.interactive = true;
        args.tty = true;
        args.detach = false;
        let s = summary_of(
            &args,
            &Policy::default(),
            Path::new("/x/lns-local-mixin.yaml"),
            &PolicySource::Found,
        );
        assert!(s.contains("Flags:     -i -t"), "flags line wrong: {s}");
    }

    #[test]
    fn flags_line_includes_detach_when_set() {
        let mut args = run_args(Some("ubuntu"));
        args.interactive = false;
        args.tty = false;
        args.detach = true;
        let s = summary_of(
            &args,
            &Policy::default(),
            Path::new("/x/lns-local-mixin.yaml"),
            &PolicySource::Found,
        );
        assert!(s.contains("Flags:     -d"), "flags line wrong: {s}");
    }

    #[test]
    fn flags_line_includes_auto_remove_when_set() {
        let mut args = run_args(Some("ubuntu"));
        args.auto_remove = true;
        let s = summary_of(
            &args,
            &Policy::default(),
            Path::new("/x/lns-local-mixin.yaml"),
            &PolicySource::Found,
        );
        assert!(s.contains("Flags:     -i -t --rm"), "flags line wrong: {s}");
    }

    #[test]
    fn flags_line_says_none_when_no_flags_are_set() {
        let mut args = run_args(Some("ubuntu"));
        args.interactive = false;
        args.tty = false;
        args.detach = false;
        let s = summary_of(
            &args,
            &Policy::default(),
            Path::new("/x/lns-local-mixin.yaml"),
            &PolicySource::Found,
        );
        assert!(s.contains("Flags:     (none)"), "flags line wrong: {s}");
    }

    #[test]
    fn a_closed_policy_does_not_claim_anything_still_asks() {
        // The catch-all answers for whatever the named rules leave, so nothing is
        // unmatched — telling the developer it asks would be false, not vague.
        let mut policy = Policy::default();
        policy.add_rule(RouteRule::allow_host("api.example.test"));
        policy.add_rule(RouteRule::deny_host("*"));
        let s = summary_of(
            &run_args(Some("ubuntu")),
            &policy,
            Path::new("./lns-local-mixin.yaml"),
            &PolicySource::Found,
        );
        assert!(
            s.contains("unmatched destinations: denied by the catch-all rule"),
            "got: {s}"
        );
        assert!(s.contains("anything else denied"), "got: {s}");
    }

    #[test]
    fn policy_block_shows_the_file_path_and_a_rule_summary() {
        let mut policy = Policy::default();
        policy.add_rule(RouteRule::allow_host("api.linear.app"));
        policy.add_rule(RouteRule::allow_host("api.example.com"));
        policy.add_rule(RouteRule::allow_host("registry.npmjs.org"));
        policy.add_rule(RouteRule::deny_host("evil.example"));
        let s = summary_of(
            &run_args(Some("ubuntu")),
            &policy,
            Path::new("./lns-local-mixin.yaml"),
            &PolicySource::Found,
        );
        assert!(s.contains("file: ./lns-local-mixin.yaml"));
        assert!(
            s.contains("unmatched destinations: ask"),
            "the default is no longer a field the file varies, so the summary states the rule: {s}"
        );
        assert!(s.contains("3 allow, 1 deny, anything else asks"));
    }

    #[test]
    fn the_rules_line_counts_the_raw_splices_apart_from_the_inspected_routes() {
        // The launch summary is the only surface that discloses this directory's own
        // policy, and a raw splice is the widest grant it can express — folded into
        // the route count it would read as one more inspected host.
        let mut policy = Policy::default();
        policy.add_rule(RouteRule::allow_host("api.example.test"));
        policy
            .network
            .egress
            .tcp
            .push(TcpEgressRule::allow_destination("0.0.0.0/0:5432"));
        let s = summary_of(
            &run_args(Some("ubuntu")),
            &policy,
            Path::new("./lns-local-mixin.yaml"),
            &PolicySource::Found,
        );
        assert!(
            s.contains("1 allow, 0 deny, 1 raw allow, 0 raw deny, anything else asks"),
            "got: {s}"
        );
    }

    #[test]
    fn the_rules_line_reports_a_raw_only_policy_rather_than_none_defined() {
        let mut policy = Policy::default();
        policy
            .network
            .egress
            .tcp
            .push(TcpEgressRule::allow_destination("db.internal:5432"));
        let s = summary_of(
            &run_args(Some("ubuntu")),
            &policy,
            Path::new("./lns-local-mixin.yaml"),
            &PolicySource::Found,
        );
        assert!(
            s.contains("0 allow, 0 deny, 1 raw allow, 0 raw deny, anything else asks"),
            "a file holding only raw rules is not a file holding none: {s}"
        );
    }

    #[test]
    fn rules_line_says_none_defined_for_an_empty_route_list() {
        let s = summary_of(
            &run_args(Some("ubuntu")),
            &Policy::default(),
            Path::new("./lns-local-mixin.yaml"),
            &PolicySource::Found,
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
        let s = summary_of(
            &run_args(Some("ubuntu")),
            &policy,
            Path::new("./lns-local-mixin.yaml"),
            &PolicySource::Found,
        );
        assert!(s.contains("1 allow, 1 deny, anything else asks"));
    }

    #[test]
    fn source_line_for_a_found_file_names_the_project_directory() {
        let s = summary_of(
            &run_args(Some("ubuntu")),
            &Policy::default(),
            Path::new("./lns-local-mixin.yaml"),
            &PolicySource::Found,
        );
        assert!(s.contains("source: found in the project directory"));
    }

    #[test]
    fn source_line_for_auto_created_calls_out_no_policy_in_the_project_directory() {
        let s = summary_of(
            &run_args(Some("ubuntu")),
            &Policy::default(),
            Path::new("./lns-local-mixin.yaml"),
            &PolicySource::AutoCreated,
        );
        assert!(s.contains("source: auto-created (no policy in the project directory)"));
    }

    #[test]
    fn resolve_policy_finds_the_default_file_of_the_project_rather_than_of_the_cwd() {
        // One directory is one project, so the run reads the decisions beside the definition it runs, wherever the developer started it.
        let cwd = tempfile::TempDir::new().unwrap();
        let project = tempfile::TempDir::new().unwrap();
        let preexisting = project.path().join(DEFAULT_POLICY_FILENAME);
        Policy::default().save_atomic(&preexisting).unwrap();
        let (resolved, source) = resolve_policy(project.path()).unwrap();
        assert_eq!(resolved, preexisting);
        assert_eq!(source, PolicySource::Found);
        assert!(!cwd.path().join(DEFAULT_POLICY_FILENAME).exists());
    }

    #[test]
    fn resolve_policy_auto_creates_default_file_when_missing() {
        let dir = tempfile::TempDir::new().unwrap();
        let (resolved, source) = resolve_policy(dir.path()).unwrap();
        assert_eq!(resolved, dir.path().join(DEFAULT_POLICY_FILENAME));
        assert_eq!(source, PolicySource::AutoCreated);
        let body = std::fs::read_to_string(&resolved).unwrap();
        assert!(
            !body.contains("defaultVerdict"),
            "a file born with the key would be born with the one value the loader tells you to delete: {body}"
        );
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
        let err = resolve_policy(dir.path()).expect_err("must reject non-file");
        assert!(format!("{err:#}").contains("not a regular file"));
    }

    #[test]
    fn print_run_summary_writes_to_provided_writer_and_returns_resolved_path() {
        let dir = tempfile::TempDir::new().unwrap();
        let args = run_args(Some("ubuntu"));
        let mut buf = Vec::<u8>::new();
        let path = print_run_summary(
            &args,
            resolved_size(Default::default(), &args),
            dir.path(),
            &mut buf,
        )
        .unwrap();
        assert_eq!(path, dir.path().join(DEFAULT_POLICY_FILENAME));
        let text = String::from_utf8(buf).unwrap();
        assert!(text.contains("lns run"));
        assert!(text.contains("Policy:"));
    }
}
