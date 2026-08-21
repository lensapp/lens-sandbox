//! Run the staged `pre-start` scripts before the workload (`docs/sandbox-spec.md` §3.1.13), after the cage and the first policy frame so a script reaches exactly what the workload may.

pub(crate) mod ids;

use std::collections::HashMap;

use async_trait::async_trait;
use lens_sandbox_core::activity::ActivityStream;
use lens_sandbox_core::child_spawner::ChildSpec;
use lens_sandbox_core::privilege::SandboxCredentials;
use lns_session::ScriptManifestStep;

use crate::config::AgentConfig;
use crate::dispatcher::agent::{build_script_env, resolve_script_cwd};

/// The exit status a failed script gives the run: a failure before the workload started, which is what `125` already means, so the workload's own statuses stay reserved (`docs/cli-spec.md` §5).
pub(crate) const BEFORE_WORKLOAD_EXIT: i32 = 125;

/// One staged script, ready to spawn, with the label a failure identifies it by.
pub(crate) struct PreparedScript {
    pub label: String,
    pub spec: ChildSpec,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ScriptFailure {
    UnresolvableUser {
        label: String,
        user: String,
        reason: String,
    },
    Spawn {
        label: String,
        position: String,
        reason: String,
    },
    Exit {
        label: String,
        position: String,
        code: i32,
    },
}

impl std::fmt::Display for ScriptFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScriptFailure::UnresolvableUser {
                label,
                user,
                reason,
            } => write!(
                f,
                "pre-start script {label:?} asks to run as {user:?}, which this sandbox cannot resolve: {reason}; the workload did not start"
            ),
            ScriptFailure::Spawn {
                label,
                position,
                reason,
            } => write!(
                f,
                "pre-start script {position} ({label:?}) could not start: {reason}; the workload did not start"
            ),
            ScriptFailure::Exit {
                label,
                position,
                code,
            } => write!(
                f,
                "pre-start script {position} ({label:?}) exited with code {code}; the workload did not start"
            ),
        }
    }
}

/// Spawns one prepared script and reports the status it exited with.
#[async_trait]
pub(crate) trait StepRunner: Send + Sync {
    async fn run(
        &self,
        script: &PreparedScript,
        position: &str,
        activity: ActivityStream,
    ) -> Result<i32, String>;
}

/// What the supervisor does when a script fails; the production impl never returns, so only a fake can let a test observe that the workload stayed unspawned.
#[async_trait]
pub(crate) trait Abort: Send + Sync {
    async fn refuse(&self, failure: &ScriptFailure);
}

/// Turn the staged manifest into spawnable specs, resolving every script's user before the first one runs — so a run naming an identity this guest lacks fails before it has half-prepared itself.
pub(crate) fn prepare(
    config: &AgentConfig,
    sandbox_creds: Option<&SandboxCredentials>,
    env: &HashMap<String, String>,
    passwd: &dyn ids::Passwd,
) -> Result<Vec<PreparedScript>, ScriptFailure> {
    config
        .scripts
        .iter()
        .map(|step| prepare_one(step, config, sandbox_creds, env, passwd))
        .collect()
}

fn prepare_one(
    step: &ScriptManifestStep,
    config: &AgentConfig,
    sandbox_creds: Option<&SandboxCredentials>,
    env: &HashMap<String, String>,
    passwd: &dyn ids::Passwd,
) -> Result<PreparedScript, ScriptFailure> {
    let creds =
        match &step.user {
            None => sandbox_creds.cloned(),
            Some(user) => Some(ids::resolve(user, passwd).map_err(|reason| {
                ScriptFailure::UnresolvableUser {
                    label: step.label.clone(),
                    user: user.clone(),
                    reason,
                }
            })?),
        };
    Ok(PreparedScript {
        label: step.label.clone(),
        spec: ChildSpec {
            // `-e` so the first failing command ends the script, per §3.1.13.
            argv: vec!["sh".into(), "-e".into(), step.script.clone()],
            cwd: Some(resolve_script_cwd(creds.as_ref())),
            env: build_script_env(config, creds.as_ref(), env),
            creds: crate::run_as::setuid_creds(creds.as_ref()),
            is_root: config.core.is_root,
        },
    })
}

