#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

mod real;
#[cfg(target_os = "linux")]
pub use real::{configure_dns, set_up};

use lns_session::{BrokerExitReason, GuestNet};
use std::net::Ipv4Addr;

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
    fn exists(&self, path: &str) -> bool;
}

pub const ETH0_SYSFS_PATH: &str = "/sys/class/net/eth0";

const ETH0: &str = "eth0";

/// What the host decided about this guest's address before it booted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NetworkPlan {
    Dhcp,
    Static(GuestNet),
}

/// The address the guest committed to, for the console line an operator reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    pub address: Ipv4Addr,
    pub prefix_len: u8,
    pub gateway: Ipv4Addr,
    pub dns: Vec<String>,
}

#[derive(Debug)]
pub enum NetworkResult {
    NoInterface,
    Dhcp,
    Applied(Applied),
    BestEffortFailed(BrokerExitReason),
    Refused(BrokerExitReason),
}

impl Applied {
    pub fn report(&self) -> String {
        format!(
            "address {}/{} gateway {} dns {}",
            self.address,
            self.prefix_len,
            self.gateway,
            self.dns.join(",")
        )
    }
}

pub fn plan_from_env(
    get: impl Fn(&str) -> Option<String>,
) -> Result<NetworkPlan, BrokerExitReason> {
    match get(lns_session::GUEST_NET_ENV) {
        None => Ok(NetworkPlan::Dhcp),
        Some(value) => GuestNet::parse(&value)
            .map(NetworkPlan::Static)
            .map_err(|e| {
                BrokerExitReason::NetworkSetupFailed(format!(
                    "the host address plan is unreadable: {e}"
                ))
            }),
    }
}

/// An unmarked boot counts as no egress, so a guest older than the marker never refuses a run over a network it does not need.
pub fn policy_allows_egress_with(get: impl Fn(&str) -> Option<String>) -> bool {
    get(lns_session::EGRESS_ALLOWED_ENV).as_deref() == Some("1")
}

/// What the guest tells its console, and whether the workload starts at all.
#[derive(Debug)]
pub struct Narration {
    pub line: String,
    pub refusal: Option<BrokerExitReason>,
    pub dhcp_dns: bool,
}

pub fn narrate(result: NetworkResult) -> Narration {
    let (line, refusal, dhcp_dns) = match result {
        NetworkResult::NoInterface => (
            "no network device; the guest runs without egress".to_string(),
            None,
            false,
        ),
        NetworkResult::Dhcp => ("leased an address over DHCP".to_string(), None, true),
        NetworkResult::Applied(applied) => (applied.report(), None, false),
        NetworkResult::BestEffortFailed(reason) => (
            format!("best-effort network setup failed: {}", reason.summary()),
            None,
            false,
        ),
        NetworkResult::Refused(reason) => (
            format!("refusing to start the workload: {}", reason.summary()),
            Some(reason),
            false,
        ),
    };
    Narration {
        line,
        refusal,
        dhcp_dns,
    }
}

pub fn set_up_with_env(
    runner: &dyn CommandRunner,
    fs: &dyn FsWriter,
    get: impl Fn(&str) -> Option<String>,
) -> NetworkResult {
    let egress_allowed = policy_allows_egress_with(&get);
    match plan_from_env(&get) {
        Ok(plan) => set_up_network(runner, fs, plan, egress_allowed),
        Err(reason) if egress_allowed => NetworkResult::Refused(reason),
        Err(reason) => NetworkResult::BestEffortFailed(reason),
    }
}

pub fn set_up_network(
    runner: &dyn CommandRunner,
    fs: &dyn FsWriter,
    plan: NetworkPlan,
    egress_allowed: bool,
) -> NetworkResult {
    if !fs.exists(ETH0_SYSFS_PATH) {
        return NetworkResult::NoInterface;
    }
    let outcome = match plan {
        NetworkPlan::Dhcp => bring_up_eth0_with(runner, fs).map(|()| None),
        NetworkPlan::Static(net) => configure_static_with(runner, fs, &net).map(Some),
    };
    match outcome {
        Ok(Some(applied)) => NetworkResult::Applied(applied),
        Ok(None) => NetworkResult::Dhcp,
        Err(reason) if egress_allowed => NetworkResult::Refused(reason),
        Err(reason) => NetworkResult::BestEffortFailed(reason),
    }
}

