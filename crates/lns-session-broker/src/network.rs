#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

mod real;
#[cfg(target_os = "linux")]
pub use real::{bring_up_eth0, configure_dns};

pub const BUSYBOX: &str = "/.lens/guest-tools/bin/busybox";
pub const UDHCPC_SCRIPT_PATH: &str = "/tmp/lns-udhcpc.script";

pub const DHCP_DNS_PATH: &str = "/.lens/dhcp-dns";
pub const RESOLV_CONF_PATH: &str = "/etc/resolv.conf";

pub const FALLBACK_DNS: &[&str] = &["1.1.1.1", "8.8.8.8"];

pub const UDHCPC_SCRIPT: &str = r#"#!/.lens/guest-tools/bin/busybox sh
BB=/.lens/guest-tools/bin/busybox
case "$1" in
deconfig)
    "$BB" ip addr flush dev "$interface" 2>/dev/null
    ;;
bound|renew)
    "$BB" ip addr flush dev "$interface" 2>/dev/null
    "$BB" ip addr add "${ip}/${mask:-24}" dev "$interface"
    if [ -n "$router" ]; then
        "$BB" ip route add default via "$router" dev "$interface" 2>/dev/null || true
    fi
    "$BB" mkdir -p /.lens
    : > /.lens/dhcp-dns
    if [ -n "$dns" ]; then
        for d in $dns; do
            echo "$d" >> /.lens/dhcp-dns
        done
    fi
    ;;
esac
"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandOutcome {
    pub code: Option<i32>,
}

impl CommandOutcome {
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }
}

pub trait CommandRunner {
    fn run(&self, program: &str, args: &[&str]) -> std::io::Result<CommandOutcome>;
}

pub trait FsWriter {
    fn write(&self, path: &str, contents: &[u8], mode: u32) -> std::io::Result<()>;
}

pub fn bring_up_eth0_with(runner: &dyn CommandRunner, fs: &dyn FsWriter) -> Result<(), String> {
    run_check(
        runner,
        "ip link set lo up",
        &["ip", "link", "set", "lo", "up"],
    )?;
    run_check(
        runner,
        "ip link set eth0 up",
        &["ip", "link", "set", "eth0", "up"],
    )?;
    fs.write(UDHCPC_SCRIPT_PATH, UDHCPC_SCRIPT.as_bytes(), 0o755)
        .map_err(|e| format!("writing {UDHCPC_SCRIPT_PATH}: {e}"))?;
    run_check(
        runner,
        "udhcpc",
        &[
            "udhcpc",
            "-i",
            "eth0",
            "-q",
            "-n",
            "-t",
            "7",
            "-T",
            "3",
            "-s",
            UDHCPC_SCRIPT_PATH,
        ],
    )?;
    Ok(())
}

fn run_check(runner: &dyn CommandRunner, label: &str, args: &[&str]) -> Result<(), String> {
    let outcome = runner
        .run(BUSYBOX, args)
        .map_err(|e| format!("spawn `{label}`: {e}"))?;
    if !outcome.success() {
        return Err(format!(
            "`{label}` exited with {}",
            outcome.code.unwrap_or(-1)
        ));
    }
    Ok(())
}

pub fn parse_dhcp_dns(contents: &str) -> Vec<String> {
    contents.split_whitespace().map(str::to_string).collect()
}

pub fn select_dns(dhcp_dns: &[String]) -> Vec<String> {
    if !dhcp_dns.is_empty() {
        dhcp_dns.to_vec()
    } else {
        FALLBACK_DNS.iter().map(|s| s.to_string()).collect()
    }
}

pub fn resolv_conf_contents(servers: &[String]) -> String {
    let mut out = String::new();
    for srv in servers {
        out.push_str("nameserver ");
        out.push_str(srv);
        out.push('\n');
    }
    out
}

