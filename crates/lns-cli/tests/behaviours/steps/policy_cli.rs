use crate::runner::CliRun;
use crate::world::BehaviourWorld;
use cucumber::{given, then, when};
use lns_cli::policy::{
    self, PolicyCommand, PolicyRemoveArgs, PolicyRuleArgs, PolicyScopeArgs, PolicyScopedRuleArgs,
};
use lns_policy::{HttpRule, Policy, RouteRule, TcpEgressRule, Verdict};
use std::path::{Path, PathBuf};

fn cwd(world: &mut BehaviourWorld) -> PathBuf {
    if world.cwd.is_none() {
        world.cwd = Some(tempfile::TempDir::new().expect("create tempdir"));
    }
    world.cwd.as_ref().unwrap().path().to_path_buf()
}

fn run_policy(world: &mut BehaviourWorld, cmd: PolicyCommand) {
    let dir = cwd(world);
    let mut buf = Vec::<u8>::new();
    let run = match policy::run(&cmd, &dir, &mut buf) {
        Ok(exit_code) => CliRun {
            exit_code,
            output: String::from_utf8_lossy(&buf).into_owned(),
        },
        Err(e) => CliRun {
            exit_code: 1,
            output: format!("{e:#}"),
        },
    };
    world.result = Some(run);
}

fn rule_args(world: &mut BehaviourWorld, pattern: &str, policy: Option<PathBuf>) -> PolicyRuleArgs {
    let _ = cwd(world);
    PolicyRuleArgs {
        pattern: pattern.to_string(),
        description: None,
        policy,
    }
}

fn allow_args(
    world: &mut BehaviourWorld,
    pattern: &str,
    policy: Option<PathBuf>,
) -> PolicyScopedRuleArgs {
    PolicyScopedRuleArgs {
        rule: rule_args(world, pattern, policy),
        binary: Vec::new(),
    }
}

fn scoped_rule(pattern: &str, binaries: &[&str]) -> RouteRule {
    RouteRule {
        binaries: Some(binaries.iter().map(|b| (*b).to_string()).collect()),
        ..RouteRule::allow_host(pattern)
    }
}

fn load(world: &mut BehaviourWorld, file: &str) -> Policy {
    let dir = cwd(world);
    Policy::load_or_default(&dir.join(file)).expect("load policy file")
}

fn has_rule(policy: &Policy, pattern: &str, verdict: Verdict) -> bool {
    policy
        .network
        .egress
        .http
        .iter()
        .any(|r| r.match_pattern == pattern && r.verdict == verdict)
}

#[given(regex = r"^no sandbox is running$")]
fn no_sandbox_running(_world: &mut BehaviourWorld) {}

#[given(regex = r#"^a sandbox is running with "[^"]+" loaded in the current directory$"#)]
fn sandbox_running_with_policy(world: &mut BehaviourWorld) {
    let _ = cwd(world);
}

#[given(regex = r#"^the developer is in a directory with no "lns-policy\.yaml"$"#)]
fn directory_without_policy(world: &mut BehaviourWorld) {
    let dir = cwd(world);
    assert!(!dir.join("lns-policy.yaml").exists());
}

#[given(regex = r#"^"([^"]+)" has an allow rule for "([^"]+)" and a deny rule for "([^"]+)"$"#)]
fn policy_has_allow_and_deny(
    world: &mut BehaviourWorld,
    file: String,
    allow: String,
    deny: String,
) {
    let dir = cwd(world);
    let mut policy = Policy::default();
    policy.add_rule(RouteRule::allow_host(allow));
    policy.add_rule(RouteRule::deny_host(deny));
    policy.save_atomic(&dir.join(file)).expect("seed policy");
}

#[given(
    regex = r#"^"([^"]+)" has an allow rule for "([^"]+)" and a raw allow rule for "([^"]+)"$"#
)]
fn policy_has_allow_and_raw_allow(
    world: &mut BehaviourWorld,
    file: String,
    allow: String,
    raw: String,
) {
    let dir = cwd(world);
    let mut policy = Policy::default();
    policy.add_rule(RouteRule::allow_host(allow));
    policy
        .network
        .egress
        .tcp
        .push(TcpEgressRule::allow_destination(raw));
    policy.save_atomic(&dir.join(file)).expect("seed policy");
}

#[given(regex = r#"^"([^"]+)" has a raw (allow|deny|ask) rule for "([^"]+)"$"#)]
fn policy_has_raw_rule(world: &mut BehaviourWorld, file: String, verdict: String, pattern: String) {
    let dir = cwd(world);
    let mut policy = Policy::default();
    policy
        .network
        .egress
        .tcp
        .push(TcpEgressRule::new(pattern, verdict_named(&verdict)));
    policy.save_atomic(&dir.join(file)).expect("seed policy");
}

#[given(
    regex = r#"^"([^"]+)" has a raw ask rule for "([^"]+)" ahead of a raw allow rule for "([^"]+)"$"#
)]
fn policy_has_raw_ask_ahead_of_allow(
    world: &mut BehaviourWorld,
    file: String,
    ask: String,
    allow: String,
) {
    seed_raw_ask_ahead_of_allow(world, &file, &ask, &allow, None);
}

#[given(
    regex = r#"^"([^"]+)" has a raw ask rule for "([^"]+)" ahead of a raw allow rule for "([^"]+)" described as "([^"]+)"$"#
)]
fn policy_has_raw_ask_ahead_of_described_allow(
    world: &mut BehaviourWorld,
    file: String,
    ask: String,
    allow: String,
    description: String,
) {
    seed_raw_ask_ahead_of_allow(world, &file, &ask, &allow, Some(description));
}

fn seed_raw_ask_ahead_of_allow(
    world: &mut BehaviourWorld,
    file: &str,
    ask: &str,
    allow: &str,
    description: Option<String>,
) {
    let dir = cwd(world);
    let mut policy = Policy::default();
    policy
        .network
        .egress
        .tcp
        .push(TcpEgressRule::new(ask, Verdict::Ask));
    policy.network.egress.tcp.push(TcpEgressRule {
        description,
        ..TcpEgressRule::allow_destination(allow)
    });
    policy.save_atomic(&dir.join(file)).expect("seed policy");
}

#[given(regex = r#"^"([^"]+)" has a raw allow rule for "([^"]+)" and a deny rule for "([^"]+)"$"#)]
fn policy_has_raw_allow_and_deny(
    world: &mut BehaviourWorld,
    file: String,
    raw: String,
    deny: String,
) {
    let dir = cwd(world);
    let mut policy = Policy::default();
    policy
        .network
        .egress
        .tcp
        .push(TcpEgressRule::allow_destination(raw));
    policy.add_rule(RouteRule::deny_host(deny));
    policy.save_atomic(&dir.join(file)).expect("seed policy");
}

