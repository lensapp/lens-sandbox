use anyhow::Result;
use lns_ipc::{Response, WireFrame};
use lns_policy::grants::WorkloadIdentity;
use tokio::sync::mpsc::Sender;

mod orchestrator;
mod scratch;
mod shutdown;
pub use orchestrator::handle;

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
) -> (u8, usize) {
    use crate::artifact::resources::resolve_size;
    use lns_artifact::resources::{DEFAULT_VM_SIZE, ResourceOverrides};
    let overrides = ResourceOverrides {
        cpus: cpus_explicit.then_some(requested_cpus),
        mem_mib: mem_explicit.then_some(requested_mem_mib),
    };
    let size = resolve_size(resources, &overrides, DEFAULT_VM_SIZE);
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
    definition_dir: Option<&str>,
    resolved_ref: Option<&str>,
    digest: Option<&str>,
) -> Result<WorkloadIdentity> {
    if let Some(dir) = definition_dir {
        Ok(WorkloadIdentity::Definition {
            dir: canonical_dir_key(dir),
        })
    } else if let (Some(reference), Some(digest)) = (resolved_ref, digest) {
        Ok(WorkloadIdentity::Reference {
            repo: normalize_repo(reference),
            digest: digest.to_string(),
        })
    } else {
        anyhow::bail!(
            "this run resolved neither a definition directory nor a published digest, so it cannot hold connector grants; run a sandbox definition or a published sandbox reference, and if `lns` was upgraded while this service kept running, restart it (`lns service stop` then `lns service start`) so the two match"
        )
    }
}

#[cfg(test)]
mod tests {
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
        assert_eq!(sandbox_vm_size(Some(&res), 1, false, 512, false), (4, 2048));
    }

    #[test]
    fn workload_identity_keys_a_local_definition_by_its_directory() {
        let id = workload_identity(Some("/Users/me/app"), Some("ghcr.io/team/base:1"), None)
            .expect("a definition dir identifies the run");
        assert_eq!(
            id,
            WorkloadIdentity::Definition {
                dir: "/Users/me/app".into()
            },
            "a local definition keys by its dir even though it carries a base image"
        );
    }

    #[test]
    fn workload_identity_canonicalizes_a_symlinked_definition_directory_to_the_same_key() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let real = dir.path().join("real");
        std::fs::create_dir(&real).expect("create real dir");
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).expect("create symlink");

        let via_link = workload_identity(Some(link.to_str().unwrap()), None, None)
            .expect("a definition dir identifies the run");
        let via_real = workload_identity(Some(real.to_str().unwrap()), None, None)
            .expect("a definition dir identifies the run");
        assert_eq!(
            via_link, via_real,
            "the same project reached through a symlink must key identically, or a grant made through one path silently misses through the other"
        );
    }

    #[test]
    fn workload_identity_falls_back_to_the_raw_dir_when_it_cannot_be_canonicalized() {
        assert_eq!(
            workload_identity(Some("/no/such/project/dir"), None, None)
                .expect("a definition dir identifies the run"),
            WorkloadIdentity::Definition {
                dir: "/no/such/project/dir".into()
            }
        );
    }

    #[test]
    fn workload_identity_keys_a_published_reference_by_repo_and_digest() {
        let id = workload_identity(None, Some("ghcr.io/acme/agent:1.4.0"), Some("sha256:abc"))
            .expect("a resolved digest identifies the run");
        assert_eq!(
            id,
            WorkloadIdentity::Reference {
                repo: "ghcr.io/acme/agent".into(),
                digest: "sha256:abc".into()
            }
        );
    }

    #[test]
    fn workload_identity_refuses_a_run_without_a_definition_or_a_resolved_digest() {
        for (definition_dir, resolved_ref) in [(None, None), (None, Some("ghcr.io/acme/agent:1"))] {
            let err = workload_identity(definition_dir, resolved_ref, None)
                .expect_err("an unidentifiable run must refuse, never share a grant bucket");
            assert!(
                format!("{err:#}").contains("cannot hold connector grants"),
                "got: {err:#}"
            );
        }
    }

    #[test]
    fn workload_identity_refusal_names_the_service_restart_a_stale_service_needs() {
        let err = workload_identity(None, None, None).expect_err("an unidentifiable run refuses");
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
        assert_eq!(sandbox_vm_size(Some(&res), 2, true, 1024, true), (2, 1024));
    }

    #[test]
    fn sandbox_vm_size_honors_an_explicit_request_that_equals_the_builtin_default() {
        let res = resources(4, 2048);
        assert_eq!(
            sandbox_vm_size(Some(&res), 1, true, 512, true),
            (1, 512),
            "a user who explicitly asks for the default size must be able to constrain a greedy sandbox"
        );
    }

    #[test]
    fn sandbox_vm_size_falls_back_to_the_request_when_the_sandbox_is_silent() {
        assert_eq!(sandbox_vm_size(None, 1, false, 512, false), (1, 512));
        assert_eq!(sandbox_vm_size(None, 8, true, 4096, true), (8, 4096));
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