pub fn configure_dns_with(dhcp_dns: &[String], fs: &dyn FsWriter) -> Result<(), String> {
    let servers = select_dns(dhcp_dns);
    let contents = resolv_conf_contents(&servers);
    fs.write(RESOLV_CONF_PATH, contents.as_bytes(), 0o644)
        .map_err(|e| format!("writing {RESOLV_CONF_PATH}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::io;

    #[test]
    fn udhcpc_script_has_expected_actions() {
        for action in ["deconfig)", "bound|renew)"] {
            assert!(
                UDHCPC_SCRIPT.contains(action),
                "script missing action arm: {action}"
            );
        }
        assert!(
            UDHCPC_SCRIPT.starts_with(&format!("#!{BUSYBOX} sh\n")),
            "script must use the bundled busybox as its interpreter so it doesn't depend on the image's /bin/sh"
        );
        for needle in [
            "$interface",
            "${ip}",
            "\"$BB\" ip route add default",
            "/.lens/dhcp-dns",
            "echo \"$d\"",
        ] {
            assert!(
                UDHCPC_SCRIPT.contains(needle),
                "script missing token: {needle}"
            );
        }
        assert!(
            !UDHCPC_SCRIPT.contains("nameserver"),
            "the udhcpc hook must NOT write resolv.conf directly; the broker is the single writer"
        );
        assert!(
            !UDHCPC_SCRIPT.contains("\nip "),
            "every `ip` invocation must go through $BB so the script doesn't depend on iproute2 in the image"
        );
    }

    #[test]
    fn select_dns_prefers_dhcp_then_fallback() {
        let dhcp = vec!["192.168.64.1".to_string()];
        assert_eq!(select_dns(&dhcp), vec!["192.168.64.1"]);
        assert_eq!(
            select_dns(&[]),
            FALLBACK_DNS
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn parse_dhcp_dns_splits_on_whitespace_and_empty_is_none() {
        assert!(parse_dhcp_dns("").is_empty());
        assert!(parse_dhcp_dns("   \n  ").is_empty());
        assert_eq!(
            parse_dhcp_dns("192.168.64.1\n1.1.1.1\n"),
            vec!["192.168.64.1", "1.1.1.1"]
        );
    }

    #[test]
    fn resolv_conf_contents_one_line_per_server() {
        assert_eq!(
            resolv_conf_contents(&["1.1.1.1".to_string(), "8.8.8.8".to_string()]),
            "nameserver 1.1.1.1\nnameserver 8.8.8.8\n"
        );
        assert_eq!(resolv_conf_contents(&[]), "");
    }

    #[test]
    fn configure_dns_with_writes_resolv_conf_by_precedence() {
        let fs = FakeFsWriter::ok();
        configure_dns_with(&["192.168.64.1".to_string()], &fs).unwrap();
        let w = fs.writes();
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].0, RESOLV_CONF_PATH);
        assert_eq!(w[0].1, b"nameserver 192.168.64.1\n");
        assert_eq!(w[0].2, 0o644);

        let fs = FakeFsWriter::ok();
        configure_dns_with(&[], &fs).unwrap();
        assert_eq!(
            fs.writes()[0].1,
            b"nameserver 1.1.1.1\nnameserver 8.8.8.8\n"
        );
    }

    #[test]
    fn configure_dns_with_surfaces_write_failure() {
        let fs = FakeFsWriter::err(std::io::Error::other("EROFS"));
        let err = configure_dns_with(&[], &fs).unwrap_err();
        assert!(err.contains(RESOLV_CONF_PATH), "got: {err}");
        assert!(err.contains("EROFS"), "got: {err}");
    }

    struct FakeCommandRunner {
        calls: RefCell<Vec<(String, Vec<String>)>>,
        outcomes: RefCell<Vec<io::Result<CommandOutcome>>>,
    }

    impl FakeCommandRunner {
        fn with_outcomes(outcomes: Vec<io::Result<CommandOutcome>>) -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                outcomes: RefCell::new(outcomes),
            }
        }
        fn calls(&self) -> Vec<(String, Vec<String>)> {
            self.calls.borrow().clone()
        }
    }

    impl CommandRunner for FakeCommandRunner {
        fn run(&self, program: &str, args: &[&str]) -> io::Result<CommandOutcome> {
            self.calls
                .borrow_mut()
                .push((program.into(), args.iter().map(|s| s.to_string()).collect()));
            self.outcomes
                .borrow_mut()
                .pop()
                .ok_or_else(|| io::Error::other("FakeCommandRunner: no more outcomes"))?
        }
    }

    struct FakeFsWriter {
        writes: RefCell<Vec<(String, Vec<u8>, u32)>>,
        next_result: RefCell<io::Result<()>>,
    }

    impl FakeFsWriter {
        fn ok() -> Self {
            Self {
                writes: RefCell::new(Vec::new()),
                next_result: RefCell::new(Ok(())),
            }
        }
        fn err(e: io::Error) -> Self {
            Self {
                writes: RefCell::new(Vec::new()),
                next_result: RefCell::new(Err(e)),
            }
        }
        fn writes(&self) -> Vec<(String, Vec<u8>, u32)> {
            self.writes.borrow().clone()
        }
    }

    impl FsWriter for FakeFsWriter {
        fn write(&self, path: &str, contents: &[u8], mode: u32) -> io::Result<()> {
            self.writes
                .borrow_mut()
                .push((path.into(), contents.to_vec(), mode));
            std::mem::replace(&mut *self.next_result.borrow_mut(), Ok(()))
        }
    }

    fn ok(code: i32) -> io::Result<CommandOutcome> {
        Ok(CommandOutcome { code: Some(code) })
    }

    #[test]
    fn bring_up_runs_lo_then_eth0_then_script_then_udhcpc() {
        let runner = FakeCommandRunner::with_outcomes(vec![ok(0), ok(0), ok(0)]);
        let fs = FakeFsWriter::ok();
        let result = bring_up_eth0_with(&runner, &fs);
        assert!(result.is_ok(), "happy path: {result:?}");

        let calls = runner.calls();
        assert_eq!(calls.len(), 3, "expected 3 subprocess invocations");
        assert_eq!(calls[0].0, BUSYBOX);
        assert_eq!(calls[0].1, vec!["ip", "link", "set", "lo", "up"]);
        assert_eq!(calls[1].1, vec!["ip", "link", "set", "eth0", "up"]);
        assert_eq!(
            calls[2].1,
            vec![
                "udhcpc",
                "-i",
                "eth0",
                "-q",
                "-n",
                "-t",
                "7",
                "-T",
                "3",
                "-s",
                UDHCPC_SCRIPT_PATH,
            ],
        );

        let writes = fs.writes();
        assert_eq!(writes.len(), 1);
        assert_eq!(writes[0].0, UDHCPC_SCRIPT_PATH);
        assert_eq!(writes[0].1, UDHCPC_SCRIPT.as_bytes());
        assert_eq!(writes[0].2, 0o755);
    }

    #[test]
    fn lo_failure_aborts_pipeline() {
        let runner = FakeCommandRunner::with_outcomes(vec![ok(1)]);
        let fs = FakeFsWriter::ok();
        let err = bring_up_eth0_with(&runner, &fs).expect_err("lo failed");
        assert!(err.contains("ip link set lo up"), "got: {err}");
        assert!(err.contains("exited with 1"), "got: {err}");
        assert_eq!(runner.calls().len(), 1, "no further commands after failure");
        assert!(fs.writes().is_empty(), "script not staged on lo failure");
    }

    #[test]
    fn eth0_failure_after_lo_success() {
        let runner = FakeCommandRunner::with_outcomes(vec![ok(2), ok(0)]);
        let fs = FakeFsWriter::ok();
        let err = bring_up_eth0_with(&runner, &fs).expect_err("eth0 failed");
        assert!(err.contains("ip link set eth0 up"), "got: {err}");
        assert!(err.contains("exited with 2"), "got: {err}");
        assert_eq!(runner.calls().len(), 2);
        assert!(fs.writes().is_empty(), "script not staged on eth0 failure");
    }

    #[test]
    fn spawn_io_error_surfaces_with_label() {
        let runner = FakeCommandRunner::with_outcomes(vec![Err(io::Error::other("ENOENT"))]);
        let fs = FakeFsWriter::ok();
        let err = bring_up_eth0_with(&runner, &fs).expect_err("spawn failed");
        assert!(err.contains("spawn"), "got: {err}");
        assert!(err.contains("ip link set lo up"), "got: {err}");
        assert!(err.contains("ENOENT"), "got: {err}");
    }

    #[test]
    fn fs_write_failure_aborts_before_udhcpc() {
        let runner = FakeCommandRunner::with_outcomes(vec![ok(0), ok(0)]);
        let fs = FakeFsWriter::err(io::Error::other("EROFS"));
        let err = bring_up_eth0_with(&runner, &fs).expect_err("fs failed");
        assert!(err.contains(UDHCPC_SCRIPT_PATH), "got: {err}");
        assert!(err.contains("EROFS"), "got: {err}");
        assert_eq!(
            runner.calls().len(),
            2,
            "lo + eth0 only; udhcpc not invoked"
        );
    }

    #[test]
    fn udhcpc_failure_after_setup_success() {
        let runner = FakeCommandRunner::with_outcomes(vec![ok(1), ok(0), ok(0)]);
        let fs = FakeFsWriter::ok();
        let err = bring_up_eth0_with(&runner, &fs).expect_err("udhcpc failed");
        assert!(err.contains("udhcpc"), "got: {err}");
        assert!(err.contains("exited with 1"), "got: {err}");
        assert_eq!(runner.calls().len(), 3);
        assert_eq!(fs.writes().len(), 1, "script was staged before udhcpc");
    }

    #[test]
    fn signal_killed_step_reports_minus_one() {
        let runner = FakeCommandRunner::with_outcomes(vec![Ok(CommandOutcome { code: None })]);
        let fs = FakeFsWriter::ok();
        let err = bring_up_eth0_with(&runner, &fs).expect_err("signal-killed lo");
        assert!(err.contains("exited with -1"), "got: {err}");
    }

    #[test]
    fn command_outcome_success_strict_zero() {
        assert!(CommandOutcome { code: Some(0) }.success());
        assert!(!CommandOutcome { code: Some(1) }.success());
        assert!(!CommandOutcome { code: None }.success());
    }
}