#[given(regex = r#"^"([^"]+)" has a raw allow rule for "([^"]+)" scoped to "([^"]+)"$"#)]
fn policy_has_scoped_raw_allow(
    world: &mut BehaviourWorld,
    file: String,
    pattern: String,
    binary: String,
) {
    let dir = cwd(world);
    let mut policy = Policy::default();
    policy.network.egress.tcp.push(TcpEgressRule {
        binaries: Some(vec![binary]),
        ..TcpEgressRule::allow_destination(pattern)
    });
    policy.save_atomic(&dir.join(file)).expect("seed policy");
}

fn verdict_named(verdict: &str) -> Verdict {
    match verdict {
        "allow" => Verdict::Allow,
        "deny" => Verdict::Deny,
        _ => Verdict::Ask,
    }
}

#[given(regex = r#"^"([^"]+)" has an allow rule for "([^"]+)"$"#)]
fn policy_has_allow(world: &mut BehaviourWorld, file: String, pattern: String) {
    let dir = cwd(world);
    let mut policy = Policy::default();
    policy.add_rule(RouteRule::allow_host(pattern));
    policy.save_atomic(&dir.join(file)).expect("seed policy");
}

#[given(regex = r#"^"([^"]+)" has an allow rule for "([^"]+)" described as "([^"]+)"$"#)]
fn policy_has_described_allow(
    world: &mut BehaviourWorld,
    file: String,
    pattern: String,
    description: String,
) {
    let dir = cwd(world);
    let mut policy = Policy::default();
    policy.add_rule(RouteRule {
        description: Some(description),
        ..RouteRule::allow_host(pattern)
    });
    policy.save_atomic(&dir.join(file)).expect("seed policy");
}

#[given(regex = r#"^"([^"]+)" has a deny rule for "([^"]+)"$"#)]
fn policy_has_deny(world: &mut BehaviourWorld, file: String, pattern: String) {
    let dir = cwd(world);
    let mut policy = Policy::default();
    policy.add_rule(RouteRule::deny_host(pattern));
    policy.save_atomic(&dir.join(file)).expect("seed policy");
}

#[given(
    regex = r#"^"([^"]+)" has a deny rule for "([^"]+)" ahead of an allow rule for "([^"]+)"$"#
)]
fn policy_has_deny_ahead_of_allow(
    world: &mut BehaviourWorld,
    file: String,
    denied: String,
    allowed: String,
) {
    let dir = cwd(world);
    let mut policy = Policy::default();
    policy.add_rule(RouteRule::deny_host(denied));
    policy.add_rule(RouteRule::allow_host(allowed));
    policy.save_atomic(&dir.join(file)).expect("seed policy");
}

#[given(
    regex = r#"^"([^"]+)" has an allow rule for "([^"]+)" ahead of a deny rule for "([^"]+)"$"#
)]
fn policy_has_allow_ahead_of_deny(
    world: &mut BehaviourWorld,
    file: String,
    allowed: String,
    denied: String,
) {
    let dir = cwd(world);
    let mut policy = Policy::default();
    policy.add_rule(RouteRule::allow_host(allowed));
    policy.add_rule(RouteRule::deny_host(denied));
    policy.save_atomic(&dir.join(file)).expect("seed policy");
}

#[given(regex = r#"^"([^"]+)" has a TLS-terminating allow rule for "([^"]+)"$"#)]
fn policy_has_tls_terminating_allow(world: &mut BehaviourWorld, file: String, pattern: String) {
    let dir = cwd(world);
    let mut policy = Policy::default();
    policy.add_rule(RouteRule {
        tls_terminate: true,
        ..RouteRule::allow_host(pattern)
    });
    policy.save_atomic(&dir.join(file)).expect("seed policy");
}

#[given(
    regex = r#"^"([^"]+)" has a TLS-terminating allow rule for "([^"]+)" described as "([^"]+)"$"#
)]
fn policy_has_described_tls_terminating_allow(
    world: &mut BehaviourWorld,
    file: String,
    pattern: String,
    description: String,
) {
    let dir = cwd(world);
    let mut policy = Policy::default();
    policy.add_rule(RouteRule {
        tls_terminate: true,
        description: Some(description),
        ..RouteRule::allow_host(pattern)
    });
    policy.save_atomic(&dir.join(file)).expect("seed policy");
}

#[given(
    regex = r#"^"([^"]+)" has an allow rule for "([^"]+)" ahead of a deny rule for "([^"]+)" described as "([^"]+)"$"#
)]
fn policy_has_allow_ahead_of_described_deny(
    world: &mut BehaviourWorld,
    file: String,
    allowed: String,
    denied: String,
    description: String,
) {
    let dir = cwd(world);
    let mut policy = Policy::default();
    policy.add_rule(RouteRule::allow_host(allowed));
    policy.add_rule(RouteRule {
        description: Some(description),
        ..RouteRule::deny_host(denied)
    });
    policy.save_atomic(&dir.join(file)).expect("seed policy");
}

#[given(
    regex = r#"^"([^"]+)" has an allow rule for "([^"]+)" scoped to "([^"]+)" ahead of an unscoped allow rule for "([^"]+)"$"#
)]
fn policy_has_scoped_ahead_of_unscoped_allow(
    world: &mut BehaviourWorld,
    file: String,
    scoped: String,
    binary: String,
    unscoped: String,
) {
    let dir = cwd(world);
    let mut policy = Policy::default();
    policy.add_rule(scoped_rule(&scoped, &[&binary]));
    policy.add_rule(RouteRule::allow_host(unscoped));
    policy.save_atomic(&dir.join(file)).expect("seed policy");
}

#[given(
    regex = r#"^"([^"]+)" has an allow rule for "([^"]+)" restricted to GET requests ahead of an unrestricted allow rule for "([^"]+)"$"#
)]
fn policy_has_restricted_ahead_of_unrestricted_allow(
    world: &mut BehaviourWorld,
    file: String,
    restricted: String,
    unrestricted: String,
) {
    let dir = cwd(world);
    let mut policy = Policy::default();
    policy.add_rule(RouteRule {
        rules: vec![HttpRule {
            method: Some("GET".into()),
            path: None,
        }],
        ..RouteRule::allow_host(restricted)
    });
    policy.add_rule(RouteRule::allow_host(unrestricted));
    policy.save_atomic(&dir.join(file)).expect("seed policy");
}

