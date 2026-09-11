use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const POLL: Duration = Duration::from_millis(50);
/// How long a reader is given to drain a pipe after the command is gone; a grandchild that outlives it must not hold the harness.
const READER_GRACE: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq)]
pub struct Output {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
    pub duration: Duration,
    pub timed_out: bool,
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
            timed_out: false,
        })
    }

    /// A command the harness gives up on: past `budget` the child is killed, so one stalled stream costs a budget rather than the run.
    pub fn run_within(&self, args: &[&str], budget: Duration) -> Result<Output> {
        let started = Instant::now();
        let context = || format!("run {} {}", self.bin.display(), args.join(" "));
        let mut child = self
            .command(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(context)?;
        let stdout = child
            .stdout
            .take()
            .map(read_in_background)
            .unwrap_or_default();
        let stderr = child
            .stderr
            .take()
            .map(read_in_background)
            .unwrap_or_default();

        let mut timed_out = false;
        let code = loop {
            match child.try_wait().with_context(context)? {
                Some(status) => break status.code().unwrap_or(-1),
                None if started.elapsed() >= budget => {
                    timed_out = true;
                    let _ = child.kill();
                    let _ = child.wait();
                    break -1;
                }
                None => std::thread::sleep(POLL),
            }
        };

        await_reader(&stdout, READER_GRACE);
        await_reader(&stderr, READER_GRACE);
        Ok(Output {
            stdout: read_so_far(&stdout),
            stderr: read_so_far(&stderr),
            code,
            duration: started.elapsed(),
            timed_out,
        })
    }

    pub fn version(&self) -> Result<String> {
        Ok(self.run(&["--version"])?.stdout.trim().to_string())
    }
}

/// A pipe read as it arrives, so a killed command still reports what it printed even when a surviving grandchild holds the other end open.
#[derive(Clone, Default)]
struct Reader {
    text: Arc<Mutex<String>>,
    done: Arc<AtomicBool>,
}

fn read_in_background(mut pipe: impl Read + Send + 'static) -> Reader {
    let reader = Reader::default();
    let thread = reader.clone();
    std::thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        while let Ok(read) = pipe.read(&mut buffer) {
            if read == 0 {
                break;
            }
            if let Ok(mut text) = thread.text.lock() {
                text.push_str(&String::from_utf8_lossy(&buffer[..read]));
            }
        }
        thread.done.store(true, Ordering::SeqCst);
    });
    reader
}

fn await_reader(reader: &Reader, grace: Duration) {
    let deadline = Instant::now() + grace;
    while !reader.done.load(Ordering::SeqCst) && Instant::now() < deadline {
        std::thread::sleep(POLL);
    }
}

fn read_so_far(reader: &Reader) -> String {
    reader
        .text
        .lock()
        .map(|text| text.clone())
        .unwrap_or_default()
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

pub fn write_sandbox(path: &Path, name: &str, image: &str, destinations: &[String]) -> Result<()> {
    let document = sandbox_document(name, image, destinations);
    std::fs::write(path, document).with_context(|| format!("write {}", path.display()))
}

/// A raw stream to a fixture is redirected to the guest's own transparent proxy, so a destination no `egress.tcp` entry decides is held rather than carried.
pub fn sandbox_document(name: &str, image: &str, destinations: &[String]) -> String {
    let mut document = format!(
        "apiVersion: lns.run/v1\nkind: sandbox\nname: {name}\nspec:\n  image: {image}\n  egress:\n    http:\n      - match: \"*\"\n        verdict: allow\n"
    );
    if destinations.is_empty() {
        return document;
    }
    document.push_str("    tcp:\n");
    for destination in destinations {
        document.push_str(&format!(
            "      - match: \"{destination}\"\n        verdict: allow\n        description: parity host fixture\n"
        ));
    }
    document
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

    #[derive(serde::Deserialize)]
    struct Document {
        spec: DocumentSpec,
    }

    #[derive(serde::Deserialize)]
    struct DocumentSpec {
        image: String,
        egress: lns_policy::Egress,
    }

    fn parse(document: &str) -> Document {
        serde_yaml::from_str(document).expect("the product's own parser reads this definition")
    }

    #[test]
    fn the_generated_definition_allows_the_egress_a_case_needs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lns.yaml");
        let destinations = ["192.168.1.49:47200".to_string()];
        write_sandbox(
            &path,
            "parity-upload",
            "docker.io/library/alpine:3.20",
            &destinations,
        )
        .unwrap();
        let document = std::fs::read_to_string(&path).unwrap();

        assert!(document.contains("kind: sandbox"), "{document}");
        assert!(document.contains("name: parity-upload"), "{document}");
        let parsed = parse(&document);
        assert_eq!(parsed.spec.image, "docker.io/library/alpine:3.20");
        assert_eq!(parsed.spec.egress.http[0].match_pattern, "*");
        assert_eq!(
            parsed.spec.egress.http[0].verdict,
            lns_policy::Verdict::Allow
        );
    }

    #[test]
    fn every_fixture_destination_is_decided_by_a_raw_tcp_rule() {
        let destinations = [
            "192.168.1.49:47200".to_string(),
            "192.168.1.49:47207".to_string(),
        ];
        let document = sandbox_document("parity-alpine", "alpine:3.20", &destinations);
        let egress = parse(&document).spec.egress;

        let written: Vec<&str> = egress
            .tcp
            .iter()
            .map(|rule| rule.match_pattern.as_str())
            .collect();
        assert_eq!(written, vec!["192.168.1.49:47200", "192.168.1.49:47207"]);
        for rule in &egress.tcp {
            assert_eq!(rule.verdict, lns_policy::Verdict::Allow);
            rule.validate().expect("the guest gate accepts this rule");
        }
    }

    #[test]
    fn a_definition_with_no_fixture_destination_carries_no_empty_tcp_table() {
        let document = sandbox_document("parity-alpine", "alpine:3.20", &[]);
        assert!(!document.contains("tcp:"), "{document}");
        assert!(parse(&document).spec.egress.tcp.is_empty());
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
        assert!(!output.timed_out);
        assert_eq!(output.combined(), "out\nerr\n");
    }

    #[test]
    fn a_command_past_its_budget_is_killed_and_says_so() {
        let lns = Lns::new(PathBuf::from("/bin/sh"), BTreeMap::new());
        let output = lns
            .run_within(
                &["-c", "echo started; sleep 30"],
                Duration::from_millis(300),
            )
            .unwrap();

        assert!(output.timed_out);
        assert!(output.duration < Duration::from_secs(10), "{output:?}");
        assert!(output.stdout.contains("started"), "{output:?}");
    }

    #[test]
    fn a_command_that_finishes_inside_its_budget_reports_what_it_printed() {
        let lns = Lns::new(PathBuf::from("/bin/sh"), BTreeMap::new());
        let output = lns
            .run_within(
                &["-c", "echo out; echo err >&2; exit 7"],
                Duration::from_secs(30),
            )
            .unwrap();

        assert!(!output.timed_out);
        assert_eq!(output.code, 7);
        assert_eq!(output.combined(), "out\nerr\n");
    }

    #[test]
    fn a_binary_that_is_not_there_names_itself_in_the_error() {
        let lns = Lns::new(PathBuf::from("/nowhere/lns"), BTreeMap::new());
        let err = format!("{:#}", lns.run(&["ps"]).unwrap_err());
        assert!(err.contains("/nowhere/lns"), "{err}");

        let err = format!(
            "{:#}",
            lns.run_within(&["ps"], Duration::from_secs(1)).unwrap_err()
        );
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
