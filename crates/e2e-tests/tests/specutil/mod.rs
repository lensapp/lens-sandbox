#![allow(dead_code)]

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub mod arg_parser;

pub fn lns_binary() -> PathBuf {
    if let Ok(p) = std::env::var("LNS_BIN") {
        return PathBuf::from(p);
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_target = manifest
        .ancestors()
        .nth(2)
        .expect("workspace root reachable from CARGO_MANIFEST_DIR")
        .join("target");
    for profile in ["debug", "release"] {
        let candidate = workspace_target.join(profile).join("lns");
        if candidate.exists() {
            return candidate;
        }
    }
    panic!(
        "could not locate `lns` binary. Set LNS_BIN, or build the workspace first \
         (\\`cargo build -p lns-cli\\`)."
    );
}

pub fn service_binary() -> PathBuf {
    if let Ok(p) = std::env::var("LNS_SERVICE_BIN") {
        return PathBuf::from(p);
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_target = manifest
        .ancestors()
        .nth(2)
        .expect("workspace root reachable from CARGO_MANIFEST_DIR")
        .join("target");
    for profile in ["debug", "release"] {
        let candidate = workspace_target.join(profile).join("lns-service");
        if candidate.exists() {
            return candidate;
        }
    }
    panic!(
        "could not locate `lns-service` binary. Set LNS_SERVICE_BIN, or build the \
         workspace first (\\`cargo build -p lns-service\\`)."
    );
}

#[derive(Debug)]
pub struct CliResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

pub fn run_cli<I, S>(args: I) -> CliResult
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    run_cli_with_env(args, std::iter::empty::<(&str, &str)>())
}

pub fn run_cli_with_env<I, S, E, K, V>(args: I, envs: E) -> CliResult
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
    E: IntoIterator<Item = (K, V)>,
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    let args: Vec<String> = args.into_iter().map(Into::into).collect();
    let mut cmd = Command::new(lns_binary());
    cmd.args(&args);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let output = cmd.output().expect("failed to spawn lns binary");
    CliResult {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        exit_code: output.status.code().unwrap_or(-1),
    }
}

pub fn run_cli_with_closed_stdout<I, S, E, K, V>(args: I, envs: E) -> CliResult
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
    E: IntoIterator<Item = (K, V)>,
    K: AsRef<OsStr>,
    V: AsRef<OsStr>,
{
    let args: Vec<String> = args.into_iter().map(Into::into).collect();
    let mut cmd = Command::new(lns_binary());
    cmd.args(&args);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("failed to spawn lns binary");
    drop(child.stdout.take());
    let output = child
        .wait_with_output()
        .expect("failed to wait for lns binary");
    CliResult {
        stdout: String::new(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        exit_code: output.status.code().unwrap_or(-1),
    }
}

pub fn cache_runs_dir(home: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library/Caches/lns/runs")
    } else {
        home.join(".cache/lns/runs")
    }
}

pub fn assert_eq_int(expected: i32, actual: i32, label: &str) -> Result<(), String> {
    if expected == actual {
        Ok(())
    } else {
        Err(format!("expected {label} {expected}, got {actual}"))
    }
}

pub fn assert_ne_int(unexpected: i32, actual: i32, label: &str) -> Result<(), String> {
    if unexpected != actual {
        Ok(())
    } else {
        Err(format!("expected {label} != {unexpected}, but it was"))
    }
}

pub fn assert_contains(haystack: &str, needle: &str, label: &str) -> Result<(), String> {
    if haystack.contains(needle) {
        Ok(())
    } else {
        Err(format!(
            "expected {label} to contain {needle:?}, got {haystack:?}"
        ))
    }
}

pub const TAMPERED_AUDIT_CHAIN: &str = concat!(
    r#"{"prev_hash":"0000000000000000000000000000000000000000000000000000000000000000","seq":1,"type":"audit_event"}"#,
    "\n",
    r#"{"prev_hash":"deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef","seq":2,"type":"audit_event"}"#,
    "\n",
);
