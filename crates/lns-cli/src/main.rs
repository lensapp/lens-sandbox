use anyhow::Result;
use lns_cli::cli::LogLevel;
use lns_cli::log;

#[tokio::main]
async fn main() -> Result<()> {
    reset_sigpipe();
    let matches =
        lns_cli::command::try_get_matches_from(std::env::args_os()).unwrap_or_else(|e| e.exit());
    let log_level = matches
        .get_one::<LogLevel>("log_level")
        .copied()
        .unwrap_or(LogLevel::Warn);
    log::init(log_level);
    let code = lns_cli::run_from_matches(matches, log_level == LogLevel::Debug).await?;
    std::process::exit(code);
}

#[cfg(unix)]
fn reset_sigpipe() {
    // SAFETY: signal(2) is async-signal-safe; called once before any thread or I/O.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

#[cfg(not(unix))]
fn reset_sigpipe() {}
