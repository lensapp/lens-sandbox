use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::FromArgMatches;
use lns_policy::{Policy, RouteRule, Transport, Verdict};

use crate::command::{CommandSpec, RunCtx, RunFuture, subcommand};
use crate::run::summary::policy_path;

#[derive(clap::Args)]
pub struct PolicyArgs {
    #[command(subcommand)]
    pub command: PolicyCommand,
}

#[derive(clap::Subcommand)]
pub enum PolicyCommand {
    #[command(about = "Add an allow rule for a destination pattern.")]
    Allow(PolicyAllowArgs),
    #[command(about = "Add a deny rule for a destination pattern.")]
    Deny(PolicyRuleArgs),
    #[command(about = "List the rules in the policy file.")]
    List(PolicyScopeArgs),
    #[command(about = "Remove every rule matching a destination pattern.")]
    Remove(PolicyRemoveArgs),
}

/// Only `allow` takes `--binary`: on a deny the listed callers are blocked by verdict and the rest fail closed, so all the scoping buys is that a later rule scoped to one of those others can still let it through — a distinction too fine to hand a flag.
#[derive(clap::Args)]
pub struct PolicyAllowArgs {
    #[command(flatten)]
    pub rule: PolicyRuleArgs,
    #[arg(
        long,
        value_name = "PATH",
        help = "Restrict the rule to callers running this guest binary, given as an absolute path (e.g. /usr/bin/git); repeatable. Every other caller is denied the destination rather than asked."
    )]
    pub binary: Vec<String>,
}

#[derive(clap::Args)]
pub struct PolicyRuleArgs {
    #[arg(help = "Destination pattern: host, wildcard (*.example.com), CIDR, or host:port.")]
    pub pattern: String,
    #[arg(long, help = "Human-readable note stored alongside the rule.")]
    pub description: Option<String>,
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
    #[command(flatten)]
    pub output: crate::output::OutputArgs,
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

pub fn augment(app: clap::Command) -> clap::Command {
    app.subcommand(subcommand::<PolicyArgs>("policy").about("Edit network rules in a policy file."))
}

pub const SPEC: CommandSpec = CommandSpec {
    name: "policy",
    augment,
    run: run_command,
    announces_update_check: true,
    owns_terminal: crate::command::never_owns_terminal,
};

pub fn run_command<'a>(matches: &'a clap::ArgMatches, ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = PolicyArgs::from_arg_matches(matches)?;
        let cwd = ctx.cwd()?;
        let mut out = ctx.out;
        run(&args.command, &cwd, &mut out)
    })
}

pub fn run(cmd: &PolicyCommand, cwd: &Path, writer: &mut impl Write) -> Result<i32> {
    match cmd {
        PolicyCommand::Allow(args) => {
            add_rule(&args.rule, Verdict::Allow, &args.binary, cwd, writer)
        }
        PolicyCommand::Deny(args) => add_rule(args, Verdict::Deny, &[], cwd, writer),
        PolicyCommand::List(args) => list_rules(args, cwd, writer),
        PolicyCommand::Remove(args) => remove_rule(args, cwd, writer),
    }
}

fn add_rule(
    args: &PolicyRuleArgs,
    verdict: Verdict,
    binaries: &[String],
    cwd: &Path,
    writer: &mut impl Write,
) -> Result<i32> {
    let rule = build_rule(args, verdict, binaries)?;
    let path = policy_path(args.policy.as_deref(), cwd);
    let mut policy = Policy::load_or_default(&path)
        .with_context(|| format!("loading policy from {}", path.display()))?;
    let placement = place_rule(&mut policy, rule)?;
    if let Placement::Redundant(reason) = &placement {
        let message = match reason {
            Redundant::AlreadyPresent => format!(
                "That {} rule for {:?} is already in {}",
                verdict_word(verdict),
                args.pattern,
                path.display()
            ),
            Redundant::DeniedBy(covering) => format!(
                "The deny rule for {covering:?} in {} already blocks {:?}, so this deny adds nothing",
                path.display(),
                args.pattern
            ),
        };
        writeln!(writer, "{message}")?;
        return Ok(0);
    }
    policy
        .save_atomic(&path)
        .with_context(|| format!("writing policy to {}", path.display()))?;
    if matches!(placement, Placement::Described) {
        writeln!(
            writer,
            "Updated the description of the {} rule for {:?} in {}",
            verdict_word(verdict),
            args.pattern,
            path.display()
        )?;
        return Ok(0);
    }
    writeln!(
        writer,
        "Added {} rule for {:?} to {}",
        verdict_word(verdict),
        args.pattern,
        path.display()
    )?;
    if let Placement::Appended(Some(note)) | Placement::Inserted(note) = placement {
        writeln!(writer, "{note}")?;
    }
    Ok(0)
}

