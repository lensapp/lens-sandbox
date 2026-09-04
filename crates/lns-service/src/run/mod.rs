use anyhow::Result;
use lns_ipc::{Response, WireFrame};
use tokio::sync::mpsc::Sender;

mod decisions;
mod orchestrator;
mod save;
pub(crate) use save::render as render_saved;
mod scratch;
mod shutdown;
pub use orchestrator::{PreparedRun, handle, prepare};
pub use scratch::{RealRemoveDir, RemoveDir, reclaim_run_dir};

pub fn broker_exit_reason(result: &Result<i32>) -> Option<lns_session::BrokerExitReason> {
    result.as_ref().err().and_then(|error| {
        error
            .downcast_ref::<crate::vm::session_client::BrokerRefusal>()
            .map(|refusal| refusal.reason.clone())
    })
}

/// How a run ended: the code its workload left, whether --rm takes its state, and when.
pub struct RunEnd {
    pub code: i32,
    pub auto_remove: bool,
    pub finished_at: String,
}

/// How long a run's end waits for the tee task to record the frames already in flight before it writes the log down.
const LOG_QUIESCE: std::time::Duration = std::time::Duration::from_secs(2);

/// Everything a run's end owes the world: its record gets the exit stamped, its output is written down, and a --rm run takes both with it.
pub async fn conclude_run<F: crate::image_store::Fs, R: RemoveDir, N: Fn(&str)>(
    fs: &F,
    remover: &R,
    cache_root: &std::path::Path,
    run_id: &str,
    end: RunEnd,
    logs: Option<std::sync::Arc<crate::run_log::RunLogBuffer>>,
    note_removed: N,
) {
    if let Err(e) =
        crate::run_record::mark_exited_with(fs, cache_root, run_id, end.code, end.finished_at).await
    {
        crate::log::warn!("run record not updated at exit: {e:#}");
    }
    if !end.auto_remove
        && let Some(buffer) = logs
    {
        write_the_log_down(fs, cache_root, run_id, &buffer, LOG_QUIESCE).await;
    }
    if end.auto_remove {
        crate::run_registry::set_exit_code(run_id, end.code);
        if let crate::run_registry::RemoveOutcome::Removed(_) =
            crate::run_registry::remove_if_exited(run_id)
        {
            note_removed(run_id);
            reclaim_run_dir(remover, cache_root, run_id);
        }
    }
}

/// The tee task may still be appending when a run ends, so the log waits for the buffer to close before it is written.
async fn write_the_log_down<F: crate::image_store::Fs>(
    fs: &F,
    cache_root: &std::path::Path,
    run_id: &str,
    buffer: &crate::run_log::RunLogBuffer,
    quiesce: std::time::Duration,
) {
    if !crate::run_log::await_close(buffer, quiesce).await {
        crate::log::warn!("run log saved before its last frames arrived; the tail is missing");
    }
    if let Err(e) = crate::run_log::save_with(fs, cache_root, run_id, buffer).await {
        crate::log::warn!("run log not saved; this boot's output ends with it: {e}");
    }
}

/// Whether a boot creates a run or revives one: a restart boots over a preserved writable layer, so it must not arm the scratch guard and must find the exact lower stack the layer was written on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchMode {
    Fresh,
    Restart {
        pinned_descriptor_sha256: String,
        created_at: String,
    },
}

/// A run is created once; every restart after that rewrites its record, and the record must keep saying when the run began.
pub fn recorded_created_at(mode: &LaunchMode, now: impl FnOnce() -> String) -> String {
    match mode {
        LaunchMode::Fresh => now(),
        LaunchMode::Restart { created_at, .. } => created_at.clone(),
    }
}

pub fn verify_pinned_descriptor(mode: &LaunchMode, built_sha256: &str) -> Result<()> {
    match mode {
        LaunchMode::Fresh => Ok(()),
        LaunchMode::Restart {
            pinned_descriptor_sha256,
            ..
        } if pinned_descriptor_sha256 == built_sha256 => Ok(()),
        LaunchMode::Restart {
            pinned_descriptor_sha256,
            ..
        } => anyhow::bail!(
            "the sandbox stack changed under this run: its layers now build descriptor {built_sha256}, but its writable layer was written on {pinned_descriptor_sha256}; remove the run with `lns rm` and start a fresh one"
        ),
    }
}

/// The rootfs-assembly progress sink: `span` is the run span captured before `spawn_blocking`, since the blocking thread has no ambient span for the frame forwarder to find the run's channel through.
fn assembling_progress(span: tracing::Span) -> impl Fn(u64, u64) {
    move |current, total| {
        span.in_scope(|| crate::log::progress("Assembling", "rootfs", current, total));
    }
}

pub(super) async fn emit_completion(frame_tx: &Sender<WireFrame>, result: Result<i32>) -> i32 {
    let code = match result {
        Ok(code) => {
            if code != 0 {
                let _ = frame_tx
                    .send(WireFrame::Json(Response::RunLog {
                        level: lns_ipc::LogLevel::Warn,
                        verb: None,
                        message: format!("workload exited with code {code}"),
                    }))
                    .await;
            }
            code
        }
        Err(e) => {
            if e.downcast_ref::<crate::vm::session_client::BrokerRefusal>()
                .is_none()
            {
                let _ = frame_tx
                    .send(WireFrame::Json(Response::RunLog {
                        level: lns_ipc::LogLevel::Error,
                        verb: None,
                        message: format!("{e:#}"),
                    }))
                    .await;
            }
            1
        }
    };
    let _ = frame_tx
        .send(WireFrame::Json(Response::RunExit { code }))
        .await;
    code
}

