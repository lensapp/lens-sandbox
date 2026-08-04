use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::FromArgMatches;
use lns_policy::{Policy, RouteRule, TcpEgressRule, Transport, Verdict};

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
    Allow(PolicyScopedRuleArgs),
    #[command(about = "Add a deny rule for a destination pattern.")]
    Deny(PolicyRuleArgs),
    #[command(
        about = "Allow a raw TCP destination, spliced through without inspection.",
        long_about = "Allow a raw TCP destination, spliced through without inspection.\n\nRaw destinations are matched before the HTTP rules and are passed through untouched: no TLS interception, no HTTP rules, no credential injection. A destination no raw rule names falls through to the HTTP rules as before. Every raw destination must name a port.\n\nA connection the sandbox cannot read raises an approval card on first use, so a raw destination does not have to be declared up front to be allowed."
    )]
    AllowTcp(PolicyScopedRuleArgs),
    #[command(about = "Deny a raw TCP destination.")]
    DenyTcp(PolicyRuleArgs),
    #[command(about = "List the rules in the policy file.")]
    List(PolicyScopeArgs),
    #[command(about = "Remove every rule matching a destination pattern.")]
    Remove(PolicyRemoveArgs),
}

/// No deny verb takes `--binary`: on a deny the listed callers are blocked by verdict and the rest fail closed, so all the scoping buys is that a later rule scoped to one of those others can still let it through — a distinction too fine to hand a flag.
#[derive(clap::Args)]
pub struct PolicyScopedRuleArgs {
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
        PolicyCommand::AllowTcp(args) => {
            add_tcp_rule(&args.rule, Verdict::Allow, &args.binary, cwd, writer)
        }
        PolicyCommand::DenyTcp(args) => add_tcp_rule(args, Verdict::Deny, &[], cwd, writer),
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
    add(args, build_rule(args, verdict, binaries)?, cwd, writer)
}

