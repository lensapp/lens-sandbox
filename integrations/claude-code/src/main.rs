use std::io::Read;
use std::path::Path;
use std::process::{Command, ExitCode};

use clap::{Parser, Subcommand};

mod admin;
mod backend;
mod boxmgr;
mod classify;
mod config;
mod hook;
mod rewrite;

use backend::{Backend, RunRequest};
use config::Config;

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
    #[command(
        about = "PreToolUse entrypoint: rewrites sandbox-worthy Bash commands to run inside lns."
    )]
    Hook,
    #[command(about = "Run a hook-rewritten command inside an lns sandbox.")]
    Exec(ExecArgs),
    #[command(about = "SessionStart health check for lns and its service.")]
    Doctor,
    #[command(about = "Show effective config, lns health, and active plugin sandboxes.")]
    Status,
    #[command(about = "Expose a host path to the sandbox (read-only by default).")]
    Grant(GrantArgs),
    #[command(about = "Remove leftover plugin sandboxes (cc-*).")]
    Clean,
}

#[derive(clap::Args)]
struct ExecArgs {
    #[arg(help = "Runtime key selecting the sandbox image.")]
    runtime: String,
    #[arg(long, help = "Base64-encoded command to run in the sandbox.")]
    b64: String,
}

#[derive(clap::Args)]
struct GrantArgs {
    #[arg(help = "Host path to expose (bare path binds at the same absolute path).")]
    path: String,
    #[arg(long, help = "Grant read-write instead of the default read-only.")]
    rw: bool,
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Cmd::Hook => run_hook(),
        Cmd::Exec(args) => run_exec(&args),
        Cmd::Doctor => run_doctor(),
        Cmd::Status => run_status(),
        Cmd::Grant(args) => run_grant(&args),
        Cmd::Clean => run_clean(),
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
    let cwd = hook::payload_cwd(&input).unwrap_or_else(|| ".".to_string());
    let config = Config::load(Path::new(&cwd));
    if let Some(output) = hook::process_hook(&input, &exe.to_string_lossy(), &config) {
        println!("{output}");
    }
    ExitCode::SUCCESS
}

fn run_exec(args: &ExecArgs) -> ExitCode {
    let Some(command) = rewrite::decode_command(&args.b64) else {
        eprintln!("lns-cc exec: could not decode --b64 payload");
        return ExitCode::FAILURE;
    };
    let cwd = match std::env::current_dir() {
        Ok(path) => path.to_string_lossy().into_owned(),
        Err(err) => {
            eprintln!("lns-cc exec: cannot determine working directory: {err}");
            return ExitCode::FAILURE;
        }
    };
    let config = Config::load(Path::new(&cwd));
    let image = config
        .image_override(&args.runtime)
        .map(str::to_string)
        .or_else(|| classify::image_for_runtime(&args.runtime).map(str::to_string));
    let Some(image) = image else {
        eprintln!("lns-cc exec: unknown runtime `{}`", args.runtime);
        return ExitCode::FAILURE;
    };
    let request = RunRequest {
        runtime_key: args.runtime.clone(),
        image,
        cwd,
        command,
        mounts: config
            .mounts
            .iter()
            .map(|m| config::normalize_mount(m))
            .collect(),
        env: config::resolve_forward_env(&config.env_forward),
        cpus: config.cpus,
        mem: config.mem.clone(),
    };
    match select_backend().run_in_sandbox(&request) {
        Ok(code) => ExitCode::from(u8::try_from(code).unwrap_or(1)),
        Err(err) => {
            eprintln!("lns-cc: {err}");
            ExitCode::FAILURE
        }
    }
}

fn select_backend() -> Box<dyn Backend> {
    match std::env::var("LNS_CC_BACKEND").ok().as_deref() {
        Some("persistent") => Box::new(boxmgr::PersistentBackend::new(boxmgr::RealLns)),
        _ => Box::new(backend::EphemeralBackend::new(backend::RealEphemeral)),
    }
}