pub(super) fn build_workload_argv(
    image_config: Option<&oci_client::config::ConfigFile>,
    override_entrypoint: Option<&str>,
    override_cmd: &[String],
    supervised: bool,
) -> Vec<String> {
    if supervised {
        return vec!["/.lens/bin/lns-supervisor".to_string()];
    }
    let joined = match image_config {
        Some(cfg) => {
            crate::workload_argv::from_image_config(cfg, override_entrypoint, override_cmd)
        }
        None => crate::workload_argv::from_imageless(override_entrypoint, override_cmd),
    };
    vec!["/bin/sh".to_string(), "-c".to_string(), joined]
}

/// The run-scoped environment sources composed around the image's own env: the user's `-e` vars, the resolved workdir, and the declared tools' bin dirs.
pub(super) struct EnvInputs<'a> {
    pub user_env: &'a [String],
    pub workdir: Option<&'a str>,
    pub tools: &'a crate::workload_env::ToolRuntime,
    /// What this run's connectors put in its environment: what a granted method fills, and the marker a declaration was left to one for.
    pub connectors: &'a crate::workload_env::ConnectorEnv,
}

pub(super) fn exec_env_strings(
    image_config: Option<&oci_client::config::ConfigFile>,
    override_entrypoint: Option<&str>,
    override_cmd: &[String],
    supervised: bool,
    inputs: EnvInputs<'_>,
) -> crate::workload_env::WorkloadEnv {
    let agent_command = supervised.then(|| match image_config {
        Some(cfg) => {
            crate::workload_argv::from_image_config(cfg, override_entrypoint, override_cmd)
        }
        None => crate::workload_argv::from_imageless(override_entrypoint, override_cmd),
    });
    let image_env = image_config
        .and_then(|c| c.config.as_ref())
        .and_then(|c| c.env.as_deref());
    crate::workload_env::run_workload_env(
        image_env,
        inputs.user_env,
        agent_command.as_deref(),
        inputs.workdir,
        inputs.tools,
        inputs.connectors,
    )
}

/// The image ref, command, and env a sandbox run boots with, once its definition is resolved.
pub(super) struct SandboxLaunch {
    pub image: String,
    pub cmd: Vec<String>,
    pub env: Vec<String>,
}

/// What the caller asked for, kept in the three layers the CLI can tell apart: the flags as typed, and the config defaults the machine fills gaps from.
pub(super) struct RequestedSize {
    pub cpus: u8,
    pub cpus_explicit: bool,
    pub mem_mib: usize,
    pub mem_explicit: bool,
    pub cpus_config: Option<u8>,
    pub mem_config: Option<usize>,
}

impl RequestedSize {
    pub fn from_args(args: &lns_ipc::RunImageArgs) -> Self {
        Self {
            cpus: args.cpus,
            cpus_explicit: args.cpus_explicit,
            mem_mib: args.mem,
            mem_explicit: args.mem_explicit,
            cpus_config: args.cpus_config,
            mem_config: args.mem_config,
        }
    }
}

/// Size a sandbox run's VM: an explicit `--cpus`/`-m` wins (even at a value equal to the built-in default), then the sandbox's own resources, then the machine's config defaults, then the built-in size.
pub(super) fn sandbox_vm_size(
    resources: Option<&lns_artifact::spec::Resources>,
    requested: &RequestedSize,
    host: Option<lns_artifact::resources::HostCapacity>,
) -> lns_artifact::resources::VmSize {
    use crate::artifact::resources::resolve_size;
    use lns_artifact::resources::{ConfiguredDefaults, DEFAULT_VM_SIZE, ResourceOverrides};
    let overrides = ResourceOverrides {
        cpus: requested.cpus_explicit.then_some(requested.cpus),
        mem_mib: requested.mem_explicit.then_some(requested.mem_mib),
    };
    let configured = ConfiguredDefaults {
        cpus: requested.cpus_config,
        mem_mib: requested.mem_config,
    };
    resolve_size(
        resources,
        &overrides,
        configured.over(DEFAULT_VM_SIZE),
        host,
    )
}

/// Merge a resolved sandbox's workload with the user's run args: boot the sandbox base image, take the sandbox command unless the user gave one after `--`, and layer env base-image < sandbox < user `-e`.
pub(super) fn sandbox_launch(
    workload: &crate::artifact::assembly::AssembledWorkload,
    user_cmd: &[String],
    user_env: &[String],
) -> SandboxLaunch {
    let cmd = if user_cmd.is_empty() {
        workload
            .command
            .as_deref()
            .map(shell_split)
            .unwrap_or_default()
    } else {
        user_cmd.to_vec()
    };
    let mut env: Vec<String> = workload
        .env
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();
    env.extend(user_env.iter().cloned());
    SandboxLaunch {
        image: workload.base_image.clone(),
        cmd,
        env,
    }
}

