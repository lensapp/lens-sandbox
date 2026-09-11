mod lease;
mod lifecycle;
mod loopback;
mod shutdown;
mod transfer;
mod udp;

use crate::config::Images;
use crate::fixtures::{ConnRecord, Fixtures, Role};
use crate::lns::{Lns, Output, write_sandbox};
use crate::result::CaseResult;
use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const MISSING_MARKER: &str = "PARITY_MISSING=";

pub type CaseFn = fn(&Ctx) -> CaseResult;

pub fn all() -> Vec<(&'static str, CaseFn)> {
    vec![
        ("lease-and-resolver", lease::lease_and_resolver as CaseFn),
        ("upload-100m", transfer::upload_100m),
        ("download-100m", transfer::download_100m),
        ("bidirectional-100m", transfer::bidirectional_100m),
        ("guest-half-close", transfer::guest_half_close),
        ("host-half-close", transfer::host_half_close),
        ("reset-mid-transfer", transfer::reset_mid_transfer),
        ("kill-mid-transfer", lifecycle::kill_mid_transfer),
        ("udp-echo", udp::udp_echo),
        ("loopback-witness", loopback::loopback_witness),
        ("create-destroy-20", lifecycle::create_destroy_20),
        (
            "service-stop-with-guests",
            shutdown::service_stop_with_guests,
        ),
    ]
}

pub fn names() -> Vec<&'static str> {
    all().into_iter().map(|(name, _)| name).collect()
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
    pub fixtures: &'a Fixtures,
    pub project: PathBuf,
    pub guest_subnet: String,
    pub expectations: BTreeMap<String, String>,
    pub service_pid: Option<u32>,
}

impl<'a> Ctx<'a> {
    pub fn prepare(
        lns: &'a Lns,
        fixtures: &'a Fixtures,
        images: &'a Images,
        project: &Path,
        guest_subnet: String,
        expectations: BTreeMap<String, String>,
        service_pid: Option<u32>,
    ) -> Result<Self> {
        std::fs::create_dir_all(project)
            .with_context(|| format!("create {}", project.display()))?;
        write_sandbox(
            &project.join("alpine.yaml"),
            "parity-alpine",
            &images.alpine,
        )?;
        write_sandbox(&project.join("curl.yaml"), "parity-curl", &images.curl)?;
        Ok(Self {
            lns,
            fixtures,
            project: project.to_path_buf(),
            guest_subnet,
            expectations,
            service_pid,
        })
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
        self.lns.run(&[
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
        ])
    }

    pub fn guest_detached(&self, image: Image, name: &str, script: &str) -> Result<Output> {
        let definition = self.definition(image);
        self.lns.run(&[
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
        ])
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
        poll(timeout, || self.new_connection(role, after))
    }

    pub fn await_closed(&self, role: Role, after: u64, timeout: Duration) -> Option<ConnRecord> {
        poll(timeout, || {
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

pub fn guest_output(case: &mut CaseResult, output: &Output) {
    case.record("guest_exit_code", output.code as i64);
    let combined = output.combined();
    let tail: String = combined.chars().rev().take(400).collect::<String>();
    case.record("guest_output_tail", tail.chars().rev().collect::<String>());
}

#[cfg(test)]
mod tests;
