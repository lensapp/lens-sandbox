pub mod real;

use crate::cli::Command;
use anyhow::Result;
use lns_ipc::UpdateStatus;
use serde::Serialize;
use std::io::Write;

pub fn should_announce(command: &Command) -> bool {
    !matches!(command, Command::Update(_))
}

pub fn announce_enabled(env_get: impl Fn(&str) -> Option<String>) -> bool {
    env_get(lns_ipc::NO_UPDATE_CHECK_ENV).is_none()
}

const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Payload {
    pub install_id: String,
    pub version: String,
    pub os: String,
    pub arch: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckOutcome {
    UpToDate,
    UpdateAvailable { latest: String },
    SecurityAdvisory { latest: String },
}

pub trait StatusReader {
    fn read_status(&self) -> Option<UpdateStatus>;
    fn read_install_id(&self) -> Option<String>;
}

pub fn build_payload(install_id: String) -> Payload {
    Payload {
        install_id,
        version: CURRENT_VERSION.to_string(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
    }
}

pub fn classify(running: &str, latest: &str, min_secure: Option<&str>) -> CheckOutcome {
    if let Some(min) = min_secure
        && version_lt(running, min)
    {
        return CheckOutcome::SecurityAdvisory {
            latest: latest.to_string(),
        };
    }
    if version_lt(running, latest) {
        return CheckOutcome::UpdateAvailable {
            latest: latest.to_string(),
        };
    }
    CheckOutcome::UpToDate
}

fn version_lt(a: &str, b: &str) -> bool {
    parse_version(a) < parse_version(b)
}

fn parse_version(v: &str) -> (u64, u64, u64) {
    let mut parts = v.split('.').map(|p| p.parse::<u64>().unwrap_or(0));
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
}

pub fn render_outcome(outcome: &CheckOutcome, out: &mut impl Write) -> std::io::Result<()> {
    match outcome {
        CheckOutcome::UpToDate => Ok(()),
        CheckOutcome::UpdateAvailable { latest } => writeln!(
            out,
            "A new version of lns ({latest}) is available. Run `lns update` to upgrade."
        ),
        CheckOutcome::SecurityAdvisory { latest } => writeln!(
            out,
            "A security update for lns ({latest}) is available. Run `lns update` to upgrade."
        ),
    }
}

pub fn announce(reader: &impl StatusReader, running: &str, out: &mut impl Write) -> Result<()> {
    if let Some(status) = reader.read_status() {
        let outcome = classify(
            running,
            &status.latest,
            status.min_secure_version.as_deref(),
        );
        render_outcome(&outcome, out)?;
    }
    Ok(())
}

pub fn dry_run(reader: &impl StatusReader, out: &mut impl Write) -> Result<()> {
    let id = reader
        .read_install_id()
        .unwrap_or_else(|| "<pending first service check>".to_string());
    writeln!(out, "{}", serde_json::to_string_pretty(&build_payload(id))?)?;
    Ok(())
}

#[cfg(test)]
mod tests;