/// Split an agent command string into argv with shell quoting honored, so `agent --prompt "hello world"` stays one argument; an unbalanced-quote string falls back to whitespace splitting rather than dropping the command.
fn shell_split(command: &str) -> Vec<String> {
    shlex::split(command)
        .unwrap_or_else(|| command.split_whitespace().map(str::to_string).collect())
}

pub(super) fn vm_ended_before_connector(
    result: std::result::Result<Result<()>, tokio::task::JoinError>,
) -> anyhow::Error {
    match result {
        Ok(Ok(())) => anyhow::anyhow!("VM exited before producing a VsockConnector"),
        Ok(Err(e)) => e.context("vm boot failed before connector arrived"),
        Err(join_err) => anyhow::anyhow!("vm boot task panicked: {join_err}"),
    }
}

pub(super) fn connector_never_arrived() -> anyhow::Error {
    anyhow::anyhow!("VM backend never produced a VsockConnector")
}

#[cfg(test)]
mod assembling_progress_tests {
    use super::*;
    use lns_ipc::{Response, WireFrame};

    fn assembling_frame(current: u64, total: u64) -> WireFrame {
        WireFrame::Json(Response::RunProgress {
            verb: "Assembling".to_string(),
            message: "rootfs".to_string(),
            current,
            total,
        })
    }

    #[test]
    fn the_sink_forwards_an_assembling_run_progress_frame() {
        let frames = crate::log::testing::capture_run_frames(|| {
            let sink = assembling_progress(tracing::Span::current());
            sink(3072, 8192);
        });
        assert_eq!(frames, vec![assembling_frame(3072, 8192)]);
    }

    #[test]
    fn the_sink_forwards_from_a_blocking_thread_outside_the_run_task() {
        let frames = crate::log::testing::capture_run_frames(|| {
            let sink = assembling_progress(tracing::Span::current());
            let dispatch = tracing::dispatcher::get_default(|d| d.clone());
            std::thread::scope(|s| {
                s.spawn(move || tracing::dispatcher::with_default(&dispatch, || sink(0, 8192)));
            });
        });
        assert_eq!(frames, vec![assembling_frame(0, 8192)]);
    }
}

#[cfg(test)]
mod tests {
    const TEST_HOST: lns_artifact::resources::HostCapacity =
        lns_artifact::resources::HostCapacity {
            cpus: 10,
            mem_mib: 16384,
        };
    use super::*;
    use lns_ipc::LogLevel;
    use tokio::sync::mpsc;

    fn resources(cpu: i64, mem_mib: i64) -> lns_artifact::spec::Resources {
        lns_artifact::spec::Resources {
            cpu: Some(lns_artifact::spec::Quantity::Int(cpu)),
            memory: Some(lns_artifact::spec::Quantity::Int(mem_mib)),
            disk: None,
        }
    }

    fn requested(cpus: Option<u8>, mem_mib: Option<usize>) -> RequestedSize {
        RequestedSize {
            cpus: cpus.unwrap_or(lns_artifact::resources::DEFAULT_VM_SIZE.cpus),
            cpus_explicit: cpus.is_some(),
            mem_mib: mem_mib.unwrap_or(lns_artifact::resources::DEFAULT_VM_SIZE.mem_mib),
            mem_explicit: mem_mib.is_some(),
            cpus_config: None,
            mem_config: None,
        }
    }

    fn configured(cpus: Option<u8>, mem_mib: Option<usize>) -> RequestedSize {
        RequestedSize {
            cpus_config: cpus,
            mem_config: mem_mib,
            ..requested(None, None)
        }
    }

    #[test]
    fn sandbox_vm_size_uses_the_sandbox_resources_when_no_flag_is_set() {
        let res = resources(4, 2048);
        assert_eq!(
            {
                let s = sandbox_vm_size(Some(&res), &requested(None, None), Some(TEST_HOST));
                (s.cpus, s.mem_mib)
            },
            (4, 2048)
        );
    }

    #[test]
    fn sandbox_vm_size_keeps_the_sandbox_resources_over_a_config_default() {
        let res = resources(2, 1024);
        assert_eq!(
            {
                let s = sandbox_vm_size(
                    Some(&res),
                    &configured(Some(4), Some(4096)),
                    Some(TEST_HOST),
                );
                (s.cpus, s.mem_mib)
            },
            (2, 1024),
            "a sandbox that declares its own resources must not be resized by whoever cloned it"
        );
    }

    #[test]
    fn sandbox_vm_size_lets_a_flag_outrank_both_the_sandbox_and_a_config_default() {
        let res = resources(2, 1024);
        let request = RequestedSize {
            cpus_config: Some(4),
            mem_config: Some(4096),
            ..requested(Some(8), Some(8192))
        };
        assert_eq!(
            {
                let s = sandbox_vm_size(Some(&res), &request, Some(TEST_HOST));
                (s.cpus, s.mem_mib)
            },
            (8, 8192)
        );
    }

    #[test]
    fn sandbox_vm_size_falls_back_to_a_config_default_when_nothing_else_decides() {
        assert_eq!(
            {
                let s = sandbox_vm_size(None, &configured(Some(4), Some(4096)), Some(TEST_HOST));
                (s.cpus, s.mem_mib)
            },
            (4, 4096)
        );
    }

