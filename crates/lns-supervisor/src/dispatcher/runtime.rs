use std::collections::HashMap;
use std::io::IsTerminal;
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use lens_sandbox_core::activity::ActivityStream;
use lens_sandbox_core::child_spawner;
use lens_sandbox_core::lifecycle::{DEFAULT_GRACE, OrphanReaper, wait_with_signal_forwarding};
use lens_sandbox_core::privilege::SandboxCredentials;

use super::agent::{AgentRunner, agent_child_spec, build_agent_command, stream_output};
use super::{CYAN, DIM, GREEN, RED, RESET};
use crate::config::AgentConfig;

/// Flush stdout and let the activity forwarder drain before `process::exit` tears down pid 1.
async fn drain_console() {
    tokio::task::yield_now().await;
    let _ = tokio::io::stdout().flush().await;
}

pub(crate) struct RealAgentRunner;

impl AgentRunner for RealAgentRunner {
    fn spawn_run(
        &self,
        config: Arc<AgentConfig>,
        creds: Option<SandboxCredentials>,
        env: HashMap<String, String>,
        activity: ActivityStream,
        reaper: Arc<OrphanReaper>,
    ) {
        tokio::spawn(async move {
            run_agent(&config, creds, env, &activity, &reaper).await;
        });
    }
}

/// Whether the agent opted into PTY mode via `TERM` (`xterm-*` yes; unset, empty, or the `linux` console no).
fn agent_wants_pty() -> bool {
    match std::env::var("TERM") {
        Ok(v) if v.is_empty() || v == "linux" => false,
        Ok(_) => true,
        Err(_) => false,
    }
}

/// Whether devpts is mounted so `openpty()` can allocate a pair; the caller falls back to pipe mode otherwise.
fn pty_available() -> bool {
    std::path::Path::new("/dev/pts/ptmx").exists()
}

/// Check if the container's stdin is a real terminal (e.g. `docker run -it`).
fn stdin_is_tty() -> bool {
    std::io::stdin().is_terminal()
}

/// Get terminal size from the container's TTY, falling back to 80x24.
fn get_terminal_size() -> (u16, u16) {
    // SAFETY: `winsize` is plain POD; zeroed is a valid representation.
    let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
    // SAFETY: `&mut ws` is a unique, properly-aligned, dereferenceable winsize pointer; ioctl writes through it.
    let ret = unsafe { libc::ioctl(libc::STDIN_FILENO, libc::TIOCGWINSZ as _, &mut ws) };
    if ret == 0 && ws.ws_col > 0 && ws.ws_row > 0 {
        (ws.ws_col, ws.ws_row)
    } else {
        (80, 24)
    }
}

async fn run_agent(
    config: &AgentConfig,
    creds: Option<SandboxCredentials>,
    env: HashMap<String, String>,
    activity: &ActivityStream,
    reaper: &OrphanReaper,
) {
    if agent_wants_pty() && pty_available() {
        run_agent_pty(config, creds, env, activity, reaper).await;
    } else {
        run_agent_piped(config, creds, env, activity, reaper).await;
    }
}

/// PTY mode: bridge I/O when stdin is a real terminal, else drain output to activity (web terminal path).
async fn run_agent_pty(
    config: &AgentConfig,
    creds: Option<SandboxCredentials>,
    env: HashMap<String, String>,
    activity: &ActivityStream,
    reaper: &OrphanReaper,
) {
    let bridged = stdin_is_tty();
    let initial_size = if bridged {
        get_terminal_size()
    } else {
        (80, 24)
    };

    tracing::info!(
        command = %config.agent_command,
        interactive = true,
        bridged,
        "starting agent process (PTY)"
    );
    activity.emit(format!(
        "{CYAN}[agent]{RESET} starting: {DIM}{}{RESET}\r\n",
        config.agent_command
    ));

    let spec = agent_child_spec(config, creds.as_ref(), &env);
    let proc = match child_spawner::spawn_pty(&spec, initial_size) {
        Ok(proc) => proc,
        Err(e) => {
            tracing::error!("failed to spawn agent process: {e}");
            activity.emit(format!("{RED}[agent]{RESET} failed to start: {e}\r\n"));
            drain_console().await;
            std::process::exit(1);
        }
    };

    let mut reader = proc.reader;
    let mut writer = proc.writer;
    let mut child = proc.child;
    let master_fd = proc.master_fd;

    // Register the PID before any await so a SIGCHLD can't make the reaper reap the agent before it's in the protected set.
    if let Some(pid) = child.id() {
        reaper.protect_pid(pid);
    }

    if bridged {
        // Put container's terminal into raw mode so keystrokes pass through
        let orig_termios = setup_raw_mode();

        // Forward SIGWINCH → resize PTY
        let raw_fd = master_fd.raw();
        tokio::spawn(async move {
            forward_sigwinch(raw_fd).await;
        });

        let output_task = tokio::spawn(async move {
            let mut stdout = tokio::io::stdout();
            let mut buf = vec![0u8; 8192];
            loop {
                match reader.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        if stdout.write_all(&buf[..n]).await.is_err() {
                            break;
                        }
                        let _ = stdout.flush().await;
                    }
                    Err(e) if e.raw_os_error() == Some(libc::EIO) => break,
                    Err(_) => break,
                }
            }
        });

        let input_task = tokio::spawn(async move {
            let mut stdin = tokio::io::stdin();
            let mut buf = vec![0u8; 8192];
            loop {
                match stdin.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        if writer.write_all(&buf[..n]).await.is_err() {
                            break;
                        }
                        let _ = writer.flush().await;
                    }
                    Err(_) => break,
                }
            }
        });

        let code = match wait_with_signal_forwarding(&mut child, DEFAULT_GRACE).await {
            Ok(status) => status.code().unwrap_or(1),
            Err(_) => 1,
        };
        let _ = output_task.await;
        input_task.abort();

        restore_termios(orig_termios);

        drain_console().await;
        std::process::exit(code);
    } else {
        let drain_activity = activity.clone();
        let drain_task = tokio::spawn(async move {
            let mut buf = vec![0u8; 8192];
            loop {
                match reader.read(&mut buf).await {
                    Ok(0) => break,
                    Ok(n) => {
                        let text = String::from_utf8_lossy(&buf[..n]);
                        drain_activity.emit(text.into_owned());
                    }
                    Err(e) if e.raw_os_error() == Some(libc::EIO) => break,
                    Err(_) => break,
                }
            }
        });

        match wait_with_signal_forwarding(&mut child, DEFAULT_GRACE).await {
            Ok(status) => {
                let _ = drain_task.await;
                let code = status.code().unwrap_or(1);
                let color = if code == 0 { GREEN } else { RED };
                activity.emit(format!("{color}[agent]{RESET} exited with code {code}\r\n"));
                tracing::info!(exit_code = code, "agent process exited");
                drain_console().await;
                std::process::exit(code);
            }
            Err(e) => {
                tracing::error!("failed to wait on agent process: {e}");
                activity.emit(format!("{RED}[agent]{RESET} wait failed: {e}\r\n"));
                drain_console().await;
                std::process::exit(1);
            }
        }
    }
}

