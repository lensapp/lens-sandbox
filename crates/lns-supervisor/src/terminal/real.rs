use std::io;
use std::os::fd::AsRawFd;
use std::time::Duration;

use lns_openshell_spike::Error;
use lns_openshell_spike::launch::real::{Child, ProcessIo};
use openshell_supervisor_process::main_session::{MainOutput, MainOutputCursor, MainSession};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::signal::unix::{SignalKind, signal};

struct RawTerminal(libc::termios);

impl RawTerminal {
    fn enter() -> io::Result<Self> {
        // SAFETY: termios is plain data and tcgetattr initializes this exclusively borrowed value.
        let mut original: libc::termios = unsafe { std::mem::zeroed() };
        // SAFETY: stdin is borrowed and original is a valid writable termios.
        if unsafe { libc::tcgetattr(libc::STDIN_FILENO, &mut original) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let raw = super::raw_settings(original);
        // SAFETY: raw is initialized and stdin remains owned by the supervisor.
        if unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &raw) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(original))
    }
}

impl Drop for RawTerminal {
    fn drop(&mut self) {
        // SAFETY: the saved termios is initialized and stdin is still borrowed, not closed.
        if unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &self.0) } != 0 {
            crate::log::error!(
                "failed to restore broker terminal: {}",
                io::Error::last_os_error()
            );
        }
    }
}

fn resize(session: &MainSession) -> io::Result<()> {
    let mut size = libc::winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: ioctl writes a winsize into the exclusively borrowed, initialized size value.
    if unsafe { libc::ioctl(libc::STDIN_FILENO, libc::TIOCGWINSZ, &mut size) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let (columns, rows) = super::dimensions(size.ws_col, size.ws_row);
    session.resize(
        columns,
        rows,
        u32::from(size.ws_xpixel),
        u32::from(size.ws_ypixel),
    );
    Ok(())
}

async fn forward(event: MainOutput) -> io::Result<()> {
    match event {
        MainOutput::Stdout(bytes) => {
            let mut stdout = tokio::io::stdout();
            stdout.write_all(&bytes).await?;
            stdout.flush().await
        }
        MainOutput::Stderr(bytes) => {
            let mut stderr = tokio::io::stderr();
            stderr.write_all(&bytes).await?;
            stderr.flush().await
        }
        MainOutput::Exit(_) => Ok(()),
    }
}

async fn drain(output: &mut MainOutputCursor) -> Result<(), Error> {
    loop {
        let event = output
            .recv()
            .await
            .map_err(|lag| format!("terminal output lost {} events", lag.skipped))?;
        if matches!(event, MainOutput::Exit(_)) {
            return Ok(());
        }
        forward(event).await?;
    }
}

pub async fn run(
    mut child: Child,
    io: ProcessIo,
    mut signals: crate::signals::Signals,
) -> Result<i32, Error> {
    let _raw = RawTerminal::enter()?;
    let mut changed = signal(SignalKind::window_change())?;
    let ProcessIo::Pty(ref master) = io else {
        return Err("expected workload terminal".into());
    };
    let foreground = master.try_clone()?;
    let session = MainSession::new(io, child.pid());
    resize(&session)?;
    let (_, input) = session.acquire_input()?;
    let mut output = session.subscribe();
    let mut stdin = tokio::io::stdin();
    let mut bytes = [0; 8192];
    let mut input_open = true;
    loop {
        tokio::select! {
            status = child.wait() => {
                let status = status?;
                let code = crate::lifecycle::exit_code(status.exit_code(), status.signal());
                tokio::time::timeout(Duration::from_secs(5), async {
                    let (_, drained) = tokio::join!(session.finish(code, false), drain(&mut output));
                    drained
                }).await??;
                return Ok(code);
            }
            event = output.recv() => {
                forward(event.map_err(|lag| format!("terminal output lost {} events", lag.skipped))?).await?;
            }
            count = stdin.read(&mut bytes), if input_open => {
                let count = count?;
                if count == 0 { input_open = false; }
                else { input.send(bytes[..count].to_vec()).await?; }
            }
            _ = changed.recv() => resize(&session)?,
            signal = signals.next() => {
                // SAFETY: foreground is an owned duplicate of the live PTY master.
                let group = unsafe { libc::tcgetpgrp(foreground.as_raw_fd()) };
                if group < 0 { return Err(io::Error::last_os_error().into()); }
                crate::signals::forward(group as u32, signal)?;
            }
        }
    }
}