fn run_status() -> ExitCode {
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| ".".to_string());
    let config = Config::load(Path::new(&cwd));
    let backend = match std::env::var("LNS_CC_BACKEND").ok().as_deref() {
        Some("persistent") => "persistent",
        _ => "ephemeral (default)",
    };

    println!("Lens Sandbox plugin — status");
    println!("  lns on PATH:      {}", yes_no(lns_available()));
    println!("  service running:  {}", yes_no(service_running()));
    println!("  backend:          {backend}");
    println!("  auto-allow:       {}", yes_no(config.auto_allow));
    println!("  package installs: {}", yes_no(config.package_installs));
    println!("  network CLIs:     {}", yes_no(config.network_clis));
    if !config.mounts.is_empty() {
        println!("  extra mounts:     {}", config.mounts.join(", "));
    }
    if !config.env_forward.is_empty() {
        println!("  env forward:      {}", config.env_forward.join(", "));
    }
    if !config.bypass.is_empty() {
        println!("  bypass:           {}", config.bypass.join(", "));
    }
    if !config.force.is_empty() {
        println!("  force:            {}", config.force.join(", "));
    }
    for (key, image) in &config.images {
        println!("  image[{key}]:        {image}");
    }
    match cc_boxes() {
        Some(boxes) if !boxes.is_empty() => println!("  active sandboxes: {}", boxes.join(", ")),
        Some(_) => println!("  active sandboxes: (none)"),
        None => {}
    }
    ExitCode::SUCCESS
}

fn run_grant(args: &GrantArgs) -> ExitCode {
    if admin::looks_secret_path(&args.path) {
        eprintln!(
            "lns-cc: refusing to bind-mount `{}` — secrets must stay outside the workload.",
            args.path
        );
        eprintln!("        Route it through the lns credential flow instead (`lns integration`).");
        return ExitCode::FAILURE;
    }
    let entry = admin::mount_entry(&args.path, args.rw);
    let cwd = match std::env::current_dir() {
        Ok(path) => path,
        Err(err) => {
            eprintln!("lns-cc: {err}");
            return ExitCode::FAILURE;
        }
    };
    let file = cwd.join(config::PROJECT_FILE);
    match add_mount_to_config(&file, &entry) {
        Ok(()) => {
            println!("Granted `{entry}` (written to {}).", file.display());
            println!("Re-run your command — new sandboxes pick up the mount.");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("lns-cc: {err}");
            ExitCode::FAILURE
        }
    }
}

fn run_clean() -> ExitCode {
    let Some(boxes) = cc_boxes() else {
        eprintln!("lns-cc: could not list sandboxes (is the service running?)");
        return ExitCode::FAILURE;
    };
    if boxes.is_empty() {
        println!("No plugin sandboxes to remove.");
        return ExitCode::SUCCESS;
    }
    let mut removed = 0;
    for name in &boxes {
        let _ = Command::new("lns").args(["sandbox", "kill", name]).output();
        let ok = Command::new("lns")
            .args(["sandbox", "rm", name])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if ok {
            removed += 1;
        }
    }
    println!("Removed {removed} of {} plugin sandbox(es).", boxes.len());
    ExitCode::SUCCESS
}

fn add_mount_to_config(file: &Path, entry: &str) -> Result<(), String> {
    let mut table: toml::Table = if file.exists() {
        let text = std::fs::read_to_string(file).map_err(|e| e.to_string())?;
        toml::from_str(&text).map_err(|e| format!("invalid {}: {e}", file.display()))?
    } else {
        toml::Table::new()
    };
    if !table.contains_key("mounts") {
        table.insert("mounts".to_string(), toml::Value::Array(Vec::new()));
    }
    let mounts = table
        .get_mut("mounts")
        .and_then(toml::Value::as_array_mut)
        .ok_or("`mounts` in the config is not an array")?;
    let value = toml::Value::String(entry.to_string());
    if !mounts.contains(&value) {
        mounts.push(value);
    }
    let text = toml::to_string(&table).map_err(|e| e.to_string())?;
    std::fs::write(file, text).map_err(|e| e.to_string())
}

fn cc_boxes() -> Option<Vec<String>> {
    let out = Command::new("lns").args(["sandbox", "ls"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(admin::cc_box_names(&String::from_utf8_lossy(&out.stdout)))
}

fn yes_no(value: bool) -> &'static str {
    if value {
        "yes"
    } else {
        "no"
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