#[given(regex = r#"^"([^"]+)" has an allow rule for "([^"]+)" restricted to GET requests$"#)]
fn policy_has_get_restricted_allow(world: &mut BehaviourWorld, file: String, pattern: String) {
    let dir = cwd(world);
    let mut policy = Policy::default();
    policy.add_rule(RouteRule {
        rules: vec![HttpRule {
            method: Some("GET".into()),
            path: None,
        }],
        ..RouteRule::allow_host(pattern)
    });
    policy.save_atomic(&dir.join(file)).expect("seed policy");
}

#[given(regex = r#"^"([^"]+)" uses the removed allowedRoutes key for "([^"]+)"$"#)]
fn policy_uses_the_removed_key(world: &mut BehaviourWorld, file: String, pattern: String) {
    let dir = cwd(world);
    std::fs::write(
        dir.join(file),
        format!(
            "network:\n  allowedRoutes:\n    - match: {pattern}\n      verdict: allow\n  defaultVerdict: ask\n"
        ),
    )
    .expect("seed a policy file in the pre-egress shape");
}

#[given(regex = r#"^"([^"]+)" has no rule for "([^"]+)"$"#)]
fn policy_has_no_rule(world: &mut BehaviourWorld, file: String, _pattern: String) {
    let dir = cwd(world);
    Policy::default()
        .save_atomic(&dir.join(file))
        .expect("seed empty policy");
}

#[when(regex = r#"^the developer adds an allow rule for "([^"]+)" to "([^"]+)"$"#)]
fn add_allow_to_file(world: &mut BehaviourWorld, pattern: String, file: String) {
    let args = allow_args(world, &pattern, Some(PathBuf::from(file)));
    run_policy(world, PolicyCommand::Allow(args));
}

#[when(regex = r#"^the developer adds an allow rule for "([^"]+)" without passing --policy$"#)]
fn add_allow_default_path(world: &mut BehaviourWorld, pattern: String) {
    let args = allow_args(world, &pattern, None);
    run_policy(world, PolicyCommand::Allow(args));
}

#[when(regex = r#"^the developer adds an allow rule for "([^"]+)" with --policy "([^"]+)"$"#)]
fn add_allow_explicit_path(world: &mut BehaviourWorld, pattern: String, path: String) {
    // Map the scenario's illustrative absolute path onto a cwd-local file so the
    // test stays isolated; the behaviour under test is "explicit path is honoured".
    let basename = Path::new(&path)
        .file_name()
        .expect("explicit path has a file name");
    let explicit = cwd(world).join(basename);
    let args = allow_args(world, &pattern, Some(explicit));
    run_policy(world, PolicyCommand::Allow(args));
}

#[when(regex = r#"^the developer adds an allow rule for "([^"]+)" with description "([^"]+)"$"#)]
fn add_allow_with_description(world: &mut BehaviourWorld, pattern: String, description: String) {
    let mut args = allow_args(world, &pattern, None);
    args.rule.description = Some(description);
    run_policy(world, PolicyCommand::Allow(args));
}

#[given(regex = r#"^"([^"]+)" has an allow rule for "([^"]+)" scoped to "([^"]+)"$"#)]
fn policy_has_scoped_allow(
    world: &mut BehaviourWorld,
    file: String,
    pattern: String,
    binary: String,
) {
    let dir = cwd(world);
    let mut policy = Policy::default();
    policy.add_rule(scoped_rule(&pattern, &[&binary]));
    policy.save_atomic(&dir.join(file)).expect("seed policy");
}

#[given(regex = r#"^"([^"]+)" has an allow rule for "([^"]+)" scoped to "([^"]+)" and "([^"]+)"$"#)]
fn policy_has_multi_scoped_allow(
    world: &mut BehaviourWorld,
    file: String,
    pattern: String,
    first: String,
    second: String,
) {
    let dir = cwd(world);
    let mut policy = Policy::default();
    policy.add_rule(scoped_rule(&pattern, &[&first, &second]));
    policy.save_atomic(&dir.join(file)).expect("seed policy");
}

#[given(
    regex = r#"^"([^"]+)" has an allow rule for "([^"]+)" and a scoped allow rule for "([^"]+)"$"#
)]
fn policy_has_open_and_scoped_allow(
    world: &mut BehaviourWorld,
    file: String,
    open: String,
    scoped: String,
) {
    let dir = cwd(world);
    let mut policy = Policy::default();
    policy.add_rule(RouteRule::allow_host(open));
    policy.add_rule(scoped_rule(&scoped, &["/usr/bin/git"]));
    policy.save_atomic(&dir.join(file)).expect("seed policy");
}

#[when(regex = r#"^the developer adds an allow rule for "([^"]+)" scoped to "([^"]+)"$"#)]
fn add_scoped_allow(world: &mut BehaviourWorld, pattern: String, binary: String) {
    let mut args = allow_args(world, &pattern, None);
    args.binary = vec![binary];
    run_policy(world, PolicyCommand::Allow(args));
}

#[when(
    regex = r#"^the developer adds an allow rule for "([^"]+)" scoped to "([^"]+)" and "([^"]+)"$"#
)]
fn add_multi_scoped_allow(
    world: &mut BehaviourWorld,
    pattern: String,
    first: String,
    second: String,
) {
    let mut args = allow_args(world, &pattern, None);
    args.binary = vec![first, second];
    run_policy(world, PolicyCommand::Allow(args));
}

#[when(regex = r#"^the developer denies "([^"]+)"$"#)]
fn add_deny(world: &mut BehaviourWorld, pattern: String) {
    let args = rule_args(world, &pattern, None);
    run_policy(world, PolicyCommand::Deny(args));
}

#[when(regex = r#"^the developer denies "([^"]+)" with description "([^"]+)"$"#)]
fn add_deny_with_description(world: &mut BehaviourWorld, pattern: String, description: String) {
    let mut args = rule_args(world, &pattern, None);
    args.description = Some(description);
    run_policy(world, PolicyCommand::Deny(args));
}

#[when(regex = r#"^the developer allows the raw destination "([^"]+)"$"#)]
fn allow_raw_destination(world: &mut BehaviourWorld, pattern: String) {
    let args = allow_args(world, &pattern, None);
    run_policy(world, PolicyCommand::AllowTcp(args));
}

