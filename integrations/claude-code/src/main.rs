use std::io::Read;
use std::process::{Command, ExitCode};

use clap::{Parser, Subcommand};

mod classify;
mod hook;
mod rewrite;

#[derive(Parser)]
#[command(
    name = "lns-cc",
    version,
    about = "Lens Sandbox bridge for Claude Code"
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// PreToolUse hook: read the Bash tool call on stdin and route code execution into an lns microVM.
    Hook,
    /// Wrapper the rewritten command points at: ensure a sandbox and run the original command inside it.
    Exec(ExecArgs),
    /// SessionStart check: warn when `lns` is missing or its service is not running.
    Doctor,
}

#[derive(clap::Args)]
struct ExecArgs {
    /// Runtime key the command was classified as (selects the sandbox image).
    runtime: String,
    /// Base64-encoded original command string to run inside the sandbox.
    #[arg(long)]
    b64: String,
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Cmd::Hook => run_hook(),
        Cmd::Exec(args) => run_exec(&args),
        Cmd::Doctor => run_doctor(),
    }
}

fn run_hook() -> ExitCode {
    let mut input = String::new();
    if std::io::stdin().read_to_string(&mut input).is_err() {
        return ExitCode::SUCCESS;
    }
    let Ok(exe) = std::env::current_exe() else {
        return ExitCode::SUCCESS;
    };
    if let Some(output) = hook::process_hook(&input, &exe.to_string_lossy(), true) {
        println!("{output}");
    }
    ExitCode::SUCCESS
}

fn run_exec(args: &ExecArgs) -> ExitCode {
    match rewrite::decode_command(&args.b64) {
        Some(command) => {
            eprintln!("lns-cc exec: not implemented yet (arrives in P2)");
            eprintln!("  would run in `{}` sandbox: {command}", args.runtime);
            ExitCode::FAILURE
        }
        None => {
            eprintln!("lns-cc exec: could not decode --b64 payload");
            ExitCode::FAILURE
        }
    }
}

fn run_doctor() -> ExitCode {
    discard_stdin();

    let issue = if !lns_available() {
        Some("`lns` was not found on PATH — install it from https://get.lns.run to enable sandboxing.")
    } else if !service_running() {
        Some("the Lens Sandbox service is not running — run `lns service start` to enable sandboxing.")
    } else {
        None
    };

    if let Some(issue) = issue {
        emit_session_start_context(&format!(
            "Lens Sandbox plugin: code-execution sandboxing is DEGRADED — {issue}"
        ));
    }
    ExitCode::SUCCESS
}

fn discard_stdin() {
    let mut buf = String::new();
    let _ = std::io::stdin().read_to_string(&mut buf);
}

fn lns_available() -> bool {
    Command::new("lns")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn service_running() -> bool {
    match Command::new("lns").args(["service", "status"]).output() {
        Ok(out) => {
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            )
            .to_lowercase();
            if text.contains("not running") {
                false
            } else {
                out.status.success()
            }
        }
        Err(_) => false,
    }
}

fn emit_session_start_context(message: &str) {
    let payload = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "SessionStart",
            "additionalContext": message,
        }
    });
    println!("{payload}");
}