/// Run every prepared script in order, stopping at the first that fails.
pub(crate) async fn run_all(
    scripts: &[PreparedScript],
    runner: &dyn StepRunner,
    activity: &ActivityStream,
) -> Result<(), ScriptFailure> {
    let total = scripts.len();
    for (index, script) in scripts.iter().enumerate() {
        let position = format!("{}/{total}", index + 1);
        match runner.run(script, &position, activity.clone()).await {
            Err(reason) => {
                return Err(ScriptFailure::Spawn {
                    label: script.label.clone(),
                    position,
                    reason,
                });
            }
            Ok(0) => {}
            Ok(code) => {
                return Err(ScriptFailure::Exit {
                    label: script.label.clone(),
                    position,
                    code,
                });
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeRunner {
        /// What each call answers, in order; a missing entry answers success.
        answers: Mutex<Vec<Result<i32, String>>>,
        seen: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl StepRunner for FakeRunner {
        async fn run(
            &self,
            script: &PreparedScript,
            position: &str,
            _activity: ActivityStream,
        ) -> Result<i32, String> {
            self.seen
                .lock()
                .expect("the fake's log is uncontended")
                .push(format!("{position} {}", script.label));
            let mut answers = self
                .answers
                .lock()
                .expect("the fake's answers are uncontended");
            if answers.is_empty() {
                return Ok(0);
            }
            answers.remove(0)
        }
    }

    fn prepared(labels: &[&str]) -> Vec<PreparedScript> {
        labels
            .iter()
            .map(|label| PreparedScript {
                label: (*label).to_string(),
                spec: ChildSpec {
                    argv: vec!["sh".into(), "-e".into(), "/.lens/scripts/000.sh".into()],
                    cwd: None,
                    env: HashMap::new(),
                    creds: None,
                    is_root: false,
                },
            })
            .collect()
    }

    async fn run(scripts: &[PreparedScript], runner: &FakeRunner) -> Result<(), ScriptFailure> {
        run_all(scripts, runner, &ActivityStream::new()).await
    }

    #[tokio::test]
    async fn the_scripts_run_in_manifest_order_each_named_by_its_position() {
        let runner = FakeRunner::default();
        run(&prepared(&["install psql", "seed the cache"]), &runner)
            .await
            .expect("both succeed");
        assert_eq!(
            *runner.seen.lock().expect("uncontended"),
            ["1/2 install psql", "2/2 seed the cache"],
            "the order is the merge's order, and the position is what tells a reader which of several scripts they are watching"
        );
    }

    #[tokio::test]
    async fn a_script_that_exits_non_zero_stops_the_sequence() {
        let runner = FakeRunner {
            answers: Mutex::new(vec![Ok(100)]),
            ..Default::default()
        };
        let failure = run(&prepared(&["install psql", "seed the cache"]), &runner)
            .await
            .expect_err("a failing script refuses the run");
        assert_eq!(
            failure,
            ScriptFailure::Exit {
                label: "install psql".into(),
                position: "1/2".into(),
                code: 100,
            }
        );
        assert_eq!(
            runner.seen.lock().expect("uncontended").len(),
            1,
            "a later script would run against an environment the failed one never finished preparing"
        );
    }

    #[tokio::test]
    async fn a_script_that_cannot_start_is_reported_with_the_reason() {
        let runner = FakeRunner {
            answers: Mutex::new(vec![Err("No such file or directory".into())]),
            ..Default::default()
        };
        let failure = run(&prepared(&["install psql"]), &runner)
            .await
            .expect_err("a script that cannot start refuses the run");
        let message = failure.to_string();
        assert!(
            message.contains("No such file or directory") && message.contains("did not start"),
            "an image shipping no sh fails here, and the reason is the only thing that tells the author why; got: {message}"
        );
    }

    #[tokio::test]
    async fn a_run_declaring_no_scripts_runs_nothing_and_succeeds() {
        let runner = FakeRunner::default();
        run(&[], &runner).await.expect("nothing to do succeeds");
        assert!(
            runner.seen.lock().expect("uncontended").is_empty(),
            "the block is optional, so a run that declares none must reach its workload exactly as it did before this existed"
        );
    }

    #[test]
    fn every_failure_reads_as_one_sentence_naming_the_script_and_the_outcome() {
        let cases = [
            ScriptFailure::UnresolvableUser {
                label: "install psql".into(),
                user: "postgres".into(),
                reason: "no user \"postgres\" in the guest's passwd".into(),
            },
            ScriptFailure::Spawn {
                label: "install psql".into(),
                position: "1/2".into(),
                reason: "No such file or directory".into(),
            },
            ScriptFailure::Exit {
                label: "install psql".into(),
                position: "1/2".into(),
                code: 100,
            },
        ];
        for failure in cases {
            let message = failure.to_string();
            assert!(
                message.contains("install psql") && message.contains("did not start"),
                "a reader has to learn which script failed and that the workload never ran, from one line; got: {message}"
            );
        }
    }
}