#[when(
    regex = r#"^the developer allows the raw destination "([^"]+)" with description "([^"]+)"$"#
)]
fn allow_raw_destination_with_description(
    world: &mut BehaviourWorld,
    pattern: String,
    description: String,
) {
    let mut args = allow_args(world, &pattern, None);
    args.rule.description = Some(description);
    run_policy(world, PolicyCommand::AllowTcp(args));
}

#[when(regex = r#"^the developer allows the raw destination "([^"]+)" scoped to "([^"]+)"$"#)]
fn allow_scoped_raw_destination(world: &mut BehaviourWorld, pattern: String, binary: String) {
    let mut args = allow_args(world, &pattern, None);
    args.binary = vec![binary];
    run_policy(world, PolicyCommand::AllowTcp(args));
}

#[when(regex = r#"^the developer asks about the raw destination "([^"]+)" scoped to "([^"]+)"$"#)]
fn ask_scoped_raw_destination(world: &mut BehaviourWorld, pattern: String, binary: String) {
    let mut args = allow_args(world, &pattern, None);
    args.binary = vec![binary];
    run_policy(world, PolicyCommand::AskTcp(args));
}

#[when(regex = r#"^the developer denies the raw destination "([^"]+)"$"#)]
fn deny_raw_destination(world: &mut BehaviourWorld, pattern: String) {
    let args = rule_args(world, &pattern, None);
    run_policy(world, PolicyCommand::DenyTcp(args));
}

#[when(regex = r#"^the developer asks about the raw destination "([^"]+)"$"#)]
fn ask_raw_destination(world: &mut BehaviourWorld, pattern: String) {
    let args = allow_args(world, &pattern, None);
    run_policy(world, PolicyCommand::AskTcp(args));
}

#[when(regex = r"^the developer lists rules$")]
fn list_rules(world: &mut BehaviourWorld) {
    let _ = cwd(world);
    run_policy(
        world,
        PolicyCommand::List(PolicyScopeArgs {
            policy: None,
            output: lns_cli::output::OutputArgs {
                format: lns_cli::output::Format::Table,
            },
        }),
    );
}

#[when(regex = r"^the developer lists rules as JSON$")]
fn list_rules_as_json(world: &mut BehaviourWorld) {
    let _ = cwd(world);
    run_policy(
        world,
        PolicyCommand::List(PolicyScopeArgs {
            policy: None,
            output: lns_cli::output::OutputArgs {
                format: lns_cli::output::Format::Json,
            },
        }),
    );
}

#[when(regex = r#"^the developer removes the rule matching "([^"]+)"$"#)]
fn remove_rule(world: &mut BehaviourWorld, pattern: String) {
    let _ = cwd(world);
    run_policy(
        world,
        PolicyCommand::Remove(PolicyRemoveArgs {
            pattern,
            policy: None,
        }),
    );
}

#[when(regex = r#"^the developer tries to remove a rule for "([^"]+)"$"#)]
fn try_remove_rule(world: &mut BehaviourWorld, pattern: String) {
    let _ = cwd(world);
    run_policy(
        world,
        PolicyCommand::Remove(PolicyRemoveArgs {
            pattern,
            policy: None,
        }),
    );
}

#[then(regex = r#"^"([^"]+)" contains an allow rule for "([^"]+)"$"#)]
fn file_contains_allow(
    world: &mut BehaviourWorld,
    file: String,
    pattern: String,
) -> Result<(), String> {
    let policy = load(world, &file);
    if has_rule(&policy, &pattern, Verdict::Allow) {
        Ok(())
    } else {
        Err(format!("{file} has no allow rule for {pattern}"))
    }
}

#[then(regex = r#"^"([^"]+)" in the current directory contains an allow rule for "([^"]+)"$"#)]
fn cwd_file_contains_allow(
    world: &mut BehaviourWorld,
    file: String,
    pattern: String,
) -> Result<(), String> {
    file_contains_allow(world, file, pattern)
}

#[then(regex = r#"^"([^"]+)" is created in the current directory$"#)]
fn file_is_created(world: &mut BehaviourWorld, file: String) -> Result<(), String> {
    let dir = cwd(world);
    if dir.join(&file).exists() {
        Ok(())
    } else {
        Err(format!("{file} was not created in the current directory"))
    }
}

#[then(regex = r#"^it contains an allow rule for "([^"]+)"$"#)]
fn it_contains_allow(world: &mut BehaviourWorld, pattern: String) -> Result<(), String> {
    file_contains_allow(world, "lns-policy.yaml".to_string(), pattern)
}

#[then(regex = r#"^its defaultVerdict is "([^"]+)"$"#)]
fn its_default_verdict_is(world: &mut BehaviourWorld, expected: String) -> Result<(), String> {
    let policy = load(world, "lns-policy.yaml");
    let actual = match policy.network.default_verdict {
        Verdict::Allow => "allow",
        Verdict::Deny => "deny",
        Verdict::Ask => "ask",
    };
    if actual == expected {
        Ok(())
    } else {
        Err(format!("defaultVerdict is {actual}, expected {expected}"))
    }
}

#[then(regex = r#"^"([^"]+)" contains the allow rule$"#)]
fn explicit_file_contains_rule(world: &mut BehaviourWorld, path: String) -> Result<(), String> {
    let basename = Path::new(&path)
        .file_name()
        .and_then(|s| s.to_str())
        .expect("path has a file name")
        .to_string();
    let policy = load(world, &basename);
    if has_rule(&policy, "api.linear.app", Verdict::Allow) {
        Ok(())
    } else {
        Err(format!("{basename} has no allow rule"))
    }
}

#[then(regex = r#"^"\./lns-policy\.yaml" is not created$"#)]
fn default_file_not_created(world: &mut BehaviourWorld) -> Result<(), String> {
    let dir = cwd(world);
    if dir.join("lns-policy.yaml").exists() {
        Err("default lns-policy.yaml should not have been created".to_string())
    } else {
        Ok(())
    }
}

#[then(regex = r#"^"([^"]+)" contains the allow rule with the description$"#)]
fn file_contains_described_rule(world: &mut BehaviourWorld, file: String) -> Result<(), String> {
    let policy = load(world, &file);
    let described = policy
        .network
        .egress
        .http
        .iter()
        .any(|r| r.verdict == Verdict::Allow && r.description.is_some());
    if described {
        Ok(())
    } else {
        Err(format!("{file} has no allow rule carrying a description"))
    }
}

