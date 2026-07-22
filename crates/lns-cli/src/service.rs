use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use lns_ipc::{SignalKind, StatusInfo};

pub mod client;
mod handshake;
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
    drive_pre_phase, exec_command, exec_image, launch_run, pre_phase_step, render_started_run,
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
    #[command(about = "Restart the Lens Sandbox background service.")]
    Restart,
    #[command(about = "Show status of the Lens Sandbox background service.")]
    Status,
    #[command(
        about = "Register a per-user login agent and start the service now and on every login."
    )]
    Enable,
    #[command(about = "Stop the service and unregister the per-user login agent.")]
    Disable,
}

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

/// A decodable Status proves liveness by itself; ping only disambiguates a
/// `None` status so a service that stopped between probes reads as stopped,
/// never as protocol-incompatible.
pub(super) async fn gate_running_service(client: &impl ServiceClient) -> Result<(), String> {
    let status = client.status().await;
    if status.is_none() {
        require_running_check(client.ping().await).map_err(str::to_string)?;
    }
    handshake::enforce_for_command(handshake::classify(
        handshake::CLI_PROTOCOL,
        handshake::CLI_BUILD,
        status.as_ref(),
    ))
}

pub(super) async fn cmd_start(client: &impl ServiceClient) -> Result<()> {
    if client.ping().await {
        let status = client.status().await;
        let compat = handshake::classify(
            handshake::CLI_PROTOCOL,
            handshake::CLI_BUILD,
            status.as_ref(),
        );
        match handshake::already_running_warning(&compat) {
            None => println!("Lens Sandbox is already running."),
            Some(warning) => crate::log::warn!("{warning}"),
        }
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

pub(super) async fn cmd_restart(client: &impl ServiceClient) -> Result<()> {
    restart_with_outcome(client, login_agent::restart().await).await
}

async fn restart_with_outcome(
    client: &impl ServiceClient,
    outcome: login_agent::RestartOutcome,
) -> Result<()> {
    match outcome {
        login_agent::RestartOutcome::Relaunched => {
            if client.wait_for_ready(START_TIMEOUT).await {
                report_relaunched(client, login_agent::agent_path().as_deref()).await
            } else {
                anyhow::bail!(
                    "lns-service did not become ready within {}s after restart",
                    START_TIMEOUT.as_secs()
                );
            }
        }
        login_agent::RestartOutcome::NotRegistered => restart_via_socket(client).await,
        login_agent::RestartOutcome::Degraded(reason) => {
            crate::log::warn!(
                "could not restart via the login agent ({reason}); restarting the service directly"
            );
            restart_via_socket(client).await
        }
    }
}

/// A relaunched login agent re-runs whatever binary its agent file froze at
/// enable time, so a Pong alone can't prove the mismatch was reconciled.
async fn report_relaunched(
    client: &impl ServiceClient,
    agent_path: Option<&std::path::Path>,
) -> Result<()> {
    let status = client.status().await;
    let compat = handshake::classify(
        handshake::CLI_PROTOCOL,
        handshake::CLI_BUILD,
        status.as_ref(),
    );
    match handshake::relaunched_warning(&compat, agent_path) {
        None => println!("Lens Sandbox restarted."),
        Some(warning) => crate::log::warn!("{warning}"),
    }
    Ok(())
}

async fn restart_via_socket(client: &impl ServiceClient) -> Result<()> {
    if client.shutdown().await.is_some() && !client.wait_for_stopped(STOP_TIMEOUT).await {
        anyhow::bail!(
            "Lens Sandbox acknowledged shutdown but did not exit within {}s",
            STOP_TIMEOUT.as_secs()
        );
    }
    cmd_start(client).await
}

fn absent_status_message(alive: bool) -> &'static str {
    if alive {
        "Lens Sandbox is running but reports an unreadable status — likely an incompatible or older lns-service. Run `lns service restart` to reconcile."
    } else {
        "Lens Sandbox is not running."
    }
}

pub(super) async fn cmd_status(client: &impl ServiceClient) -> Result<()> {
    let Some(StatusInfo {
        pid,
        uptime_secs,
        version,
        protocol,
        build,
    }) = client.status().await
    else {
        println!("{}", absent_status_message(client.ping().await));
        return Ok(());
    };

    println!("Lens Sandbox is running.");
    println!("  PID:      {pid}");
    println!("  Uptime:   {uptime_secs}s");
    println!("  Version:  {version}");
    println!("  Build:    {build}");
    println!("  Protocol: {protocol}");
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
pub(super) fn friendly_started(rfc3339: &str) -> String {
    let trimmed = rfc3339.trim_end_matches('Z');
    trimmed.replacen('T', " ", 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use client::BoxFuture;

    #[test]
    fn build_sha_is_embedded() {
        assert!(
            !env!("LNS_BUILD_SHA").is_empty(),
            "build.rs must stamp a non-empty LNS_BUILD_SHA"
        );
    }

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

    fn matching_status() -> StatusInfo {
        StatusInfo {
            pid: 1,
            uptime_secs: 1,
            version: env!("CARGO_PKG_VERSION").into(),
            protocol: lns_ipc::IPC_PROTOCOL_VERSION,
            build: env!("LNS_BUILD_SHA").into(),
        }
    }

    #[tokio::test]
    async fn cmd_start_reports_already_running_when_ping_succeeds() {
        let client = FakeClient::default();
        client.ping_responses.lock().unwrap().push_back(true);
        *client.status_response.lock().unwrap() = Some(Some(matching_status()));

        cmd_start(&client).await.expect("cmd_start should succeed");

        assert_eq!(client.calls(), vec!["ping", "status"]);
    }

    #[test]
    fn cmd_start_warns_to_restart_when_the_running_build_differs() {
        let client = FakeClient::default();
        client.ping_responses.lock().unwrap().push_back(true);
        *client.status_response.lock().unwrap() = Some(Some(StatusInfo {
            pid: 1,
            uptime_secs: 1,
            version: "0.0.0".into(),
            protocol: lns_ipc::IPC_PROTOCOL_VERSION,
            build: "a-different-build".into(),
        }));

        let events = capture_warn(|| {
            futures_block_on(cmd_start(&client)).expect("a build drift must not fail start");
        });

        assert_eq!(client.calls(), vec!["ping", "status"]);
        assert!(
            events.iter().any(|e| e.contains("lns service restart")),
            "expected a restart hint: {events:?}"
        );
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
    async fn cmd_status_reports_not_running_when_status_is_none_and_ping_fails() {
        let client = FakeClient::default();
        *client.status_response.lock().unwrap() = Some(None);
        client.ping_responses.lock().unwrap().push_back(false);

        cmd_status(&client)
            .await
            .expect("cmd_status should succeed");

        assert_eq!(client.calls(), vec!["status", "ping"]);
    }

    #[tokio::test]
    async fn cmd_status_pings_to_disambiguate_an_alive_but_undecodable_service() {
        let client = FakeClient::default();
        *client.status_response.lock().unwrap() = Some(None);
        client.ping_responses.lock().unwrap().push_back(true);

        cmd_status(&client)
            .await
            .expect("cmd_status should succeed");

        assert_eq!(client.calls(), vec!["status", "ping"]);
    }

    #[tokio::test]
    async fn gate_reports_not_running_when_the_service_died_between_probes() {
        let client = FakeClient::default();
        *client.status_response.lock().unwrap() = Some(None);
        client.ping_responses.lock().unwrap().push_back(false);

        let err = gate_running_service(&client)
            .await
            .expect_err("a dead service must not pass the gate");
        assert!(err.contains("not running"), "{err}");
        assert!(
            !err.contains("incompatible"),
            "a stopped service must never read as a protocol mismatch: {err}"
        );
        assert_eq!(client.calls(), vec!["status", "ping"]);
    }

    #[tokio::test]
    async fn gate_treats_an_alive_but_unreadable_service_as_a_protocol_mismatch() {
        let client = FakeClient::default();
        *client.status_response.lock().unwrap() = Some(None);
        client.ping_responses.lock().unwrap().push_back(true);

        let err = gate_running_service(&client)
            .await
            .expect_err("an undecodable status from a live service must abort");
        assert!(err.contains("incompatible"), "{err}");
    }

    #[tokio::test]
    async fn gate_passes_a_matching_service_on_the_status_probe_alone() {
        let client = FakeClient::default();
        *client.status_response.lock().unwrap() = Some(Some(matching_status()));

        gate_running_service(&client)
            .await
            .expect("a matching pair passes");

        assert_eq!(
            client.calls(),
            vec!["status"],
            "a decodable Status proves liveness; no extra ping round trip"
        );
    }

    #[test]
    fn absent_status_message_distinguishes_an_incompatible_service_from_a_stopped_one() {
        let alive = absent_status_message(true);
        assert!(
            alive.contains("lns service restart"),
            "unexpected: {alive:?}"
        );
        assert!(!absent_status_message(false).contains("restart"));
    }

    #[tokio::test]
    async fn cmd_status_reports_details_when_status_returned() {
        let client = FakeClient::default();
        *client.status_response.lock().unwrap() = Some(Some(StatusInfo {
            pid: 4242,
            uptime_secs: 17,
            version: "test-version".into(),
            protocol: 1,
            build: "abc123def456".into(),
        }));

        cmd_status(&client)
            .await
            .expect("cmd_status should succeed");

        assert_eq!(client.calls(), vec!["status"]);
    }

    use crate::test_env::EnvScope;
    use lns_ipc::SignalKind as IpcSignal;

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
        crate::test_env::capture_events(emit)
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
        *client.status_response.lock().unwrap() = Some(Some(matching_status()));

        cmd_enable(&client).await.expect("enable must not fail");

        assert_eq!(client.calls(), vec!["ping", "status"]);
    }

    #[tokio::test]
    async fn restart_with_outcome_relaunched_verifies_the_running_build_after_ready() {
        let client = FakeClient::default();
        *client.wait_for_ready_response.lock().unwrap() = Some(true);
        *client.status_response.lock().unwrap() = Some(Some(matching_status()));

        restart_with_outcome(&client, login_agent::RestartOutcome::Relaunched)
            .await
            .expect("a relaunched managed service that becomes ready succeeds");

        assert_eq!(client.calls(), vec!["wait_for_ready", "status"]);
    }

    #[test]
    fn restart_with_outcome_relaunched_warns_when_the_agent_relaunched_a_stale_binary() {
        let client = FakeClient::default();
        *client.wait_for_ready_response.lock().unwrap() = Some(true);
        *client.status_response.lock().unwrap() = Some(Some(StatusInfo {
            pid: 1,
            uptime_secs: 1,
            version: "0.0.0".into(),
            protocol: lns_ipc::IPC_PROTOCOL_VERSION,
            build: "a-stale-build".into(),
        }));

        let events = capture_warn(|| {
            futures_block_on(restart_with_outcome(
                &client,
                login_agent::RestartOutcome::Relaunched,
            ))
            .expect("a hollow relaunch warns instead of failing");
        });

        assert_eq!(client.calls(), vec!["wait_for_ready", "status"]);
        assert!(
            events
                .iter()
                .any(|e| e.contains("a-stale-build") && e.contains("lns service enable")),
            "expected a stale-binary warning with the repoint remedy: {events:?}"
        );
    }

    #[tokio::test]
    async fn restart_with_outcome_relaunched_bails_when_never_ready() {
        let client = FakeClient::default();
        *client.wait_for_ready_response.lock().unwrap() = Some(false);

        let err = restart_with_outcome(&client, login_agent::RestartOutcome::Relaunched)
            .await
            .expect_err("a relaunch that never becomes ready must surface");
        assert!(err.to_string().contains("did not become ready"), "{err}");
    }

    #[tokio::test]
    async fn restart_with_outcome_not_registered_stops_then_starts_over_the_socket() {
        let client = FakeClient::default();
        *client.shutdown_response.lock().unwrap() = Some(Some(()));
        *client.wait_for_stopped_response.lock().unwrap() = Some(true);
        client.ping_responses.lock().unwrap().push_back(false);
        *client.start_response.lock().unwrap() = Some(Ok(true));

        restart_with_outcome(&client, login_agent::RestartOutcome::NotRegistered)
            .await
            .expect("an unmanaged restart stops then starts");

        assert_eq!(
            client.calls(),
            vec![
                "shutdown",
                "wait_for_stopped",
                "ping",
                "start_and_wait_for_ready"
            ]
        );
    }

    #[test]
    fn restart_with_outcome_degraded_warns_then_restarts_directly() {
        let client = FakeClient::default();
        *client.shutdown_response.lock().unwrap() = Some(None);
        client.ping_responses.lock().unwrap().push_back(false);
        *client.start_response.lock().unwrap() = Some(Ok(true));

        let events = capture_warn(|| {
            futures_block_on(restart_with_outcome(
                &client,
                login_agent::RestartOutcome::Degraded("no gui session".into()),
            ))
            .expect("a degraded login-agent restart still restarts directly");
        });

        assert_eq!(
            client.calls(),
            vec!["shutdown", "ping", "start_and_wait_for_ready"]
        );
        assert!(
            events.iter().any(|e| e.contains("no gui session")),
            "expected the degraded reason in the warning: {events:?}"
        );
    }

    #[tokio::test]
    async fn restart_via_socket_bails_when_stop_times_out() {
        let client = FakeClient::default();
        *client.shutdown_response.lock().unwrap() = Some(Some(()));
        *client.wait_for_stopped_response.lock().unwrap() = Some(false);

        let err = restart_via_socket(&client)
            .await
            .expect_err("a stop that never completes must surface");
        assert!(err.to_string().contains("did not exit within"), "{err}");
    }

    #[tokio::test]
    #[serial_test::serial(env)]
    async fn cmd_restart_falls_back_to_socket_when_home_unset() {
        let _g = crate::test_env::EnvScope::unset("HOME");
        let client = FakeClient::default();
        *client.shutdown_response.lock().unwrap() = Some(None);
        client.ping_responses.lock().unwrap().push_back(false);
        *client.start_response.lock().unwrap() = Some(Ok(true));

        cmd_restart(&client)
            .await
            .expect("with no login agent, restart reconciles over the socket");

        assert_eq!(
            client.calls(),
            vec!["shutdown", "ping", "start_and_wait_for_ready"]
        );
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
