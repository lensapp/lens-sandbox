use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use super::traits::{FileMeta, Fs, Holders};
use crate::upperfs::{Plan, write_ext4};

pub(super) struct RealFs;

pub(super) struct RegistryHolders;

impl Holders for RegistryHolders {
    fn claims_on(&self, name: &str) -> super::VolumeClaims {
        crate::run_registry::volume_claims(name)
    }
}

fn unix_secs(t: io::Result<SystemTime>) -> u64 {
    t.ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Fs for RealFs {
    async fn exists(&self, p: &Path) -> bool {
        tokio::fs::try_exists(p).await.unwrap_or(false)
    }

    async fn create_dir_all(&self, p: &Path) -> io::Result<()> {
        tokio::fs::create_dir_all(p).await
    }

    async fn read_dir(&self, dir: &Path) -> io::Result<Vec<PathBuf>> {
        let mut entries = tokio::fs::read_dir(dir).await?;
        let mut out = Vec::new();
        while let Some(entry) = entries.next_entry().await? {
            out.push(entry.path());
        }
        Ok(out)
    }

    async fn metadata(&self, p: &Path) -> io::Result<FileMeta> {
        use std::os::unix::fs::MetadataExt;
        let md = tokio::fs::metadata(p).await?;
        Ok(FileMeta {
            size_bytes: md.len(),
            allocated_bytes: md.blocks() * 512,
            created_unix_secs: unix_secs(md.created()),
        })
    }

    async fn remove_file(&self, p: &Path) -> io::Result<()> {
        tokio::fs::remove_file(p).await
    }

    async fn create_ext4_image(&self, p: &Path, size_bytes: u64) -> io::Result<()> {
        let uuid: [u8; 16] = rand::random();
        let mkfs_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as u32)
            .unwrap_or(0);
        let path = p.to_path_buf();
        tokio::task::spawn_blocking(move || {
            let plan = Plan::new(size_bytes, uuid, "lns-vol", mkfs_time)?;
            write_ext4(&plan, &path)
        })
        .await
        .map_err(io::Error::other)?
        .map_err(io::Error::other)
    }

    async fn grow_ext4_image(&self, p: &Path, size_bytes: u64) -> io::Result<()> {
        let path = p.to_path_buf();
        tokio::task::spawn_blocking(move || crate::upperfs::grow_ext4(&path, size_bytes))
            .await
            .map_err(io::Error::other)?
            .map_err(io::Error::other)
    }
}
