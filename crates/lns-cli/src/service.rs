use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use lns_ipc::{RunStatus, RunSummary, SignalKind, StatusInfo};

pub mod client;
mod login_agent;
pub(crate) mod real;

pub use client::ServiceClient;

const START_TIMEOUT: Duration = Duration::from_secs(5);
const STOP_TIMEOUT: Duration = Duration::from_secs(5);

const NOT_RUNNING_MESSAGE: &str =
    "Lens Sandbox is not running. Run `lns service start` to start it.";

mod orchestrator;
pub use orchestrator::{
    DetachBehaviour, PrePhaseOutcome, PrePhaseStep, dispatch, drive_attached_session_with_writers,
    drive_pre_phase, exec_command, exec_image, kill, ls, pre_phase_step, render_started_run,
    render_status_line, require_running, run_command, run_image,
};

use crate::command::{CommandSpec, subcommand};

#[derive(clap::Args)]
pub struct ServiceArgs {
    #[command(subcommand)]
    pub command: ServiceCommand,
}

#[derive(clap::Subcommand)]
pub enum ServiceCommand {
    #[command(about = "Start the Lens Sandbox background service.")]
    Start,
    #[command(about = "Stop the Lens Sandbox background service.")]
    Stop,
    #[command(about = "Show status of the Lens Sandbox background service.")]
    Status,
    #[command(
        about = "Register a per-user login agent and start the service now and on every login."
    )]
    Enable,
    #[command(about = "Stop the service and unregister the per-user login agent.")]
    Disable,
}

pub fn augment_kill(app: clap::Command) -> clap::Command {
    app.subcommand(subcommand::<crate::cli::KillArgs>("kill").hide(true))
}

pub const KILL_SPEC: CommandSpec = CommandSpec {
    name: "kill",
    augment: augment_kill,
    run: orchestrator::kill_command,
    announces_update_check: true,
    owns_terminal: false,
};

pub fn augment_ls(app: clap::Command) -> clap::Command {
    app.subcommand(
        subcommand::<crate::cli::LsArgs>("ls")
            .visible_alias("list")
            .visible_alias("ps")
            .hide(true),
    )
}

pub const LS_SPEC: CommandSpec = CommandSpec {
    name: "ls",
    augment: augment_ls,
    run: orchestrator::ls_command,
    announces_update_check: true,
    owns_terminal: false,
};

pub fn augment(app: clap::Command) -> clap::Command {
    app.subcommand(
        subcommand::<ServiceArgs>("service").about("Manage the Lens Sandbox background service."),
    )
}

pub const SPEC: CommandSpec = CommandSpec {
    name: "service",
    augment,
    run: orchestrator::service_command,
    announces_update_check: true,
    owns_terminal: false,
};

fn require_running_check(alive: bool) -> Result<(), &'static str> {
    if alive {
        Ok(())
    } else {
        Err(NOT_RUNNING_MESSAGE)
    }
}

pub(super) async fn cmd_start(client: &impl ServiceClient) -> Result<()> {
    if client.ping().await {
        println!("Lens Sandbox is already running.");
        return Ok(());
    }

    if client.start_and_wait_for_ready(START_TIMEOUT).await? {
        println!("Lens Sandbox started.");
        Ok(())
    } else {
        anyhow::bail!(
            "lns-service failed to become ready within {}s",
            START_TIMEOUT.as_secs()
        );
    }
}

pub(super) async fn cmd_stop(client: &impl ServiceClient) -> Result<()> {
    if client.shutdown().await.is_none() {
        println!("Lens Sandbox is not running.");
        return Ok(());
    }

    if client.wait_for_stopped(STOP_TIMEOUT).await {
        println!("Lens Sandbox stopped.");
        Ok(())
    } else {
        anyhow::bail!(
            "Lens Sandbox acknowledged shutdown but did not exit within {}s",
            STOP_TIMEOUT.as_secs()
        );
    }
}

pub(super) async fn cmd_status(client: &impl ServiceClient) -> Result<()> {
    let Some(StatusInfo {
        pid,
        uptime_secs,
        version,
    }) = client.status().await
    else {
        println!("Lens Sandbox is not running.");
        return Ok(());
    };

    println!("Lens Sandbox is running.");
    println!("  PID:     {pid}");
    println!("  Uptime:  {uptime_secs}s");
    println!("  Version: {version}");
    Ok(())
}

pub(super) async fn cmd_enable(client: &impl ServiceClient) -> Result<()> {
    enable_with_outcome(client, login_agent::enable().await).await
}

pub(super) async fn cmd_disable(client: &impl ServiceClient) -> Result<()> {
    disable_with_outcome(client, login_agent::disable().await).await
}