fn add_tcp_rule(
    args: &PolicyRuleArgs,
    verdict: Verdict,
    binaries: &[String],
    cwd: &Path,
    writer: &mut impl Write,
) -> Result<i32> {
    add(args, build_tcp_rule(args, verdict, binaries)?, cwd, writer)
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

fn build_tcp_rule(
    args: &PolicyRuleArgs,
    verdict: Verdict,
    binaries: &[String],
) -> Result<TcpEgressRule> {
    let rule = TcpEgressRule {
        description: args.description.clone(),
        binaries: (!binaries.is_empty()).then(|| binaries.to_vec()),
        ..TcpEgressRule::new(args.pattern.clone(), verdict)
    };
    // A rule the guest cannot parse force-denies the whole policy, so it is refused before it reaches the file.
    rule.validate().map_err(|e| anyhow::anyhow!(e))?;
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

/// How the CLI names one table's rules in the lines it prints.
struct Vocabulary {
    /// Fronts the verdict word where a line announces a rule: "Added raw TCP allow rule for …".
    announced: &'static str,
    /// Qualifies a rule a line refers back to: "the raw deny rule for …".
    qualifier: &'static str,
    /// What the table's rules match, for the line saying a deny already covers every one of them.
    matched: &'static str,
}

/// A rule table the guest gate scans first-match-wins, and the vocabulary the CLI describes it in.
trait Placeable: Clone + PartialEq {
    const WORDS: Vocabulary;

    fn verdict(&self) -> Verdict;
    fn match_pattern(&self) -> &str;
    fn binaries(&self) -> Option<&[String]>;
    fn description(&self) -> Option<&str>;
    fn set_description(&mut self, description: Option<String>);
    fn table(policy: &mut Policy) -> &mut Vec<Self>;
    fn first_shadowing(policy: &Policy, rule: &Self) -> Option<(usize, Self)>;

    /// Whether this rule is the file's backstop; a raw rule must name a port, so it can never be one.
    fn is_catch_all(&self) -> bool {
        false
    }

    /// Takes on the treatment the fronted rule carries and returns the note saying so, since fronting a rule is not a request to stop applying it.
    fn inherit_from(&mut self, _shadowing: &Self) -> Option<&'static str> {
        None
    }

    /// Refuses a fronting the fronted rule's own shape makes a widening; binary scope is checked for every table by [`reopens_a_scoped_rule`].
    fn refuse_fronting(&self, _shadowing: &Self) -> Result<()> {
        Ok(())
    }

    /// What writing this rule takes over from the other table, read before placement.
    fn displaced(_policy: &Policy, _rule: &Self) -> Displaced {
        Displaced::none()
    }
}

impl Placeable for RouteRule {
    fn is_catch_all(&self) -> bool {
        RouteRule::is_catch_all(self)
    }

    const WORDS: Vocabulary = Vocabulary {
        announced: "",
        qualifier: "",
        matched: "request",
    };

    fn verdict(&self) -> Verdict {
        self.verdict
    }

    fn match_pattern(&self) -> &str {
        &self.match_pattern
    }

    fn binaries(&self) -> Option<&[String]> {
        self.binaries.as_deref()
    }

    fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    fn set_description(&mut self, description: Option<String>) {
        self.description = description;
    }

    fn table(policy: &mut Policy) -> &mut Vec<Self> {
        &mut policy.network.egress.http
    }

    fn first_shadowing(policy: &Policy, rule: &Self) -> Option<(usize, Self)> {
        policy
            .network
            .first_shadowing_rule(rule)
            .map(|(index, shadowing)| (index, shadowing.clone()))
    }

    /// The rule going in front inherits TLS termination the fronted rule was actually performing, since narrowing who may reach a destination is not a request to stop intercepting it; a deny blocks the request before there is anything to intercept, on either side.
    fn inherit_from(&mut self, shadowing: &Self) -> Option<&'static str> {
        let intercepts = shadowing.tls_terminate && shadowing.verdict != Verdict::Deny;
        if !intercepts || self.tls_terminate || self.verdict == Verdict::Deny {
            return None;
        }
        self.tls_terminate = true;
        Some(" It terminates TLS as that rule does.")
    }

    /// A rule carrying an http `rules` list allows only the requests it names and denies the rest, so an unrestricted rule in front of it lifts that filter for every request it matches — everything but a deny, which blocks them all either way.
    fn refuse_fronting(&self, shadowing: &Self) -> Result<()> {
        if shadowing.rules.is_empty() || self.verdict == Verdict::Deny {
            return Ok(());
        }
        bail!(
            "the rule for {:?} allows only the requests its rules list names, and placing this {} rule in front of it would lift that restriction for every request it matches — narrow this rule's destination, or reorder the file by hand if that is what you mean",
            shadowing.match_pattern,
            verdict_word(self.verdict)
        )
    }
}

impl Placeable for TcpEgressRule {
    const WORDS: Vocabulary = Vocabulary {
        announced: "raw TCP ",
        qualifier: "raw ",
        matched: "connection",
    };

    fn verdict(&self) -> Verdict {
        self.verdict
    }

    fn match_pattern(&self) -> &str {
        &self.match_pattern
    }

    fn binaries(&self) -> Option<&[String]> {
        self.binaries.as_deref()
    }

    fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    fn set_description(&mut self, description: Option<String>) {
        self.description = description;
    }

    fn table(policy: &mut Policy) -> &mut Vec<Self> {
        &mut policy.network.egress.tcp
    }

    fn first_shadowing(policy: &Policy, rule: &Self) -> Option<(usize, Self)> {
        policy
            .network
            .first_shadowing_tcp_rule(rule)
            .map(|(index, shadowing)| (index, shadowing.clone()))
    }

    fn displaced(policy: &Policy, rule: &Self) -> Displaced {
        let pre_empted = policy.network.http_rules_pre_empted_by(rule);
        Displaced {
            lifted_deny: pre_empted
                .iter()
                .find(|route| route.verdict == Verdict::Deny)
                .map(|route| route.match_pattern.clone()),
            patterns: pre_empted
                .iter()
                .map(|route| format!("{:?}", route.match_pattern))
                .collect(),
        }
    }
}

/// The `egress.http` rules a raw rule takes its destination over from, empty for an `egress.http` rule.
struct Displaced {
    lifted_deny: Option<String>,
    patterns: Vec<String>,
}

