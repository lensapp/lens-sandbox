use anyhow::Result;
use std::path::{Path, PathBuf};

pub fn root() -> Result<PathBuf> {
    Ok(lns_ipc::lns_home()?)
}

pub fn run_dir(root: &Path, run_id: &str) -> PathBuf {
    root.join("runs").join(run_id)
}

/// `sandbox-spec.md` §8.3 keeps a run's decisions in the run's own directory, so removing the run removes what it decided.
pub fn decisions_path(root: &Path, run_id: &str) -> PathBuf {
    run_dir(root, run_id).join(DECISIONS_FILENAME)
}

pub const DECISIONS_FILENAME: &str = "decisions.yaml";

/// A run keeps what it was asked beside what it decided, so one sweep takes both.
pub fn approvals_path(root: &Path, run_id: &str) -> PathBuf {
    run_dir(root, run_id).join(APPROVALS_FILENAME)
}

pub const APPROVALS_FILENAME: &str = "approvals.json";