fn build_rule(args: &PolicyRuleArgs, verdict: Verdict, binaries: &[String]) -> Result<RouteRule> {
    let rule = RouteRule {
        match_pattern: args.pattern.clone(),
        verdict,
        transport: Transport::Direct,
        scheme: None,
        description: args.description.clone(),
        tls_terminate: false,
        rules: Vec::new(),
        binaries: (!binaries.is_empty()).then(|| binaries.to_vec()),
    };
    rule.validate_binaries()?;
    Ok(rule)
}

enum Placement {
    Appended(Option<String>),
    Inserted(String),
    Described,
    Redundant(Redundant),
}

enum Redundant {
    AlreadyPresent,
    DeniedBy(String),
}

/// The guest gate stops at the first matching rule, so a rule appended behind one that already covers it would never fire; put it in front of that rule instead and say so.
fn place_rule(policy: &mut Policy, mut rule: RouteRule) -> Result<Placement> {
    let shadowing = policy
        .network
        .first_shadowing_rule(&rule)
        .map(|(index, shadowing)| (index, shadowing.clone()));
    let Some((index, shadowing)) = shadowing else {
        // The scoping is what fails the destination closed, so it is announced wherever the rule lands, not only where it had to displace another.
        let note = rule
            .binaries
            .is_some()
            .then(|| format!("{}.", fail_closed_note(&rule.match_pattern)));
        policy.network.egress.http.push(rule);
        return Ok(Placement::Appended(note));
    };
    let inherited = inherit_treatment(&mut rule, &shadowing);
    // The rule the gate reaches first is the only one in force, so that is the one this grant can already be: a copy stranded behind it never fires, whether it is pre-empted by verdict, by scope, or by a request filter.
    if grants_the_same(&shadowing, &rule) {
        let held = &mut policy.network.egress.http[index];
        return Ok(renote(held, rule.description));
    }
    if shadowing.verdict == Verdict::Deny {
        return behind_a_deny(&shadowing, &rule);
    }
    if reopens_a_scoped_rule(&shadowing, &rule) {
        return behind_a_scoped_rule(&shadowing, &rule);
    }
    if lifts_a_request_filter(&shadowing, &rule) {
        return behind_a_request_filter(&shadowing, &rule);
    }
    Ok(Placement::Inserted(insert_ahead(
        policy, index, rule, &shadowing, inherited,
    )))
}

/// The rule going in front inherits the fronted rule's TLS termination: narrowing who may reach a destination is not a request to stop intercepting it, and a deny blocks the request before there is anything to intercept.
fn inherit_treatment(rule: &mut RouteRule, shadowing: &RouteRule) -> bool {
    let inherit = shadowing.tls_terminate && !rule.tls_terminate && rule.verdict != Verdict::Deny;
    if inherit {
        rule.tls_terminate = true;
    }
    inherit
}

/// Places the rule where it fires, taking over from any copy of the grant left stranded behind it — including the note that copy carried, which a command passing no `--description` is not asking to forget.
fn insert_ahead(
    policy: &mut Policy,
    index: usize,
    mut rule: RouteRule,
    shadowing: &RouteRule,
    inherited: bool,
) -> String {
    let mut note = placement_note(shadowing, &rule);
    if inherited {
        note.push_str(" It terminates TLS as that rule does.");
    }
    let rules = &mut policy.network.egress.http;
    if rule.description.is_none() {
        rule.description = rules
            .iter()
            .find(|held| grants_the_same(held, &rule))
            .and_then(|stranded| stranded.description.clone());
    }
    rules.retain(|held| !grants_the_same(held, &rule));
    rules.insert(index, rule);
    note
}

/// Whether the file already holds this grant: the note is the one field neither the gate nor the developer's intent reads, so a rule differing only there is the same rule, not a second one to place in front of it.
fn grants_the_same(held: &RouteRule, rule: &RouteRule) -> bool {
    let renoted = RouteRule {
        description: rule.description.clone(),
        ..held.clone()
    };
    renoted == *rule
}

