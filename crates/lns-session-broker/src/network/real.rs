#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::os::unix::fs::PermissionsExt;
#[cfg(target_os = "linux")]
use std::sync::mpsc::{self, Receiver};
#[cfg(target_os = "linux")]
use std::thread::JoinHandle;
#[cfg(target_os = "linux")]
use std::time::Duration;

use super::{CommandOutcome, CommandRunner, FsWriter};
#[cfg(target_os = "linux")]
use super::{
    DHCP_DNS_PATH, KeepaliveWaiter, configure_dns_with, keep_dhcp_alive_with, parse_dhcp_dns,
};

#[derive(Default)]
struct RealCommandRunner;

impl CommandRunner for RealCommandRunner {
    fn run(&self, program: &str, args: &[&str]) -> std::io::Result<CommandOutcome> {
        let status = std::process::Command::new(program).args(args).status()?;
        Ok(CommandOutcome {
            code: status.code(),
        })
    }
}

#[derive(Default)]
struct RealFsWriter;

impl FsWriter for RealFsWriter {
    fn write(&self, path: &str, contents: &[u8], mode: u32) -> std::io::Result<()> {
        std::fs::write(path, contents)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
        Ok(())
    }
}

#[cfg(target_os = "linux")]
pub fn bring_up_eth0() -> Result<(), String> {
    super::bring_up_eth0_with(&RealCommandRunner, &RealFsWriter)
}

#[cfg(target_os = "linux")]
pub fn configure_dns() -> Result<(), String> {
    let dhcp_dns = parse_dhcp_dns(&std::fs::read_to_string(DHCP_DNS_PATH).unwrap_or_default());
    configure_dns_with(&dhcp_dns, &RealFsWriter)
}

#[cfg(target_os = "linux")]
struct StopWaiter {
    stop: Receiver<()>,
}

#[cfg(target_os = "linux")]
impl KeepaliveWaiter for StopWaiter {
    fn wait(&self, duration: Duration) -> bool {
        matches!(
            self.stop.recv_timeout(duration),
            Err(mpsc::RecvTimeoutError::Timeout)
        )
    }
}

#[cfg(target_os = "linux")]
pub struct DhcpKeepalive {
    stop: mpsc::Sender<()>,
    thread: Option<JoinHandle<()>>,
}

#[cfg(target_os = "linux")]
impl Drop for DhcpKeepalive {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(target_os = "linux")]
pub fn start_dhcp_keepalive() -> DhcpKeepalive {
    let (stop, receiver) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        keep_dhcp_alive_with(&RealCommandRunner, &StopWaiter { stop: receiver });
    });
    DhcpKeepalive {
        stop,
        thread: Some(thread),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn real_command_runner_success_returns_code_zero() {
        let runner = RealCommandRunner;
        let outcome = runner.run("/usr/bin/true", &[]).expect("spawn true");
        assert_eq!(outcome.code, Some(0));
        assert!(outcome.success());
    }

    #[test]
    fn real_command_runner_failure_returns_nonzero_code() {
        let runner = RealCommandRunner;
        let outcome = runner.run("/usr/bin/false", &[]).expect("spawn false");
        assert!(matches!(outcome.code, Some(c) if c != 0));
        assert!(!outcome.success());
    }

    #[test]
    fn real_command_runner_propagates_spawn_failure() {
        let runner = RealCommandRunner;
        let err = runner
            .run("/does/not/exist/binary", &[])
            .expect_err("spawn should fail");
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn real_fs_writer_writes_bytes_and_sets_mode() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("test.script");
        let path_str = path.to_str().expect("utf-8 path");

        let fs = RealFsWriter;
        fs.write(path_str, b"#!/bin/sh\necho hi\n", 0o755)
            .expect("write");

        let bytes = std::fs::read(&path).expect("read back");
        assert_eq!(bytes, b"#!/bin/sh\necho hi\n");

        let mode = std::fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode, 0o755, "executable mode bits applied");
    }

    #[test]
    fn real_fs_writer_propagates_write_failure() {
        let fs = RealFsWriter;
        let err = fs
            .write("/does/not/exist/file.script", b"x", 0o755)
            .expect_err("write should fail");
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }
}
