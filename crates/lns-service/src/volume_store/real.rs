use std::io;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use super::traits::Fs;
use crate::upperfs::{Plan, write_ext4};

pub(super) struct RealFs;

impl Fs for RealFs {
    async fn exists(&self, p: &Path) -> bool {
        tokio::fs::try_exists(p).await.unwrap_or(false)
    }

    async fn create_dir_all(&self, p: &Path) -> io::Result<()> {
        tokio::fs::create_dir_all(p).await
    }

    async fn create_ext4_image(&self, p: &Path, size_bytes: u64) -> io::Result<()> {
        let uuid: [u8; 16] = rand::random();
        let mkfs_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as u32)
            .unwrap_or(0);
        let path = p.to_path_buf();
        tokio::task::spawn_blocking(move || {
            let plan = Plan::new(size_bytes, uuid, "lns-vol", mkfs_time);
            write_ext4(&plan, &path)
        })
        .await
        .map_err(io::Error::other)?
        .map_err(io::Error::other)
    }
}
