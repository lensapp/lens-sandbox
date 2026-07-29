use crate::runner::CliRun;
use crate::world::BehaviourWorld;
use cucumber::{given, then, when};
use lns_cli::policy::{self, PolicyCommand, PolicyRemoveArgs, PolicyRuleArgs, PolicyScopeArgs};
use lns_policy::{Policy, RouteRule, Verdict};
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

#[given(regex = r#"^"([^"]+)" has an allow rule for "([^"]+)"$"#)]
fn policy_has_allow(world: &mut BehaviourWorld, file: String, pattern: String) {
    let dir = cwd(world);
    let mut policy = Policy::default();
    policy.add_rule(RouteRule::allow_host(pattern));
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
    let args = rule_args(world, &pattern, Some(PathBuf::from(file)));
    run_policy(world, PolicyCommand::Allow(args));
}

#[when(regex = r#"^the developer adds an allow rule for "([^"]+)" without passing --policy$"#)]
fn add_allow_default_path(world: &mut BehaviourWorld, pattern: String) {
    let args = rule_args(world, &pattern, None);
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
    let args = rule_args(world, &pattern, Some(explicit));
    run_policy(world, PolicyCommand::Allow(args));
}

#[when(regex = r#"^the developer adds an allow rule for "([^"]+)" with description "([^"]+)"$"#)]
fn add_allow_with_description(world: &mut BehaviourWorld, pattern: String, description: String) {
    let mut args = rule_args(world, &pattern, None);
    args.description = Some(description);
    run_policy(world, PolicyCommand::Allow(args));
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