async fn enable_with_outcome(
    client: &impl ServiceClient,
    outcome: login_agent::EnableOutcome,
) -> Result<()> {
    let login_agent_owns_start = matches!(
        outcome,
        login_agent::EnableOutcome::Registered | login_agent::EnableOutcome::AlreadyRegistered
    );
    report_enable_outcome(outcome);
    if login_agent_owns_start {
        if !client.wait_for_ready(START_TIMEOUT).await {
            crate::log::warn!(
                "the login agent is registered but the service is not responding yet; it will start on the next login"
            );
        }
    } else if let Err(e) = cmd_start(client).await {
        crate::log::warn!(
            "the service did not start for this session ({e}); it will start on the next login"
        );
    }
    Ok(())
}

async fn disable_with_outcome(
    client: &impl ServiceClient,
    outcome: login_agent::DisableOutcome,
) -> Result<()> {
    cmd_stop(client).await?;
    report_disable_outcome(outcome);
    Ok(())
}

fn report_enable_outcome(outcome: login_agent::EnableOutcome) {
    match outcome {
        login_agent::EnableOutcome::Registered => {
            println!("Login auto-start enabled. Lens Sandbox will start on every login.");
        }
        login_agent::EnableOutcome::AlreadyRegistered => {
            println!("Login auto-start is already enabled.");
        }
        login_agent::EnableOutcome::Degraded(reason) => {
            crate::log::warn!(
                "could not register login auto-start ({reason}); the service is started for this session only. Re-run `lns service enable` from a graphical login session."
            );
        }
    }
}

fn report_disable_outcome(outcome: login_agent::DisableOutcome) {
    match outcome {
        login_agent::DisableOutcome::Unregistered => {
            println!("Login auto-start disabled. Lens Sandbox will not start on the next login.");
        }
        login_agent::DisableOutcome::WasNotRegistered => {
            println!("Login auto-start was not enabled.");
        }
    }
}

pub(crate) fn socket_path() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("LNS_SOCKET_PATH") {
        return Ok(PathBuf::from(p));
    }
    Ok(lns_ipc::default_socket_path()?)
}

pub(crate) fn find_service_binary() -> PathBuf {
    if let Ok(p) = std::env::var("LNS_SERVICE_BIN") {
        return PathBuf::from(p);
    }
    if let Some(candidate) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|d| d.join("lns-service")))
        .filter(|p| p.exists())
    {
        return candidate;
    }
    PathBuf::from("lns-service")
}

pub(crate) fn parse_signal_name(name: &str) -> Result<SignalKind> {
    let upper = name.to_ascii_uppercase();
    let bare = upper.strip_prefix("SIG").unwrap_or(&upper);
    Ok(match bare {
        "TERM" => SignalKind::Term,
        "INT" => SignalKind::Int,
        "QUIT" => SignalKind::Quit,
        "HUP" => SignalKind::Hup,
        "WINCH" => SignalKind::Winch,
        "KILL" => SignalKind::Kill,
        other => {
            anyhow::bail!("unknown signal {other:?} (accepted: TERM, INT, QUIT, HUP, WINCH, KILL)")
        }
    })
}
pub(crate) fn render_run_list<W: std::io::Write>(
    out: &mut W,
    runs: &[RunSummary],
    args: &crate::cli::LsArgs,
) -> Result<()> {
    let mut rows: Vec<&RunSummary> = runs
        .iter()
        .filter(|run| crate::cli::ls_filter_matches(&args.filters, run))
        .collect();
    rows.sort_by(|a, b| b.started.cmp(&a.started));
    if matches!(args.format, Some(crate::cli::LsFormat::Json)) {
        render_ls_json(out, &rows)
    } else if args.quiet {
        render_ls_quiet(out, &rows)?;
        Ok(())
    } else {
        render_ls_table(out, &rows)?;
        Ok(())
    }
}

fn render_ls_quiet<W: std::io::Write>(out: &mut W, rows: &[&RunSummary]) -> std::io::Result<()> {
    for r in rows {
        writeln!(out, "{}", lns_ipc::short_run_id(&r.id))?;
    }
    Ok(())
}

fn render_ls_json<W: std::io::Write>(out: &mut W, rows: &[&RunSummary]) -> Result<()> {
    let items = rows
        .iter()
        .map(|&r| run_summary_json(r))
        .collect::<Result<Vec<_>>>()?;
    let rendered = serde_json::to_string_pretty(&serde_json::Value::Array(items))?;
    writeln!(out, "{rendered}")?;
    Ok(())
}

fn run_summary_json(run: &RunSummary) -> Result<serde_json::Value> {
    let ports = run
        .published_ports
        .iter()
        .map(|p| serde_json::Value::from(format_port(p)))
        .collect();
    let mut obj = serde_json::Map::new();
    obj.insert("id".into(), run.id.clone().into());
    obj.insert("name".into(), run.name.clone().into());
    obj.insert("image".into(), run.image.clone().into());
    obj.insert("command".into(), run.command.clone().into());
    obj.insert("status".into(), serde_json::to_value(run.status)?);
    obj.insert("started".into(), run.started.clone().into());
    obj.insert("ports".into(), serde_json::Value::Array(ports));
    Ok(serde_json::Value::Object(obj))
}