#[then(regex = r#"^the error names "([^"]+)"$"#)]
fn error_names(world: &mut BehaviourWorld, needle: String) -> Result<(), String> {
    let out = world
        .result
        .as_ref()
        .map(|r| r.output.clone())
        .unwrap_or_default();
    if out.contains(&needle) {
        Ok(())
    } else {
        Err(format!("error does not name {needle:?}:\n{out}"))
    }
}

#[then(regex = r"^the output shows both rules with their verdicts$")]
fn output_shows_both_rules(world: &mut BehaviourWorld) -> Result<(), String> {
    let out = world
        .result
        .as_ref()
        .map(|r| r.output.clone())
        .unwrap_or_default();
    for needle in ["allow", "api.linear.app", "deny", "evil.example"] {
        if !out.contains(needle) {
            return Err(format!("list output missing {needle:?}:\n{out}"));
        }
    }
    Ok(())
}

#[then(regex = r#"^"([^"]+)" no longer contains a rule for "([^"]+)"$"#)]
fn file_no_longer_contains(
    world: &mut BehaviourWorld,
    file: String,
    pattern: String,
) -> Result<(), String> {
    let policy = load(world, &file);
    if policy
        .network
        .egress
        .http
        .iter()
        .any(|r| r.match_pattern == pattern)
    {
        Err(format!("{file} still contains a rule for {pattern}"))
    } else {
        Ok(())
    }
}

#[then(regex = r#"^"([^"]+)" scopes the allow rule for "([^"]+)" to "([^"]+)"$"#)]
fn file_scopes_allow(
    world: &mut BehaviourWorld,
    file: String,
    pattern: String,
    binaries: String,
) -> Result<(), String> {
    let expected: Vec<String> = binaries.split(',').map(str::to_string).collect();
    let policy = load(world, &file);
    let found = policy
        .network
        .egress
        .http
        .iter()
        .find(|r| r.match_pattern == pattern && r.verdict == Verdict::Allow)
        .ok_or_else(|| format!("{file} has no allow rule for {pattern}"))?;
    if found.binaries.as_deref() == Some(expected.as_slice()) {
        Ok(())
    } else {
        Err(format!(
            "expected {pattern} scoped to {expected:?}, got {:?}",
            found.binaries
        ))
    }
}

#[then(regex = r#"^"([^"]+)" contains a raw (allow|deny|ask) rule for "([^"]+)"$"#)]
fn file_contains_raw_rule(
    world: &mut BehaviourWorld,
    file: String,
    verdict: String,
    pattern: String,
) -> Result<(), String> {
    let expected = match verdict.as_str() {
        "allow" => Verdict::Allow,
        "deny" => Verdict::Deny,
        _ => Verdict::Ask,
    };
    let policy = load(world, &file);
    if policy
        .network
        .egress
        .tcp
        .iter()
        .any(|r| r.match_pattern == pattern && r.verdict == expected)
    {
        Ok(())
    } else {
        Err(format!(
            "{file} has no raw {verdict} rule for {pattern}: {:?}",
            policy.network.egress.tcp
        ))
    }
}

#[then(regex = r#"^"([^"]+)" contains a raw (allow|ask) rule for "([^"]+)" scoped to "([^"]+)"$"#)]
fn file_contains_scoped_raw_rule(
    world: &mut BehaviourWorld,
    file: String,
    verdict: String,
    pattern: String,
    binary: String,
) -> Result<(), String> {
    let expected = verdict_named(&verdict);
    let policy = load(world, &file);
    if policy.network.egress.tcp.iter().any(|r| {
        r.match_pattern == pattern
            && r.verdict == expected
            && r.binaries.as_deref() == Some(&[binary.clone()][..])
    }) {
        Ok(())
    } else {
        Err(format!(
            "{file} has no raw {verdict} rule for {pattern} scoped to {binary}: {:?}",
            policy.network.egress.tcp
        ))
    }
}

#[then(
    regex = r#"^"([^"]+)" lists the raw rule scoped to "([^"]+)" before the raw rule for "([^"]+)"$"#
)]
fn scoped_raw_rule_comes_first(
    world: &mut BehaviourWorld,
    file: String,
    binary: String,
    pattern: String,
) -> Result<(), String> {
    let tcp = load(world, &file).network.egress.tcp;
    let scoped = tcp.iter().position(|r| r.binaries.is_some());
    let open = tcp
        .iter()
        .position(|r| r.match_pattern == pattern && r.binaries.is_none());
    match (scoped, open) {
        (Some(scoped), Some(open)) if scoped < open => Ok(()),
        _ => Err(format!(
            "expected the raw rule scoped to {binary} ahead of the open raw rule for {pattern}: {tcp:?}"
        )),
    }
}

#[then(regex = r#"^"([^"]+)" no longer contains a raw rule for "([^"]+)"$"#)]
fn file_no_longer_contains_raw(
    world: &mut BehaviourWorld,
    file: String,
    pattern: String,
) -> Result<(), String> {
    let policy = load(world, &file);
    if policy
        .network
        .egress
        .tcp
        .iter()
        .any(|r| r.match_pattern == pattern)
    {
        Err(format!("{file} still has a raw rule for {pattern}"))
    } else {
        Ok(())
    }
}

#[then(regex = r"^the failure says a relative path can never match the kernel-resolved path$")]
fn failure_explains_relative_path(world: &mut BehaviourWorld) -> Result<(), String> {
    output_mentions(world, "is not an absolute path")?;
    output_mentions(world, "kernel-resolved /proc/<pid>/exe")
}

#[then(regex = r"^the failure says a \.\. segment can never match the kernel-resolved path$")]
fn failure_explains_parent_segment(world: &mut BehaviourWorld) -> Result<(), String> {
    output_mentions(world, "climbs through a \"..\" segment")?;
    output_mentions(world, "kernel-resolved /proc/<pid>/exe")
}

#[then(regex = r"^the error explains that a raw destination needs a port$")]
fn error_explains_port_requirement(world: &mut BehaviourWorld) -> Result<(), String> {
    output_mentions(world, "must specify a port")
}

#[then(regex = r"^the error explains it would open the raw destination to every caller$")]
fn error_explains_reopened_raw_scope(world: &mut BehaviourWorld) -> Result<(), String> {
    output_mentions(world, "open the destination to every caller in the sandbox")
}

#[then(regex = r#"^the error tells the developer to narrow the raw rule for "([^"]+)"$"#)]
fn error_names_the_raw_rule_to_narrow(
    world: &mut BehaviourWorld,
    pattern: String,
) -> Result<(), String> {
    output_mentions(
        world,
        &format!("narrow or remove the raw rule for {pattern:?}"),
    )
}

