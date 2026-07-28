use serde_json::{Map, Value};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkloadEnv {
    pub env: Vec<String>,
    pub refused: Vec<String>,
}

/// A var is managed when the run's providers — its custom providers and the catalog connectors — declare it; managed vars are seeded as placeholders, never carried from `-e`.
fn is_run_managed(env_var: &str, extra_managed: &[String]) -> bool {
    extra_managed.iter().any(|m| m == env_var)
}

pub const REDACTED_ENV_VALUE: &str = "<redacted>";

/// The audit record needs the set of injected var names, never their values — a mistyped secret in `-e KEY=VALUE` must not become durable audit data, so every value is redacted.
pub fn injected_env(user_env: &[String], extra_managed: &[String]) -> Option<Map<String, Value>> {
    let mut env = Map::new();
    for kv in user_env {
        let Some((k, _v)) = kv.split_once('=') else {
            continue;
        };
        if is_run_managed(k, extra_managed) {
            continue;
        }
        env.insert(k.to_string(), Value::String(REDACTED_ENV_VALUE.to_string()));
    }
    if env.is_empty() { None } else { Some(env) }
}

pub fn compose_workload_env(
    image_env: Option<&[String]>,
    user_env: &[String],
    extra_managed: &[String],
) -> WorkloadEnv {
    let mut entries: Vec<(String, String)> = Vec::new();
    if let Some(image) = image_env {
        for kv in image {
            if let Some((k, v)) = kv.split_once('=') {
                upsert(&mut entries, k, v);
            }
        }
    }
    let mut refused = Vec::new();
    for kv in user_env {
        let Some((k, v)) = kv.split_once('=') else {
            continue;
        };
        if is_run_managed(k, extra_managed) {
            refused.push(k.to_string());
            continue;
        }
        upsert(&mut entries, k, v);
    }
    WorkloadEnv {
        env: entries
            .into_iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect(),
        refused,
    }
}

/// The PATH lns-init hands the guest; the workload inherits the same one so declared tools never silently drop the guest-tools dir the kernel PATH carries.
pub const GUEST_DEFAULT_PATH: &str =
    "/.lens/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

/// The one PATH rule for a guest: declared tool dirs win over whatever the image (or `-e`) composed, and no blank segment survives either way. `lns exec` into the same guest applies this same rule, so the tool dirs it resolves match the workload's — two copies of this drift into exactly the "run finds node, exec does not" bug.
pub fn compose_guest_path(env: &mut Vec<String>, tool_bin_paths: &[String]) {
    let declared = env.iter().find_map(|kv| kv.strip_prefix("PATH="));
    // Nothing to prepend and nothing to sanitize: lns-init already exports the default, so composing one here would only duplicate it.
    if tool_bin_paths.is_empty() && declared.is_none() {
        return;
    }
    let existing = declared
        .filter(|path| !path.is_empty())
        .unwrap_or(GUEST_DEFAULT_PATH)
        .to_string();
    let value = format!("PATH={}", join_path(tool_bin_paths, &existing));
    match env.iter_mut().find(|kv| kv.starts_with("PATH=")) {
        Some(slot) => *slot = value,
        None => env.push(value),
    }
}

/// An empty PATH segment is the current directory to `execvp`, so a blank entry — from `PATH=` in the image config, or a stray `::` — must never survive into the workload's PATH.
fn join_path(tool_bin_paths: &[String], existing: &str) -> String {
    let mut seen = std::collections::HashSet::new();
    tool_bin_paths
        .iter()
        .map(String::as_str)
        .chain(existing.split(':'))
        .filter(|segment| !segment.is_empty())
        .filter(|segment| seen.insert(*segment))
        .collect::<Vec<&str>>()
        .join(":")
}

pub fn run_workload_env(
    image_env: Option<&[String]>,
    user_env: &[String],
    agent_command: Option<&str>,
    workdir: Option<&str>,
    extra_managed: &[String],
    tool_bin_paths: &[String],
) -> WorkloadEnv {
    const SUPERVISOR_PTY_OPT_IN_TERM: &str = "xterm-256color";
    let mut composed = compose_workload_env(image_env, user_env, extra_managed);
    // The broker's last-wins putenv would otherwise let the image PATH shadow the tool dirs.
    compose_guest_path(&mut composed.env, tool_bin_paths);
    if let Some(agent_command) = agent_command {
        // Internal vars go last: the broker's last-wins putenv means a user `-e TERM=…` can't clobber the supervisor PTY opt-in, the command, or the agent cwd.
        composed.env.push(format!("AGENT_COMMAND={agent_command}"));
        if let Some(workdir) = workdir {
            composed.env.push(format!("WORKSPACE_PATH={workdir}"));
        }
        // nexus-agent-sandbox treats TERM unset or "linux" as "no PTY"; xterm-256color opts it into the PTY path needed for `lns run -it --policy`.
        composed
            .env
            .push(format!("TERM={SUPERVISOR_PTY_OPT_IN_TERM}"));
    }
    composed
}