/// A `lns policy allow` with no `--description` is not a request to forget the note the rule already carries.
fn renote(held: &mut RouteRule, description: Option<String>) -> Placement {
    if description.is_none() || held.description == description {
        return Placement::Redundant(Redundant::AlreadyPresent);
    }
    held.description = description;
    Placement::Described
}

/// A deny already blocks every request the new rule could match: a deny asks for what the file delivers, while anything else would never fire and is refused rather than reordered ahead of a deny.
fn behind_a_deny(shadowing: &RouteRule, rule: &RouteRule) -> Result<Placement> {
    let pattern = &shadowing.match_pattern;
    if rule.verdict == Verdict::Deny {
        return Ok(Placement::Redundant(Redundant::DeniedBy(pattern.clone())));
    }
    bail!(
        "the deny rule for {pattern:?} already blocks every request this rule could match, and the guest stops at the first matching rule, so this {} rule would never fire — narrow that deny to the destinations you still mean to block, or reorder the file by hand if this rule is meant to win",
        verdict_word(rule.verdict)
    )
}

/// The guest refuses to re-open a destination a binary-scoped rule claimed, so the only place this rule would reach the excluded callers is in front of that rule — a widening of the file's own grant, which is the developer's call to make, not ours.
fn reopens_a_scoped_rule(shadowing: &RouteRule, rule: &RouteRule) -> bool {
    shadowing.binaries.is_some() && rule.binaries.is_none() && rule.verdict != Verdict::Deny
}

fn behind_a_scoped_rule(shadowing: &RouteRule, rule: &RouteRule) -> Result<Placement> {
    let pattern = &shadowing.match_pattern;
    let scope = shadowing.binaries.as_deref().unwrap_or_default().join(", ");
    let fix = if *pattern == rule.match_pattern {
        format!("drop the scoped rule with `lns policy remove {pattern}`")
    } else {
        format!("narrow or remove the rule for {pattern:?}")
    };
    bail!(
        "the rule for {pattern:?} is scoped to {scope}, and placing this {} rule in front of it would open the destination to every caller in the sandbox — {fix} first if that is what you mean",
        verdict_word(rule.verdict)
    )
}

/// A rule carrying an http `rules` list allows only the requests it names and denies the rest, so an unrestricted rule in front of it lifts that filter for every request it matches — everything but a deny, which blocks them all either way.
fn lifts_a_request_filter(shadowing: &RouteRule, rule: &RouteRule) -> bool {
    !shadowing.rules.is_empty() && rule.verdict != Verdict::Deny
}

fn behind_a_request_filter(shadowing: &RouteRule, rule: &RouteRule) -> Result<Placement> {
    let pattern = &shadowing.match_pattern;
    bail!(
        "the rule for {pattern:?} allows only the requests its rules list names, and placing this {} rule in front of it would lift that restriction for every request it matches — narrow this rule's destination, or reorder the file by hand if that is what you mean",
        verdict_word(rule.verdict)
    )
}

fn fail_closed_note(pattern: &str) -> String {
    format!("Every other caller is now denied {pattern:?} without being asked")
}

fn placement_note(shadowing: &RouteRule, rule: &RouteRule) -> String {
    let pattern = &shadowing.match_pattern;
    let placed = format!(
        "Placed it before the existing rule for {pattern:?}, which covers the same destination and would otherwise pre-empt it."
    );
    match (&shadowing.binaries, &rule.binaries) {
        (Some(binaries), None) => format!(
            "{placed} That rule's scoping to {} no longer applies to {:?}.",
            binaries.join(", "),
            rule.match_pattern
        ),
        // The guest refuses to re-open a destination a scoped rule claimed, so the rule behind this one is now dead for everyone it excludes — silence there would leave the file reading as if it still served them.
        (None, Some(_)) => format!(
            "{placed} {}, and that rule no longer serves them.",
            fail_closed_note(&rule.match_pattern)
        ),
        _ => placed,
    }
}