    #[test]
    fn sandbox_vm_size_falls_back_to_the_builtin_when_no_source_says_anything() {
        assert_eq!(
            {
                let s = sandbox_vm_size(None, &requested(None, None), Some(TEST_HOST));
                (s.cpus, s.mem_mib)
            },
            (
                lns_artifact::resources::DEFAULT_VM_SIZE.cpus,
                lns_artifact::resources::DEFAULT_VM_SIZE.mem_mib
            )
        );
    }

    fn run_args() -> lns_ipc::RunImageArgs {
        serde_json::from_value(serde_json::json!({
            "image": "x:1",
            "cpus": 2,
            "mem": 2048,
            "policy_path": null,
            "cmd": [],
            "debug": false,
        }))
        .expect("the fixture names every field the wire requires")
    }

    #[test]
    fn requested_size_reads_the_three_layers_the_wire_carries() {
        let mut args = run_args();
        args.cpus = 8;
        args.cpus_explicit = true;
        args.mem = 8192;
        args.mem_explicit = false;
        args.cpus_config = Some(4);
        args.mem_config = Some(4096);
        let request = RequestedSize::from_args(&args);
        assert_eq!(
            (
                request.cpus,
                request.cpus_explicit,
                request.mem_mib,
                request.mem_explicit,
                request.cpus_config,
                request.mem_config
            ),
            (8, true, 8192, false, Some(4), Some(4096))
        );
    }

    #[test]
    fn sandbox_vm_size_lets_an_explicit_request_override_the_sandbox() {
        let res = resources(4, 2048);
        assert_eq!(
            {
                let s =
                    sandbox_vm_size(Some(&res), &requested(Some(2), Some(1024)), Some(TEST_HOST));
                (s.cpus, s.mem_mib)
            },
            (2, 1024)
        );
    }

    #[test]
    fn sandbox_vm_size_honors_an_explicit_request_that_equals_the_builtin_default() {
        let res = resources(4, 2048);
        assert_eq!(
            {
                let s =
                    sandbox_vm_size(Some(&res), &requested(Some(1), Some(512)), Some(TEST_HOST));
                (s.cpus, s.mem_mib)
            },
            (1, 512),
            "a user who explicitly asks for the default size must be able to constrain a greedy sandbox"
        );
    }

    #[test]
    fn sandbox_vm_size_falls_back_to_the_request_when_the_sandbox_is_silent() {
        assert_eq!(
            {
                let s = sandbox_vm_size(None, &requested(Some(8), Some(4096)), Some(TEST_HOST));
                (s.cpus, s.mem_mib)
            },
            (8, 4096)
        );
    }

    #[tokio::test]
    async fn emit_completion_ok_zero_sends_only_run_exit_zero() {
        let (tx, mut rx) = mpsc::channel::<WireFrame>(4);
        let code = emit_completion(&tx, Ok(0)).await;
        assert_eq!(code, 0);
        match rx.recv().await {
            Some(WireFrame::Json(Response::RunExit { code: 0 })) => {}
            other => panic!("expected RunExit{{0}}, got {other:?}"),
        }
        assert!(
            matches!(rx.try_recv(), Err(mpsc::error::TryRecvError::Empty)),
            "ok path must not emit extra frames",
        );
    }

