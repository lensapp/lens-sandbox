use anyhow::Result;
use std::path::{Path, PathBuf};

pub fn root() -> Result<PathBuf> {
    Ok(lns_ipc::lns_home()?)
}

pub fn run_dir(root: &Path, run_id: &str) -> PathBuf {
    root.join("runs").join(run_id)
}
