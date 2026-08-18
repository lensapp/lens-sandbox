use anyhow::{Context, Result};
use lns_ipc::{Response, WireFrame};
use lns_policy::grants::WorkloadIdentity;
use tokio::sync::mpsc::Sender;

mod orchestrator;
mod scratch;
mod shutdown;
pub use orchestrator::{PreparedRun, handle, prepare};
pub use scratch::{RealRemoveDir, RemoveDir, reclaim_run_dir};

/// Everything a run's end owes the world: its record gets the exit stamped, and a --rm run takes its state with it.
pub async fn conclude_run<F: crate::image_store::Fs, R: RemoveDir>(
    fs: &F,
    remover: &R,
    cache_root: &std::path::Path,
    run_id: &str,
    code: i32,
    auto_remove: bool,
    finished_at: String,
) {
    if let Err(e) =
        crate::run_record::mark_exited_with(fs, cache_root, run_id, code, finished_at).await
    {
        crate::log::warn!("run record not updated at exit: {e:#}");
    }
    if auto_remove {
        crate::run_registry::set_exit_code(run_id, code);
        if crate::run_registry::remove_if_exited(run_id)
            == crate::run_registry::RemoveOutcome::Removed
        {
            reclaim_run_dir(remover, cache_root, run_id);
        }
    }
}

/// Whether a boot creates a run or revives one: a restart boots over a preserved writable layer, so it must not arm the scratch guard and must find the exact lower stack the layer was written on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchMode {
    Fresh,
    Restart { pinned_descriptor_sha256: String },
}

