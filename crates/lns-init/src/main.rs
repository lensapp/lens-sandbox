#[cfg(not(target_os = "linux"))]
mod boot;
mod cmdline;
mod mount;
mod seed;

use std::io::Write;
use std::process::ExitCode;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            let _ = writeln!(std::io::stderr(), "lns-init: {msg}");
            ExitCode::from(1)
        }
    }
}

#[cfg(target_os = "linux")]
fn run() -> Result<(), String> {
    match mount::mount_and_exec() {
        Ok(_never) => Ok(()),
        Err(e) => Err(format!("{e}")),
    }
}

#[cfg(not(target_os = "linux"))]
struct RealCmdlineSource;

#[cfg(not(target_os = "linux"))]
impl boot::CmdlineSource for RealCmdlineSource {
    fn read_cmdline(&self) -> std::io::Result<String> {
        std::fs::read_to_string("/proc/cmdline")
    }
}

#[cfg(not(target_os = "linux"))]
fn run() -> Result<(), String> {
    boot::run_host(&RealCmdlineSource)
}