impl Displaced {
    fn none() -> Self {
        Self {
            lifted_deny: None,
            patterns: Vec::new(),
        }
    }

    /// Lifting a block the author wrote widens their own policy, which is theirs to decide, so the rule is refused rather than written with a note.
    fn refuse_a_widening(&self, destination: &str) -> Result<()> {
        match &self.lifted_deny {
            Some(pattern) => bail!(
                "the HTTP deny rule for {pattern:?} would no longer apply to {destination:?}, which a raw rule splices through unread — narrow or remove that deny first if that is what you mean"
            ),
            None => Ok(()),
        }
    }

    fn note(&self, destination: &str) -> Option<String> {
        if self.patterns.is_empty() {
            return None;
        }
        let (rule, applies) = match self.patterns.len() {
            1 => ("rule", "applies"),
            _ => ("rules", "apply"),
        };
        Some(format!(
            "The HTTP {rule} for {} no longer {applies} to {destination:?}, which is now spliced raw.",
            self.patterns.join(", ")
        ))
    }
}

fn add<R: Placeable>(
    args: &PolicyRuleArgs,
    rule: R,
    cwd: &Path,
    writer: &mut impl Write,
) -> Result<i32> {
    let path = policy_path(args.policy.as_deref(), cwd);
    let mut policy = Policy::load_or_default(&path)
        .with_context(|| format!("loading policy from {}", path.display()))?;
    let announced = R::WORDS.announced;
    let verdict = verdict_word(rule.verdict());
    let displaced = R::displaced(&policy, &rule);
    let placement = place(&mut policy, rule)?;
    if let Placement::Redundant(reason) = &placement {
        let message = match reason {
            Redundant::AlreadyPresent => format!(
                "That {announced}{verdict} rule for {:?} is already in {}",
                args.pattern,
                path.display()
            ),
            Redundant::DeniedBy(covering) => format!(
                "The {}deny rule for {covering:?} in {} already blocks {:?}, so this deny adds nothing",
                R::WORDS.qualifier,
                path.display(),
                args.pattern
            ),
        };
        writeln!(writer, "{message}")?;
        return Ok(0);
    }
    // Only a rule that takes the destination over is refused: a description update grants nothing the file did not already grant.
    if !matches!(placement, Placement::Described) {
        displaced.refuse_a_widening(&args.pattern)?;
    }
    policy
        .save_atomic(&path)
        .with_context(|| format!("writing policy to {}", path.display()))?;
    if matches!(placement, Placement::Described) {
        writeln!(
            writer,
            "Updated the description of the {announced}{verdict} rule for {:?} in {}",
            args.pattern,
            path.display()
        )?;
        return Ok(0);
    }
    writeln!(
        writer,
        "Added {announced}{verdict} rule for {:?} to {}",
        args.pattern,
        path.display()
    )?;
    if let Placement::Appended(Some(note)) | Placement::Inserted(note) = placement {
        writeln!(writer, "{note}")?;
    }
    if let Some(note) = displaced.note(&args.pattern) {
        writeln!(writer, "{note}")?;
    }
    Ok(0)
}