fn format_ports(ports: &[lns_ipc::PortPublish]) -> String {
    ports.iter().map(format_port).collect::<Vec<_>>().join(", ")
}

fn format_port(p: &lns_ipc::PortPublish) -> String {
    let bind = std::net::SocketAddr::new(p.host_ip, p.host_port);
    format!("{bind}->{}/{}", p.container_port, proto_str(p.protocol))
}

fn proto_str(protocol: lns_ipc::Protocol) -> &'static str {
    match protocol {
        lns_ipc::Protocol::Tcp => "tcp",
        lns_ipc::Protocol::Udp => "udp",
    }
}

fn render_ls_table<W: std::io::Write>(out: &mut W, rows: &[&RunSummary]) -> std::io::Result<()> {
    let status_str = |s: RunStatus| match s {
        RunStatus::Running => "running".to_string(),
        RunStatus::Exited { code } => format!("exited ({code})"),
    };

    let id_w = "ID".len().max(
        rows.iter()
            .map(|r| lns_ipc::short_run_id(&r.id).len())
            .max()
            .unwrap_or(0),
    );
    let name_w = "NAME"
        .len()
        .max(rows.iter().map(|r| r.name.len()).max().unwrap_or(0));
    let status_w = "STATUS".len().max(
        rows.iter()
            .map(|r| status_str(r.status).len())
            .max()
            .unwrap_or(0),
    );
    let image_w = "IMAGE"
        .len()
        .max(rows.iter().map(|r| r.image.len()).max().unwrap_or(0));
    let cmd_w = "COMMAND"
        .len()
        .max(rows.iter().map(|r| r.command.len()).max().unwrap_or(0));
    let ports_w = "PORTS".len().max(
        rows.iter()
            .map(|r| format_ports(&r.published_ports).len())
            .max()
            .unwrap_or(0),
    );

    writeln!(
        out,
        "{:<id_w$}  {:<name_w$}  {:<status_w$}  {:<image_w$}  {:<cmd_w$}  {:<ports_w$}  STARTED",
        "ID",
        "NAME",
        "STATUS",
        "IMAGE",
        "COMMAND",
        "PORTS",
        id_w = id_w,
        name_w = name_w,
        status_w = status_w,
        image_w = image_w,
        cmd_w = cmd_w,
        ports_w = ports_w,
    )?;
    for r in rows {
        writeln!(
            out,
            "{:<id_w$}  {:<name_w$}  {:<status_w$}  {:<image_w$}  {:<cmd_w$}  {:<ports_w$}  {}",
            lns_ipc::short_run_id(&r.id),
            r.name,
            status_str(r.status),
            r.image,
            r.command,
            format_ports(&r.published_ports),
            friendly_started(&r.started),
            id_w = id_w,
            name_w = name_w,
            status_w = status_w,
            image_w = image_w,
            cmd_w = cmd_w,
            ports_w = ports_w,
        )?;
    }
    Ok(())
}