    #[tokio::test]
    async fn emit_completion_nonzero_ok_warns_before_run_exit() {
        for code in [1_i32, 42, 130, 143] {
            let (tx, mut rx) = mpsc::channel::<WireFrame>(4);
            let returned = emit_completion(&tx, Ok(code)).await;
            assert_eq!(returned, code, "return value should mirror input");
            match rx.recv().await {
                Some(WireFrame::Json(Response::RunLog {
                    level: LogLevel::Warn,
                    verb,
                    message,
                })) => {
                    assert!(verb.is_none(), "the exit announcement carries no verb");
                    assert!(
                        message.contains("exited"),
                        "announcement must say the workload exited: {message}"
                    );
                    assert!(
                        message.contains(&code.to_string()),
                        "announcement must name the exit code: {message}"
                    );
                }
                other => panic!("expected a RunLog{{Warn}} announcement, got {other:?}"),
            }
            match rx.recv().await {
                Some(WireFrame::Json(Response::RunExit { code: c })) => {
                    assert_eq!(c, code, "frame must carry workload's exit code");
                }
                other => panic!("expected RunExit{{{code}}}, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn emit_completion_err_sends_runlog_error_then_run_exit_one() {
        let (tx, mut rx) = mpsc::channel::<WireFrame>(4);
        let err = anyhow::anyhow!("boot blew up: kernel ENOENT");
        let code = emit_completion(&tx, Err(err)).await;
        assert_eq!(code, 1);

        match rx.recv().await {
            Some(WireFrame::Json(Response::RunLog {
                level: LogLevel::Error,
                verb,
                message,
            })) => {
                assert!(verb.is_none(), "error frames carry no verb");
                assert!(message.contains("boot blew up"));
            }
            other => panic!("expected RunLog{{Error}}, got {other:?}"),
        }
        match rx.recv().await {
            Some(WireFrame::Json(Response::RunExit { code: 1 })) => {}
            other => panic!("expected RunExit{{1}}, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_dhcp_refusal_is_not_misreported_as_a_workload_exit() {
        let (tx, mut rx) = mpsc::channel::<WireFrame>(2);
        let result = Err(
            anyhow::Error::new(crate::vm::session_client::BrokerRefusal {
                reason: lns_session::BrokerExitReason::NoDhcpLease,
            })
            .context("reading broker session frames"),
        );
        assert_eq!(
            broker_exit_reason(&result),
            Some(lns_session::BrokerExitReason::NoDhcpLease),
            "the service owns the stable reason shown in the audit"
        );
        let code = emit_completion(&tx, result).await;
        assert_eq!(code, 1);
        match rx.recv().await {
            Some(WireFrame::Json(Response::RunExit { code: reported })) => {
                assert_eq!(reported, code)
            }
            other => panic!("expected only the typed RunExit, got {other:?}"),
        }
        assert!(matches!(
            rx.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
    }

    #[test]
    fn build_workload_argv_supervised_returns_supervisor_wrapper() {
        let argv = build_workload_argv(None, None, &["echo".into(), "hi".into()], true);
        assert_eq!(argv, vec!["/.lens/bin/lns-supervisor".to_string()]);
    }

    #[test]
    fn build_workload_argv_unsupervised_imageless_wraps_in_sh_dash_c() {
        let argv = build_workload_argv(None, None, &["echo".into(), "hello world".into()], false);
        assert_eq!(argv.len(), 3);
        assert_eq!(argv[0], "/bin/sh");
        assert_eq!(argv[1], "-c");
        assert_eq!(argv[2], "echo 'hello world'");
    }

    #[test]
    fn exec_env_strings_supervised_derives_agent_command_from_the_override_cmd() {
        let env = exec_env_strings(
            None,
            None,
            &["echo".into(), "hi".into()],
            true,
            EnvInputs {
                user_env: &[],
                workdir: None,
                tools: &Default::default(),
                connectors: &Default::default(),
            },
        );
        assert!(
            env.env.contains(&"AGENT_COMMAND=echo hi".to_string()),
            "expected AGENT_COMMAND in supervised env, got: {env:?}"
        );
    }

    #[test]
    fn exec_env_strings_supervised_preserves_whitespace_and_quotes() {
        let env = exec_env_strings(
            None,
            None,
            &[
                "/bin/sh".into(),
                "-c".into(),
                "echo \"hi $USER\"; uname -a".into(),
            ],
            true,
            EnvInputs {
                user_env: &[],
                workdir: None,
                tools: &Default::default(),
                connectors: &Default::default(),
            },
        );
        let agent = env
            .env
            .iter()
            .find_map(|e| e.strip_prefix("AGENT_COMMAND="))
            .expect("AGENT_COMMAND entry");
        assert!(
            agent.contains("echo \"hi $USER\""),
            "multi-token shell strings must be preserved verbatim inside the quoted arg; got: {agent}"
        );
    }

    #[test]
    fn exec_env_strings_unsupervised_carries_user_env_without_supervisor_vars() {
        let env = exec_env_strings(
            None,
            None,
            &["echo".into(), "hi".into()],
            false,
            EnvInputs {
                user_env: &["FOO=bar".into()],
                workdir: None,
                tools: &Default::default(),
                connectors: &Default::default(),
            },
        );
        assert_eq!(
            env.env,
            ["FOO=bar"],
            "a policy-less run must still inject -e and add no supervisor vars"
        );
    }

    #[test]
    fn build_workload_argv_unsupervised_with_image_uses_entrypoint_plus_override() {
        let cfg = oci_client::config::ConfigFile {
            architecture: "arm64".into(),
            os: "linux".into(),
            config: Some(oci_client::config::Config {
                entrypoint: Some(vec!["/entry".into()]),
                cmd: Some(vec!["default-arg".into()]),
                ..Default::default()
            }),
            ..Default::default()
        };
        let argv = build_workload_argv(Some(&cfg), None, &[], false);
        assert_eq!(argv[2], "/entry default-arg");
        let argv = build_workload_argv(Some(&cfg), None, &["override".into()], false);
        assert_eq!(argv[2], "/entry override");
    }

    #[test]
    fn exec_env_strings_supervised_with_image_uses_image_entrypoint_and_cmd() {
        let cfg = oci_client::config::ConfigFile {
            architecture: "arm64".into(),
            os: "linux".into(),
            config: Some(oci_client::config::Config {
                entrypoint: Some(vec!["/srv".into()]),
                cmd: Some(vec!["arg".into()]),
                ..Default::default()
            }),
            ..Default::default()
        };
        let env = exec_env_strings(
            Some(&cfg),
            None,
            &[],
            true,
            EnvInputs {
                user_env: &[],
                workdir: None,
                tools: &Default::default(),
                connectors: &Default::default(),
            },
        );
        assert!(env.env.contains(&"AGENT_COMMAND=/srv arg".to_string()));
    }

    #[test]
    fn exec_env_strings_honors_image_env_and_lets_user_override_it() {
        let cfg = oci_client::config::ConfigFile {
            architecture: "arm64".into(),
            os: "linux".into(),
            config: Some(oci_client::config::Config {
                env: Some(vec!["PORT=3003".into(), "NODE_ENV=production".into()]),
                ..Default::default()
            }),
            ..Default::default()
        };
        let env = exec_env_strings(
            Some(&cfg),
            None,
            &[],
            true,
            EnvInputs {
                user_env: &["PORT=4000".into()],
                workdir: None,
                tools: &Default::default(),
                connectors: &Default::default(),
            },
        );
        assert!(
            env.env.contains(&"PORT=4000".to_string()),
            "user overrides image: {env:?}"
        );
        assert!(
            !env.env.contains(&"PORT=3003".to_string()),
            "image value replaced: {env:?}"
        );
        assert!(
            env.env.contains(&"NODE_ENV=production".to_string()),
            "image env kept: {env:?}"
        );
    }

    #[test]
    fn vm_ended_before_connector_reports_a_clean_vm_exit() {
        let err = vm_ended_before_connector(Ok(Ok(())));
        assert_eq!(
            format!("{err:#}"),
            "VM exited before producing a VsockConnector"
        );
    }

    #[test]
    fn vm_ended_before_connector_wraps_a_boot_failure_with_context() {
        let err = vm_ended_before_connector(Ok(Err(anyhow::anyhow!("kernel ENOENT"))));
        let rendered = format!("{err:#}");
        assert!(
            rendered.contains("vm boot failed before connector arrived"),
            "missing context: {rendered}"
        );
        assert!(
            rendered.contains("kernel ENOENT"),
            "missing cause: {rendered}"
        );
    }

    #[tokio::test]
    async fn vm_ended_before_connector_reports_a_panicked_boot_task() {
        let join_err = tokio::spawn(async { panic!("boom in boot") })
            .await
            .expect_err("the task panicked, so join must fail");
        let err = vm_ended_before_connector(Err(join_err));
        assert!(
            format!("{err:#}").starts_with("vm boot task panicked:"),
            "got: {err:#}"
        );
    }

    #[test]
    fn connector_never_arrived_names_the_dropped_connector() {
        let err = connector_never_arrived();
        assert_eq!(
            format!("{err:#}"),
            "VM backend never produced a VsockConnector"
        );
    }

    fn workload(
        command: Option<&str>,
        env: &[(&str, &str)],
    ) -> crate::artifact::assembly::AssembledWorkload {
        let resolved = crate::artifact::assembly::ResolvedSandbox {
            base_image: "registry.example.test/base@sha256:abc".into(),
            command: command.map(str::to_string),
            env: env
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            ..Default::default()
        };
        crate::artifact::assembly::assemble(&resolved)
    }

    #[test]
    fn sandbox_launch_boots_the_base_image_with_the_agent_command() {
        let w = workload(Some("agent --serve"), &[]);
        let launch = sandbox_launch(&w, &[], &[]);
        assert_eq!(launch.image, "registry.example.test/base@sha256:abc");
        assert_eq!(launch.cmd, vec!["agent".to_string(), "--serve".to_string()]);
    }

    #[test]
    fn sandbox_launch_keeps_a_quoted_agent_argument_as_one_token() {
        let w = workload(Some(r#"agent --prompt "hello world" --flag"#), &[]);
        let launch = sandbox_launch(&w, &[], &[]);
        assert_eq!(
            launch.cmd,
            vec![
                "agent".to_string(),
                "--prompt".to_string(),
                "hello world".to_string(),
                "--flag".to_string()
            ],
            "a quoted argument must survive as a single argv entry, not be split on the space"
        );
    }

    #[test]
    fn sandbox_launch_falls_back_to_whitespace_on_an_unbalanced_quote() {
        let w = workload(Some(r#"agent "unclosed"#), &[]);
        let launch = sandbox_launch(&w, &[], &[]);
        assert_eq!(
            launch.cmd,
            vec!["agent".to_string(), "\"unclosed".to_string()]
        );
    }

    #[test]
    fn sandbox_launch_lets_a_user_command_after_dashdash_override_the_agent() {
        let w = workload(Some("agent --serve"), &[]);
        let launch = sandbox_launch(&w, &["bash".to_string()], &[]);
        assert_eq!(launch.cmd, vec!["bash".to_string()]);
    }

    #[test]
    fn sandbox_launch_leaves_the_command_empty_when_the_agent_declares_none() {
        let w = workload(None, &[]);
        let launch = sandbox_launch(&w, &[], &[]);
        assert!(
            launch.cmd.is_empty(),
            "no agent command → fall back to image"
        );
    }

    #[test]
    fn sandbox_launch_layers_user_env_after_the_agent_env_so_the_user_wins() {
        let w = workload(Some("agent"), &[("MODE", "research"), ("PORT", "3003")]);
        let launch = sandbox_launch(&w, &[], &["PORT=4000".to_string()]);
        assert_eq!(
            launch.env,
            vec![
                "MODE=research".to_string(),
                "PORT=3003".to_string(),
                "PORT=4000".to_string(),
            ],
            "agent env first, user -e appended so last-wins env resolution prefers the user",
        );
    }
}

#[cfg(test)]
mod launch_mode_tests {
    use super::*;

    struct EmptyFs;
    impl crate::image_store::Fs for EmptyFs {
        async fn read_dir(&self, _: &std::path::Path) -> std::io::Result<Vec<std::path::PathBuf>> {
            Err(std::io::Error::from(std::io::ErrorKind::NotFound))
        }
        async fn read(&self, _: &std::path::Path) -> std::io::Result<Vec<u8>> {
            Err(std::io::Error::from(std::io::ErrorKind::NotFound))
        }
        async fn write(&self, _: &std::path::Path, _: &[u8]) -> std::io::Result<()> {
            Ok(())
        }
        async fn remove_file(&self, _: &std::path::Path) -> std::io::Result<()> {
            Ok(())
        }
    }

    struct NoopRemover;
    impl RemoveDir for NoopRemover {
        fn remove_dir_all(&self, _: &std::path::Path) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn the_test_fakes_honour_their_whole_port_surface() {
        use crate::image_store::Fs as _;
        assert!(EmptyFs.read_dir(std::path::Path::new("/")).await.is_err());
        assert!(EmptyFs.write(std::path::Path::new("/x"), b"").await.is_ok());
        assert!(
            EmptyFs
                .remove_file(std::path::Path::new("/x"))
                .await
                .is_ok()
        );
        assert!(
            NoopRemover
                .remove_dir_all(std::path::Path::new("/x"))
                .is_ok()
        );
    }

    #[derive(Default)]
    struct RecordingFs(std::sync::Mutex<Vec<(std::path::PathBuf, Vec<u8>)>>);

    impl crate::image_store::Fs for RecordingFs {
        async fn read_dir(&self, _: &std::path::Path) -> std::io::Result<Vec<std::path::PathBuf>> {
            Err(std::io::Error::from(std::io::ErrorKind::NotFound))
        }
        async fn read(&self, _: &std::path::Path) -> std::io::Result<Vec<u8>> {
            Err(std::io::Error::from(std::io::ErrorKind::NotFound))
        }
        async fn write(&self, p: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
            self.0
                .lock()
                .unwrap()
                .push((p.to_path_buf(), bytes.to_vec()));
            Ok(())
        }
        async fn remove_file(&self, _: &std::path::Path) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn logs_with(line: &[u8]) -> std::sync::Arc<crate::run_log::RunLogBuffer> {
        let buffer = std::sync::Arc::new(crate::run_log::RunLogBuffer::default());
        buffer.append(crate::run_log::StreamKind::Stdout, line);
        buffer.close(0);
        buffer
    }

    async fn conclude<F: crate::image_store::Fs>(
        fs: &F,
        run_id: &str,
        end: RunEnd,
        logs: Option<std::sync::Arc<crate::run_log::RunLogBuffer>>,
    ) -> Vec<String> {
        let removed = std::sync::Mutex::new(Vec::new());
        conclude_run(
            fs,
            &NoopRemover,
            std::path::Path::new("/cache"),
            run_id,
            end,
            logs,
            |id| removed.lock().unwrap().push(id.to_string()),
        )
        .await;
        removed.into_inner().unwrap()
    }

    fn ended(auto_remove: bool) -> RunEnd {
        RunEnd {
            code: 0,
            auto_remove,
            finished_at: "t".into(),
        }
    }

    #[tokio::test]
    async fn a_runs_output_is_written_beside_its_record_when_the_run_ends() {
        let fs = RecordingFs::default();
        conclude(&fs, "aa07", ended(false), Some(logs_with(b"hello"))).await;
        let written = fs.0.lock().unwrap().clone();
        let (_, bytes) = written
            .iter()
            .find(|(p, _)| p == &crate::run_log::log_path(std::path::Path::new("/cache"), "aa07"))
            .expect("a stopped run's logs must outlive the boot that wrote them");
        let restored = crate::run_log::hydrate(bytes, crate::run_log::DEFAULT_CAPACITY_BYTES);
        assert_eq!(restored.read_from(0).chunks[0].bytes, b"hello".to_vec());
    }

    #[derive(Default)]
    struct UnwritableFs(std::sync::Mutex<Vec<std::path::PathBuf>>);

    impl crate::image_store::Fs for UnwritableFs {
        async fn read_dir(&self, _: &std::path::Path) -> std::io::Result<Vec<std::path::PathBuf>> {
            Err(std::io::Error::from(std::io::ErrorKind::NotFound))
        }
        async fn read(&self, _: &std::path::Path) -> std::io::Result<Vec<u8>> {
            Err(std::io::Error::from(std::io::ErrorKind::NotFound))
        }
        async fn write(&self, p: &std::path::Path, _: &[u8]) -> std::io::Result<()> {
            self.0.lock().unwrap().push(p.to_path_buf());
            Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied))
        }
        async fn remove_file(&self, _: &std::path::Path) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn the_log_write_waits_for_frames_the_tee_has_not_recorded_yet() {
        let fs = RecordingFs::default();
        let logs = std::sync::Arc::new(crate::run_log::RunLogBuffer::default());
        logs.append(crate::run_log::StreamKind::Stdout, b"early");
        let tee = logs.clone();
        tokio::spawn(async move {
            tokio::task::yield_now().await;
            tee.append(crate::run_log::StreamKind::Stdout, b"last line before exit");
            tee.close(0);
        });
        write_the_log_down(
            &fs,
            std::path::Path::new("/cache"),
            "aa07",
            &logs,
            std::time::Duration::from_secs(5),
        )
        .await;
        let written = fs.0.lock().unwrap().clone();
        let (_, bytes) = written
            .iter()
            .find(|(p, _)| p.ends_with("logs.wire"))
            .expect("the log is written");
        assert_eq!(
            crate::run_log::hydrate(bytes, crate::run_log::DEFAULT_CAPACITY_BYTES)
                .read_from(0)
                .chunks
                .iter()
                .map(|c| c.bytes.clone())
                .collect::<Vec<_>>(),
            vec![b"early".to_vec(), b"last line before exit".to_vec()],
            "the write must wait for the tee, or a run's last output is lost at exit"
        );
    }

    #[tokio::test]
    async fn a_log_still_open_when_the_run_ends_is_written_with_what_it_has() {
        let fs = RecordingFs::default();
        let logs = std::sync::Arc::new(crate::run_log::RunLogBuffer::default());
        logs.append(crate::run_log::StreamKind::Stdout, b"partial");
        write_the_log_down(
            &fs,
            std::path::Path::new("/cache"),
            "aa07",
            &logs,
            std::time::Duration::from_millis(20),
        )
        .await;
        let written = fs.0.lock().unwrap().clone();
        let (_, bytes) = written
            .iter()
            .find(|(p, _)| p.ends_with("logs.wire"))
            .expect("a stalled tee must cost the tail, never the whole log");
        let restored = crate::run_log::hydrate(bytes, crate::run_log::DEFAULT_CAPACITY_BYTES);
        assert_eq!(restored.read_from(0).chunks[0].bytes, b"partial".to_vec());
    }

    #[tokio::test]
    async fn a_log_that_cannot_be_written_does_not_stop_the_run_from_ending() {
        let fs = UnwritableFs::default();
        let removed = conclude(&fs, "aa07", ended(false), Some(logs_with(b"hello"))).await;
        assert!(removed.is_empty(), "nothing is removed without --rm");
        assert!(
            fs.0.lock()
                .unwrap()
                .iter()
                .any(|p| p.ends_with("logs.wire")),
            "the write must be attempted, so this cannot pass by skipping the log entirely"
        );
    }

    #[tokio::test]
    async fn the_conclude_fakes_honour_their_whole_port_surface() {
        use crate::image_store::Fs as _;
        let fs = RecordingFs::default();
        assert!(fs.read_dir(std::path::Path::new("/")).await.is_err());
        assert!(fs.read(std::path::Path::new("/x")).await.is_err());
        assert!(fs.remove_file(std::path::Path::new("/x")).await.is_ok());
        let unwritable = UnwritableFs::default();
        assert!(
            unwritable
                .read_dir(std::path::Path::new("/"))
                .await
                .is_err()
        );
        assert!(unwritable.read(std::path::Path::new("/x")).await.is_err());
        assert!(
            unwritable
                .remove_file(std::path::Path::new("/x"))
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    #[serial_test::serial(env, global_runs)]
    async fn a_run_that_takes_its_state_with_it_writes_no_log() {
        let id = crate::run_registry::allocate_run_id();
        let (handle, _rx) = crate::run_registry::test_handle();
        crate::run_registry::register(id.clone(), handle);
        let fs = RecordingFs::default();
        let removed = conclude(&fs, &id, ended(true), Some(logs_with(b"hello"))).await;
        assert_eq!(removed, vec![id], "a --rm run is swept as it ends");
        assert!(
            !fs.0
                .lock()
                .unwrap()
                .iter()
                .any(|(p, _)| p.ends_with("logs.wire")),
            "a --rm run's dir is about to go, so writing its log is only work"
        );
    }

    #[tokio::test]
    async fn a_recordless_exit_warns_and_still_concludes() {
        conclude_run(
            &EmptyFs,
            &NoopRemover,
            std::path::Path::new("/cache"),
            "ghost-run-id-00000000000000000000",
            RunEnd {
                code: 0,
                auto_remove: false,
                finished_at: "t".into(),
            },
            None,
            |_| panic!("nothing to remove without --rm"),
        )
        .await;
    }

    #[test]
    fn a_fresh_launch_accepts_any_descriptor() {
        assert!(verify_pinned_descriptor(&LaunchMode::Fresh, "sha256:anything").is_ok());
    }

    #[test]
    fn a_restart_keeps_the_creation_time_its_record_already_carries() {
        let mode = LaunchMode::Restart {
            pinned_descriptor_sha256: "sha256:pinned".into(),
            created_at: "2026-08-01T00:00:00Z".into(),
        };
        assert_eq!(
            recorded_created_at(&mode, || "2026-09-02T12:00:00Z".to_string()),
            "2026-08-01T00:00:00Z",
            "a restart re-writes the record, so it must carry the original creation time forward"
        );
    }

    #[test]
    fn a_fresh_run_is_created_now() {
        assert_eq!(
            recorded_created_at(&LaunchMode::Fresh, || "2026-09-02T12:00:00Z".to_string()),
            "2026-09-02T12:00:00Z"
        );
    }

    #[test]
    fn a_restart_accepts_only_the_descriptor_its_upper_was_written_on() {
        let mode = LaunchMode::Restart {
            pinned_descriptor_sha256: "sha256:pinned".into(),
            created_at: "2026-08-01T00:00:00Z".into(),
        };
        assert!(verify_pinned_descriptor(&mode, "sha256:pinned").is_ok());
        let err = verify_pinned_descriptor(&mode, "sha256:drifted").unwrap_err();
        assert!(err.to_string().contains("sha256:pinned"), "{err}");
        assert!(err.to_string().contains("lns rm"), "{err}");
    }
}
