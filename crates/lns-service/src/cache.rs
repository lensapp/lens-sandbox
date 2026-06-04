use anyhow::Result;
use std::path::PathBuf;

pub fn root() -> Result<PathBuf> {
    Ok(lns_ipc::cache_root()?)
}