pub fn refusal_warning(key: &str) -> String {
    format!("{key} not set: it is a managed credential — use the credential flow, not -e")
}

fn upsert(entries: &mut Vec<(String, String)>, key: &str, value: &str) {
    match entries.iter_mut().find(|(k, _)| k == key) {
        Some(slot) => slot.1 = value.to_string(),
        None => entries.push((key.to_string(), value.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(c: &WorkloadEnv) -> &[String] {
        &c.env
    }

    #[test]
    fn a_single_user_var_is_carried() {
        let c = compose_workload_env(None, &["CLAUDE_CODE_USE_BEDROCK=1".into()], &[]);
        assert_eq!(env(&c), ["CLAUDE_CODE_USE_BEDROCK=1"]);
        assert!(c.refused.is_empty());
    }

    #[test]
    fn multiple_user_vars_are_all_carried() {
        let c = compose_workload_env(None, &["A=1".into(), "B=2".into()], &[]);
        assert!(c.env.contains(&"A=1".to_string()));
        assert!(c.env.contains(&"B=2".to_string()));
    }

    #[test]
    fn value_is_split_on_the_first_equals_only() {
        let c = compose_workload_env(None, &["DSN=user=admin;pw=x".into()], &[]);
        assert_eq!(env(&c), ["DSN=user=admin;pw=x"]);
    }

    #[test]
    fn an_empty_value_is_preserved() {
        let c = compose_workload_env(None, &["FEATURE_X=".into()], &[]);
        assert_eq!(env(&c), ["FEATURE_X="]);
    }

    #[test]
    fn user_value_overrides_the_image_env() {
        let c = compose_workload_env(Some(&["PORT=3003".into()]), &["PORT=4000".into()], &[]);
        assert_eq!(env(&c), ["PORT=4000"]);
    }

    #[test]
    fn image_vars_not_overridden_are_kept() {
        let c = compose_workload_env(Some(&["FOO=bar".into()]), &["BAZ=1".into()], &[]);
        assert!(c.env.contains(&"FOO=bar".to_string()));
        assert!(c.env.contains(&"BAZ=1".to_string()));
    }

    #[test]
    fn a_user_override_of_a_managed_credential_is_refused_and_dropped() {
        let c = compose_workload_env(
            None,
            &["SOME_TOKEN=some-real".into()],
            &["SOME_TOKEN".to_string()],
        );
        assert!(c.env.is_empty(), "credential var must not reach workload");
        assert_eq!(c.refused, ["SOME_TOKEN"]);
    }

    #[test]
    fn a_connected_connectors_env_var_is_refused_and_dropped() {
        let c = compose_workload_env(
            None,
            &["GITLAB_TOKEN=glpat_real".into(), "SAFE=1".into()],
            &["GITLAB_TOKEN".to_string()],
        );
        assert_eq!(
            c.env,
            ["SAFE=1"],
            "the connector token must not reach the workload"
        );
        assert_eq!(c.refused, ["GITLAB_TOKEN"]);
    }

    #[test]
    fn a_malformed_user_entry_without_equals_is_ignored() {
        let c = compose_workload_env(None, &["NOTANASSIGNMENT".into()], &[]);
        assert!(c.env.is_empty());
        assert!(c.refused.is_empty());
    }

    #[test]
    fn a_malformed_image_entry_without_equals_is_ignored() {
        let c = compose_workload_env(Some(&["JUSTAKEY".into()]), &[], &[]);
        assert!(c.env.is_empty());
    }

    #[test]
    fn run_workload_env_carries_user_env_for_an_unsupervised_run() {
        let c = run_workload_env(None, &["FOO=bar".into()], None, None, &[], &[]);
        assert_eq!(c.env, ["FOO=bar"], "user -e must reach a policy-less run");
    }

    #[test]
    fn run_workload_env_adds_no_supervisor_vars_when_unsupervised() {
        let c = run_workload_env(None, &["FOO=bar".into()], None, None, &[], &[]);
        assert!(
            !c.env
                .iter()
                .any(|e| e.starts_with("AGENT_COMMAND=") || e.starts_with("TERM=")),
            "no agent command means no supervisor vars: {:?}",
            c.env
        );
    }

    #[test]
    fn run_workload_env_appends_agent_command_and_term_when_supervised() {
        let c = run_workload_env(None, &["FOO=bar".into()], Some("echo hi"), None, &[], &[]);
        assert!(c.env.contains(&"FOO=bar".to_string()), "got: {:?}", c.env);
        assert!(c.env.contains(&"AGENT_COMMAND=echo hi".to_string()));
        assert!(c.env.contains(&"TERM=xterm-256color".to_string()));
    }

    #[test]
    fn run_workload_env_keeps_supervisor_term_after_a_user_term_so_it_cannot_be_clobbered() {
        let c = run_workload_env(None, &["TERM=dumb".into()], Some("sh"), None, &[], &[]);
        let last_term = c.env.iter().rposition(|e| e.starts_with("TERM=")).unwrap();
        assert_eq!(c.env[last_term], "TERM=xterm-256color");
    }

    #[test]
    fn run_workload_env_surfaces_refused_credentials() {
        let c = run_workload_env(
            None,
            &["SOME_TOKEN=x".into()],
            None,
            None,
            &["SOME_TOKEN".to_string()],
            &[],
        );
        assert_eq!(c.refused, ["SOME_TOKEN"]);
        assert!(c.env.is_empty());
    }

    #[test]
    fn run_workload_env_refuses_a_connected_connector_var_via_extra_managed() {
        let c = run_workload_env(
            None,
            &["GITLAB_TOKEN=real".into()],
            None,
            None,
            &["GITLAB_TOKEN".to_string()],
            &[],
        );
        assert_eq!(c.refused, ["GITLAB_TOKEN"]);
        assert!(c.env.is_empty());
    }

    #[test]
    fn run_workload_env_pins_workspace_path_for_a_supervised_run() {
        let c = run_workload_env(None, &[], Some("sh"), Some("/app"), &[], &[]);
        assert!(
            c.env.contains(&"WORKSPACE_PATH=/app".to_string()),
            "got: {:?}",
            c.env
        );
    }

    #[test]
    fn run_workload_env_omits_workspace_path_without_a_workdir() {
        let c = run_workload_env(None, &[], Some("sh"), None, &[], &[]);
        assert!(
            !c.env.iter().any(|e| e.starts_with("WORKSPACE_PATH=")),
            "got: {:?}",
            c.env
        );
    }

    #[test]
    fn run_workload_env_keeps_the_workdir_workspace_path_after_a_user_override() {
        let c = run_workload_env(
            None,
            &["WORKSPACE_PATH=/evil".into()],
            Some("sh"),
            Some("/app"),
            &[],
            &[],
        );
        let last = c
            .env
            .iter()
            .rposition(|e| e.starts_with("WORKSPACE_PATH="))
            .unwrap();
        assert_eq!(
            c.env[last], "WORKSPACE_PATH=/app",
            "the broker's last-wins putenv must land on the -w dir: {:?}",
            c.env
        );
    }

    #[test]
    fn run_workload_env_adds_no_workspace_path_when_unsupervised() {
        let c = run_workload_env(None, &[], None, Some("/app"), &[], &[]);
        assert!(
            c.env.is_empty(),
            "an unsupervised run's cwd travels via the session, not env: {:?}",
            c.env
        );
    }

    #[test]
    fn tool_bin_paths_prepend_and_beat_the_image_path() {
        let c = run_workload_env(
            Some(&["PATH=/usr/bin".into()]),
            &[],
            None,
            None,
            &[],
            &["/.lens/tools/some-tool/1.2.3/bin".into()],
        );
        assert_eq!(c.env, ["PATH=/.lens/tools/some-tool/1.2.3/bin:/usr/bin"]);
    }

    #[test]
    fn tool_bin_paths_extend_the_guest_default_when_the_image_sets_no_path() {
        let c = run_workload_env(None, &[], None, None, &[], &["/t/bin".into()]);
        assert_eq!(c.env, [format!("PATH=/t/bin:{GUEST_DEFAULT_PATH}")]);
    }

    #[test]
    fn an_empty_image_path_never_leaves_the_current_directory_on_the_workload_path() {
        // A blank PATH segment is the cwd to execvp, and the workdir is usually a writable project bind.
        let c = run_workload_env(
            Some(&["PATH=".into()]),
            &[],
            None,
            None,
            &[],
            &["/t/bin".into()],
        );
        assert_eq!(c.env, [format!("PATH=/t/bin:{GUEST_DEFAULT_PATH}")]);
    }

    #[test]
    fn blank_and_duplicate_path_segments_are_dropped() {
        let c = run_workload_env(
            Some(&["PATH=/usr/bin::/t/bin:/usr/bin".into()]),
            &[],
            None,
            None,
            &[],
            &["/t/bin".into()],
        );
        assert_eq!(c.env, ["PATH=/t/bin:/usr/bin"]);
    }

    #[test]
    fn a_blank_image_path_segment_is_dropped_even_with_no_declared_tools() {
        // The cwd-on-PATH hazard comes from the image, so the sanitation cannot be a side effect of happening to declare a tool.
        let c = run_workload_env(
            Some(&["PATH=/usr/bin::/usr/bin".into()]),
            &[],
            None,
            None,
            &[],
            &[],
        );
        assert_eq!(c.env, ["PATH=/usr/bin"]);
    }

    #[test]
    fn a_run_with_no_tools_and_no_image_path_is_left_to_the_kernel_default() {
        // Composing one here would put a second copy of the same value in the env for no reason; lns-init already exports it.
        let c = run_workload_env(None, &[], None, None, &[], &[]);
        assert!(
            !c.env.iter().any(|kv| kv.starts_with("PATH=")),
            "got: {:?}",
            c.env
        );
    }

    #[test]
    fn the_guest_default_path_carries_the_guest_tools_dir() {
        // The workload inherits the kernel PATH, so declaring a tool must not drop /.lens/bin as a side effect.
        assert!(GUEST_DEFAULT_PATH.starts_with("/.lens/bin:"));
        let c = run_workload_env(None, &[], None, None, &[], &["/t/bin".into()]);
        assert!(c.env[0].contains("/.lens/bin"), "got: {:?}", c.env[0]);
    }

    #[test]
    fn tool_bin_paths_keep_declaration_order_and_precede_the_supervisor_appends() {
        let c = run_workload_env(
            None,
            &[],
            Some("sh"),
            None,
            &[],
            &["/t/a/bin".into(), "/t/b".into()],
        );
        let path = c.env.iter().position(|e| e.starts_with("PATH=")).unwrap();
        assert!(
            c.env[path].starts_with("PATH=/t/a/bin:/t/b:"),
            "got: {:?}",
            c.env
        );
        let agent = c
            .env
            .iter()
            .position(|e| e.starts_with("AGENT_COMMAND="))
            .unwrap();
        assert!(path < agent, "PATH must precede the last-wins appends");
    }

    #[test]
    fn refusal_warning_names_the_key_and_the_reason() {
        let msg = refusal_warning("OPENAI_API_KEY");
        assert!(msg.contains("OPENAI_API_KEY"));
        assert!(msg.contains("managed credential"));
    }

    #[test]
    fn injected_env_records_non_credential_var_names() {
        let env = injected_env(&["CLAUDE_CODE_USE_BEDROCK=1".into()], &[]).expect("env built");
        assert_eq!(
            env.get("CLAUDE_CODE_USE_BEDROCK").unwrap(),
            REDACTED_ENV_VALUE
        );
    }

    #[test]
    fn injected_env_redacts_values_so_a_mistyped_secret_never_persists() {
        let env = injected_env(&["SOME_PRIVATE_TOKEN=super-secret-value".into()], &[])
            .expect("env built");
        assert!(
            env.contains_key("SOME_PRIVATE_TOKEN"),
            "the var name is still recorded for the audit trail"
        );
        assert_eq!(
            env.get("SOME_PRIVATE_TOKEN").unwrap(),
            REDACTED_ENV_VALUE,
            "the raw -e value must never land in a durable audit record"
        );
    }

    #[test]
    fn injected_env_omits_managed_credentials() {
        let env = injected_env(
            &["A=1".into(), "SOME_TOKEN=x".into()],
            &["SOME_TOKEN".to_string()],
        )
        .expect("env built");
        assert!(env.contains_key("A"));
        assert!(!env.contains_key("SOME_TOKEN"));
    }

    #[test]
    fn injected_env_omits_a_connected_connectors_value_from_the_log() {
        let env = injected_env(
            &["A=1".into(), "GITLAB_TOKEN=glpat_real".into()],
            &["GITLAB_TOKEN".to_string()],
        )
        .expect("env built");
        assert!(env.contains_key("A"));
        assert!(
            !env.contains_key("GITLAB_TOKEN"),
            "a connected connector's real token must never land in the audit log"
        );
    }

    #[test]
    fn injected_env_is_none_when_nothing_is_injected() {
        assert!(injected_env(&[], &[]).is_none());
        assert!(injected_env(&["SOME_TOKEN=x".into()], &["SOME_TOKEN".to_string()]).is_none());
    }

    #[test]
    fn injected_env_skips_malformed_entries() {
        let env = injected_env(&["NOEQUALS".into(), "A=1".into()], &[]).expect("env built");
        assert!(!env.contains_key("NOEQUALS"));
        assert!(env.contains_key("A"));
    }
}
