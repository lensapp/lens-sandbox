use super::{StatusReader, announce, dry_run};
use anyhow::Result;
use lns_ipc::UpdateStatus;
use std::path::PathBuf;

pub struct RealStatusReader;

impl RealStatusReader {
    fn root() -> Option<PathBuf> {
        lns_ipc::lns_home().ok()
    }
}

impl StatusReader for RealStatusReader {
    fn read_status(&self) -> Option<UpdateStatus> {
        let path = Self::root()?.join("update-status.json");
        let bytes = std::fs::read(path).ok()?;
        serde_json::from_slice(&bytes).ok()
    }
    fn read_install_id(&self) -> Option<String> {
        let path = Self::root()?.join("install-id");
        let s = std::fs::read_to_string(path).ok()?;
        let trimmed = s.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    }
}

pub fn run_announce() -> Result<()> {
    announce(
        &RealStatusReader,
        super::CURRENT_VERSION,
        &mut std::io::stderr(),
    )
}

pub fn run_dry_run() -> Result<()> {
    dry_run(&RealStatusReader, &mut std::io::stdout())
}
