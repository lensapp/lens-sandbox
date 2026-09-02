//! Run the staged `pre-start` scripts before the workload (`docs/sandbox-spec.md` §3.1.13), after the cage and the first policy frame so a script reaches exactly what the workload may.

use std::collections::HashMap;

use async_trait::async_trait;
use lens_sandbox_core::child_spawner::ChildSpec;
use lens_sandbox_core::prestart::{
    self, PreStartFailure, PreStartStep, PreparedScript, ResolvedStep,
};
use lens_sandbox_core::privilege::{Passwd, SandboxCredentials};

use crate::config::AgentConfig;
use crate::dispatcher::agent::build_script_env;

/// The exit status a failed script gives the run: a failure before the workload started, which is what `125` already means, so the workload's own statuses stay reserved (`docs/cli-spec.md` §5).
pub(crate) const BEFORE_WORKLOAD_EXIT: i32 = 125;

/// The core states what went wrong; this states what it cost, which is the supervisor's own vocabulary rather than the library's.
pub(crate) fn refusal(failure: &PreStartFailure) -> String {
    format!("{failure}; the workload did not start")
}

/// What the supervisor does when a script fails; the production impl never returns, so only a fake can let a test observe that the workload stayed unspawned.
#[async_trait]
pub(crate) trait Abort: Send + Sync {
    async fn refuse(&self, failure: &PreStartFailure);
}

/// Turn the staged manifest into spawnable specs, resolving every script's user before the first one runs — so a run naming an identity this guest lacks fails before it has half-prepared itself.
pub(crate) fn prepare(
    config: &AgentConfig,
    sandbox_creds: Option<&SandboxCredentials>,
    env: &HashMap<String, String>,
    passwd: &dyn Passwd,
) -> Result<Vec<PreparedScript>, PreStartFailure> {
    let steps: Vec<PreStartStep> = config
        .scripts
        .iter()
        .map(|step| PreStartStep {
            script: step.script.clone(),
            user: step.user.clone(),
            label: step.label.clone(),
        })
        .collect();
    Ok(prestart::resolve_steps(&steps, sandbox_creds, passwd)?
        .into_iter()
        .map(|step| prepared(step, config, env))
        .collect())
}

fn prepared(
    step: ResolvedStep,
    config: &AgentConfig,
    env: &HashMap<String, String>,
) -> PreparedScript {
    PreparedScript {
        label: step.label,
        spec: ChildSpec {
            // `-e` so the first failing command ends the script, per §3.1.13.
            argv: vec!["sh".into(), "-e".into(), step.script],
            cwd: Some(prestart::script_cwd(step.creds.as_ref())),
            env: build_script_env(config, step.creds.as_ref(), env),
            creds: step.creds,
            is_root: config.core.is_root,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_failure_reads_as_one_sentence_naming_the_script_and_the_outcome() {
        let cases = [
            PreStartFailure::UnresolvableUser {
                label: "install psql".into(),
                user: "postgres".into(),
                reason: "no user \"postgres\" in passwd".into(),
            },
            PreStartFailure::Spawn {
                label: "install psql".into(),
                position: "1/2".into(),
                reason: "No such file or directory".into(),
            },
            PreStartFailure::Exit {
                label: "install psql".into(),
                position: "1/2".into(),
                code: 100,
            },
        ];
        for failure in cases {
            let message = refusal(&failure);
            assert!(
                message.contains("install psql") && message.contains("the workload did not start"),
                "a reader has to learn which script failed and that the workload never ran, from one line; got: {message}"
            );
        }
    }
}
