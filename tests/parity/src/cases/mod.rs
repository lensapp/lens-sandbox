mod lease;
mod lifecycle;
mod loopback;
mod preflight;
mod shutdown;
mod transfer;
mod udp;

use crate::config::Images;
use crate::fixtures::source::FixtureSource;
use crate::fixtures::{Activity, ConnRecord, Role};
use crate::lns::{Lns, Output, write_sandbox};
use crate::result::CaseResult;
use anyhow::{Context, Result};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const MISSING_MARKER: &str = "PARITY_MISSING=";
pub const PREFLIGHT: &str = "fixture-reachable";

/// What one boot of a guest is given before the case that needs it is over budget.
const BOOT_ALLOWANCE: u64 = 60;
/// A 100 MB transfer, the boot that carries it included.
const TRANSFER_BUDGET: u64 = 120;
/// One lifecycle cycle group — [`lifecycle::CYCLE_GROUP`] runs of boot, reach, kill and remove.
const CYCLE_GROUP_BUDGET: u64 = 60;
/// A case that boots one guest and asks it one question.
const PROBE_BUDGET: u64 = BOOT_ALLOWANCE + 60;

pub type CaseFn = fn(&Ctx) -> CaseResult;

/// One registered case: what it is called, what it does, how long it may take, and whether a raw stream to the fixtures is what it needs.
pub struct Case {
    pub name: &'static str,
    pub run: CaseFn,
    pub budget: Duration,
    pub needs_fixture_stream: bool,
}

fn case(name: &'static str, run: CaseFn, seconds: u64, needs_fixture_stream: bool) -> Case {
    Case {
        name,
        run,
        budget: Duration::from_secs(seconds),
        needs_fixture_stream,
    }
}

pub fn all() -> Vec<Case> {
    let cycle_groups = (lifecycle::CYCLES / lifecycle::CYCLE_GROUP) as u64;
    vec![
        case(PREFLIGHT, preflight::fixture_reachable, PROBE_BUDGET, false),
        case(
            "lease-and-resolver",
            lease::lease_and_resolver,
            PROBE_BUDGET,
            false,
        ),
        case("upload-100m", transfer::upload_100m, TRANSFER_BUDGET, true),
        case(
            "download-100m",
            transfer::download_100m,
            TRANSFER_BUDGET,
            true,
        ),
        case(
            "bidirectional-100m",
            transfer::bidirectional_100m,
            TRANSFER_BUDGET,
            true,
        ),
        case(
            "guest-half-close",
            transfer::guest_half_close,
            TRANSFER_BUDGET,
            true,
        ),
        case(
            "host-half-close",
            transfer::host_half_close,
            TRANSFER_BUDGET,
            true,
        ),
        case(
            "reset-mid-transfer",
            transfer::reset_mid_transfer,
            TRANSFER_BUDGET,
            true,
        ),
        case(
            "kill-mid-transfer",
            lifecycle::kill_mid_transfer,
            BOOT_ALLOWANCE + CYCLE_GROUP_BUDGET,
            true,
        ),
        case("udp-echo", udp::udp_echo, PROBE_BUDGET, false),
        case(
            "loopback-witness",
            loopback::loopback_witness,
            PROBE_BUDGET,
            false,
        ),
        case(
            "create-destroy-20",
            lifecycle::create_destroy_20,
            CYCLE_GROUP_BUDGET * cycle_groups,
            true,
        ),
        case(
            "service-stop-with-guests",
            shutdown::service_stop_with_guests,
            BOOT_ALLOWANCE + CYCLE_GROUP_BUDGET,
            true,
        ),
    ]
}

pub fn names() -> Vec<&'static str> {
    all().into_iter().map(|case| case.name).collect()
}

pub fn needs_fixture_stream(name: &str) -> bool {
    all()
        .into_iter()
        .any(|case| case.name == name && case.needs_fixture_stream)
}

pub enum Outcome {
    Pass,
    Fail(String),
    Skip(String),
    Blocked(String),
}

