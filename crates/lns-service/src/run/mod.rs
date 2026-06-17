use anyhow::Result;
use lns_ipc::{Response, WireFrame};
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
    override_cmd: &[String],
    supervised: bool,
) -> Vec<String> {
    if supervised {
        return vec!["/.lens/bin/lns-supervisor".to_string()];
    }
    let joined = match image_config {
        Some(cfg) => crate::workload_argv::from_image_config(cfg, override_cmd),
        None => crate::workload_argv::shell_quote_argv(override_cmd),
    };
    vec!["/bin/sh".to_string(), "-c".to_string(), joined]
}

pub(super) fn exec_env_strings(
    image_config: Option<&oci_client::config::ConfigFile>,
    override_cmd: &[String],
    user_env: &[String],
    supervised: bool,
    extra_managed: &[String],
    workdir: Option<&str>,
) -> crate::workload_env::WorkloadEnv {
    let agent_command = supervised.then(|| match image_config {
        Some(cfg) => crate::workload_argv::from_image_config(cfg, override_cmd),
        None => crate::workload_argv::shell_quote_argv(override_cmd),
    });
    let image_env = image_config
        .and_then(|c| c.config.as_ref())
        .and_then(|c| c.env.as_deref());
    crate::workload_env::run_workload_env(
        image_env,
        user_env,
        agent_command.as_deref(),
        workdir,
        extra_managed,
    )
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
mod tests {
    use super::*;
    use lns_ipc::LogLevel;
    use tokio::sync::mpsc;

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
        let argv = build_workload_argv(None, &["echo".into(), "hi".into()], true);
        assert_eq!(argv, vec!["/.lens/bin/lns-supervisor".to_string()]);
    }

    #[test]
    fn build_workload_argv_unsupervised_imageless_wraps_in_sh_dash_c() {
        let argv = build_workload_argv(None, &["echo".into(), "hello world".into()], false);
        assert_eq!(argv.len(), 3);
        assert_eq!(argv[0], "/bin/sh");
        assert_eq!(argv[1], "-c");
        assert_eq!(argv[2], "echo 'hello world'");
    }

    #[test]
    fn exec_env_strings_supervised_derives_agent_command_from_the_override_cmd() {
        let env = exec_env_strings(None, &["echo".into(), "hi".into()], &[], true, &[], None);
        assert!(
            env.env.contains(&"AGENT_COMMAND=echo hi".to_string()),
            "expected AGENT_COMMAND in supervised env, got: {env:?}"
        );
    }

    #[test]
    fn exec_env_strings_supervised_preserves_whitespace_and_quotes() {
        let env = exec_env_strings(
            None,
            &[
                "/bin/sh".into(),
                "-c".into(),
                "echo \"hi $USER\"; uname -a".into(),
            ],
            &[],
            true,
            &[],
            None,
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
            &["echo".into(), "hi".into()],
            &["FOO=bar".into()],
            false,
            &[],
            None,
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
        let argv = build_workload_argv(Some(&cfg), &[], false);
        assert_eq!(argv[2], "/entry default-arg");
        let argv = build_workload_argv(Some(&cfg), &["override".into()], false);
        assert_eq!(argv[2], "override");
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
        let env = exec_env_strings(Some(&cfg), &[], &[], true, &[], None);
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
        let env = exec_env_strings(Some(&cfg), &[], &["PORT=4000".into()], true, &[], None);
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
}
