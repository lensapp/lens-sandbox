use std::time::{SystemTime, UNIX_EPOCH};

use super::{provision_image, write_ext4};

pub async fn provision(run_id: &str) -> anyhow::Result<std::path::PathBuf> {
    let root = crate::cache::root()?;
    let run_id = run_id.to_string();
    let uuid: [u8; 16] = rand::random();
    let mkfs_time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as u32)
        .unwrap_or(0);

    tokio::task::spawn_blocking(move || {
        provision_image(&root, &run_id, uuid, mkfs_time, write_ext4)
    })
    .await?
}