/// The guest gate stops at the first matching rule, so a rule appended behind one that already covers it would never fire; put it in front of that rule instead and say so.
fn place<R: Placeable>(policy: &mut Policy, mut rule: R) -> Result<Placement> {
    let Some((index, shadowing)) = R::first_shadowing(policy, &rule) else {
        // The scoping is what fails the destination closed, so it is announced wherever the rule lands, not only where it had to displace another.
        let note = rule
            .binaries()
            .is_some()
            .then(|| format!("{}.", fail_closed_note(rule.match_pattern())));
        R::table(policy).push(rule);
        return Ok(Placement::Appended(note));
    };
    let inherited = rule.inherit_from(&shadowing);
    // The rule the gate reaches first is the only one in force, so that is the one this grant can already be: a copy stranded behind it never fires, whether it is pre-empted by verdict, by scope, or by a request filter.
    if grants_the_same(&shadowing, &rule) {
        let described = rule.description().map(str::to_string);
        return Ok(renote(&mut R::table(policy)[index], described));
    }
    // A second catch-all decides everything the first one would, so the file keeps only one — including any the fronted rule was itself in front of.
    if rule.is_catch_all() && shadowing.is_catch_all() && rule.verdict() != shadowing.verdict() {
        let note = format!(
            "Replaced the catch-all {} rule, which decided the same destinations.",
            verdict_word(shadowing.verdict())
        );
        let rules = R::table(policy);
        rules.retain(|held| !held.is_catch_all());
        rules.insert(index, rule);
        return Ok(Placement::Inserted(note));
    }
    // An allow must be able to front the catch-all deny that closes a directory, or a closed policy is editable by hand alone; a deny aimed at a destination is still a decision, so an allow behind that stays refused.
    if shadowing.verdict() == Verdict::Deny
        && (!shadowing.is_catch_all() || rule.verdict() == Verdict::Deny)
    {
        return behind_a_deny(&shadowing, &rule);
    }
    if reopens_a_scoped_rule(&shadowing, &rule) {
        return behind_a_scoped_rule(&shadowing, &rule);
    }
    rule.refuse_fronting(&shadowing)?;
    Ok(Placement::Inserted(insert_ahead(
        policy, index, rule, &shadowing, inherited,
    )))
}

/// Places the rule where it fires, taking over from any copy of the grant left stranded behind it — including the note that copy carried, which a command passing no `--description` is not asking to forget.
fn insert_ahead<R: Placeable>(
    policy: &mut Policy,
    index: usize,
    mut rule: R,
    shadowing: &R,
    inherited: Option<&'static str>,
) -> String {
    let mut note = placement_note(shadowing, &rule);
    if let Some(inherited) = inherited {
        note.push_str(inherited);
    }
    let rules = R::table(policy);
    if rule.description().is_none() {
        let stranded = rules
            .iter()
            .find(|held| grants_the_same(*held, &rule))
            .and_then(|stranded| stranded.description().map(str::to_string));
        rule.set_description(stranded);
    }
    rules.retain(|held| !grants_the_same(held, &rule));
    rules.insert(index, rule);
    note
}

/// Whether the file already holds this grant: the note is the one field neither the gate nor the developer's intent reads, so a rule differing only there is the same rule, not a second one to place in front of it.
fn grants_the_same<R: Placeable>(held: &R, rule: &R) -> bool {
    let mut renoted = held.clone();
    renoted.set_description(rule.description().map(str::to_string));
    renoted == *rule
}

/// A `lns policy allow` with no `--description` is not a request to forget the note the rule already carries.
fn renote<R: Placeable>(held: &mut R, description: Option<String>) -> Placement {
    if description.is_none() || held.description() == description.as_deref() {
        return Placement::Redundant(Redundant::AlreadyPresent);
    }
    held.set_description(description);
    Placement::Described
}

/// A deny already blocks everything the new rule could match: a deny asks for what the file delivers, while anything else would never fire and is refused rather than reordered ahead of a deny.
fn behind_a_deny<R: Placeable>(shadowing: &R, rule: &R) -> Result<Placement> {
    let pattern = shadowing.match_pattern();
    if rule.verdict() == Verdict::Deny {
        return Ok(Placement::Redundant(Redundant::DeniedBy(
            pattern.to_string(),
        )));
    }
    bail!(
        "the {qualifier}deny rule for {pattern:?} already blocks every {matched} this rule could match, and the guest stops at the first matching rule, so this {qualifier}{verdict} rule would never fire — narrow that deny to the destinations you still mean to block, or reorder the file by hand if this rule is meant to win",
        qualifier = R::WORDS.qualifier,
        matched = R::WORDS.matched,
        verdict = verdict_word(rule.verdict()),
    )
}

/// The guest refuses to re-open a destination a binary-scoped rule claimed, so the only place this rule would reach the excluded callers is in front of that rule — a widening of the file's own grant, which is the developer's call to make, not ours.
fn reopens_a_scoped_rule<R: Placeable>(shadowing: &R, rule: &R) -> bool {
    shadowing.binaries().is_some() && rule.binaries().is_none() && rule.verdict() != Verdict::Deny
}

