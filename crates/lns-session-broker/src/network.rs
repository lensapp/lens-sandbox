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

/// Which step of the bring-up failed: only a failed lease request means the host gave no address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkStep {
    LinkLo,
    LinkEth0,
    HookScript,
    Dhcp,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkSetupError {
    pub step: NetworkStep,
    pub message: String,
}

pub fn bring_up_eth0_with(
    runner: &dyn CommandRunner,
    fs: &dyn FsWriter,
) -> Result<(), NetworkSetupError> {
    run_check(
        runner,
        NetworkStep::LinkLo,
        "ip link set lo up",
        &["ip", "link", "set", "lo", "up"],
    )?;
    run_check(
        runner,
        NetworkStep::LinkEth0,
        "ip link set eth0 up",
        &["ip", "link", "set", "eth0", "up"],
    )?;
    fs.write(UDHCPC_SCRIPT_PATH, UDHCPC_SCRIPT.as_bytes(), 0o755)
        .map_err(|e| NetworkSetupError {
            step: NetworkStep::HookScript,
            message: format!("writing {UDHCPC_SCRIPT_PATH}: {e}"),
        })?;
    run_check(
        runner,
        NetworkStep::Dhcp,
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

pub fn policy_allows_egress_with(env_get: impl Fn(&str) -> Option<String>) -> bool {
    env_get(lns_session::EGRESS_ALLOWED_ENV).as_deref() != Some("0")
}

fn refusal_for(error: NetworkSetupError) -> lns_session::BrokerExitReason {
    match error.step {
        NetworkStep::Dhcp => lns_session::BrokerExitReason::NoDhcpLease,
        _ => lns_session::BrokerExitReason::NetworkSetupFailed(error.message),
    }
}

/// A run that needs egress refuses without a network; one that allows none keeps the best-effort path.
pub fn bring_up_network_with(
    bring_up: impl FnOnce() -> Result<(), NetworkSetupError>,
    egress_allowed: bool,
) -> Result<(), lns_session::BrokerExitReason> {
    match bring_up() {
        Ok(()) => Ok(()),
        Err(error) if egress_allowed => Err(refusal_for(error)),
        Err(error) => {
            eprintln!(
                "lns-session-broker: best-effort network setup failed: {}",
                error.message
            );
            Ok(())
        }
    }
}

fn run_check(
    runner: &dyn CommandRunner,
    step: NetworkStep,
    label: &str,
    args: &[&str],
) -> Result<(), NetworkSetupError> {
    let outcome = runner.run(BUSYBOX, args).map_err(|e| NetworkSetupError {
        step,
        message: format!("spawn `{label}`: {e}"),
    })?;
    if !outcome.success() {
        return Err(NetworkSetupError {
            step,
            message: format!("`{label}` exited with {}", outcome.code.unwrap_or(-1)),
        });
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

pub const NO_LEASE_DNS_WARNING: &str = "no DHCP lease: DNS upstream falls back to 1.1.1.1";

pub fn dns_warning(dhcp_dns: &[String]) -> Option<&'static str> {
    dhcp_dns.is_empty().then_some(NO_LEASE_DNS_WARNING)
}

pub fn configure_dns_with(dhcp_dns: &[String], fs: &dyn FsWriter) -> Result<(), String> {
    if let Some(warning) = dns_warning(dhcp_dns) {
        eprintln!("lns-session-broker: WARN {warning}");
    }
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
    fn an_empty_dhcp_dns_list_warns_that_the_upstream_is_a_public_resolver() {
        assert_eq!(dns_warning(&[]), Some(NO_LEASE_DNS_WARNING));
        assert!(NO_LEASE_DNS_WARNING.contains("no DHCP lease"));
        assert!(NO_LEASE_DNS_WARNING.contains(FALLBACK_DNS[0]));
        assert_eq!(dns_warning(&["192.168.64.1".to_string()]), None);
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
    fn udhcpc_asks_seven_times_at_three_seconds_so_the_guest_waits_21_s_for_a_lease() {
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
        assert_eq!(err.step, NetworkStep::LinkLo);
        assert!(err.message.contains("ip link set lo up"), "got: {err:?}");
        assert!(err.message.contains("exited with 1"), "got: {err:?}");
        assert_eq!(runner.calls().len(), 1, "no further commands after failure");
        assert!(fs.writes().is_empty(), "script not staged on lo failure");
    }

    #[test]
    fn eth0_failure_after_lo_success() {
        let runner = FakeCommandRunner::with_outcomes(vec![ok(2), ok(0)]);
        let fs = FakeFsWriter::ok();
        let err = bring_up_eth0_with(&runner, &fs).expect_err("eth0 failed");
        assert_eq!(err.step, NetworkStep::LinkEth0);
        assert!(err.message.contains("ip link set eth0 up"), "got: {err:?}");
        assert!(err.message.contains("exited with 2"), "got: {err:?}");
        assert_eq!(runner.calls().len(), 2);
        assert!(fs.writes().is_empty(), "script not staged on eth0 failure");
    }

    #[test]
    fn spawn_io_error_surfaces_with_label() {
        let runner = FakeCommandRunner::with_outcomes(vec![Err(io::Error::other("ENOENT"))]);
        let fs = FakeFsWriter::ok();
        let err = bring_up_eth0_with(&runner, &fs).expect_err("spawn failed");
        assert_eq!(err.step, NetworkStep::LinkLo);
        assert!(err.message.contains("spawn"), "got: {err:?}");
        assert!(err.message.contains("ip link set lo up"), "got: {err:?}");
        assert!(err.message.contains("ENOENT"), "got: {err:?}");
    }

    #[test]
    fn fs_write_failure_aborts_before_udhcpc() {
        let runner = FakeCommandRunner::with_outcomes(vec![ok(0), ok(0)]);
        let fs = FakeFsWriter::err(io::Error::other("EROFS"));
        let err = bring_up_eth0_with(&runner, &fs).expect_err("fs failed");
        assert_eq!(err.step, NetworkStep::HookScript);
        assert!(err.message.contains(UDHCPC_SCRIPT_PATH), "got: {err:?}");
        assert!(err.message.contains("EROFS"), "got: {err:?}");
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
        assert_eq!(
            err.step,
            NetworkStep::Dhcp,
            "only this step means the host gave no lease"
        );
        assert!(err.message.contains("udhcpc"), "got: {err:?}");
        assert!(err.message.contains("exited with 1"), "got: {err:?}");
        assert_eq!(runner.calls().len(), 3);
        assert_eq!(fs.writes().len(), 1, "script was staged before udhcpc");
    }

    #[test]
    fn signal_killed_step_reports_minus_one() {
        let runner = FakeCommandRunner::with_outcomes(vec![Ok(CommandOutcome { code: None })]);
        let fs = FakeFsWriter::ok();
        let err = bring_up_eth0_with(&runner, &fs).expect_err("signal-killed lo");
        assert!(err.message.contains("exited with -1"), "got: {err:?}");
    }

    #[test]
    fn a_guest_with_a_lease_starts_its_workload() {
        let runner = FakeCommandRunner::with_outcomes(vec![ok(0), ok(0), ok(0)]);
        let fs = FakeFsWriter::ok();
        bring_up_network_with(|| bring_up_eth0_with(&runner, &fs), true)
            .expect("a lease refuses nothing");
    }

    #[test]
    fn a_failed_lease_request_refuses_a_run_whose_policy_allows_egress() {
        let runner = FakeCommandRunner::with_outcomes(vec![ok(1), ok(0), ok(0)]);
        let fs = FakeFsWriter::ok();
        let reason = bring_up_network_with(|| bring_up_eth0_with(&runner, &fs), true)
            .expect_err("an egress workload without a lease cannot do its work");
        assert_eq!(reason, lns_session::BrokerExitReason::NoDhcpLease);
    }

    #[test]
    fn an_earlier_step_refuses_with_its_own_error_rather_than_the_dhcp_story() {
        let runner = FakeCommandRunner::with_outcomes(vec![ok(3)]);
        let fs = FakeFsWriter::ok();
        let reason = bring_up_network_with(|| bring_up_eth0_with(&runner, &fs), true)
            .expect_err("an egress workload with no interface cannot do its work");
        assert_eq!(reason.as_str(), "network_setup_failed");
        let text = reason.summary();
        assert!(text.contains("ip link set lo up"), "got: {text}");
        assert!(text.contains("exited with 3"), "got: {text}");
    }

    #[test]
    fn a_network_failure_remains_best_effort_when_policy_allows_no_egress() {
        let runner = FakeCommandRunner::with_outcomes(vec![ok(1), ok(0), ok(0)]);
        let fs = FakeFsWriter::ok();
        bring_up_network_with(|| bring_up_eth0_with(&runner, &fs), false)
            .expect("an offline workload does not need a lease");
    }

    #[test]
    fn the_broker_reads_the_host_resolved_egress_allowance() {
        assert!(policy_allows_egress_with(|_| Some("1".into())));
        assert!(!policy_allows_egress_with(|_| Some("0".into())));
    }

    #[test]
    fn a_missing_egress_marker_fails_closed_on_a_network_error() {
        assert!(policy_allows_egress_with(|_| None));
    }

    #[test]
    fn command_outcome_success_strict_zero() {
        assert!(CommandOutcome { code: Some(0) }.success());
        assert!(!CommandOutcome { code: Some(1) }.success());
        assert!(!CommandOutcome { code: None }.success());
    }
}