pub fn timed(name: &str, body: impl FnOnce(&mut CaseResult) -> Result<Outcome>) -> CaseResult {
    let started = Instant::now();
    let mut case = CaseResult::new(name);
    let outcome = body(&mut case);
    let duration = started.elapsed().as_millis() as u64;
    let mut case = match outcome {
        Ok(Outcome::Pass) => case.pass(),
        Ok(Outcome::Fail(why)) => case.fail(why),
        Ok(Outcome::Skip(why)) => case.skip(why),
        Ok(Outcome::Blocked(why)) => case.blocked(why),
        Err(err) => case.fail(format!("{err:#}")),
    };
    case.duration_ms = duration;
    case
}

pub struct Ctx<'a> {
    pub lns: &'a Lns,
    pub fixtures: &'a FixtureSource,
    pub project: PathBuf,
    pub guest_subnet: String,
    pub expectations: BTreeMap<String, String>,
    pub service_pid: Option<u32>,
    deadline: Cell<Option<Instant>>,
    launched: RefCell<Vec<String>>,
    case_mark: Cell<u64>,
}

impl<'a> Ctx<'a> {
    pub fn prepare(
        lns: &'a Lns,
        fixtures: &'a FixtureSource,
        images: &'a Images,
        project: &Path,
        guest_subnet: String,
        expectations: BTreeMap<String, String>,
        service_pid: Option<u32>,
    ) -> Result<Self> {
        std::fs::create_dir_all(project)
            .with_context(|| format!("create {}", project.display()))?;
        let destinations = fixtures.guest_destinations();
        write_sandbox(
            &project.join("alpine.yaml"),
            "parity-alpine",
            &images.alpine,
            &destinations,
        )?;
        write_sandbox(
            &project.join("curl.yaml"),
            "parity-curl",
            &images.curl,
            &destinations,
        )?;
        Ok(Self {
            lns,
            fixtures,
            project: project.to_path_buf(),
            guest_subnet,
            expectations,
            service_pid,
            deadline: Cell::new(None),
            launched: RefCell::new(Vec::new()),
            case_mark: Cell::new(0),
        })
    }

    pub fn begin_case(&self, budget: Duration) {
        self.deadline.set(Some(Instant::now() + budget));
        self.launched.borrow_mut().clear();
        self.case_mark.set(self.conn_mark());
    }

    /// What is left of the running case's budget; a case run outside the runner gets [`PROBE_BUDGET`].
    pub fn remaining(&self) -> Duration {
        match self.deadline.get() {
            Some(deadline) => deadline.saturating_duration_since(Instant::now()),
            None => Duration::from_secs(PROBE_BUDGET),
        }
    }

    pub fn within_budget(&self, timeout: Duration) -> Duration {
        timeout.min(self.remaining())
    }

    pub fn activity(&self) -> Activity {
        self.fixtures.activity_since(self.case_mark.get())
    }

    /// Kills and removes every guest the case started, so a stalled one does not outlive its budget into the next case.
    pub fn cleanup_case(&self) {
        let launched: Vec<String> = self.launched.borrow_mut().drain(..).collect();
        for name in launched {
            let _ = self.kill(&name);
            let _ = self.remove(&name);
        }
    }

    pub fn lan(&self) -> Ipv4Addr {
        self.fixtures.bind()
    }

    pub fn port(&self, role: Role) -> u16 {
        self.fixtures.port(role)
    }

    pub fn expectation(&self, case: &str) -> Option<&str> {
        self.expectations.get(case).map(String::as_str)
    }

    fn definition(&self, image: Image) -> String {
        let file = match image {
            Image::Alpine => "alpine.yaml",
            Image::Curl => "curl.yaml",
        };
        self.project.join(file).display().to_string()
    }

    pub fn guest(&self, image: Image, name: &str, script: &str) -> Result<Output> {
        let definition = self.definition(image);
        self.launched.borrow_mut().push(name.to_string());
        self.lns.run_within(
            &[
                "run",
                "--rm",
                "--yes",
                "--quiet",
                "--name",
                name,
                "--entrypoint",
                "/bin/sh",
                "-f",
                &definition,
                "--",
                "-c",
                script,
            ],
            self.remaining(),
        )
    }

    pub fn guest_detached(&self, image: Image, name: &str, script: &str) -> Result<Output> {
        let definition = self.definition(image);
        self.launched.borrow_mut().push(name.to_string());
        self.lns.run_within(
            &[
                "run",
                "-d",
                "--yes",
                "--quiet",
                "--name",
                name,
                "--entrypoint",
                "/bin/sh",
                "-f",
                &definition,
                "--",
                "-c",
                script,
            ],
            self.remaining(),
        )
    }