/// No DHCP client runs here and no step falls back to one: a guest the host addressed either takes that address or refuses.
fn configure_static_with(
    runner: &dyn CommandRunner,
    fs: &dyn FsWriter,
    net: &GuestNet,
) -> Result<Applied, BrokerExitReason> {
    link_up(runner)?;
    let address = claim_address(runner, net)?;
    let cidr = format!("{address}/{}", net.prefix_len);
    let gateway = net.gateway.to_string();
    run_check(
        runner,
        "ip addr add",
        &["ip", "addr", "add", &cidr, "dev", ETH0],
    )?;
    run_check(
        runner,
        "ip route replace default",
        &[
            "ip", "route", "replace", "default", "via", &gateway, "dev", ETH0,
        ],
    )?;
    let address_text = address.to_string();
    run_check(
        runner,
        "arping -U",
        &["arping", "-U", "-c", "1", "-I", ETH0, &address_text],
    )?;
    confirm_gateway(runner, &gateway, &address_text)?;
    let dns = select_dns(&net.dns.iter().map(Ipv4Addr::to_string).collect::<Vec<_>>());
    configure_dns_with(&dns, fs).map_err(BrokerExitReason::NetworkSetupFailed)?;
    Ok(Applied {
        address,
        prefix_len: net.prefix_len,
        gateway: net.gateway,
        dns,
    })
}

/// busybox `arping -D` exits 1 when the address answers, so a reply eliminates a candidate instead of failing the boot.
fn claim_address(runner: &dyn CommandRunner, net: &GuestNet) -> Result<Ipv4Addr, BrokerExitReason> {
    for candidate in &net.candidates {
        let text = candidate.to_string();
        let outcome = runner
            .run(
                BUSYBOX,
                &["arping", "-D", "-c", "2", "-w", "3", "-I", ETH0, &text],
            )
            .map_err(|e| {
                BrokerExitReason::NetworkSetupFailed(format!("spawn `arping -D {text}`: {e}"))
            })?;
        match outcome.code {
            Some(0) => return Ok(*candidate),
            Some(1) => continue,
            other => {
                return Err(BrokerExitReason::NetworkSetupFailed(format!(
                    "`arping -D {text}` exited with {}",
                    other.unwrap_or(-1)
                )));
            }
        }
    }
    Err(BrokerExitReason::NoStaticAddress {
        offered: net.candidates.iter().map(Ipv4Addr::to_string).collect(),
    })
}

/// This replaces the readiness signal a lease used to give: an address with no route out is not a working guest.
fn confirm_gateway(
    runner: &dyn CommandRunner,
    gateway: &str,
    address: &str,
) -> Result<(), BrokerExitReason> {
    let outcome = runner
        .run(
            BUSYBOX,
            &["arping", "-c", "2", "-w", "3", "-I", ETH0, gateway],
        )
        .map_err(|e| {
            BrokerExitReason::NetworkSetupFailed(format!("spawn `arping {gateway}`: {e}"))
        })?;
    if outcome.success() {
        return Ok(());
    }
    Err(BrokerExitReason::GatewayUnreachable {
        gateway: gateway.to_string(),
        address: address.to_string(),
    })
}

fn link_up(runner: &dyn CommandRunner) -> Result<(), BrokerExitReason> {
    run_check(
        runner,
        "ip link set lo up",
        &["ip", "link", "set", "lo", "up"],
    )?;
    run_check(
        runner,
        "ip link set eth0 up",
        &["ip", "link", "set", ETH0, "up"],
    )
}

