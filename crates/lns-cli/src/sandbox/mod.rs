use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use lns_ipc::{
    Request, Response, RunDetails, RunStatsInfo, WireFrame, decode_wire_frame_from_bytes,
    read_frame_bytes_async,
};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};

use crate::cli::{
    SandboxAttachArgs, SandboxCommand, SandboxInspectArgs, SandboxLogsArgs, SandboxStatsArgs,
    SandboxStopArgs,
};
use crate::service::client::BoxFuture;

pub mod real;

pub trait SandboxService: Send + Sync {
    type Stream: AsyncRead + AsyncWrite + Unpin + Send + 'static;
    fn one_shot(&self, request: Request) -> BoxFuture<'_, Result<Response>>;
    fn open_stream(&self, request: Request) -> BoxFuture<'_, Result<Self::Stream>>;
    fn aux_socket(&self) -> Option<PathBuf>;
    fn load_policy(&self, path: &str) -> Option<serde_json::Value>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct TermInfo {
    pub stdin_is_tty: bool,
    pub stdout_is_terminal: bool,
}

pub async fn run_with_writers<S, W, O, E>(
    cmd: &SandboxCommand,
    svc: &S,
    term: TermInfo,
    out: &mut W,
    stdout: &mut O,
    stderr: &mut E,
) -> Result<i32>
where
    S: SandboxService,
    W: std::io::Write,
    O: AsyncWriteExt + Unpin,
    E: AsyncWriteExt + Unpin,
{
    match cmd {
        SandboxCommand::Stop(args) => stop(svc, args, out).await,
        SandboxCommand::Inspect(args) => inspect(svc, args, out).await,
        SandboxCommand::Stats(args) => stats(svc, args, out).await,
        SandboxCommand::Logs(args) => logs(svc, args, stdout, stderr).await,
        SandboxCommand::Attach(args) => attach(svc, args, term, stdout, stderr).await,
    }
}

async fn stop<W: std::io::Write>(
    svc: &impl SandboxService,
    args: &SandboxStopArgs,
    out: &mut W,
) -> Result<i32> {
    let response = svc
        .one_shot(Request::StopRun {
            run_id: args.run_id,
            timeout_secs: args.timeout,
        })
        .await?;
    match response {
        Response::RunStopped { forced: false } => {
            writeln!(out, "stopped run #{}", args.run_id)?;
            Ok(0)
        }
        Response::RunStopped { forced: true } => {
            writeln!(
                out,
                "killed run #{} after the {}s timeout",
                args.run_id, args.timeout
            )?;
            Ok(0)
        }
        Response::Error { message } => bail!("daemon error: {message}"),
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

async fn inspect<W: std::io::Write>(
    svc: &impl SandboxService,
    args: &SandboxInspectArgs,
    out: &mut W,
) -> Result<i32> {
    let response = svc
        .one_shot(Request::InspectRun {
            run_id: args.run_id,
        })
        .await?;
    match response {
        Response::RunInspect { details } => {
            let policy = details
                .config
                .policy_path
                .as_deref()
                .map(|path| policy_doc(path, svc.load_policy(path)));
            render_inspect(&details, policy, out)?;
            Ok(0)
        }
        Response::Error { message } => bail!("daemon error: {message}"),
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

fn policy_doc(path: &str, loaded: Option<serde_json::Value>) -> serde_json::Value {
    match loaded {
        Some(doc) => serde_json::json!({ "path": path, "policy": doc }),
        None => serde_json::json!({ "path": path, "error": "policy file could not be read" }),
    }
}

fn render_inspect<W: std::io::Write>(
    details: &RunDetails,
    policy: Option<serde_json::Value>,
    out: &mut W,
) -> Result<()> {
    let doc = serde_json::json!({
        "id": details.summary.id,
        "image": details.summary.image,
        "command": details.summary.command,
        "status": details.summary.status,
        "started": details.summary.started,
        "config": {
            "cpus": details.config.cpus,
            "memMib": details.config.mem_mib,
            "env": details.config.env,
            "publishedPorts": details.config.published_ports,
            "volumes": details.config.volumes,
            "sandboxUser": details.config.sandbox_user,
            "sandboxUid": details.config.sandbox_uid,
            "detached": details.config.detached,
        },
        "policy": policy,
    });
    writeln!(out, "{}", serde_json::to_string_pretty(&doc)?)?;
    Ok(())
}

async fn stats<W: std::io::Write>(
    svc: &impl SandboxService,
    args: &SandboxStatsArgs,
    out: &mut W,
) -> Result<i32> {
    let response = svc
        .one_shot(Request::RunStats {
            run_id: args.run_id,
        })
        .await?;
    match response {
        Response::RunStats { stats } => {
            render_stats(&stats, out)?;
            Ok(0)
        }
        Response::Error { message } => bail!("daemon error: {message}"),
        other => bail!("unexpected response from daemon: {other:?}"),
    }
}

fn render_stats<W: std::io::Write>(stats: &RunStatsInfo, out: &mut W) -> Result<()> {
    writeln!(out, "CPU %   MEM USAGE / LIMIT")?;
    writeln!(
        out,
        "{:<6}  {} / {}",
        format_permille(stats.cpu_permille),
        format_bytes(stats.mem_used_bytes),
        format_bytes(stats.mem_total_bytes),
    )?;
    Ok(())
}

fn format_permille(permille: u32) -> String {
    format!("{}.{}%", permille / 10, permille % 10)
}

fn format_bytes(n: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;
    if n >= GIB {
        format!("{:.1} GiB", n as f64 / GIB as f64)
    } else if n >= MIB {
        format!("{:.1} MiB", n as f64 / MIB as f64)
    } else if n >= KIB {
        format!("{:.1} KiB", n as f64 / KIB as f64)
    } else {
        format!("{n} B")
    }
}

async fn logs<S, O, E>(
    svc: &S,
    args: &SandboxLogsArgs,
    stdout: &mut O,
    stderr: &mut E,
) -> Result<i32>
where
    S: SandboxService,
    O: AsyncWriteExt + Unpin,
    E: AsyncWriteExt + Unpin,
{
    let mut stream = svc
        .open_stream(Request::RunLogs {
            run_id: args.run_id,
            follow: args.follow,
        })
        .await?;
    expect_run_started(&mut stream).await?;
    drive_logs(stream, stdout, stderr).await
}

async fn attach<S, O, E>(
    svc: &S,
    args: &SandboxAttachArgs,
    term: TermInfo,
    stdout: &mut O,
    stderr: &mut E,
) -> Result<i32>
where
    S: SandboxService,
    O: AsyncWriteExt + Unpin,
    E: AsyncWriteExt + Unpin,
{
    let mut stream = svc
        .open_stream(Request::AttachRun {
            run_id: args.run_id,
        })
        .await?;
    let run_id = expect_run_started(&mut stream).await?;
    crate::service::drive_attached_session_with_writers(
        stream,
        svc.aux_socket(),
        run_id,
        term.stdin_is_tty,
        term.stdout_is_terminal,
        args.detach_keys.0.clone(),
        stdout,
        stderr,
    )
    .await
}

async fn expect_run_started<S: AsyncRead + Unpin>(stream: &mut S) -> Result<u32> {
    let bytes = read_frame_bytes_async(stream)
        .await
        .context("reading stream handshake")?;
    match decode_wire_frame_from_bytes(&bytes).context("decoding stream handshake")? {
        WireFrame::Json(Response::RunStarted { run_id }) => Ok(run_id),
        WireFrame::Json(Response::Error { message }) => bail!("daemon error: {message}"),
        other => bail!("expected RunStarted, got {other:?}"),
    }
}

async fn drive_logs<S, O, E>(mut stream: S, stdout: &mut O, stderr: &mut E) -> Result<i32>
where
    S: AsyncRead + Unpin,
    O: AsyncWriteExt + Unpin,
    E: AsyncWriteExt + Unpin,
{
    loop {
        let bytes = read_frame_bytes_async(&mut stream)
            .await
            .context("reading logs frame")?;
        match decode_wire_frame_from_bytes(&bytes).context("decoding logs frame")? {
            WireFrame::Stdout(b) => {
                stdout.write_all(&b).await?;
                stdout.flush().await.ok();
            }
            WireFrame::Stderr(b) => {
                stderr.write_all(&b).await?;
                stderr.flush().await.ok();
            }
            WireFrame::Json(Response::Acknowledged) => return Ok(0),
            WireFrame::Json(Response::RunExit { .. }) => return Ok(0),
            WireFrame::Json(Response::Error { message }) => bail!("daemon error: {message}"),
            other => bail!("unexpected logs frame: {other:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lns_ipc::encode_frame;
    use std::sync::{Arc, Mutex};

    struct CannedService {
        response: Response,
        frames: Vec<Vec<u8>>,
        requests: Arc<Mutex<Vec<Request>>>,
    }

    impl CannedService {
        fn new(response: Response) -> Self {
            Self {
                response,
                frames: Vec::new(),
                requests: Arc::new(Mutex::new(Vec::new())),
            }
        }

        fn with_frames(frames: Vec<Vec<u8>>) -> Self {
            Self {
                response: Response::Pong,
                frames,
                requests: Arc::new(Mutex::new(Vec::new())),
            }
        }
    }

    impl SandboxService for CannedService {
        type Stream = tokio::io::DuplexStream;
        fn one_shot(&self, request: Request) -> BoxFuture<'_, Result<Response>> {
            self.requests.lock().unwrap().push(request);
            let response = self.response.clone();
            Box::pin(async move { Ok(response) })
        }
        fn open_stream(&self, _request: Request) -> BoxFuture<'_, Result<Self::Stream>> {
            let frames = self.frames.clone();
            Box::pin(async move {
                if frames.is_empty() {
                    bail!("the daemon refused the stream");
                }
                Ok(stream_with(&frames).await)
            })
        }
        fn aux_socket(&self) -> Option<PathBuf> {
            None
        }
        fn load_policy(&self, _path: &str) -> Option<serde_json::Value> {
            None
        }
    }

    fn stop_args(run_id: u32) -> SandboxStopArgs {
        SandboxStopArgs {
            run_id,
            timeout: 10,
        }
    }

    #[tokio::test]
    async fn stop_rejects_an_unrelated_response_variant() {
        let svc = CannedService::new(Response::Pong);
        let mut out = Vec::new();
        let err = stop(&svc, &stop_args(1), &mut out).await.unwrap_err();
        assert!(format!("{err:#}").contains("unexpected response"));
    }

    #[tokio::test]
    async fn inspect_rejects_an_unrelated_response_variant() {
        let svc = CannedService::new(Response::Pong);
        let mut out = Vec::new();
        let err = inspect(&svc, &SandboxInspectArgs { run_id: 1 }, &mut out)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("unexpected response"));
    }

    #[tokio::test]
    async fn inspect_surfaces_a_daemon_error() {
        let svc = CannedService::new(Response::Error {
            message: "no active run with id 1".into(),
        });
        let mut out = Vec::new();
        let err = inspect(&svc, &SandboxInspectArgs { run_id: 1 }, &mut out)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("no active run with id 1"));
    }

    #[tokio::test]
    async fn stats_rejects_an_unrelated_response_variant() {
        let svc = CannedService::new(Response::Pong);
        let mut out = Vec::new();
        let err = stats(&svc, &SandboxStatsArgs { run_id: 1 }, &mut out)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("unexpected response"));
    }

    #[tokio::test]
    async fn stats_surfaces_a_daemon_error() {
        let svc = CannedService::new(Response::Error {
            message: "macOS-only".into(),
        });
        let mut out = Vec::new();
        let err = stats(&svc, &SandboxStatsArgs { run_id: 1 }, &mut out)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("macOS-only"));
    }

    #[test]
    fn format_bytes_picks_the_natural_unit() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2048), "2.0 KiB");
        assert_eq!(format_bytes(92_274_688), "88.0 MiB");
        assert_eq!(format_bytes(2 * 1024 * 1024 * 1024), "2.0 GiB");
    }

    #[test]
    fn format_permille_renders_tenths_of_a_percent() {
        assert_eq!(format_permille(0), "0.0%");
        assert_eq!(format_permille(125), "12.5%");
        assert_eq!(format_permille(1000), "100.0%");
    }

    async fn stream_with(frames: &[Vec<u8>]) -> tokio::io::DuplexStream {
        let (client, mut server) = tokio::io::duplex(64 * 1024);
        let payload: Vec<u8> = frames.concat();
        tokio::spawn(async move {
            let _ = server.write_all(&payload).await;
        });
        client
    }

    #[tokio::test]
    async fn handshake_rejects_an_unexpected_first_frame() {
        let frame = lns_ipc::encode_wire_frame(&WireFrame::Stdout(b"early".to_vec())).unwrap();
        let mut stream = stream_with(&[frame]).await;
        let err = expect_run_started(&mut stream).await.unwrap_err();
        assert!(format!("{err:#}").contains("expected RunStarted"));
    }

    #[tokio::test]
    async fn drive_logs_rejects_an_unexpected_json_frame() {
        let frame = encode_frame(&Response::Pong).unwrap();
        let stream = stream_with(&[frame]).await;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let err = drive_logs(stream, &mut stdout, &mut stderr)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("unexpected logs frame"));
    }

    #[tokio::test]
    async fn drive_logs_surfaces_a_mid_stream_daemon_error() {
        let frame = encode_frame(&Response::Error {
            message: "buffer dropped".into(),
        })
        .unwrap();
        let stream = stream_with(&[frame]).await;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let err = drive_logs(stream, &mut stdout, &mut stderr)
            .await
            .unwrap_err();
        assert!(format!("{err:#}").contains("buffer dropped"));
    }

    #[tokio::test]
    async fn drive_logs_writes_stderr_chunks_to_the_stderr_writer() {
        let frames = [
            lns_ipc::encode_wire_frame(&WireFrame::Stderr(b"warning!".to_vec())).unwrap(),
            encode_frame(&Response::Acknowledged).unwrap(),
        ];
        let stream = stream_with(&frames).await;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = drive_logs(stream, &mut stdout, &mut stderr).await.unwrap();
        assert_eq!(code, 0);
        assert_eq!(stderr, b"warning!");
        assert!(stdout.is_empty());
    }

    #[tokio::test]
    async fn logs_surfaces_a_failure_to_open_the_stream() {
        let svc = CannedService::with_frames(Vec::new());
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let err = logs(
            &svc,
            &SandboxLogsArgs {
                run_id: 1,
                follow: false,
            },
            &mut stdout,
            &mut stderr,
        )
        .await
        .unwrap_err();
        assert!(format!("{err:#}").contains("refused the stream"));
    }

    #[tokio::test]
    async fn attach_relays_output_and_adopts_the_workloads_exit_code() {
        let svc = CannedService::with_frames(vec![
            encode_frame(&Response::RunStarted { run_id: 9 }).unwrap(),
            lns_ipc::encode_wire_frame(&WireFrame::Stdout(b"live".to_vec())).unwrap(),
            encode_frame(&Response::RunExit { code: 4 }).unwrap(),
        ]);
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let code = attach(
            &svc,
            &SandboxAttachArgs {
                run_id: 9,
                detach_keys: crate::cli::DetachChord(Vec::new()),
            },
            TermInfo::default(),
            &mut stdout,
            &mut stderr,
        )
        .await
        .unwrap();
        assert_eq!(code, 4);
        assert_eq!(stdout, b"live");
        assert!(
            svc.aux_socket().is_none(),
            "the canned service offers no aux socket"
        );
    }

    #[tokio::test]
    async fn inspect_marks_the_policy_unreadable_when_the_loader_finds_nothing() {
        let svc = CannedService::new(Response::RunInspect {
            details: Box::new(RunDetails {
                summary: lns_ipc::RunSummary {
                    id: 1,
                    image: "some-image".into(),
                    command: String::new(),
                    status: lns_ipc::RunStatus::Running,
                    started: "2026-01-01T00:00:00Z".into(),
                },
                config: lns_ipc::RunConfig {
                    policy_path: Some("/work/lns-policy.yaml".into()),
                    ..Default::default()
                },
            }),
        });
        let mut out = Vec::new();
        let code = inspect(&svc, &SandboxInspectArgs { run_id: 1 }, &mut out)
            .await
            .unwrap();
        assert_eq!(code, 0);
        let text = String::from_utf8(out).unwrap();
        assert!(
            text.contains("policy file could not be read"),
            "got: {text}"
        );
    }

    #[test]
    fn policy_doc_marks_an_unreadable_file() {
        let doc = policy_doc("/work/lns-policy.yaml", None);
        assert_eq!(doc["path"], "/work/lns-policy.yaml");
        assert!(doc["error"].as_str().unwrap().contains("could not be read"));
    }

    #[test]
    fn policy_doc_embeds_a_parsed_policy() {
        let doc = policy_doc(
            "/work/lns-policy.yaml",
            Some(serde_json::json!({"network": {"defaultVerdict": "ask"}})),
        );
        assert_eq!(doc["policy"]["network"]["defaultVerdict"], "ask");
    }
}