fn behind_a_scoped_rule<R: Placeable>(shadowing: &R, rule: &R) -> Result<Placement> {
    let pattern = shadowing.match_pattern();
    let qualifier = R::WORDS.qualifier;
    let scope = shadowing.binaries().unwrap_or_default().join(", ");
    let fix = if pattern == rule.match_pattern() {
        format!("drop the scoped rule with `lns policy remove {pattern}`")
    } else {
        format!("narrow or remove the {qualifier}rule for {pattern:?}")
    };
    bail!(
        "the {qualifier}rule for {pattern:?} is scoped to {scope}, and placing this {qualifier}{} rule in front of it would open the destination to every caller in the sandbox — {fix} first if that is what you mean",
        verdict_word(rule.verdict())
    )
}

fn fail_closed_note(pattern: &str) -> String {
    format!("Every other caller is now denied {pattern:?} without being asked")
}

fn placement_note<R: Placeable>(shadowing: &R, rule: &R) -> String {
    let placed = format!(
        "Placed it before the existing {}rule for {:?}, which covers the same destination and would otherwise pre-empt it.",
        R::WORDS.qualifier,
        shadowing.match_pattern()
    );
    match (shadowing.binaries(), rule.binaries()) {
        (Some(binaries), None) => format!(
            "{placed} That rule's scoping to {} no longer applies to {:?}.",
            binaries.join(", "),
            rule.match_pattern()
        ),
        // The guest refuses to re-open a destination a scoped rule claimed, so the rule behind this one is now dead for everyone it excludes — silence there would leave the file reading as if it still served them.
        (None, Some(_)) => format!(
            "{placed} {}, and that rule no longer serves them.",
            fail_closed_note(rule.match_pattern())
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
        .map(RuleRow::http)
        .chain(policy.network.egress.tcp.iter().map(RuleRow::tcp))
        .collect();
    let note = format!("No rules in {}", path.display());
    crate::output::emit_or_note(args.output.format, &rows, &note, writer)?;
    Ok(0)
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RuleRow {
    verdict: &'static str,
    /// Which egress table the rule lives in — `tcp` rules are spliced through unread, `http` rules are inspected.
    table: &'static str,
    pattern: String,
    binaries: Option<Vec<String>>,
    description: Option<String>,
}

impl RuleRow {
    fn http(rule: &RouteRule) -> Self {
        Self {
            verdict: verdict_word(rule.verdict),
            table: "http",
            pattern: rule.match_pattern.clone(),
            binaries: rule.binaries.clone(),
            description: rule.description.clone(),
        }
    }

    fn tcp(rule: &TcpEgressRule) -> Self {
        Self {
            verdict: verdict_word(rule.verdict),
            table: "tcp",
            pattern: rule.match_pattern.clone(),
            binaries: rule.binaries.clone(),
            description: rule.description.clone(),
        }
    }
}

impl crate::output::TableRow for RuleRow {
    const HEADERS: &'static [&'static str] =
        &["VERDICT", "TABLE", "PATTERN", "BINARIES", "DESCRIPTION"];

    fn cells(&self) -> Vec<String> {
        vec![
            self.verdict.to_string(),
            self.table.to_string(),
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
    let before = policy.network.egress.http.len() + policy.network.egress.tcp.len();
    policy
        .network
        .egress
        .http
        .retain(|rule| rule.match_pattern != args.pattern);
    policy
        .network
        .egress
        .tcp
        .retain(|rule| rule.match_pattern != args.pattern);
    let removed = before - policy.network.egress.http.len() - policy.network.egress.tcp.len();
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
    fn the_pre_emption_note_agrees_with_how_many_rules_it_names() {
        let two = Displaced {
            lifted_deny: None,
            patterns: vec!["\"db.internal\"".into(), "\"*.internal\"".into()],
        };
        assert_eq!(
            two.note("db.internal:5432").as_deref(),
            Some(
                "The HTTP rules for \"db.internal\", \"*.internal\" no longer apply to \"db.internal:5432\", which is now spliced raw."
            ),
            "a raw rule can displace several http rules at once, and the line has to read as one sentence either way"
        );
    }

    fn seeded(dir: &Path, rules: Vec<RouteRule>) -> PathBuf {
        let path = dir.join("lns-policy.yaml");
        let mut policy = Policy::default();
        policy.network.egress.http = rules;
        policy.save_atomic(&path).unwrap();
        path
    }

    fn tls_deny(pattern: &str) -> RouteRule {
        RouteRule {
            tls_terminate: true,
            ..RouteRule::deny_host(pattern)
        }
    }

    fn held(path: &Path) -> Vec<RouteRule> {
        Policy::load_or_default(path).unwrap().network.egress.http
    }

    #[test]
    fn a_catch_all_deny_the_file_already_carries_keeps_its_note() {
        // A `tlsTerminate` a deny never acted on must not read as a different rule:
        // re-running the deny would rewrite it and forget the note it carries.
        let dir = TempDir::new().unwrap();
        let path = seeded(
            dir.path(),
            vec![RouteRule {
                description: Some("closed for audit".into()),
                ..tls_deny("*")
            }],
        );

        let mut out = Vec::new();
        add_rule(
            &rule_args("*", None),
            Verdict::Deny,
            &[],
            dir.path(),
            &mut out,
        )
        .unwrap();

        assert_eq!(
            held(&path)[0].description.as_deref(),
            Some("closed for audit")
        );
        assert!(
            String::from_utf8(out).unwrap().contains("already blocks"),
            "the file already decides this, so nothing is rewritten"
        );
    }

    #[test]
    fn replacing_a_catch_all_leaves_no_other_catch_all_behind() {
        // The one it replaced is not the only one it has to take with it — a
        // hand-edited file can hold a second the gate never reaches either.
        let dir = TempDir::new().unwrap();
        let path = seeded(dir.path(), vec![RouteRule::allow_host("*"), tls_deny("*")]);

        let mut out = Vec::new();
        add_rule(
            &rule_args("*", None),
            Verdict::Deny,
            &[],
            dir.path(),
            &mut out,
        )
        .unwrap();

        assert_eq!(
            held(&path)
                .iter()
                .map(|r| (r.match_pattern.clone(), r.verdict))
                .collect::<Vec<_>>(),
            vec![("*".to_string(), Verdict::Deny)]
        );
    }

    #[test]
    fn replacing_a_catch_all_takes_the_copy_stranded_behind_it_along() {
        // A hand-edited file can hold the catch-all the gate never reaches. Replacing
        // the one in front of it must not leave the file with two.
        let dir = TempDir::new().unwrap();
        let mut seeded = Policy::default();
        seeded.add_rule(RouteRule::deny_host("*"));
        seeded.add_rule(RouteRule::allow_host("*"));
        seeded
            .save_atomic(&dir.path().join("lns-policy.yaml"))
            .unwrap();

        let mut out = Vec::new();
        add_rule(
            &rule_args("*", None),
            Verdict::Allow,
            &[],
            dir.path(),
            &mut out,
        )
        .unwrap();

        let held = Policy::load_or_default(&dir.path().join("lns-policy.yaml"))
            .unwrap()
            .network
            .egress
            .http;
        assert_eq!(
            held.iter()
                .map(|r| (r.match_pattern.as_str(), r.verdict))
                .collect::<Vec<_>>(),
            vec![("*", Verdict::Allow)]
        );
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
        let args = PolicyScopedRuleArgs::from_arg_matches(allow).unwrap();
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
    fn list_reports_each_verdict_and_its_description() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("lns-policy.yaml");
        let mut policy = Policy::default();
        policy.add_rule(RouteRule {
            match_pattern: "blocked.example".into(),
            verdict: Verdict::Deny,
            transport: Transport::Direct,
            scheme: None,
            description: Some("phishing kit".into()),
            tls_terminate: false,
            rules: Vec::new(),
            binaries: None,
        });
        policy.save_atomic(&path).unwrap();
        let mut out = Vec::new();
        list_rules(&scope_args(), dir.path(), &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("VERDICT  TABLE  PATTERN"), "got: {text}");
        assert!(
            text.contains("deny     http   blocked.example            phishing kit"),
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