/// Put the container's stdin into raw mode, returning the original settings.
fn setup_raw_mode() -> Option<libc::termios> {
    // SAFETY: termios is POD-zeroable; tcgetattr/tcsetattr/cfmakeraw take a unique &mut to a valid termios.
    unsafe {
        let mut orig: libc::termios = std::mem::zeroed();
        if libc::tcgetattr(libc::STDIN_FILENO, &mut orig) < 0 {
            return None;
        }
        let mut raw = orig;
        libc::cfmakeraw(&mut raw);
        if libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &raw) < 0 {
            tracing::warn!("failed to set raw mode");
        }
        Some(orig)
    }
}

fn restore_termios(orig: Option<libc::termios>) {
    if let Some(orig) = orig {
        // SAFETY: `&orig` points to a fully-initialised termios returned earlier by tcgetattr.
        unsafe {
            libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &orig);
        }
    }
}

/// Listen for SIGWINCH and forward the new terminal size to the PTY master.
async fn forward_sigwinch(master_fd: std::os::fd::RawFd) {
    use tokio::signal::unix::{SignalKind, signal};
    let mut sigwinch = match signal(SignalKind::window_change()) {
        Ok(s) => s,
        Err(_) => return,
    };
    while sigwinch.recv().await.is_some() {
        // SAFETY: `winsize` is plain POD; zeroed is a valid representation.
        let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
        // SAFETY: `&mut ws` is a unique, properly-aligned, dereferenceable winsize pointer; ioctl writes through it.
        let ret = unsafe { libc::ioctl(libc::STDIN_FILENO, libc::TIOCGWINSZ as _, &mut ws) };
        if ret == 0 && ws.ws_col > 0 && ws.ws_row > 0 {
            let _ = lens_sandbox_core::pty::resize(master_fd, ws.ws_col, ws.ws_row);
        }
    }
}

/// Non-interactive pipe mode: stream stdout/stderr as activity events (current behavior).
async fn run_agent_piped(
    config: &AgentConfig,
    creds: Option<SandboxCredentials>,
    env: HashMap<String, String>,
    activity: &ActivityStream,
    reaper: &OrphanReaper,
) {
    let mut cmd = build_agent_command(config, creds.as_ref(), &env);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    tracing::info!(command = %config.agent_command, "starting agent process");
    activity.emit(format!(
        "{CYAN}[agent]{RESET} starting: {DIM}{}{RESET}\r\n",
        config.agent_command
    ));

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            tracing::error!("failed to spawn agent process: {e}");
            activity.emit(format!("{RED}[agent]{RESET} failed to start: {e}\r\n"));
            drain_console().await;
            std::process::exit(1);
        }
    };

    if let Some(pid) = child.id() {
        reaper.protect_pid(pid);
    }

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    let stdout_task = stdout.map(|pipe| {
        let activity = activity.clone();
        tokio::spawn(async move {
            stream_output(pipe, activity, "[stdout]", DIM).await;
        })
    });

    let stderr_task = stderr.map(|pipe| {
        let activity = activity.clone();
        tokio::spawn(async move {
            stream_output(pipe, activity, "[stderr]", RED).await;
        })
    });

    match wait_with_signal_forwarding(&mut child, DEFAULT_GRACE).await {
        Ok(status) => {
            if let Some(t) = stdout_task {
                let _ = t.await;
            }
            if let Some(t) = stderr_task {
                let _ = t.await;
            }

            let code = status.code().unwrap_or(1);
            let color = if code == 0 { GREEN } else { RED };
            activity.emit(format!("{color}[agent]{RESET} exited with code {code}\r\n"));
            tracing::info!(exit_code = code, "agent process exited");
            drain_console().await;
            std::process::exit(code);
        }
        Err(e) => {
            tracing::error!("failed to wait on agent process: {e}");
            activity.emit(format!("{RED}[agent]{RESET} wait failed: {e}\r\n"));
            drain_console().await;
            std::process::exit(1);
        }
    }
}