pub fn bring_up_eth0_with(
    runner: &dyn CommandRunner,
    fs: &dyn FsWriter,
) -> Result<(), BrokerExitReason> {
    link_up(runner)?;
    fs.write(UDHCPC_SCRIPT_PATH, UDHCPC_SCRIPT.as_bytes(), 0o755)
        .map_err(|e| {
            BrokerExitReason::NetworkSetupFailed(format!("writing {UDHCPC_SCRIPT_PATH}: {e}"))
        })?;
    lease(
        runner,
        &[
            "udhcpc",
            "-i",
            "eth0",
            "-q",
            "-n",
            "-t",
            "2",
            "-T",
            "3",
            "-s",
            UDHCPC_SCRIPT_PATH,
        ],
    )
}

fn lease(runner: &dyn CommandRunner, args: &[&str]) -> Result<(), BrokerExitReason> {
    let outcome = runner
        .run(BUSYBOX, args)
        .map_err(|e| BrokerExitReason::NetworkSetupFailed(format!("spawn `udhcpc`: {e}")))?;
    if outcome.success() {
        return Ok(());
    }
    Err(BrokerExitReason::NoDhcpLease)
}

fn run_check(
    runner: &dyn CommandRunner,
    label: &str,
    args: &[&str],
) -> Result<(), BrokerExitReason> {
    let outcome = runner
        .run(BUSYBOX, args)
        .map_err(|e| BrokerExitReason::NetworkSetupFailed(format!("spawn `{label}`: {e}")))?;
    if !outcome.success() {
        return Err(BrokerExitReason::NetworkSetupFailed(format!(
            "`{label}` exited with {}",
            outcome.code.unwrap_or(-1)
        )));
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

    pub(super) struct FakeCommandRunner {
        calls: RefCell<Vec<(String, Vec<String>)>>,
        outcomes: RefCell<Vec<io::Result<CommandOutcome>>>,
    }

    impl FakeCommandRunner {
        pub(super) fn with_outcomes(outcomes: Vec<io::Result<CommandOutcome>>) -> Self {
            Self {
                calls: RefCell::new(Vec::new()),
                outcomes: RefCell::new(outcomes),
            }
        }
        pub(super) fn calls(&self) -> Vec<(String, Vec<String>)> {
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

    pub(super) struct FakeFsWriter {
        writes: RefCell<Vec<(String, Vec<u8>, u32)>>,
        next_result: RefCell<io::Result<()>>,
        interface: bool,
    }

    impl FakeFsWriter {
        pub(super) fn ok() -> Self {
            Self {
                writes: RefCell::new(Vec::new()),
                next_result: RefCell::new(Ok(())),
                interface: true,
            }
        }
        pub(super) fn err(e: io::Error) -> Self {
            Self {
                writes: RefCell::new(Vec::new()),
                next_result: RefCell::new(Err(e)),
                interface: true,
            }
        }
        pub(super) fn missing() -> Self {
            Self {
                interface: false,
                ..Self::ok()
            }
        }
        pub(super) fn writes(&self) -> Vec<(String, Vec<u8>, u32)> {
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

        fn exists(&self, _path: &str) -> bool {
            self.interface
        }
    }

    pub(super) fn ok(code: i32) -> io::Result<CommandOutcome> {
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
                "2",
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
        let err = bring_up_eth0_with(&runner, &fs)
            .expect_err("lo failed")
            .summary();
        assert!(err.contains("ip link set lo up"), "got: {err}");
        assert!(err.contains("exited with 1"), "got: {err}");
        assert_eq!(runner.calls().len(), 1, "no further commands after failure");
        assert!(fs.writes().is_empty(), "script not staged on lo failure");
    }

    #[test]
    fn eth0_failure_after_lo_success() {
        let runner = FakeCommandRunner::with_outcomes(vec![ok(2), ok(0)]);
        let fs = FakeFsWriter::ok();
        let err = bring_up_eth0_with(&runner, &fs)
            .expect_err("eth0 failed")
            .summary();
        assert!(err.contains("ip link set eth0 up"), "got: {err}");
        assert!(err.contains("exited with 2"), "got: {err}");
        assert_eq!(runner.calls().len(), 2);
        assert!(fs.writes().is_empty(), "script not staged on eth0 failure");
    }

    #[test]
    fn spawn_io_error_surfaces_with_label() {
        let runner = FakeCommandRunner::with_outcomes(vec![Err(io::Error::other("ENOENT"))]);
        let fs = FakeFsWriter::ok();
        let err = bring_up_eth0_with(&runner, &fs)
            .expect_err("spawn failed")
            .summary();
        assert!(err.contains("spawn"), "got: {err}");
        assert!(err.contains("ip link set lo up"), "got: {err}");
        assert!(err.contains("ENOENT"), "got: {err}");
    }

    #[test]
    fn fs_write_failure_aborts_before_udhcpc() {
        let runner = FakeCommandRunner::with_outcomes(vec![ok(0), ok(0)]);
        let fs = FakeFsWriter::err(io::Error::other("EROFS"));
        let err = bring_up_eth0_with(&runner, &fs)
            .expect_err("fs failed")
            .summary();
        assert!(err.contains(UDHCPC_SCRIPT_PATH), "got: {err}");
        assert!(err.contains("EROFS"), "got: {err}");
        assert_eq!(
            runner.calls().len(),
            2,
            "lo + eth0 only; udhcpc not invoked"
        );
    }

    #[test]
    fn a_udhcpc_that_never_leases_is_its_own_reason_not_a_setup_failure() {
        let runner = FakeCommandRunner::with_outcomes(vec![ok(1), ok(0), ok(0)]);
        let fs = FakeFsWriter::ok();
        let reason = bring_up_eth0_with(&runner, &fs).expect_err("udhcpc failed");
        assert_eq!(reason, BrokerExitReason::NoDhcpLease);
        assert_eq!(runner.calls().len(), 3);
        assert_eq!(fs.writes().len(), 1, "script was staged before udhcpc");
    }

    #[test]
    fn signal_killed_step_reports_minus_one() {
        let runner = FakeCommandRunner::with_outcomes(vec![Ok(CommandOutcome { code: None })]);
        let fs = FakeFsWriter::ok();
        let err = bring_up_eth0_with(&runner, &fs)
            .expect_err("signal-killed lo")
            .summary();
        assert!(err.contains("exited with -1"), "got: {err}");
    }

    #[test]
    fn command_outcome_success_strict_zero() {
        assert!(CommandOutcome { code: Some(0) }.success());
        assert!(!CommandOutcome { code: Some(1) }.success());
        assert!(!CommandOutcome { code: None }.success());
    }
}

#[cfg(test)]
mod static_tests {
    use super::*;
    use lns_session::{BrokerExitReason, GuestNet};
    use std::net::Ipv4Addr;

    fn plan() -> GuestNet {
        GuestNet {
            candidates: vec![
                Ipv4Addr::new(192, 168, 64, 254),
                Ipv4Addr::new(192, 168, 64, 253),
            ],
            prefix_len: 24,
            gateway: Ipv4Addr::new(192, 168, 64, 1),
            dns: vec![Ipv4Addr::new(192, 168, 64, 1)],
        }
    }

    #[test]
    fn a_guest_with_no_network_device_is_left_alone_and_never_refuses() {
        let runner = tests::FakeCommandRunner::with_outcomes(vec![]);
        let fs = tests::FakeFsWriter::missing();
        let result = set_up_network(&runner, &fs, NetworkPlan::Static(plan()), true);
        assert!(matches!(result, NetworkResult::NoInterface), "{result:?}");
        assert!(
            runner.calls().is_empty(),
            "the Linux backend attaches no device, so nothing is configured and nothing fails"
        );
    }

    #[test]
    fn the_first_free_candidate_is_taken_probed_announced_and_the_gateway_confirmed() {
        let runner = tests::FakeCommandRunner::with_outcomes(vec![
            tests::ok(0), // gateway answers
            tests::ok(0), // announce
            tests::ok(0), // ip route replace
            tests::ok(0), // ip addr add
            tests::ok(0), // arping -D: no reply, address is free
            tests::ok(0), // ip link set eth0 up
            tests::ok(0), // ip link set lo up
        ]);
        let fs = tests::FakeFsWriter::ok();
        let result = set_up_network(&runner, &fs, NetworkPlan::Static(plan()), true);
        let NetworkResult::Applied(applied) = result else {
            panic!("expected a configured guest, got {result:?}");
        };
        assert_eq!(applied.address, Ipv4Addr::new(192, 168, 64, 254));
        assert_eq!(applied.gateway, Ipv4Addr::new(192, 168, 64, 1));

        let args: Vec<Vec<String>> = runner.calls().into_iter().map(|(_, a)| a).collect();
        assert_eq!(
            args,
            vec![
                vec!["ip", "link", "set", "lo", "up"],
                vec!["ip", "link", "set", "eth0", "up"],
                vec![
                    "arping",
                    "-D",
                    "-c",
                    "2",
                    "-w",
                    "3",
                    "-I",
                    "eth0",
                    "192.168.64.254"
                ],
                vec!["ip", "addr", "add", "192.168.64.254/24", "dev", "eth0"],
                vec![
                    "ip",
                    "route",
                    "replace",
                    "default",
                    "via",
                    "192.168.64.1",
                    "dev",
                    "eth0"
                ],
                vec!["arping", "-U", "-c", "1", "-I", "eth0", "192.168.64.254"],
                vec!["arping", "-c", "2", "-w", "3", "-I", "eth0", "192.168.64.1"],
            ],
        );
        assert!(
            runner.calls().iter().all(|(program, _)| program == BUSYBOX),
            "every call goes through the bundled busybox, never the image's own tools"
        );
        assert!(
            !runner
                .calls()
                .iter()
                .any(|(_, a)| a.iter().any(|arg| arg == "udhcpc")),
            "the static path never speaks DHCP"
        );

        let writes = fs.writes();
        assert_eq!(
            writes.len(),
            1,
            "resolv.conf only; no hook script is staged"
        );
        assert_eq!(writes[0].0, RESOLV_CONF_PATH);
        assert_eq!(writes[0].1, b"nameserver 192.168.64.1\n");
    }

    #[test]
    fn a_taken_candidate_is_skipped_and_the_next_one_is_used() {
        let runner = tests::FakeCommandRunner::with_outcomes(vec![
            tests::ok(0),
            tests::ok(0),
            tests::ok(0),
            tests::ok(0),
            tests::ok(0), // arping -D on .253: free
            tests::ok(1), // arping -D on .254: someone answered
            tests::ok(0),
            tests::ok(0),
        ]);
        let fs = tests::FakeFsWriter::ok();
        let NetworkResult::Applied(applied) =
            set_up_network(&runner, &fs, NetworkPlan::Static(plan()), true)
        else {
            panic!("the second candidate is usable");
        };
        assert_eq!(applied.address, Ipv4Addr::new(192, 168, 64, 253));
        assert!(
            runner
                .calls()
                .iter()
                .any(|(_, a)| a.contains(&"192.168.64.253/24".to_string())),
            "the address it configures is the one it probed"
        );
    }

    #[test]
    fn every_candidate_taken_refuses_and_names_all_of_them() {
        let runner = tests::FakeCommandRunner::with_outcomes(vec![
            tests::ok(1),
            tests::ok(1),
            tests::ok(0),
            tests::ok(0),
        ]);
        let fs = tests::FakeFsWriter::ok();
        let result = set_up_network(&runner, &fs, NetworkPlan::Static(plan()), true);
        let NetworkResult::Refused(BrokerExitReason::NoStaticAddress { offered }) = result else {
            panic!("expected a typed exhaustion refusal, got {result:?}");
        };
        assert_eq!(offered, vec!["192.168.64.254", "192.168.64.253"]);
    }

    #[test]
    fn a_silent_gateway_refuses_with_its_own_reason() {
        let runner = tests::FakeCommandRunner::with_outcomes(vec![
            tests::ok(1), // gateway never answers
            tests::ok(0),
            tests::ok(0),
            tests::ok(0),
            tests::ok(0),
            tests::ok(0),
            tests::ok(0),
        ]);
        let fs = tests::FakeFsWriter::ok();
        let result = set_up_network(&runner, &fs, NetworkPlan::Static(plan()), true);
        let NetworkResult::Refused(BrokerExitReason::GatewayUnreachable { gateway, address }) =
            result
        else {
            panic!("expected a gateway refusal, got {result:?}");
        };
        assert_eq!(gateway, "192.168.64.1");
        assert_eq!(address, "192.168.64.254");
    }

    #[test]
    fn a_failed_step_keeps_its_own_error_text() {
        let runner = tests::FakeCommandRunner::with_outcomes(vec![
            tests::ok(2), // ip addr add fails
            tests::ok(0),
            tests::ok(0),
            tests::ok(0),
        ]);
        let fs = tests::FakeFsWriter::ok();
        let result = set_up_network(&runner, &fs, NetworkPlan::Static(plan()), true);
        let NetworkResult::Refused(BrokerExitReason::NetworkSetupFailed(error)) = result else {
            panic!("expected the step's own failure, got {result:?}");
        };
        assert!(error.contains("ip addr add"), "{error}");
        assert!(error.contains("exited with 2"), "{error}");
    }

    #[test]
    fn a_run_that_allows_no_egress_logs_the_failure_and_still_starts() {
        let runner = tests::FakeCommandRunner::with_outcomes(vec![
            tests::ok(1),
            tests::ok(1),
            tests::ok(0),
            tests::ok(0),
        ]);
        let fs = tests::FakeFsWriter::ok();
        let result = set_up_network(&runner, &fs, NetworkPlan::Static(plan()), false);
        let NetworkResult::BestEffortFailed(reason) = result else {
            panic!("a workload with no egress must not be refused a boot, got {result:?}");
        };
        assert_eq!(reason.as_str(), "no_static_address");
    }

    #[test]
    fn a_guest_left_to_dhcp_still_leases_and_reads_the_stashed_servers() {
        let runner =
            tests::FakeCommandRunner::with_outcomes(vec![tests::ok(0), tests::ok(0), tests::ok(0)]);
        let fs = tests::FakeFsWriter::ok();
        let result = set_up_network(&runner, &fs, NetworkPlan::Dhcp, true);
        assert!(matches!(result, NetworkResult::Dhcp), "{result:?}");
        assert!(
            runner
                .calls()
                .iter()
                .any(|(_, a)| a.first().is_some_and(|arg| arg == "udhcpc")),
        );
    }

    #[test]
    fn a_guest_with_no_dhcp_lease_refuses_when_its_policy_allows_egress() {
        let runner =
            tests::FakeCommandRunner::with_outcomes(vec![tests::ok(1), tests::ok(0), tests::ok(0)]);
        let fs = tests::FakeFsWriter::ok();
        let result = set_up_network(&runner, &fs, NetworkPlan::Dhcp, true);
        let NetworkResult::Refused(reason) = result else {
            panic!("expected a refusal, got {result:?}");
        };
        assert_eq!(reason.as_str(), "no_dhcp_lease");
    }

    #[test]
    fn the_host_plan_decides_which_path_the_guest_takes() {
        let stash: std::collections::HashMap<&str, &str> = [(
            lns_session::GUEST_NET_ENV,
            "192.168.64.254/24|192.168.64.1|192.168.64.1",
        )]
        .into_iter()
        .collect();
        let planned = plan_from_env(|key| stash.get(key).map(|v| v.to_string()));
        assert_eq!(
            planned,
            Ok(NetworkPlan::Static(GuestNet {
                candidates: vec![Ipv4Addr::new(192, 168, 64, 254)],
                prefix_len: 24,
                gateway: Ipv4Addr::new(192, 168, 64, 1),
                dns: vec![Ipv4Addr::new(192, 168, 64, 1)],
            }))
        );
        assert_eq!(plan_from_env(|_| None), Ok(NetworkPlan::Dhcp));
    }

    #[test]
    fn a_malformed_plan_is_a_refusal_and_never_a_quiet_fall_back_to_dhcp() {
        let reason = plan_from_env(|key| {
            (key == lns_session::GUEST_NET_ENV).then(|| "192.168.64.254".to_string())
        })
        .expect_err("a plan the guest cannot read is fatal");
        assert_eq!(reason.as_str(), "network_setup_failed");
        assert!(reason.summary().contains("192.168.64.254"), "{reason:?}");
    }

    #[test]
    fn the_policy_marker_decides_whether_a_network_failure_is_fatal() {
        assert!(policy_allows_egress_with(|key| (key
            == lns_session::EGRESS_ALLOWED_ENV)
            .then(|| "1".to_string())));
        assert!(!policy_allows_egress_with(|key| (key
            == lns_session::EGRESS_ALLOWED_ENV)
            .then(|| "0".to_string())));
        assert!(
            !policy_allows_egress_with(|_| None),
            "an unmarked boot is treated as no egress, so an old marker never refuses a run by accident"
        );
    }

    #[test]
    fn the_environment_carries_both_the_plan_and_whether_a_failure_is_fatal() {
        let runner = tests::FakeCommandRunner::with_outcomes(vec![
            tests::ok(0),
            tests::ok(0),
            tests::ok(0),
            tests::ok(0),
            tests::ok(0),
            tests::ok(0),
            tests::ok(0),
        ]);
        let fs = tests::FakeFsWriter::ok();
        let result = set_up_with_env(&runner, &fs, |key| match key {
            k if k == lns_session::GUEST_NET_ENV => {
                Some("192.168.64.254/24|192.168.64.1|192.168.64.1".to_string())
            }
            k if k == lns_session::EGRESS_ALLOWED_ENV => Some("1".to_string()),
            _ => None,
        });
        assert!(matches!(result, NetworkResult::Applied(_)), "{result:?}");
    }

    #[test]
    fn an_unreadable_plan_is_only_fatal_for_a_run_that_wants_egress() {
        let runner = tests::FakeCommandRunner::with_outcomes(vec![]);
        let fs = tests::FakeFsWriter::ok();
        let broken = |egress: &'static str| {
            move |key: &str| match key {
                k if k == lns_session::GUEST_NET_ENV => Some("nonsense".to_string()),
                k if k == lns_session::EGRESS_ALLOWED_ENV => Some(egress.to_string()),
                _ => None,
            }
        };
        assert!(matches!(
            set_up_with_env(&runner, &fs, broken("1")),
            NetworkResult::Refused(_)
        ));
        assert!(matches!(
            set_up_with_env(&runner, &fs, broken("0")),
            NetworkResult::BestEffortFailed(_)
        ));
    }

    #[test]
    fn each_outcome_gets_one_console_line_and_only_a_refusal_stops_the_workload() {
        let applied = narrate(NetworkResult::Applied(Applied {
            address: Ipv4Addr::new(192, 168, 64, 254),
            prefix_len: 24,
            gateway: Ipv4Addr::new(192, 168, 64, 1),
            dns: vec!["192.168.64.1".into()],
        }));
        assert!(applied.line.contains("192.168.64.254/24"), "{applied:?}");
        assert!(applied.refusal.is_none());
        assert!(
            !applied.dhcp_dns,
            "the static path already wrote resolv.conf from the host's servers"
        );

        let leased = narrate(NetworkResult::Dhcp);
        assert!(leased.dhcp_dns, "only a lease stashes servers to read back");
        assert!(leased.refusal.is_none());

        let absent = narrate(NetworkResult::NoInterface);
        assert!(
            absent.refusal.is_none(),
            "a guest with no device still runs"
        );
        assert!(!absent.dhcp_dns);
        assert!(absent.line.contains("no network device"), "{absent:?}");

        let best_effort = narrate(NetworkResult::BestEffortFailed(
            BrokerExitReason::NoDhcpLease,
        ));
        assert!(best_effort.refusal.is_none());
        assert!(best_effort.line.contains("best-effort"), "{best_effort:?}");

        let refused = narrate(NetworkResult::Refused(BrokerExitReason::NoDhcpLease));
        assert_eq!(refused.refusal, Some(BrokerExitReason::NoDhcpLease));
        assert!(
            refused
                .line
                .contains(&BrokerExitReason::NoDhcpLease.summary()),
            "the console log carries the cause even when the host renders it too: {refused:?}"
        );
    }
}