fn list_rules(args: &PolicyScopeArgs, cwd: &Path, writer: &mut impl Write) -> Result<i32> {
    let path = policy_path(args.policy.as_deref(), cwd);
    let policy = Policy::load_or_default(&path)
        .with_context(|| format!("loading policy from {}", path.display()))?;
    let rows: Vec<RuleRow> = policy
        .network
        .egress
        .http
        .iter()
        .map(RuleRow::new)
        .collect();
    let note = format!("No rules in {}", path.display());
    crate::output::emit_or_note(args.output.format, &rows, &note, writer)?;
    Ok(0)
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RuleRow {
    verdict: &'static str,
    pattern: String,
    binaries: Option<Vec<String>>,
    description: Option<String>,
}

impl RuleRow {
    fn new(rule: &RouteRule) -> Self {
        Self {
            verdict: verdict_word(rule.verdict),
            pattern: rule.match_pattern.clone(),
            binaries: rule.binaries.clone(),
            description: rule.description.clone(),
        }
    }
}

impl crate::output::TableRow for RuleRow {
    const HEADERS: &'static [&'static str] = &["VERDICT", "PATTERN", "BINARIES", "DESCRIPTION"];

    fn cells(&self) -> Vec<String> {
        vec![
            self.verdict.to_string(),
            self.pattern.clone(),
            self.binaries
                .as_ref()
                .map(|b| b.join(","))
                .unwrap_or_default(),
            self.description.clone().unwrap_or_default(),
        ]
    }
}

fn remove_rule(args: &PolicyRemoveArgs, cwd: &Path, writer: &mut impl Write) -> Result<i32> {
    let path = policy_path(args.policy.as_deref(), cwd);
    let mut policy = Policy::load_or_default(&path)
        .with_context(|| format!("loading policy from {}", path.display()))?;
    let before = policy.network.egress.http.len();
    policy
        .network
        .egress
        .http
        .retain(|rule| rule.match_pattern != args.pattern);
    let removed = before - policy.network.egress.http.len();
    if removed == 0 {
        bail!("no rule matching {:?} in {}", args.pattern, path.display());
    }
    policy
        .save_atomic(&path)
        .with_context(|| format!("writing policy to {}", path.display()))?;
    // Removal goes by pattern alone, so the count is the developer's only signal that a rule they didn't have in mind — a binary-scoped one — went with it.
    writeln!(
        writer,
        "Removed {removed} {} for {:?} from {}",
        if removed == 1 { "rule" } else { "rules" },
        args.pattern,
        path.display()
    )?;
    Ok(0)
}