pub fn verify_pinned_descriptor(mode: &LaunchMode, built_sha256: &str) -> Result<()> {
    match mode {
        LaunchMode::Fresh => Ok(()),
        LaunchMode::Restart {
            pinned_descriptor_sha256,
        } if pinned_descriptor_sha256 == built_sha256 => Ok(()),
        LaunchMode::Restart {
            pinned_descriptor_sha256,
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
            let _ = frame_tx
                .send(WireFrame::Json(Response::RunLog {
                    level: lns_ipc::LogLevel::Error,
                    verb: None,
                    message: format!("{e:#}"),
                }))
                .await;
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

/// The run-scoped environment sources composed around the image's own env: the user's `-e` vars, the managed credential names they may not clobber, the resolved workdir, and the declared tools' bin dirs.
pub(super) struct EnvInputs<'a> {
    pub user_env: &'a [String],
    pub extra_managed: &'a [String],
    pub workdir: Option<&'a str>,
    pub tools: &'a crate::workload_env::ToolRuntime,
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
        inputs.extra_managed,
        inputs.tools,
    )
}

/// The image ref, command, and env a sandbox run boots with, once its definition is resolved.
pub(super) struct SandboxLaunch {
    pub image: String,
    pub cmd: Vec<String>,
    pub env: Vec<String>,
}

/// Size a sandbox run's VM: the sandbox's resources are authoritative unless the user set `--cpus`/`-m` explicitly (even to a value equal to the built-in default), in which case the explicit request wins.
pub(super) fn sandbox_vm_size(
    resources: Option<&lns_artifact::spec::Resources>,
    requested_cpus: u8,
    cpus_explicit: bool,
    requested_mem_mib: usize,
    mem_explicit: bool,
    host: Option<lns_artifact::resources::HostCapacity>,
) -> (u8, usize) {
    use crate::artifact::resources::resolve_size;
    use lns_artifact::resources::{DEFAULT_VM_SIZE, ResourceOverrides};
    let overrides = ResourceOverrides {
        cpus: cpus_explicit.then_some(requested_cpus),
        mem_mib: mem_explicit.then_some(requested_mem_mib),
    };
    let size = resolve_size(resources, &overrides, DEFAULT_VM_SIZE, host);
    (size.cpus, size.mem_mib)
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

/// A published reference reduced to its bare `registry/repository`, so `:tag` and `@digest` of one repo share a workload identity; an unparseable reference is keyed verbatim.
fn normalize_repo(reference: &str) -> String {
    match reference.parse::<oci_client::Reference>() {
        Ok(r) => format!("{}/{}", r.registry(), r.repository()),
        Err(_) => reference.to_string(),
    }
}

/// Canonicalizes a definition directory for identity purposes, falling back to the raw string when canonicalization can't resolve it (e.g. it no longer exists) — mirrors `lns_policy::grants::project_key`, so a project reached through a symlink or a differently-cased path keys identically either way.
fn canonical_dir_key(dir: &str) -> String {
    std::path::Path::new(dir)
        .canonicalize()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| dir.to_string())
}

/// The identity a run's connector grants key against: a local definition by its directory (every `-f` variant of one project, and every symlink alias of it, shares it), a published sandbox by `repo@digest` (a republished digest re-offers). A run resolving neither refuses — plain-image runs are retired, and a shared fallback bucket would let one run's grant arm another's without a card.
pub(super) fn workload_identity(
    args: &lns_ipc::RunImageArgs,
    resolved_ref: Option<&str>,
    digest: Option<&str>,
) -> Result<WorkloadIdentity> {
    // A grant keys on what the run is composed of, never on `mixins`, which is only what the boot still has to merge — a local run sends that empty because its document already carries them.
    crate::artifact::mixin::require_pinned_extras(&args.composed_mixins)
        .context("keying this run's connector grants")?;
    let composed = args.composed_mixins.clone();
    if let Some(dir) = args.definition_dir.as_deref() {
        Ok(WorkloadIdentity::definition(canonical_dir_key(dir)).composed_with(composed))
    } else if let (Some(reference), Some(digest)) = (resolved_ref, digest) {
        Ok(
            WorkloadIdentity::reference(normalize_repo(reference), digest.to_string())
                .composed_with(composed),
        )
    } else {
        anyhow::bail!(
            "this run resolved neither a definition directory nor a published digest, so it cannot hold connector grants; run a sandbox definition or a published sandbox reference, and if `lns` was upgraded while this service kept running, restart it (`lns service stop` then `lns service start`) so the two match"
        )
    }
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
        }
    }

    #[test]
    fn sandbox_vm_size_uses_the_sandbox_resources_when_no_flag_is_set() {
        let res = resources(4, 2048);
        assert_eq!(
            sandbox_vm_size(Some(&res), 1, false, 512, false, Some(TEST_HOST)),
            (4, 2048)
        );
    }

    /// A run request carrying only what the identity reads, so a test can vary the definition dir and the composition without spelling forty launch settings.
    fn run_args(definition_dir: Option<&str>, composed: &[String]) -> lns_ipc::RunImageArgs {
        serde_json::from_value(serde_json::json!({
            "image": "x:1",
            "cpus": 2,
            "mem": 2048,
            "policy_path": null,
            "cmd": [],
            "debug": false,
            "definition_dir": definition_dir,
            "composed_mixins": composed,
        }))
        .expect("the fixture names every field the wire requires")
    }

    #[test]
    fn workload_identity_keys_a_local_definition_by_its_directory() {
        let id = workload_identity(
            &run_args(Some("/Users/me/app"), &[]),
            Some("ghcr.io/team/base:1"),
            None,
        )
        .expect("a definition dir identifies the run");
        assert_eq!(
            id,
            WorkloadIdentity::definition("/Users/me/app"),
            "a local definition keys by its dir even though it carries a base image"
        );
    }

    #[test]
    fn workload_identity_carries_the_mixins_the_run_was_composed_with() {
        let composed = [format!("ghcr.io/acme/obs@sha256:{}", "d".repeat(64))];
        let id = workload_identity(&run_args(Some("/Users/me/app"), &composed), None, None)
            .expect("a definition dir identifies the run");
        assert_ne!(
            id,
            workload_identity(&run_args(Some("/Users/me/app"), &[]), None, None)
                .expect("a definition dir identifies the run"),
            "a run the user layered a mixin onto must not spend the grant the bare run earned"
        );
    }

    #[test]
    fn workload_identity_refuses_a_composition_named_by_a_moving_reference() {
        let err = workload_identity(
            &run_args(Some("/Users/me/app"), &["ghcr.io/acme/obs:2".to_string()]),
            None,
            None,
        )
        .expect_err(
            "a tag resolves to different bytes tomorrow, so a grant keyed on it means nothing",
        );
        assert!(
            format!("{err:#}").contains("keying this run's connector grants"),
            "got: {err:#}"
        );
    }

    #[test]
    fn workload_identity_canonicalizes_a_symlinked_definition_directory_to_the_same_key() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let real = dir.path().join("real");
        std::fs::create_dir(&real).expect("create real dir");
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).expect("create symlink");

        let via_link = workload_identity(&run_args(Some(link.to_str().unwrap()), &[]), None, None)
            .expect("a definition dir identifies the run");
        let via_real = workload_identity(&run_args(Some(real.to_str().unwrap()), &[]), None, None)
            .expect("a definition dir identifies the run");
        assert_eq!(
            via_link, via_real,
            "the same project reached through a symlink must key identically, or a grant made through one path silently misses through the other"
        );
    }

    #[test]
    fn workload_identity_falls_back_to_the_raw_dir_when_it_cannot_be_canonicalized() {
        assert_eq!(
            workload_identity(&run_args(Some("/no/such/project/dir"), &[]), None, None)
                .expect("a definition dir identifies the run"),
            WorkloadIdentity::definition("/no/such/project/dir")
        );
    }

    #[test]
    fn workload_identity_keys_a_published_reference_by_repo_and_digest() {
        let id = workload_identity(
            &run_args(None, &[]),
            Some("ghcr.io/acme/agent:1.4.0"),
            Some("sha256:abc"),
        )
        .expect("a resolved digest identifies the run");
        assert_eq!(
            id,
            WorkloadIdentity::reference("ghcr.io/acme/agent", "sha256:abc")
        );
    }

    #[test]
    fn workload_identity_refuses_a_run_without_a_definition_or_a_resolved_digest() {
        for (definition_dir, resolved_ref) in [(None, None), (None, Some("ghcr.io/acme/agent:1"))] {
            let err = workload_identity(&run_args(definition_dir, &[]), resolved_ref, None)
                .expect_err("an unidentifiable run must refuse, never share a grant bucket");
            assert!(
                format!("{err:#}").contains("cannot hold connector grants"),
                "got: {err:#}"
            );
        }
    }

    #[test]
    fn workload_identity_refusal_names_the_service_restart_a_stale_service_needs() {
        let err = workload_identity(&run_args(None, &[]), None, None)
            .expect_err("an unidentifiable run refuses");
        assert!(
            format!("{err:#}").contains("lns service stop"),
            "a matching CLI always resolves an identity, so the developer who sees this is most likely running an upgraded `lns` against a service left running from before; got: {err:#}"
        );
    }

    #[test]
    fn normalize_repo_strips_a_tag_and_a_digest_to_the_bare_repo() {
        assert_eq!(
            normalize_repo("ghcr.io/acme/agent:1.4.0"),
            "ghcr.io/acme/agent"
        );
        assert_eq!(
            normalize_repo(&format!("ghcr.io/acme/agent@sha256:{}", "a".repeat(64))),
            "ghcr.io/acme/agent"
        );
    }

    #[test]
    fn normalize_repo_keeps_an_unparseable_reference_verbatim() {
        assert_eq!(normalize_repo("not a valid ref"), "not a valid ref");
    }

    #[test]
    fn sandbox_vm_size_lets_an_explicit_request_override_the_sandbox() {
        let res = resources(4, 2048);
        assert_eq!(
            sandbox_vm_size(Some(&res), 2, true, 1024, true, Some(TEST_HOST)),
            (2, 1024)
        );
    }

    #[test]
    fn sandbox_vm_size_honors_an_explicit_request_that_equals_the_builtin_default() {
        let res = resources(4, 2048);
        assert_eq!(
            sandbox_vm_size(Some(&res), 1, true, 512, true, Some(TEST_HOST)),
            (1, 512),
            "a user who explicitly asks for the default size must be able to constrain a greedy sandbox"
        );
    }

    #[test]
    fn sandbox_vm_size_falls_back_to_the_request_when_the_sandbox_is_silent() {
        assert_eq!(
            sandbox_vm_size(None, 1, false, 512, false, Some(TEST_HOST)),
            (1, 512)
        );
        assert_eq!(
            sandbox_vm_size(None, 8, true, 4096, true, Some(TEST_HOST)),
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
                extra_managed: &[],
                workdir: None,
                tools: &Default::default(),
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
                extra_managed: &[],
                workdir: None,
                tools: &Default::default(),
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
                extra_managed: &[],
                workdir: None,
                tools: &Default::default(),
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
                extra_managed: &[],
                workdir: None,
                tools: &Default::default(),
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
                extra_managed: &[],
                workdir: None,
                tools: &Default::default(),
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

    #[test]
    fn a_fresh_launch_accepts_any_descriptor() {
        assert!(verify_pinned_descriptor(&LaunchMode::Fresh, "sha256:anything").is_ok());
    }

    #[test]
    fn a_restart_accepts_only_the_descriptor_its_upper_was_written_on() {
        let mode = LaunchMode::Restart {
            pinned_descriptor_sha256: "sha256:pinned".into(),
        };
        assert!(verify_pinned_descriptor(&mode, "sha256:pinned").is_ok());
        let err = verify_pinned_descriptor(&mode, "sha256:drifted").unwrap_err();
        assert!(err.to_string().contains("sha256:pinned"), "{err}");
        assert!(err.to_string().contains("lns rm"), "{err}");
    }
}
