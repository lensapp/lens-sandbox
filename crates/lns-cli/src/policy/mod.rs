use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, bail};
use lns_policy::{Policy, RouteRule, Transport, Verdict};

use crate::cli::{
    PolicyCommand, PolicyPullArgs, PolicyPushArgs, PolicyRemoveArgs, PolicyRuleArgs,
    PolicyScopeArgs, TransportArg,
};
use crate::run::summary::policy_path;

mod real;
mod registry;

pub use real::RealPolicyRegistry;
pub use registry::{LocalBoxFuture, PolicyRegistry};

pub async fn run(
    cmd: &PolicyCommand,
    cwd: &Path,
    registry: &dyn PolicyRegistry,
    writer: &mut impl Write,
) -> Result<i32> {
    match cmd {
        PolicyCommand::Allow(args) => add_rule(args, Verdict::Allow, cwd, writer),
        PolicyCommand::Deny(args) => add_rule(args, Verdict::Deny, cwd, writer),
        PolicyCommand::List(args) => list_rules(args, cwd, writer),
        PolicyCommand::Remove(args) => remove_rule(args, cwd, writer),
        PolicyCommand::Push(args) => push(args, cwd, registry, writer).await,
        PolicyCommand::Pull(args) => pull(args, registry, writer).await,
    }
}

/// Derives a config-blob `name`/`version` from the last path segment of a registry reference (e.g. `…/policies/pii:v1` → name `pii`, version `v1`).
fn name_and_version(reference: &str) -> (Option<String>, Option<String>) {
    let last = reference.rsplit('/').next().unwrap_or(reference);
    let non_empty = |s: &str| (!s.is_empty()).then(|| s.to_string());
    if let Some((name, digest)) = last.split_once('@') {
        return (non_empty(name), non_empty(digest));
    }
    match last.split_once(':') {
        Some((name, tag)) => (non_empty(name), non_empty(tag)),
        None => (non_empty(last), None),
    }
}

async fn push(
    args: &PolicyPushArgs,
    cwd: &Path,
    registry: &dyn PolicyRegistry,
    writer: &mut impl Write,
) -> Result<i32> {
    let path = if args.file.is_absolute() {
        args.file.clone()
    } else {
        cwd.join(&args.file)
    };
    if !path.exists() {
        bail!("policy file {} does not exist", path.display());
    }
    let policy = Policy::load_or_default(&path)
        .with_context(|| format!("loading policy from {}", path.display()))?;
    let (name, version) = name_and_version(&args.reference);
    let blob = lns_policy::artifact::encode(&policy, name.as_deref(), version.as_deref())
        .context("encoding policy artifact")?;
    let digest = registry.push(&args.reference, &blob).await?;
    writeln!(
        writer,
        "Pushed {} ({} bytes) to {}",
        digest,
        blob.len(),
        args.reference
    )?;
    Ok(0)
}