fn verdict_word(v: Verdict) -> &'static str {
    match v {
        Verdict::Allow => "allow",
        Verdict::Deny => "deny",
        Verdict::Ask => "ask",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn rule_args(pattern: &str, policy: Option<&Path>) -> PolicyRuleArgs {
        PolicyRuleArgs {
            pattern: pattern.to_string(),
            description: None,
            policy: policy.map(Path::to_path_buf),
        }
    }

    #[test]
    fn clap_rejects_transport_flag_for_allow_rules() {
        let err = crate::command::build_cli()
            .try_get_matches_from([
                "lns",
                "policy",
                "allow",
                "api.example.test",
                "--transport",
                "upstream",
            ])
            .unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::UnknownArgument);
    }

    #[test]
    fn clap_collects_repeated_binary_flags_into_one_rule_scope() {
        let matches = crate::command::build_cli()
            .try_get_matches_from([
                "lns",
                "policy",
                "allow",
                "git.example.test",
                "--binary",
                "/usr/bin/git",
                "--binary",
                "/usr/bin/curl",
            ])
            .unwrap();
        let (_, policy) = matches.subcommand().unwrap();
        let (verb, allow) = policy.subcommand().unwrap();
        assert_eq!(verb, "allow");
        let args = PolicyAllowArgs::from_arg_matches(allow).unwrap();
        assert_eq!(args.binary, ["/usr/bin/git", "/usr/bin/curl"]);
    }

    #[test]
    fn re_adding_a_rule_with_the_note_it_already_carries_reports_it_as_already_present() {
        let dir = TempDir::new().unwrap();
        let mut args = rule_args("api.example.test", None);
        args.description = Some("issue sync".into());
        let mut out = Vec::new();
        add_rule(&args, Verdict::Allow, &[], dir.path(), &mut out).unwrap();
        out.clear();

        add_rule(&args, Verdict::Allow, &[], dir.path(), &mut out).unwrap();

        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("is already in"),
            "nothing about the rule changed, so reporting an edit would be a lie:\n{text}"
        );
        let policy = Policy::load_or_default(&dir.path().join("lns-policy.yaml")).unwrap();
        assert_eq!(policy.network.egress.http.len(), 1);
    }

    #[test]
    fn allow_writes_an_allow_rule_with_direct_transport() {
        let dir = TempDir::new().unwrap();
        let args = rule_args("api.acme.corp", None);
        let mut out = Vec::new();
        let code = add_rule(&args, Verdict::Allow, &[], dir.path(), &mut out).unwrap();
        assert_eq!(code, 0);
        let policy = Policy::load_or_default(&dir.path().join("lns-policy.yaml")).unwrap();
        assert_eq!(policy.network.egress.http.len(), 1);
        assert_eq!(policy.network.egress.http[0].verdict, Verdict::Allow);
        assert_eq!(policy.network.egress.http[0].transport, Transport::Direct);
    }

    #[test]
    fn deny_through_run_writes_a_deny_rule() {
        let dir = TempDir::new().unwrap();
        let mut out = Vec::new();
        let code = run(
            &PolicyCommand::Deny(rule_args("evil.example", None)),
            dir.path(),
            &mut out,
        )
        .unwrap();
        assert_eq!(code, 0);
        let policy = Policy::load_or_default(&dir.path().join("lns-policy.yaml")).unwrap();
        assert!(
            policy
                .network
                .egress
                .http
                .iter()
                .any(|r| r.match_pattern == "evil.example" && r.verdict == Verdict::Deny)
        );
    }

    #[tokio::test]
    async fn run_command_applies_clap_matches_against_the_ctx_cwd() {
        let dir = TempDir::new().unwrap();
        let matches = crate::command::build_cli()
            .try_get_matches_from(["lns", "policy", "allow", "api.acme.corp"])
            .unwrap();
        let (_, sub) = matches.subcommand().unwrap();
        let mut input: &[u8] = b"";
        let mut out: Vec<u8> = Vec::new();
        let ctx = RunCtx {
            debug: false,
            cwd: Some(dir.path().to_path_buf()),
            input: &mut input,
            out: &mut out,
        };
        assert_eq!(run_command(sub, ctx).await.unwrap(), 0);
        let policy = Policy::load_or_default(&dir.path().join("lns-policy.yaml")).unwrap();
        assert!(
            policy
                .network
                .egress
                .http
                .iter()
                .any(|r| r.match_pattern == "api.acme.corp" && r.verdict == Verdict::Allow)
        );
    }

    #[test]
    fn list_reports_each_verdict_including_ask_and_descriptions() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("lns-policy.yaml");
        let mut policy = Policy::default();
        policy.add_rule(RouteRule {
            match_pattern: "ask.example".into(),
            verdict: Verdict::Ask,
            transport: Transport::Direct,
            scheme: None,
            description: Some("undecided".into()),
            tls_terminate: false,
            rules: Vec::new(),
            binaries: None,
        });
        policy.save_atomic(&path).unwrap();
        let mut out = Vec::new();
        list_rules(&scope_args(), dir.path(), &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("VERDICT  PATTERN"), "got: {text}");
        assert!(
            text.contains("ask      ask.example            undecided"),
            "an unscoped rule leaves the BINARIES column blank and still lines up:\n{text}"
        );
    }

    fn scope_args() -> PolicyScopeArgs {
        PolicyScopeArgs {
            policy: None,
            output: crate::output::OutputArgs {
                format: crate::output::Format::Table,
            },
        }
    }

    #[test]
    fn list_reports_no_rules_for_an_empty_policy() {
        let dir = TempDir::new().unwrap();
        let mut out = Vec::new();
        list_rules(&scope_args(), dir.path(), &mut out).unwrap();
        assert!(String::from_utf8(out).unwrap().starts_with("No rules in "));
    }

    #[test]
    fn remove_on_a_missing_rule_errors_and_leaves_the_file_untouched() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("lns-policy.yaml");
        let mut policy = Policy::default();
        policy.add_rule(RouteRule::allow_host("keep.example"));
        policy.save_atomic(&path).unwrap();
        let before = std::fs::read_to_string(&path).unwrap();

        let mut out = Vec::new();
        let err = remove_rule(
            &PolicyRemoveArgs {
                pattern: "ghost.example".into(),
                policy: None,
            },
            dir.path(),
            &mut out,
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("ghost.example"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    }

    #[test]
    fn add_surfaces_a_clear_error_for_a_malformed_policy_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("lns-policy.yaml");
        std::fs::write(&path, "network: not-a-map\n").unwrap();
        let mut out = Vec::new();
        let err = add_rule(
            &rule_args("x", None),
            Verdict::Allow,
            &[],
            dir.path(),
            &mut out,
        )
        .unwrap_err();
        assert!(format!("{err:#}").contains("loading policy from"));
    }
}
