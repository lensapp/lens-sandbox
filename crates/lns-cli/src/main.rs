use anyhow::Result;
use clap::Parser;
use lns_cli::cli::Cli;
use lns_cli::log;

#[tokio::main]
async fn main() -> Result<()> {
    reset_sigpipe();
    let cli = Cli::parse();
    log::init(cli.log_level);
    let code = lns_cli::run(cli).await?;
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
