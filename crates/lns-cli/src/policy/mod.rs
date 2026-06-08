use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, bail};
use lns_policy::{Policy, RouteRule, Transport, Verdict};

use crate::cli::{PolicyCommand, PolicyRemoveArgs, PolicyRuleArgs, PolicyScopeArgs, TransportArg};
use crate::run::summary::policy_path;

pub fn run(cmd: &PolicyCommand, cwd: &Path, writer: &mut impl Write) -> Result<i32> {
    match cmd {
        PolicyCommand::Allow(args) => add_rule(args, Verdict::Allow, cwd, writer),
        PolicyCommand::Deny(args) => add_rule(args, Verdict::Deny, cwd, writer),
        PolicyCommand::List(args) => list_rules(args, cwd, writer),
        PolicyCommand::Remove(args) => remove_rule(args, cwd, writer),
    }
}

fn add_rule(
    args: &PolicyRuleArgs,
    verdict: Verdict,
    cwd: &Path,
    writer: &mut impl Write,
) -> Result<i32> {
    let path = policy_path(args.policy.as_deref(), cwd);
    let mut policy = Policy::load_or_default(&path)
        .with_context(|| format!("loading policy from {}", path.display()))?;
    policy.network.allowed_routes.push(RouteRule {
        match_pattern: args.pattern.clone(),
        verdict,
        transport: transport_of(args.transport),
        scheme: None,
        description: args.description.clone(),
        tls_terminate: false,
        rules: Vec::new(),
    });
    policy
        .save_atomic(&path)
        .with_context(|| format!("writing policy to {}", path.display()))?;
    writeln!(
        writer,
        "Added {} rule for {:?} to {}",
        verdict_word(verdict),
        args.pattern,
        path.display()
    )?;
    Ok(0)
}

fn list_rules(args: &PolicyScopeArgs, cwd: &Path, writer: &mut impl Write) -> Result<i32> {
    let path = policy_path(args.policy.as_deref(), cwd);
    let policy = Policy::load_or_default(&path)
        .with_context(|| format!("loading policy from {}", path.display()))?;
    let routes = &policy.network.allowed_routes;
    if routes.is_empty() {
        writeln!(writer, "No rules in {}", path.display())?;
        return Ok(0);
    }
    for rule in routes {
        match &rule.description {
            Some(desc) => writeln!(
                writer,
                "{}  {}  ({desc})",
                verdict_word(rule.verdict),
                rule.match_pattern
            )?,
            None => writeln!(
                writer,
                "{}  {}",
                verdict_word(rule.verdict),
                rule.match_pattern
            )?,
        }
    }
    Ok(0)
}

fn remove_rule(args: &PolicyRemoveArgs, cwd: &Path, writer: &mut impl Write) -> Result<i32> {
    let path = policy_path(args.policy.as_deref(), cwd);
    let mut policy = Policy::load_or_default(&path)
        .with_context(|| format!("loading policy from {}", path.display()))?;
    let before = policy.network.allowed_routes.len();
    policy
        .network
        .allowed_routes
        .retain(|rule| rule.match_pattern != args.pattern);
    if policy.network.allowed_routes.len() == before {
        bail!("no rule matching {:?} in {}", args.pattern, path.display());
    }
    policy
        .save_atomic(&path)
        .with_context(|| format!("writing policy to {}", path.display()))?;
    writeln!(
        writer,
        "Removed rule for {:?} from {}",
        args.pattern,
        path.display()
    )?;
    Ok(0)
}

fn transport_of(t: TransportArg) -> Transport {
    match t {
        TransportArg::Direct => Transport::Direct,
        TransportArg::Upstream => Transport::Upstream,
    }
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
            transport: TransportArg::Direct,
            policy: policy.map(Path::to_path_buf),
        }
    }

    #[test]
    fn allow_writes_an_allow_rule_with_the_requested_transport() {
        let dir = TempDir::new().unwrap();
        let mut args = rule_args("api.acme.corp", None);
        args.transport = TransportArg::Upstream;
        let mut out = Vec::new();
        let code = add_rule(&args, Verdict::Allow, dir.path(), &mut out).unwrap();
        assert_eq!(code, 0);
        let policy = Policy::load_or_default(&dir.path().join("lns-policy.yaml")).unwrap();
        assert_eq!(policy.network.allowed_routes.len(), 1);
        assert_eq!(policy.network.allowed_routes[0].verdict, Verdict::Allow);
        assert_eq!(
            policy.network.allowed_routes[0].transport,
            Transport::Upstream
        );
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
                .allowed_routes
                .iter()
                .any(|r| r.match_pattern == "evil.example" && r.verdict == Verdict::Deny)
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
        });
        policy.save_atomic(&path).unwrap();
        let mut out = Vec::new();
        list_rules(&PolicyScopeArgs { policy: None }, dir.path(), &mut out).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("ask  ask.example  (undecided)"),
            "got: {text}"
        );
    }

    #[test]
    fn list_reports_no_rules_for_an_empty_policy() {
        let dir = TempDir::new().unwrap();
        let mut out = Vec::new();
        list_rules(&PolicyScopeArgs { policy: None }, dir.path(), &mut out).unwrap();
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
        let err =
            add_rule(&rule_args("x", None), Verdict::Allow, dir.path(), &mut out).unwrap_err();
        assert!(format!("{err:#}").contains("loading policy from"));
    }
}