pub(super) fn friendly_started(rfc3339: &str) -> String {
    let trimmed = rfc3339.trim_end_matches('Z');
    trimmed.replacen('T', " ", 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use client::BoxFuture;

    #[test]
    fn require_running_check_ok_when_alive() {
        assert!(require_running_check(true).is_ok());
    }

    #[test]
    fn require_running_check_errors_when_not_alive() {
        let err = require_running_check(false).expect_err("expected error");
        assert!(err.contains("Lens Sandbox is not running"));
        assert!(err.contains("`lns service start`"));
    }

    #[derive(Default)]
    struct FakeClient {
        ping_responses: std::sync::Mutex<std::collections::VecDeque<bool>>,
        status_response: std::sync::Mutex<Option<Option<StatusInfo>>>,
        shutdown_response: std::sync::Mutex<Option<Option<()>>>,
        start_response: std::sync::Mutex<Option<Result<bool, String>>>,
        wait_for_ready_response: std::sync::Mutex<Option<bool>>,
        wait_for_stopped_response: std::sync::Mutex<Option<bool>>,
        calls: std::sync::Mutex<Vec<String>>,
    }

    impl FakeClient {
        fn record(&self, method: &str) {
            self.calls.lock().unwrap().push(method.to_string());
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }

        fn next_ping(&self) -> bool {
            self.ping_responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("FakeClient: ping called with no queued response")
        }
    }

    impl ServiceClient for FakeClient {
        fn ping(&self) -> BoxFuture<'_, bool> {
            self.record("ping");
            let v = self.next_ping();
            Box::pin(async move { v })
        }

        fn status(&self) -> BoxFuture<'_, Option<StatusInfo>> {
            self.record("status");
            let v = self
                .status_response
                .lock()
                .unwrap()
                .take()
                .expect("FakeClient: status called with no queued response");
            Box::pin(async move { v })
        }

        fn shutdown(&self) -> BoxFuture<'_, Option<()>> {
            self.record("shutdown");
            let v = self
                .shutdown_response
                .lock()
                .unwrap()
                .take()
                .expect("FakeClient: shutdown called with no queued response");
            Box::pin(async move { v })
        }

        fn start_and_wait_for_ready(
            &self,
            _total_timeout: Duration,
        ) -> BoxFuture<'_, Result<bool>> {
            self.record("start_and_wait_for_ready");
            let v = self
                .start_response
                .lock()
                .unwrap()
                .take()
                .expect("FakeClient: start_and_wait_for_ready called with no queued response");
            Box::pin(async move { v.map_err(|msg| anyhow::anyhow!(msg)) })
        }

        fn wait_for_ready(&self, _total_timeout: Duration) -> BoxFuture<'_, bool> {
            self.record("wait_for_ready");
            let v = self
                .wait_for_ready_response
                .lock()
                .unwrap()
                .take()
                .expect("FakeClient: wait_for_ready called with no queued response");
            Box::pin(async move { v })
        }

        fn wait_for_stopped(&self, _total_timeout: Duration) -> BoxFuture<'_, bool> {
            self.record("wait_for_stopped");
            let v = self
                .wait_for_stopped_response
                .lock()
                .unwrap()
                .take()
                .expect("FakeClient: wait_for_stopped called with no queued response");
            Box::pin(async move { v })
        }

        fn cancel_run(&self, _run_id: String) -> BoxFuture<'_, ()> {
            self.record("cancel_run");
            Box::pin(async {})
        }
    }

    #[tokio::test]
    async fn cmd_start_reports_already_running_when_ping_succeeds() {
        let client = FakeClient::default();
        client.ping_responses.lock().unwrap().push_back(true);

        cmd_start(&client).await.expect("cmd_start should succeed");

        assert_eq!(client.calls(), vec!["ping"]);
    }

    #[tokio::test]
    async fn cmd_start_spawns_and_waits_when_not_running() {
        let client = FakeClient::default();
        client.ping_responses.lock().unwrap().push_back(false);
        *client.start_response.lock().unwrap() = Some(Ok(true));

        cmd_start(&client).await.expect("cmd_start should succeed");

        assert_eq!(client.calls(), vec!["ping", "start_and_wait_for_ready"]);
    }

    #[tokio::test]
    async fn cmd_start_bails_when_ready_times_out() {
        let client = FakeClient::default();
        client.ping_responses.lock().unwrap().push_back(false);
        *client.start_response.lock().unwrap() = Some(Ok(false));

        let err = cmd_start(&client)
            .await
            .expect_err("cmd_start should bail on ready timeout");
        assert!(
            err.to_string().contains("failed to become ready"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn cmd_start_propagates_spawn_failure() {
        let client = FakeClient::default();
        client.ping_responses.lock().unwrap().push_back(false);
        *client.start_response.lock().unwrap() = Some(Err("spawn refused".into()));

        let err = cmd_start(&client)
            .await
            .expect_err("cmd_start should propagate the spawn failure");
        assert!(
            err.to_string().contains("spawn refused"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn cmd_stop_reports_not_running_when_no_shutdown_response() {
        let client = FakeClient::default();
        *client.shutdown_response.lock().unwrap() = Some(None);

        cmd_stop(&client).await.expect("cmd_stop should succeed");

        assert_eq!(client.calls(), vec!["shutdown"]);
    }

    #[tokio::test]
    async fn cmd_stop_waits_after_shutdown_acknowledged() {
        let client = FakeClient::default();
        *client.shutdown_response.lock().unwrap() = Some(Some(()));
        *client.wait_for_stopped_response.lock().unwrap() = Some(true);

        cmd_stop(&client).await.expect("cmd_stop should succeed");

        assert_eq!(client.calls(), vec!["shutdown", "wait_for_stopped"]);
    }

    #[tokio::test]
    async fn cmd_stop_bails_when_stop_times_out() {
        let client = FakeClient::default();
        *client.shutdown_response.lock().unwrap() = Some(Some(()));
        *client.wait_for_stopped_response.lock().unwrap() = Some(false);

        let err = cmd_stop(&client)
            .await
            .expect_err("cmd_stop should bail on stop timeout");
        assert!(
            err.to_string().contains("did not exit within"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn cmd_status_reports_not_running_when_status_is_none() {
        let client = FakeClient::default();
        *client.status_response.lock().unwrap() = Some(None);

        cmd_status(&client)
            .await
            .expect("cmd_status should succeed");

        assert_eq!(client.calls(), vec!["status"]);
    }

    #[tokio::test]
    async fn cmd_status_reports_details_when_status_returned() {
        let client = FakeClient::default();
        *client.status_response.lock().unwrap() = Some(Some(StatusInfo {
            pid: 4242,
            uptime_secs: 17,
            version: "test-version".into(),
        }));

        cmd_status(&client)
            .await
            .expect("cmd_status should succeed");

        assert_eq!(client.calls(), vec!["status"]);
    }

    use crate::test_env::EnvScope;
    use lns_ipc::SignalKind as IpcSignal;
    use lns_ipc::{RunStatus, RunSummary};

    #[test]
    #[serial_test::serial(env)]
    fn socket_path_honours_env_override() {
        let _g = EnvScope::set("LNS_SOCKET_PATH", "/tmp/override.sock");
        assert_eq!(socket_path().unwrap(), PathBuf::from("/tmp/override.sock"));
    }

    #[test]
    #[serial_test::serial(env)]
    fn socket_path_falls_back_to_platform_default() {
        let _g = EnvScope::unset("LNS_SOCKET_PATH");
        let p = socket_path().expect("default socket_path");
        assert!(!p.as_os_str().is_empty());
    }

    #[test]
    #[serial_test::serial(env)]
    fn find_service_binary_honours_env_override() {
        let _g = EnvScope::set("LNS_SERVICE_BIN", "/tmp/fake-lns-service");
        assert_eq!(
            find_service_binary(),
            PathBuf::from("/tmp/fake-lns-service")
        );
    }

    #[test]
    #[serial_test::serial(env)]
    fn find_service_binary_falls_back_to_sibling_when_present() {
        let _g = EnvScope::unset("LNS_SERVICE_BIN");
        let exe = std::env::current_exe().expect("current_exe");
        let parent = exe.parent().expect("exe parent");
        let sibling = parent.join("lns-service");
        let pre_existed = sibling.exists();
        if !pre_existed {
            std::fs::write(
                &sibling,
                b"#!/bin/sh
exit 0
",
            )
            .expect("write sibling");
        }
        let p = find_service_binary();
        assert_eq!(p, sibling);
        if !pre_existed {
            let _ = std::fs::remove_file(&sibling);
        }
    }

    #[test]
    #[serial_test::serial(env)]
    fn find_service_binary_falls_back_to_relative_path_when_sibling_missing() {
        let _g = EnvScope::unset("LNS_SERVICE_BIN");
        let exe = std::env::current_exe().expect("current_exe");
        let parent = exe.parent().expect("exe parent");
        let sibling = parent.join("lns-service");
        let pre_existed = sibling.exists();
        if !pre_existed {
            std::fs::write(&sibling, b"placeholder").expect("seed sibling");
        }
        let stash = sibling.with_extension("stash-for-test");
        std::fs::rename(&sibling, &stash).expect("stash sibling");
        let p = find_service_binary();
        assert_eq!(p, PathBuf::from("lns-service"));
        std::fs::rename(&stash, &sibling).expect("restore sibling");
        if !pre_existed {
            let _ = std::fs::remove_file(&sibling);
        }
    }

    #[test]
    fn parse_signal_name_recognizes_canonical_names() {
        assert_eq!(parse_signal_name("INT").unwrap(), IpcSignal::Int);
        assert_eq!(parse_signal_name("TERM").unwrap(), IpcSignal::Term);
        assert_eq!(parse_signal_name("QUIT").unwrap(), IpcSignal::Quit);
        assert_eq!(parse_signal_name("HUP").unwrap(), IpcSignal::Hup);
        assert_eq!(parse_signal_name("WINCH").unwrap(), IpcSignal::Winch);
        assert_eq!(parse_signal_name("KILL").unwrap(), IpcSignal::Kill);
        assert_eq!(parse_signal_name("SIGTERM").unwrap(), IpcSignal::Term);
        assert_eq!(parse_signal_name("term").unwrap(), IpcSignal::Term);
    }

    #[test]
    fn parse_signal_name_rejects_unknown() {
        let err = parse_signal_name("bogus").unwrap_err();
        assert!(format!("{err:#}").to_lowercase().contains("signal"));
    }

    #[test]
    fn friendly_started_renders_short_form() {
        let s = friendly_started("2024-03-15T08:30:00Z");
        assert!(!s.is_empty(), "got empty: {s:?}");
    }

    #[test]
    fn friendly_started_passes_through_unparseable_input() {
        let s = friendly_started("not a date");
        assert!(!s.is_empty());
    }

    fn port(
        host_port: u16,
        container_port: u16,
        protocol: lns_ipc::Protocol,
    ) -> lns_ipc::PortPublish {
        lns_ipc::PortPublish {
            host_ip: std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
            host_port,
            container_port,
            protocol,
        }
    }

    fn summary(
        id: &str,
        name: &str,
        image: &str,
        status: RunStatus,
        ports: Vec<lns_ipc::PortPublish>,
    ) -> RunSummary {
        RunSummary {
            id: id.to_string(),
            name: name.into(),
            image: image.into(),
            command: "echo hi".into(),
            status,
            started: "2024-03-15T08:30:00Z".into(),
            published_ports: ports,
        }
    }

    fn ls_args() -> crate::cli::LsArgs {
        crate::cli::LsArgs {
            all: false,
            quiet: false,
            filters: Vec::new(),
            format: None,
        }
    }

    #[test]
    fn render_ls_table_emits_header_and_one_row_per_run() {
        let rows = [summary(
            "1a2b3c4d0000000000000000000000aa",
            "reviewer",
            "alpine:3.20",
            RunStatus::Running,
            Vec::new(),
        )];
        let refs: Vec<&RunSummary> = rows.iter().collect();
        let mut buf: Vec<u8> = Vec::new();
        render_ls_table(&mut buf, &refs).expect("render");
        let out = String::from_utf8(buf).unwrap();
        for tok in [
            "ID", "NAME", "STATUS", "STARTED", "IMAGE", "COMMAND", "PORTS",
        ] {
            assert!(
                out.contains(tok),
                "expected header token {tok:?} in {out:?}"
            );
        }
        assert!(out.contains("1a2b3c4d0000"), "short id missing in {out:?}");
        assert!(out.contains("reviewer"), "name missing in {out:?}");
        assert!(out.contains("alpine:3.20"), "image missing in {out:?}");
    }

    #[test]
    fn render_ls_table_handles_empty_run_list() {
        let mut buf: Vec<u8> = Vec::new();
        render_ls_table(&mut buf, &[]).expect("render empty");
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("ID"));
    }

    #[test]
    fn render_ls_table_renders_exited_status_with_code() {
        let rows = [summary(
            "5e6f7a8b0000000000000000000000bb",
            "auditor",
            "<imageless>",
            RunStatus::Exited { code: 0 },
            Vec::new(),
        )];
        let refs: Vec<&RunSummary> = rows.iter().collect();
        let mut buf: Vec<u8> = Vec::new();
        render_ls_table(&mut buf, &refs).expect("render");
        let out = String::from_utf8(buf).unwrap();
        assert!(
            out.to_lowercase().contains("exited") || out.contains('0'),
            "expected exited indicator in {out:?}"
        );
    }

    #[test]
    fn format_port_renders_the_docker_style_mapping_for_tcp_and_udp() {
        assert_eq!(
            format_port(&port(8080, 80, lns_ipc::Protocol::Tcp)),
            "127.0.0.1:8080->80/tcp"
        );
        assert_eq!(
            format_port(&port(5353, 53, lns_ipc::Protocol::Udp)),
            "127.0.0.1:5353->53/udp"
        );
    }

    #[test]
    fn format_port_brackets_an_ipv6_host_the_way_docker_does() {
        let p = lns_ipc::PortPublish {
            host_ip: std::net::IpAddr::V6(std::net::Ipv6Addr::LOCALHOST),
            host_port: 8080,
            container_port: 80,
            protocol: lns_ipc::Protocol::Tcp,
        };
        assert_eq!(format_port(&p), "[::1]:8080->80/tcp");
    }

    #[test]
    fn format_ports_is_empty_for_none_and_comma_joins_many() {
        assert_eq!(format_ports(&[]), "");
        assert_eq!(
            format_ports(&[
                port(8080, 80, lns_ipc::Protocol::Tcp),
                port(9090, 90, lns_ipc::Protocol::Tcp),
            ]),
            "127.0.0.1:8080->80/tcp, 127.0.0.1:9090->90/tcp"
        );
    }

    #[test]
    fn render_run_list_table_shows_the_published_ports_column() {
        let runs = [summary(
            "1a2b3c4d0000000000000000000000aa",
            "reviewer",
            "alpine:3.20",
            RunStatus::Running,
            vec![port(8080, 80, lns_ipc::Protocol::Tcp)],
        )];
        let mut buf: Vec<u8> = Vec::new();
        render_run_list(&mut buf, &runs, &ls_args()).expect("render");
        let out = String::from_utf8(buf).unwrap();
        assert!(
            out.contains("127.0.0.1:8080->80/tcp"),
            "ports missing: {out:?}"
        );
    }

    #[test]
    fn render_run_list_quiet_prints_short_ids_only() {
        let runs = [
            summary(
                "1a2b3c4d0000000000000000000000aa",
                "reviewer",
                "alpine:3.20",
                RunStatus::Running,
                Vec::new(),
            ),
            summary(
                "5e6f7a8b0000000000000000000000bb",
                "auditor",
                "busybox",
                RunStatus::Exited { code: 0 },
                Vec::new(),
            ),
        ];
        let args = crate::cli::LsArgs {
            quiet: true,
            ..ls_args()
        };
        let mut buf: Vec<u8> = Vec::new();
        render_run_list(&mut buf, &runs, &args).expect("render");
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("1a2b3c4d0000"), "first id missing: {out:?}");
        assert!(out.contains("5e6f7a8b0000"), "second id missing: {out:?}");
        assert!(
            !out.contains("IMAGE"),
            "quiet must omit the header: {out:?}"
        );
        assert!(
            !out.contains("alpine"),
            "quiet must omit the image: {out:?}"
        );
    }

    #[test]
    fn render_run_list_json_emits_an_array_with_status_and_ports() {
        let runs = [
            summary(
                "1a2b3c4d0000000000000000000000aa",
                "reviewer",
                "alpine:3.20",
                RunStatus::Running,
                vec![port(8080, 80, lns_ipc::Protocol::Tcp)],
            ),
            summary(
                "5e6f7a8b0000000000000000000000bb",
                "auditor",
                "busybox",
                RunStatus::Exited { code: 7 },
                Vec::new(),
            ),
        ];
        let args = crate::cli::LsArgs {
            format: Some(crate::cli::LsFormat::Json),
            ..ls_args()
        };
        let mut buf: Vec<u8> = Vec::new();
        render_run_list(&mut buf, &runs, &args).expect("render");
        let doc: serde_json::Value = serde_json::from_slice(&buf).expect("valid json");
        let arr = doc.as_array().expect("top-level array");
        assert_eq!(arr.len(), 2);
        let running = arr
            .iter()
            .find(|r| r["name"] == "reviewer")
            .expect("reviewer present");
        assert_eq!(running["status"]["state"], "running");
        assert_eq!(running["ports"][0], "127.0.0.1:8080->80/tcp");
        let exited = arr
            .iter()
            .find(|r| r["name"] == "auditor")
            .expect("auditor present");
        assert_eq!(exited["status"]["state"], "exited");
        assert_eq!(exited["status"]["code"], 7);
        assert!(exited["ports"].as_array().unwrap().is_empty());
    }

    #[test]
    fn render_run_list_filters_by_status_and_name() {
        let runs = [
            summary(
                "1a2b3c4d0000000000000000000000aa",
                "reviewer",
                "alpine:3.20",
                RunStatus::Running,
                Vec::new(),
            ),
            summary(
                "5e6f7a8b0000000000000000000000bb",
                "auditor",
                "busybox",
                RunStatus::Exited { code: 0 },
                Vec::new(),
            ),
        ];
        let running_only = crate::cli::LsArgs {
            filters: vec![crate::cli::LsFilter::Status(
                crate::cli::StatusFilter::Running,
            )],
            ..ls_args()
        };
        let mut buf: Vec<u8> = Vec::new();
        render_run_list(&mut buf, &runs, &running_only).expect("render");
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("reviewer"), "running run kept: {out:?}");
        assert!(!out.contains("auditor"), "exited run dropped: {out:?}");

        let named = crate::cli::LsArgs {
            filters: vec![crate::cli::LsFilter::Name("audit".into())],
            ..ls_args()
        };
        let mut buf: Vec<u8> = Vec::new();
        render_run_list(&mut buf, &runs, &named).expect("render");
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("auditor"), "name match kept: {out:?}");
        assert!(!out.contains("reviewer"), "non-match dropped: {out:?}");
    }

    #[tokio::test]
    async fn fake_service_client_cancel_run_records_invocation() {
        let client = std::sync::Arc::new(FakeClient::default());
        client.cancel_run("aa07".to_string()).await;
        assert_eq!(client.calls(), vec!["cancel_run"]);
    }

    use login_agent::{DisableOutcome, EnableOutcome};

    #[test]
    fn report_enable_outcome_covers_registered_and_already_arms() {
        report_enable_outcome(EnableOutcome::Registered);
        report_enable_outcome(EnableOutcome::AlreadyRegistered);
    }

    fn capture_warn(emit: impl FnOnce()) -> Vec<String> {
        use std::sync::{Arc, Mutex, Once};
        use tracing::field::{Field, Visit};
        use tracing::{Event, Subscriber};
        use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

        static OPEN_GATE: Once = Once::new();
        OPEN_GATE.call_once(|| {
            let gate = tracing_subscriber::fmt()
                .with_writer(std::io::sink)
                .with_max_level(tracing::Level::TRACE)
                .finish();
            tracing::subscriber::set_global_default(gate).ok();
        });

        type Sink = Arc<Mutex<Vec<String>>>;
        struct CapturingLayer(Sink);
        impl<S: Subscriber> Layer<S> for CapturingLayer {
            fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
                struct V<'a>(&'a mut String);
                impl Visit for V<'_> {
                    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
                        if field.name() == "message" {
                            *self.0 = format!("{value:?}");
                        }
                    }
                }
                let mut message = String::new();
                event.record(&mut V(&mut message));
                self.0.lock().unwrap().push(message);
            }
        }

        let sink: Sink = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::registry().with(CapturingLayer(sink.clone()));
        tracing::subscriber::with_default(subscriber, emit);
        let captured = sink.lock().unwrap();
        captured.clone()
    }

    #[test]
    fn report_enable_outcome_degraded_emits_actionable_warn() {
        let events = capture_warn(|| {
            report_enable_outcome(EnableOutcome::Degraded("no GUI session".to_string()));
        });
        assert_eq!(events.len(), 1);
        assert!(events[0].contains("no GUI session"), "{:?}", events[0]);
        assert!(events[0].contains("lns service enable"), "{:?}", events[0]);
    }

    #[test]
    fn report_disable_outcome_covers_both_arms() {
        report_disable_outcome(DisableOutcome::Unregistered);
        report_disable_outcome(DisableOutcome::WasNotRegistered);
    }

    #[tokio::test]
    async fn enable_with_outcome_waits_for_managed_start_without_spawning() {
        let client = FakeClient::default();
        *client.wait_for_ready_response.lock().unwrap() = Some(true);

        enable_with_outcome(&client, EnableOutcome::Registered)
            .await
            .expect("enable should succeed");

        assert_eq!(
            client.calls(),
            vec!["wait_for_ready"],
            "the login agent owns the start; we must never spawn a competing instance"
        );
    }

    #[test]
    fn enable_with_outcome_warns_when_managed_service_never_responds() {
        let client = FakeClient::default();
        *client.wait_for_ready_response.lock().unwrap() = Some(false);

        let events = capture_warn(|| {
            futures_block_on(enable_with_outcome(
                &client,
                EnableOutcome::AlreadyRegistered,
            ))
            .expect("enable must not fail the install when the service is slow to respond");
        });
        assert_eq!(client.calls(), vec!["wait_for_ready"]);
        assert!(
            events.iter().any(|e| e.contains("start on the next login")),
            "expected a not-responding-yet warn: {events:?}"
        );
    }

    #[test]
    fn enable_with_outcome_warns_but_succeeds_when_start_fails() {
        let client = FakeClient::default();
        client.ping_responses.lock().unwrap().push_back(false);
        *client.start_response.lock().unwrap() = Some(Ok(false));

        let events = capture_warn(|| {
            futures_block_on(enable_with_outcome(
                &client,
                EnableOutcome::Degraded("headless".to_string()),
            ))
            .expect("enable must not fail the install even when start fails");
        });
        assert!(
            events
                .iter()
                .any(|e| e.contains("did not start for this session")),
            "expected start-failure warn: {events:?}"
        );
        assert!(
            events.iter().any(|e| e.contains("headless")),
            "expected degraded warn: {events:?}"
        );
    }

    #[tokio::test]
    async fn disable_with_outcome_stops_then_reports_unregistered() {
        let client = FakeClient::default();
        *client.shutdown_response.lock().unwrap() = Some(Some(()));
        *client.wait_for_stopped_response.lock().unwrap() = Some(true);

        disable_with_outcome(&client, DisableOutcome::Unregistered)
            .await
            .expect("disable should succeed");

        assert_eq!(client.calls(), vec!["shutdown", "wait_for_stopped"]);
    }

    #[tokio::test]
    async fn disable_with_outcome_propagates_stop_timeout_error() {
        let client = FakeClient::default();
        *client.shutdown_response.lock().unwrap() = Some(Some(()));
        *client.wait_for_stopped_response.lock().unwrap() = Some(false);

        let err = disable_with_outcome(&client, DisableOutcome::WasNotRegistered)
            .await
            .expect_err("a stop timeout must surface");
        assert!(err.to_string().contains("did not exit within"), "{err}");
    }

    fn futures_block_on<F: std::future::Future>(fut: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(fut)
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn cmd_enable_degrades_to_session_start_when_home_unset() {
        let _g = crate::test_env::EnvScope::unset("HOME");
        let client = FakeClient::default();
        client.ping_responses.lock().unwrap().push_back(true);

        cmd_enable(&client).await.expect("enable must not fail");

        assert_eq!(client.calls(), vec!["ping"]);
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn cmd_disable_stops_when_home_unset() {
        let _g = crate::test_env::EnvScope::unset("HOME");
        let client = FakeClient::default();
        *client.shutdown_response.lock().unwrap() = Some(None);

        cmd_disable(&client).await.expect("disable must succeed");

        assert_eq!(client.calls(), vec!["shutdown"]);
    }
}