#[then(regex = r"^the error explains the HTTP deny would stop applying$")]
fn error_explains_lifted_http_deny(world: &mut BehaviourWorld) -> Result<(), String> {
    output_mentions(world, "would no longer apply")?;
    output_mentions(world, "narrow or remove that deny first")
}

#[then(regex = r#"^the output says the HTTP rule for "([^"]+)" no longer applies$"#)]
fn output_says_http_rule_pre_empted(
    world: &mut BehaviourWorld,
    pattern: String,
) -> Result<(), String> {
    output_mentions(
        world,
        &format!("The HTTP rule for {pattern:?} no longer applies"),
    )
}

#[then(regex = r"^the output says the broader raw deny already blocks it$")]
fn output_says_broader_raw_deny_covers(world: &mut BehaviourWorld) -> Result<(), String> {
    output_mentions(world, "so this deny adds nothing")
}

#[then(regex = r"^the error explains the raw rule would never fire$")]
fn error_explains_dead_raw_rule(world: &mut BehaviourWorld) -> Result<(), String> {
    output_mentions(world, "would never fire")
}

#[then(regex = r"^the output says the scoping of the rule behind it no longer applies$")]
fn output_says_scoping_stopped_applying(world: &mut BehaviourWorld) -> Result<(), String> {
    output_mentions(world, "Placed it before the existing raw rule")?;
    output_mentions(world, "scoping to /usr/bin/psql no longer applies")
}

#[then(regex = r"^the output says it was placed before the existing raw rule$")]
fn output_says_placed_before_raw(world: &mut BehaviourWorld) -> Result<(), String> {
    output_mentions(world, "Placed it before the existing raw rule")
}

#[then(regex = r"^the output says the raw rule is already present$")]
fn output_says_raw_already_present(world: &mut BehaviourWorld) -> Result<(), String> {
    output_mentions(world, "is already in")
}

#[then(regex = r#"^the raw allow rule for "([^"]+)" sits ahead of the raw ask rule$"#)]
fn raw_allow_precedes_ask(world: &mut BehaviourWorld, pattern: String) -> Result<(), String> {
    let policy = load(world, "lns-policy.yaml");
    let verdicts: Vec<Verdict> = policy
        .network
        .egress
        .tcp
        .iter()
        .filter(|r| r.match_pattern == pattern)
        .map(|r| r.verdict)
        .collect();
    if verdicts != vec![Verdict::Allow, Verdict::Ask] {
        return Err(format!(
            "the gate stops at the first match, so an allow behind the ask rule is dead: {verdicts:?}"
        ));
    }
    Ok(())
}

#[then(regex = r#"^"([^"]+)" has exactly one raw (allow|deny|ask) rule for "([^"]+)"$"#)]
fn file_has_one_raw_rule_of_verdict(
    world: &mut BehaviourWorld,
    file: String,
    verdict: String,
    pattern: String,
) -> Result<(), String> {
    let expected = verdict_named(&verdict);
    let policy = load(world, &file);
    let count = policy
        .network
        .egress
        .tcp
        .iter()
        .filter(|r| r.match_pattern == pattern && r.verdict == expected)
        .count();
    if count == 1 {
        return Ok(());
    }
    Err(format!(
        "a copy stranded behind the rule that pre-empts it is the same rule, not one more to keep: {file} has {count} raw {verdict} rules for {pattern}"
    ))
}

#[then(regex = r#"^"([^"]+)" describes the raw allow rule for "([^"]+)" as "([^"]+)"$"#)]
fn file_describes_raw_allow(
    world: &mut BehaviourWorld,
    file: String,
    pattern: String,
    description: String,
) -> Result<(), String> {
    let policy = load(world, &file);
    let described = policy.network.egress.tcp.iter().any(|r| {
        r.match_pattern == pattern
            && r.verdict == Verdict::Allow
            && r.description.as_deref() == Some(description.as_str())
    });
    if described {
        return Ok(());
    }
    Err(format!(
        "a command passing no --description is not asking to forget the note the rule already carries: {:?}",
        policy.network.egress.tcp
    ))
}

#[then(regex = r#"^"([^"]+)" has exactly one raw rule for "([^"]+)"$"#)]
fn file_has_one_raw_rule(
    world: &mut BehaviourWorld,
    file: String,
    pattern: String,
) -> Result<(), String> {
    let policy = load(world, &file);
    let count = policy
        .network
        .egress
        .tcp
        .iter()
        .filter(|r| r.match_pattern == pattern)
        .count();
    if count == 1 {
        Ok(())
    } else {
        Err(format!("{file} has {count} raw rules for {pattern}"))
    }
}

fn output_mentions(world: &BehaviourWorld, needle: &str) -> Result<(), String> {
    let out = world
        .result
        .as_ref()
        .map(|r| r.output.clone())
        .unwrap_or_default();
    if out.contains(needle) {
        Ok(())
    } else {
        Err(format!("failure output missing {needle:?}:\n{out}"))
    }
}

#[then(regex = r#"^the output shows the rule scoped to "([^"]+)"$"#)]
fn output_shows_scoping(world: &mut BehaviourWorld, binary: String) -> Result<(), String> {
    let out = world
        .result
        .as_ref()
        .map(|r| r.output.clone())
        .unwrap_or_default();
    for needle in ["BINARIES", binary.as_str()] {
        if !out.contains(needle) {
            return Err(format!("list output missing {needle:?}:\n{out}"));
        }
    }
    Ok(())
}

#[then(
    regex = r#"^the output shows "([^"]+)" in the "([^"]+)" table and "([^"]+)" in the "([^"]+)" table$"#
)]
fn output_shows_table_column(
    world: &mut BehaviourWorld,
    first: String,
    first_table: String,
    second: String,
    second_table: String,
) -> Result<(), String> {
    let out = world
        .result
        .as_ref()
        .map(|r| r.output.clone())
        .unwrap_or_default();
    for (pattern, table) in [(first, first_table), (second, second_table)] {
        let row = out
            .lines()
            .find(|line| line.contains(&pattern))
            .ok_or_else(|| format!("list output has no row for {pattern:?}:\n{out}"))?;
        if !row.split_whitespace().any(|cell| cell == table) {
            return Err(format!(
                "row for {pattern:?} is not in table {table:?}: {row}"
            ));
        }
    }
    Ok(())
}

fn position_of(
    world: &mut BehaviourWorld,
    file: &str,
    of: impl Fn(&RouteRule) -> bool,
    what: &str,
) -> Result<usize, String> {
    load(world, file)
        .network
        .egress
        .http
        .iter()
        .position(of)
        .ok_or_else(|| format!("{file} has no {what}"))
}

