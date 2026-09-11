use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
    pub duration: Duration,
}

impl Output {
    pub fn combined(&self) -> String {
        format!("{}{}", self.stdout, self.stderr)
    }

    pub fn ok(&self) -> bool {
        self.code == 0
    }
}

#[derive(Debug, Clone)]
pub struct Lns {
    pub bin: PathBuf,
    pub env: BTreeMap<String, String>,
}

impl Lns {
    pub fn new(bin: PathBuf, env: BTreeMap<String, String>) -> Self {
        Self { bin, env }
    }

    pub fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(&self.bin);
        command.args(args);
        for (key, value) in &self.env {
            command.env(key, value);
        }
        command
    }

    pub fn run(&self, args: &[&str]) -> Result<Output> {
        let started = Instant::now();
        let output = self
            .command(args)
            .output()
            .with_context(|| format!("run {} {}", self.bin.display(), args.join(" ")))?;
        Ok(Output {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            code: output.status.code().unwrap_or(-1),
            duration: started.elapsed(),
        })
    }

    pub fn version(&self) -> Result<String> {
        Ok(self.run(&["--version"])?.stdout.trim().to_string())
    }
}

pub fn parse_status_pid(stdout: &str) -> Option<u32> {
    stdout
        .lines()
        .filter_map(|line| line.trim().strip_prefix("PID:"))
        .filter_map(|rest| rest.trim().parse().ok())
        .next()
}

pub fn pid_alive(pid: u32) -> bool {
    // SAFETY: signal 0 only probes whether the pid exists; it delivers nothing.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

pub fn wait_until(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if condition() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

pub fn write_sandbox(path: &Path, name: &str, image: &str) -> Result<()> {
    let document = format!(
        "apiVersion: lns.run/v1\nkind: sandbox\nname: {name}\nspec:\n  image: {image}\n  egress:\n    http:\n      - match: \"*\"\n        verdict: allow\n"
    );
    std::fs::write(path, document).with_context(|| format!("write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_service_pid_is_read_from_the_status_report() {
        let status = "LNS is running.\n  PID: 4242\n  Uptime: 12s\n  Version: 0.25.0\n";
        assert_eq!(parse_status_pid(status), Some(4242));
        assert_eq!(parse_status_pid("LNS is not running.\n"), None);
        assert_eq!(parse_status_pid("  PID: not-a-number\n"), None);
    }

    #[test]
    fn a_live_process_is_told_apart_from_one_that_is_gone() {
        assert!(pid_alive(std::process::id()));
        assert!(!pid_alive(0x7fff_fffe));
    }

    #[test]
    fn waiting_returns_as_soon_as_the_condition_holds_and_gives_up_at_the_timeout() {
        let mut calls = 0;
        assert!(wait_until(Duration::from_secs(5), || {
            calls += 1;
            calls >= 2
        }));
        assert!(!wait_until(Duration::from_millis(200), || false));
    }

    #[test]
    fn the_generated_definition_allows_the_egress_a_case_needs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lns.yaml");
        write_sandbox(&path, "parity-upload", "docker.io/library/alpine:3.20").unwrap();
        let document = std::fs::read_to_string(&path).unwrap();

        assert!(document.contains("kind: sandbox"), "{document}");
        assert!(document.contains("name: parity-upload"), "{document}");
        assert!(
            document.contains("image: docker.io/library/alpine:3.20"),
            "{document}"
        );
        assert!(document.contains("match: \"*\""), "{document}");
    }

    #[test]
    fn a_command_carries_the_backend_environment_the_run_was_started_with() {
        let lns = Lns::new(
            PathBuf::from("/bin/echo"),
            BTreeMap::from([("LNS_NETDEV".to_string(), "netstack".to_string())]),
        );
        let command = lns.command(&["ps"]);
        let envs: Vec<_> = command.get_envs().collect();

        assert_eq!(
            envs,
            vec![(
                std::ffi::OsStr::new("LNS_NETDEV"),
                Some(std::ffi::OsStr::new("netstack"))
            )]
        );
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec![std::ffi::OsStr::new("ps")]
        );
    }

    #[test]
    fn a_command_reports_its_output_its_exit_code_and_how_long_it_took() {
        let lns = Lns::new(PathBuf::from("/bin/sh"), BTreeMap::new());
        let output = lns.run(&["-c", "echo out; echo err >&2; exit 3"]).unwrap();

        assert_eq!(output.code, 3);
        assert!(!output.ok());
        assert_eq!(output.combined(), "out\nerr\n");
    }

    #[test]
    fn a_binary_that_is_not_there_names_itself_in_the_error() {
        let lns = Lns::new(PathBuf::from("/nowhere/lns"), BTreeMap::new());
        let err = format!("{:#}", lns.run(&["ps"]).unwrap_err());

        assert!(err.contains("/nowhere/lns"), "{err}");
    }

    #[test]
    fn the_version_of_the_binary_under_test_is_recorded_verbatim() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("lns");
        std::fs::write(
            &fake,
            "#!/bin/sh\necho 'lns 0.25.0 (aarch64-apple-darwin)'\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

        let lns = Lns::new(fake, BTreeMap::new());
        assert_eq!(lns.version().unwrap(), "lns 0.25.0 (aarch64-apple-darwin)");
    }
}