async fn pull(
    args: &PolicyPullArgs,
    registry: &dyn PolicyRegistry,
    writer: &mut impl Write,
) -> Result<i32> {
    let (blob, digest) = registry.pull(&args.reference).await?;
    let (policy, _name, _version) =
        lns_policy::artifact::decode(&blob).context("decoding pulled policy artifact")?;
    match &args.output {
        Some(path) => {
            policy
                .save_atomic(path)
                .with_context(|| format!("writing policy to {}", path.display()))?;
            writeln!(writer, "Pulled {} to {}", digest, path.display())?;
        }
        None => {
            let yaml = policy.to_yaml().context("rendering pulled policy")?;
            write!(writer, "{yaml}")?;
        }
    }
    Ok(0)
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
    use std::sync::Mutex;
    use tempfile::TempDir;

    fn rule_args(pattern: &str, policy: Option<&Path>) -> PolicyRuleArgs {
        PolicyRuleArgs {
            pattern: pattern.to_string(),
            description: None,
            transport: TransportArg::Direct,
            policy: policy.map(Path::to_path_buf),
        }
    }

    #[derive(Default)]
    struct FakeRegistry {
        pushed: Mutex<Option<(String, Vec<u8>)>>,
        push_digest: String,
        pull_blob: Vec<u8>,
        pull_digest: String,
        fail: Option<String>,
    }

    impl PolicyRegistry for FakeRegistry {
        fn push<'a>(
            &'a self,
            reference: &'a str,
            config_blob: &'a [u8],
        ) -> LocalBoxFuture<'a, Result<String>> {
            Box::pin(async move {
                if let Some(msg) = &self.fail {
                    bail!("{msg}");
                }
                *self.pushed.lock().unwrap() = Some((reference.to_string(), config_blob.to_vec()));
                Ok(self.push_digest.clone())
            })
        }

        fn pull<'a>(
            &'a self,
            _reference: &'a str,
        ) -> LocalBoxFuture<'a, Result<(Vec<u8>, String)>> {
            Box::pin(async move {
                if let Some(msg) = &self.fail {
                    bail!("{msg}");
                }
                Ok((self.pull_blob.clone(), self.pull_digest.clone()))
            })
        }
    }

    fn no_registry() -> FakeRegistry {
        FakeRegistry::default()
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

    #[tokio::test]
    async fn deny_through_run_writes_a_deny_rule() {
        let dir = TempDir::new().unwrap();
        let mut out = Vec::new();
        let code = run(
            &PolicyCommand::Deny(rule_args("evil.example", None)),
            dir.path(),
            &no_registry(),
            &mut out,
        )
        .await
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
    fn name_and_version_splits_the_last_segment_on_tag() {
        assert_eq!(
            name_and_version("registry.example.com:5000/org/acme/policies/pii:v1"),
            (Some("pii".into()), Some("v1".into()))
        );
        assert_eq!(
            name_and_version("registry.example.com/org/acme/policies/pii"),
            (Some("pii".into()), None)
        );
        assert_eq!(
            name_and_version("reg/x@sha256:abc"),
            (Some("x".into()), Some("sha256:abc".into()))
        );
    }

    #[tokio::test]
    async fn push_encodes_the_local_policy_and_reports_the_digest() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("lns-policy.yaml");
        let mut policy = Policy::default();
        policy.add_rule(RouteRule::allow_host("api.example.test"));
        policy.save_atomic(&path).unwrap();

        let registry = FakeRegistry {
            push_digest: "sha256:abc".into(),
            ..Default::default()
        };
        let args = PolicyPushArgs {
            file: path.clone(),
            reference: "registry.example.test/org/acme/policies/pii:v1".into(),
        };
        let mut out = Vec::new();
        let code = push(&args, dir.path(), &registry, &mut out).await.unwrap();
        assert_eq!(code, 0);

        let (reference, blob) = registry.pushed.lock().unwrap().clone().unwrap();
        assert_eq!(reference, "registry.example.test/org/acme/policies/pii:v1");
        let (decoded, name, version) = lns_policy::artifact::decode(&blob).unwrap();
        assert_eq!(decoded, policy);
        assert_eq!(name.as_deref(), Some("pii"));
        assert_eq!(version.as_deref(), Some("v1"));
        assert!(String::from_utf8(out).unwrap().contains("sha256:abc"));
    }

    #[tokio::test]
    async fn push_errors_when_the_policy_file_is_missing() {
        let dir = TempDir::new().unwrap();
        let args = PolicyPushArgs {
            file: dir.path().join("absent.yaml"),
            reference: "reg/p:1".into(),
        };
        let err = push(&args, dir.path(), &no_registry(), &mut Vec::new())
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("does not exist"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn push_surfaces_a_registry_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("lns-policy.yaml");
        Policy::default().save_atomic(&path).unwrap();
        let registry = FakeRegistry {
            fail: Some("registry said no".into()),
            ..Default::default()
        };
        let args = PolicyPushArgs {
            file: path,
            reference: "reg/p:1".into(),
        };
        let err = push(&args, dir.path(), &registry, &mut Vec::new())
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("registry said no"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn pull_writes_the_policy_to_the_output_file_and_reports_the_digest() {
        let dir = TempDir::new().unwrap();
        let mut policy = Policy::default();
        policy.add_rule(RouteRule::allow_host("api.example.test"));
        let blob = lns_policy::artifact::encode(&policy, Some("pii"), Some("v1")).unwrap();
        let registry = FakeRegistry {
            pull_blob: blob,
            pull_digest: "sha256:def".into(),
            ..Default::default()
        };
        let out_path = dir.path().join("pulled.yaml");
        let args = PolicyPullArgs {
            reference: "reg/p:1".into(),
            output: Some(out_path.clone()),
        };
        let mut out = Vec::new();
        pull(&args, &registry, &mut out).await.unwrap();
        assert_eq!(Policy::load_or_default(&out_path).unwrap(), policy);
        assert!(String::from_utf8(out).unwrap().contains("sha256:def"));
    }

    #[tokio::test]
    async fn pull_renders_yaml_to_stdout_when_no_output_is_given() {
        let mut policy = Policy::default();
        policy.add_rule(RouteRule::allow_host("api.example.test"));
        let blob = lns_policy::artifact::encode(&policy, None, None).unwrap();
        let registry = FakeRegistry {
            pull_blob: blob,
            pull_digest: "sha256:def".into(),
            ..Default::default()
        };
        let args = PolicyPullArgs {
            reference: "reg/p:1".into(),
            output: None,
        };
        let mut out = Vec::new();
        pull(&args, &registry, &mut out).await.unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.contains("api.example.test"), "got: {text}");
        assert!(
            !text.contains("sha256:def"),
            "stdout must be clean YAML: {text}"
        );
    }

    #[tokio::test]
    async fn pull_surfaces_a_malformed_artifact() {
        let registry = FakeRegistry {
            pull_blob: b"not json".to_vec(),
            pull_digest: "sha256:def".into(),
            ..Default::default()
        };
        let args = PolicyPullArgs {
            reference: "reg/p:1".into(),
            output: None,
        };
        let err = pull(&args, &registry, &mut Vec::new()).await.unwrap_err();
        assert!(
            format!("{err:#}").contains("decoding pulled policy"),
            "got: {err:#}"
        );
    }

    #[tokio::test]
    async fn pull_surfaces_a_registry_error() {
        let registry = FakeRegistry {
            fail: Some("not found".into()),
            ..Default::default()
        };
        let args = PolicyPullArgs {
            reference: "reg/p:1".into(),
            output: None,
        };
        let err = pull(&args, &registry, &mut Vec::new()).await.unwrap_err();
        assert!(format!("{err:#}").contains("not found"), "got: {err:#}");
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