fn scoped_to(binaries: String) -> impl Fn(&RouteRule) -> bool {
    let binaries: Vec<String> = binaries.split(',').map(str::to_string).collect();
    move |rule| rule.binaries.as_deref() == Some(binaries.as_slice())
}

fn open_rule_for(pattern: String) -> impl Fn(&RouteRule) -> bool {
    move |rule| rule.match_pattern == pattern && rule.binaries.is_none()
}

fn deny_rule_for(pattern: String) -> impl Fn(&RouteRule) -> bool {
    move |rule| rule.match_pattern == pattern && rule.verdict == Verdict::Deny
}

#[then(
    regex = r#"^"([^"]+)" lists the deny rule for "([^"]+)" before the rule scoped to "([^"]+)"$"#
)]
fn deny_comes_first(
    world: &mut BehaviourWorld,
    file: String,
    pattern: String,
    binary: String,
) -> Result<(), String> {
    let deny = position_of(
        world,
        &file,
        deny_rule_for(pattern.clone()),
        &format!("deny rule for {pattern}"),
    )?;
    let scoped = position_of(
        world,
        &file,
        scoped_to(binary.clone()),
        &format!("rule scoped to {binary}"),
    )?;
    if deny < scoped {
        Ok(())
    } else {
        Err(format!(
            "the scoped allow at {scoped} still serves its listed callers, so the deny at {deny} never blocks them"
        ))
    }
}

fn allow_rule_for(pattern: String) -> impl Fn(&RouteRule) -> bool {
    move |rule| rule.match_pattern == pattern && rule.verdict == Verdict::Allow
}

#[then(
    regex = r#"^"([^"]+)" lists the deny rule for "([^"]+)" before the allow rule for "([^"]+)"$"#
)]
fn deny_comes_before_the_allow(
    world: &mut BehaviourWorld,
    file: String,
    denied: String,
    allowed: String,
) -> Result<(), String> {
    let deny = position_of(
        world,
        &file,
        deny_rule_for(denied.clone()),
        &format!("deny rule for {denied}"),
    )?;
    let allow = position_of(
        world,
        &file,
        allow_rule_for(allowed.clone()),
        &format!("allow rule for {allowed}"),
    )?;
    if deny < allow {
        Ok(())
    } else {
        Err(format!(
            "the gate stops at the first match, so the allow at {allow} serves the destination and the deny at {deny} blocks nobody"
        ))
    }
}

#[then(regex = r#"^"([^"]+)" terminates TLS on the rule scoped to "([^"]+)"$"#)]
fn file_terminates_tls_on_the_scoped_rule(
    world: &mut BehaviourWorld,
    file: String,
    binary: String,
) -> Result<(), String> {
    let scoped = scoped_to(binary.clone());
    let found = load(world, &file)
        .network
        .egress
        .http
        .iter()
        .find(|rule| scoped(rule))
        .cloned()
        .ok_or_else(|| format!("{file} has no rule scoped to {binary}"))?;
    if found.tls_terminate {
        Ok(())
    } else {
        Err(format!(
            "the rule it was placed in front of terminates TLS, so dropping it here stops intercepting the destination for {binary}: {found:?}"
        ))
    }
}

#[then(regex = r#"^"([^"]+)" does not terminate TLS on the deny rule for "([^"]+)"$"#)]
fn file_does_not_terminate_tls_on_the_deny(
    world: &mut BehaviourWorld,
    file: String,
    pattern: String,
) -> Result<(), String> {
    let deny = deny_rule_for(pattern.clone());
    let found = load(world, &file)
        .network
        .egress
        .http
        .iter()
        .find(|rule| deny(rule))
        .cloned()
        .ok_or_else(|| format!("{file} has no deny rule for {pattern}"))?;
    if found.tls_terminate {
        Err(format!(
            "a deny blocks the request before there is anything to intercept, so terminating TLS on it is noise in the file: {found:?}"
        ))
    } else {
        Ok(())
    }
}

#[then(regex = r#"^"([^"]+)" describes the (allow|deny) rule for "([^"]+)" as "([^"]+)"$"#)]
fn file_describes_rule(
    world: &mut BehaviourWorld,
    file: String,
    verdict: String,
    pattern: String,
    description: String,
) -> Result<(), String> {
    let wanted = match verdict.as_str() {
        "allow" => Verdict::Allow,
        _ => Verdict::Deny,
    };
    let found = load(world, &file)
        .network
        .egress
        .http
        .iter()
        .find(|rule| rule.match_pattern == pattern && rule.verdict == wanted)
        .cloned()
        .ok_or_else(|| format!("{file} has no {verdict} rule for {pattern}"))?;
    if found.description.as_deref() == Some(description.as_str()) {
        Ok(())
    } else {
        Err(format!(
            "a command passing no --description is not a request to forget the note the rule carries, got {:?}",
            found.description
        ))
    }
}

#[then(regex = r"^the output says the placed rule terminates TLS too$")]
fn output_reports_inherited_tls(world: &mut BehaviourWorld) -> Result<(), String> {
    output_mentions(world, "terminates TLS as that rule does")
}

#[then(regex = r#"^"([^"]+)" lists the rule scoped to "([^"]+)" before the rule for "([^"]+)"$"#)]
fn scoped_rule_comes_first(
    world: &mut BehaviourWorld,
    file: String,
    binary: String,
    pattern: String,
) -> Result<(), String> {
    let scoped = position_of(
        world,
        &file,
        scoped_to(binary.clone()),
        &format!("rule scoped to {binary}"),
    )?;
    let open = position_of(
        world,
        &file,
        open_rule_for(pattern.clone()),
        &format!("open rule for {pattern}"),
    )?;
    if scoped < open {
        Ok(())
    } else {
        Err(format!(
            "the gate stops at the first match, so the scoped rule at {scoped} never fires behind the open rule for {pattern} at {open}"
        ))
    }
}

#[then(regex = r#"^"([^"]+)" still holds (\d+) rules?$"#)]
fn file_still_holds(
    world: &mut BehaviourWorld,
    file: String,
    expected: usize,
) -> Result<(), String> {
    let found = load(world, &file).network.egress.http.len();
    if found == expected {
        Ok(())
    } else {
        Err(format!(
            "expected {expected} rules in {file}, found {found}"
        ))
    }
}

