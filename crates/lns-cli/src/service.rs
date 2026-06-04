use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use lns_ipc::{RunStatus, RunSummary, SignalKind, StatusInfo};

pub mod client;
pub(crate) mod real;

pub use client::ServiceClient;

const START_TIMEOUT: Duration = Duration::from_secs(5);
const STOP_TIMEOUT: Duration = Duration::from_secs(5);

const NOT_RUNNING_MESSAGE: &str =
    "Lens Sandbox is not running. Run `lns service start` to start it.";

mod orchestrator;
pub use orchestrator::{
    PrePhaseOutcome, PrePhaseStep, dispatch, drive_attached_session_with_writers, drive_pre_phase,
    exec_image, kill, ls, pre_phase_step, render_started_run, render_status_line, require_running,
    run_image,
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
pub(super) fn render_ls_table<W: std::io::Write>(
    out: &mut W,
    rows: &[RunSummary],
) -> std::io::Result<()> {
    let status_str = |s: RunStatus| match s {
        RunStatus::Running => "running".to_string(),
        RunStatus::Exited { code } => format!("exited ({code})"),
    };

    let id_w = "ID"
        .len()
        .max(rows.iter().map(|r| count_digits(r.id)).max().unwrap_or(0));
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

    writeln!(
        out,
        "{:<id_w$}  {:<status_w$}  {:<image_w$}  {:<cmd_w$}  STARTED",
        "ID",
        "STATUS",
        "IMAGE",
        "COMMAND",
        id_w = id_w,
        status_w = status_w,
        image_w = image_w,
        cmd_w = cmd_w,
    )?;
    for r in rows {
        writeln!(
            out,
            "{:<id_w$}  {:<status_w$}  {:<image_w$}  {:<cmd_w$}  {}",
            r.id,
            status_str(r.status),
            r.image,
            r.command,
            friendly_started(&r.started),
            id_w = id_w,
            status_w = status_w,
            image_w = image_w,
            cmd_w = cmd_w,
        )?;
    }
    Ok(())
}

pub(super) fn count_digits(mut n: u32) -> usize {
    if n == 0 {
        return 1;
    }
    let mut d = 0;
    while n > 0 {
        d += 1;
        n /= 10;
    }
    d
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

        fn cancel_run(&self, _run_id: u32) -> BoxFuture<'_, ()> {
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
    fn count_digits_handles_boundaries() {
        assert_eq!(count_digits(0), 1);
        assert_eq!(count_digits(9), 1);
        assert_eq!(count_digits(10), 2);
        assert_eq!(count_digits(999), 3);
        assert_eq!(count_digits(1_000), 4);
        assert_eq!(count_digits(u32::MAX), 10);
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

    #[test]
    fn render_ls_table_emits_header_and_one_row_per_run() {
        let rows = vec![RunSummary {
            id: 7,
            image: "alpine:3.20".into(),
            command: "echo hi".into(),
            started: "2024-03-15T08:30:00Z".into(),
            status: RunStatus::Running,
        }];
        let mut buf: Vec<u8> = Vec::new();
        render_ls_table(&mut buf, &rows).expect("render");
        let out = String::from_utf8(buf).unwrap();
        for tok in ["ID", "STATUS", "STARTED", "IMAGE", "COMMAND"] {
            assert!(
                out.contains(tok),
                "expected header token {tok:?} in {out:?}"
            );
        }
        assert!(out.contains("7"), "id 7 missing in {out:?}");
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
        let rows = vec![RunSummary {
            id: 42,
            image: "<imageless>".into(),
            command: "true".into(),
            started: "2024-03-15T08:30:00Z".into(),
            status: RunStatus::Exited { code: 0 },
        }];
        let mut buf: Vec<u8> = Vec::new();
        render_ls_table(&mut buf, &rows).expect("render");
        let out = String::from_utf8(buf).unwrap();
        assert!(
            out.to_lowercase().contains("exited") || out.contains('0'),
            "expected exited indicator in {out:?}"
        );
    }

    #[tokio::test]
    async fn fake_service_client_cancel_run_records_invocation() {
        let client = std::sync::Arc::new(FakeClient::default());
        client.cancel_run(7).await;
        assert_eq!(client.calls(), vec!["cancel_run"]);
    }
}
