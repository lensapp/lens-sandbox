use crate::credential_flow::providers;
use serde_json::{Map, Value};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkloadEnv {
    pub env: Vec<String>,
    pub refused: Vec<String>,
}

/// A var is managed if a built-in claims it or the run's custom providers / connected integrations declare it; managed vars are seeded as placeholders, never carried from `-e`.
fn is_run_managed(env_var: &str, extra_managed: &[String]) -> bool {
    providers::is_managed_env(env_var) || extra_managed.iter().any(|m| m == env_var)
}

pub fn injected_env_audit(
    user_env: &[String],
    extra_managed: &[String],
) -> Option<Map<String, Value>> {
    let mut env = Map::new();
    for kv in user_env {
        let Some((k, v)) = kv.split_once('=') else {
            continue;
        };
        if is_run_managed(k, extra_managed) {
            continue;
        }
        env.insert(k.to_string(), Value::String(v.to_string()));
    }
    if env.is_empty() {
        return None;
    }
    let mut obj = Map::new();
    obj.insert("type".to_string(), Value::String("audit_event".to_string()));
    obj.insert("event".to_string(), Value::String("run_env".to_string()));
    obj.insert("env".to_string(), Value::Object(env));
    Some(obj)
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

pub fn run_workload_env(
    image_env: Option<&[String]>,
    user_env: &[String],
    agent_command: Option<&str>,
    extra_managed: &[String],
) -> WorkloadEnv {
    const SUPERVISOR_PTY_OPT_IN_TERM: &str = "xterm-256color";
    let mut composed = compose_workload_env(image_env, user_env, extra_managed);
    if let Some(agent_command) = agent_command {
        // Internal vars go last: the broker's last-wins putenv means a user `-e TERM=…` can't clobber the supervisor PTY opt-in or the command.
        composed.env.push(format!("AGENT_COMMAND={agent_command}"));
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
        let c = compose_workload_env(None, &["OPENAI_API_KEY=sk-real".into()], &[]);
        assert!(c.env.is_empty(), "credential var must not reach workload");
        assert_eq!(c.refused, ["OPENAI_API_KEY"]);
    }

    #[test]
    fn a_connected_integrations_env_var_is_refused_and_dropped() {
        let c = compose_workload_env(
            None,
            &["GITLAB_TOKEN=glpat_real".into(), "SAFE=1".into()],
            &["GITLAB_TOKEN".to_string()],
        );
        assert_eq!(
            c.env,
            ["SAFE=1"],
            "the integration token must not reach the workload"
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
        let c = run_workload_env(None, &["FOO=bar".into()], None, &[]);
        assert_eq!(c.env, ["FOO=bar"], "user -e must reach a policy-less run");
    }

    #[test]
    fn run_workload_env_adds_no_supervisor_vars_when_unsupervised() {
        let c = run_workload_env(None, &["FOO=bar".into()], None, &[]);
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
        let c = run_workload_env(None, &["FOO=bar".into()], Some("echo hi"), &[]);
        assert!(c.env.contains(&"FOO=bar".to_string()), "got: {:?}", c.env);
        assert!(c.env.contains(&"AGENT_COMMAND=echo hi".to_string()));
        assert!(c.env.contains(&"TERM=xterm-256color".to_string()));
    }

    #[test]
    fn run_workload_env_keeps_supervisor_term_after_a_user_term_so_it_cannot_be_clobbered() {
        let c = run_workload_env(None, &["TERM=dumb".into()], Some("sh"), &[]);
        let last_term = c.env.iter().rposition(|e| e.starts_with("TERM=")).unwrap();
        assert_eq!(c.env[last_term], "TERM=xterm-256color");
    }

    #[test]
    fn run_workload_env_surfaces_refused_credentials() {
        let c = run_workload_env(None, &["OPENAI_API_KEY=x".into()], None, &[]);
        assert_eq!(c.refused, ["OPENAI_API_KEY"]);
        assert!(c.env.is_empty());
    }

    #[test]
    fn run_workload_env_refuses_a_connected_integration_var_via_extra_managed() {
        let c = run_workload_env(
            None,
            &["GITLAB_TOKEN=real".into()],
            None,
            &["GITLAB_TOKEN".to_string()],
        );
        assert_eq!(c.refused, ["GITLAB_TOKEN"]);
        assert!(c.env.is_empty());
    }

    #[test]
    fn refusal_warning_names_the_key_and_the_reason() {
        let msg = refusal_warning("OPENAI_API_KEY");
        assert!(msg.contains("OPENAI_API_KEY"));
        assert!(msg.contains("managed credential"));
    }

    #[test]
    fn injected_env_audit_records_non_credential_vars() {
        let obj =
            injected_env_audit(&["CLAUDE_CODE_USE_BEDROCK=1".into()], &[]).expect("event built");
        assert_eq!(obj.get("type").unwrap(), "audit_event");
        assert_eq!(obj.get("event").unwrap(), "run_env");
        let env = obj.get("env").unwrap().as_object().unwrap();
        assert_eq!(env.get("CLAUDE_CODE_USE_BEDROCK").unwrap(), "1");
    }

    #[test]
    fn injected_env_audit_omits_managed_credentials() {
        let obj = injected_env_audit(&["A=1".into(), "OPENAI_API_KEY=x".into()], &[])
            .expect("event built");
        let env = obj.get("env").unwrap().as_object().unwrap();
        assert!(env.contains_key("A"));
        assert!(!env.contains_key("OPENAI_API_KEY"));
    }

    #[test]
    fn injected_env_audit_omits_a_connected_integrations_value_from_the_log() {
        let obj = injected_env_audit(
            &["A=1".into(), "GITLAB_TOKEN=glpat_real".into()],
            &["GITLAB_TOKEN".to_string()],
        )
        .expect("event built");
        let env = obj.get("env").unwrap().as_object().unwrap();
        assert!(env.contains_key("A"));
        assert!(
            !env.contains_key("GITLAB_TOKEN"),
            "a connected integration's real token must never land in the audit log"
        );
    }

    #[test]
    fn injected_env_audit_is_none_when_nothing_is_injected() {
        assert!(injected_env_audit(&[], &[]).is_none());
        assert!(injected_env_audit(&["OPENAI_API_KEY=x".into()], &[]).is_none());
    }

    #[test]
    fn injected_env_audit_skips_malformed_entries() {
        let obj = injected_env_audit(&["NOEQUALS".into(), "A=1".into()], &[]).expect("event built");
        let env = obj.get("env").unwrap().as_object().unwrap();
        assert!(!env.contains_key("NOEQUALS"));
        assert!(env.contains_key("A"));
    }
}