#[then(regex = r#"^the output says it was placed before the rule for "([^"]+)"$"#)]
fn output_reports_placement(world: &mut BehaviourWorld, pattern: String) -> Result<(), String> {
    output_mentions(world, &format!("before the existing rule for {pattern:?}"))
}

#[then(
    regex = r#"^"([^"]+)" lists the rule scoped to "([^"]+)" before the rule scoped to "([^"]+)"$"#
)]
fn narrower_scope_comes_first(
    world: &mut BehaviourWorld,
    file: String,
    narrower: String,
    wider: String,
) -> Result<(), String> {
    let first = position_of(
        world,
        &file,
        scoped_to(narrower.clone()),
        &format!("rule scoped to {narrower}"),
    )?;
    let second = position_of(
        world,
        &file,
        scoped_to(wider.clone()),
        &format!("rule scoped to {wider}"),
    )?;
    if first < second {
        Ok(())
    } else {
        Err(format!(
            "the gate stops at the first match, so the rule at {first} narrows nothing behind the rule scoped to {wider} at {second}"
        ))
    }
}

#[then(regex = r#"^the output says every other caller is now denied "([^"]+)"$"#)]
fn output_reports_the_destination_is_claimed(
    world: &mut BehaviourWorld,
    pattern: String,
) -> Result<(), String> {
    output_mentions(
        world,
        &format!("Every other caller is now denied {pattern:?} without being asked"),
    )
}

#[then(
    regex = r#"^"([^"]+)" lists the deny rule for "([^"]+)" before the rule restricted to GET requests$"#
)]
fn deny_comes_before_the_restricted_rule(
    world: &mut BehaviourWorld,
    file: String,
    pattern: String,
) -> Result<(), String> {
    let deny = position_of(
        world,
        &file,
        deny_rule_for(pattern.clone()),
        &format!("deny rule for {pattern}"),
    )?;
    let restricted = position_of(
        world,
        &file,
        |rule| !rule.rules.is_empty(),
        "rule restricted to GET requests",
    )?;
    if deny < restricted {
        Ok(())
    } else {
        Err(format!(
            "the restricted allow at {restricted} still serves the requests it names, so the deny at {deny} never blocks them"
        ))
    }
}

#[then(regex = r"^the failure says the rule would lift the request restriction$")]
fn failure_reports_a_lifted_restriction(world: &mut BehaviourWorld) -> Result<(), String> {
    output_mentions(world, "allows only the requests its rules list names")?;
    output_mentions(world, "would lift that restriction")
}

#[then(regex = r"^the failure says the rule would open the destination to every caller$")]
fn failure_reports_a_widening(world: &mut BehaviourWorld) -> Result<(), String> {
    output_mentions(world, "open the destination to every caller")
}

#[then(regex = r"^the output does not claim any scoping is spent$")]
fn output_does_not_claim_spent_scoping(world: &mut BehaviourWorld) -> Result<(), String> {
    let out = world
        .result
        .as_ref()
        .map(|r| r.output.clone())
        .unwrap_or_default();
    if out.contains("no longer applies") {
        return Err(format!(
            "the rule it was placed in front of still serves the callers it lists, so nothing about its scoping is spent:\n{out}"
        ));
    }
    Ok(())
}

#[then(regex = r#"^the output says the scoped rule for "([^"]+)" no longer applies$"#)]
fn output_reports_spent_scoping(world: &mut BehaviourWorld, pattern: String) -> Result<(), String> {
    output_mentions(world, &format!("no longer applies to {pattern:?}"))
}

#[then(regex = r"^the output does not claim any caller is denied$")]
fn output_does_not_claim_a_denied_caller(world: &mut BehaviourWorld) -> Result<(), String> {
    let out = world
        .result
        .as_ref()
        .map(|r| r.output.clone())
        .unwrap_or_default();
    if out.contains("is now denied") {
        return Err(format!(
            "an unscoped rule denies nobody, so claiming it does would send the developer looking for a filter that isn't there:\n{out}"
        ));
    }
    Ok(())
}

#[then(regex = r#"^the output says (\d+) rules? (?:were|was) removed for "([^"]+)"$"#)]
fn output_reports_the_removal_count(
    world: &mut BehaviourWorld,
    count: usize,
    pattern: String,
) -> Result<(), String> {
    let noun = if count == 1 { "rule" } else { "rules" };
    output_mentions(world, &format!("Removed {count} {noun} for {pattern:?}"))
}

#[then(regex = r"^the output says the description was updated$")]
fn output_reports_a_described_rule(world: &mut BehaviourWorld) -> Result<(), String> {
    output_mentions(world, "Updated the description of")
}

#[then(regex = r"^the output says the rule is already there$")]
fn output_reports_already_present(world: &mut BehaviourWorld) -> Result<(), String> {
    output_mentions(world, "is already in")
}

#[then(regex = r"^the command succeeds$")]
fn command_succeeds(world: &mut BehaviourWorld) -> Result<(), String> {
    match world.result.as_ref() {
        Some(run) if run.exit_code == 0 => Ok(()),
        Some(run) => Err(format!("exited {}:\n{}", run.exit_code, run.output)),
        None => Err("the command did not run".to_string()),
    }
}

#[then(regex = r"^the output says the deny adds nothing$")]
fn output_reports_deny_adds_nothing(world: &mut BehaviourWorld) -> Result<(), String> {
    output_mentions(world, "already blocks")?;
    output_mentions(world, "adds nothing")
}

#[then(regex = r"^the failure does not tell the developer to remove the deny$")]
fn failure_does_not_advise_removing_the_deny(world: &mut BehaviourWorld) -> Result<(), String> {
    let out = world
        .result
        .as_ref()
        .map(|r| r.output.clone())
        .unwrap_or_default();
    if out.contains("policy remove") {
        return Err(format!(
            "removing the deny widens egress for every destination it covers, so the refusal must not propose it:\n{out}"
        ));
    }
    Ok(())
}

#[then(regex = r"^the command fails with an exit code other than 0$")]
fn command_fails(world: &mut BehaviourWorld) -> Result<(), String> {
    match world.result.as_ref().map(|r| r.exit_code) {
        Some(0) | None => Err("expected a non-zero exit code".to_string()),
        Some(_) => Ok(()),
    }
}

#[then(regex = r"^the policy file is unchanged$")]
fn policy_file_unchanged(world: &mut BehaviourWorld) -> Result<(), String> {
    let policy = load(world, "lns-policy.yaml");
    if policy.network.egress.http.is_empty() {
        Ok(())
    } else {
        Err("policy file was modified by a failed remove".to_string())
    }
}