    pub fn kill(&self, name: &str) -> Result<Output> {
        self.lns.run(&["kill", name])
    }

    pub fn remove(&self, name: &str) -> Result<Output> {
        self.lns.run(&["sandbox", "rm", "-f", name])
    }

    pub fn open_fds(&self) -> Option<u64> {
        self.service_pid.and_then(crate::sample::open_fds)
    }

    pub fn conn_mark(&self) -> u64 {
        self.fixtures
            .report()
            .connections
            .last()
            .map(|record| record.id)
            .unwrap_or(0)
    }

    pub fn new_connection(&self, role: Role, after: u64) -> Option<ConnRecord> {
        let port = self.port(role);
        self.fixtures
            .report()
            .connections
            .into_iter()
            .find(|record| record.port == port && record.id > after)
    }

    pub fn await_connection(
        &self,
        role: Role,
        after: u64,
        timeout: Duration,
    ) -> Option<ConnRecord> {
        poll(self.within_budget(timeout), || {
            self.new_connection(role, after)
        })
    }

    pub fn await_closed(&self, role: Role, after: u64, timeout: Duration) -> Option<ConnRecord> {
        poll(self.within_budget(timeout), || {
            self.new_connection(role, after)
                .filter(|record| record.closed_ms.is_some())
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Image {
    Alpine,
    Curl,
}

pub fn poll<T>(timeout: Duration, mut probe: impl FnMut() -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(value) = probe() {
            return Some(value);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

pub fn requires(commands: &[&str]) -> String {
    let list = commands.join(" ");
    format!(
        "for c in {list}; do command -v $c >/dev/null 2>&1 || {{ echo \"{MISSING_MARKER}$c\"; exit 42; }}; done\n"
    )
}

pub fn missing_command(output: &Output) -> Option<String> {
    output
        .combined()
        .lines()
        .find_map(|line| line.trim().strip_prefix(MISSING_MARKER))
        .map(|command| format!("the guest image has no `{command}`"))
}

pub fn field(output: &str, key: &str) -> Option<String> {
    output
        .split_whitespace()
        .find_map(|token| token.strip_prefix(key))
        .map(str::to_string)
}

pub fn parse_dd_bytes(output: &str) -> Option<u64> {
    output
        .lines()
        .find(|line| line.contains(" bytes"))
        .and_then(|line| line.split_whitespace().next())
        .and_then(|token| token.parse().ok())
}

pub fn throughput_mib_s(bytes: u64, record: &ConnRecord) -> Option<f64> {
    let closed = record.closed_ms?;
    let millis = closed.saturating_sub(record.opened_ms);
    if millis == 0 {
        return None;
    }
    Some((bytes as f64 / (1024.0 * 1024.0)) / (millis as f64 / 1000.0))
}

pub fn record_transfer(case: &mut CaseResult, prefix: &str, record: &ConnRecord) {
    case.record(&format!("{prefix}_bytes_in"), record.bytes_in);
    case.record(&format!("{prefix}_bytes_out"), record.bytes_out);
    case.record(&format!("{prefix}_saw_eof"), record.saw_eof);
    if let Some(sha) = &record.sha_in {
        case.record(&format!("{prefix}_sha_in"), sha.clone());
    }
    if let Some(sha) = &record.sha_out {
        case.record(&format!("{prefix}_sha_out"), sha.clone());
    }
    if let Some(error) = &record.error {
        case.record(&format!("{prefix}_error"), error.clone());
    }
}

/// What the fixtures saw while this case ran, recorded whatever the verdict, so a stall is diagnosable from the result alone.
pub fn record_activity(case: &mut CaseResult, activity: Activity) {
    case.record("fixture_seen_connections", activity.connections);
    case.record("fixture_seen_bytes_in", activity.bytes_in);
    case.record("fixture_seen_bytes_out", activity.bytes_out);
}

pub fn guest_output(case: &mut CaseResult, output: &Output) {
    case.record("guest_exit_code", output.code as i64);
    if output.timed_out {
        case.record("guest_timed_out", true);
    }
    let combined = output.combined();
    let tail: String = combined.chars().rev().take(400).collect::<String>();
    case.record("guest_output_tail", tail.chars().rev().collect::<String>());
}

#[cfg(test)]
mod tests;
