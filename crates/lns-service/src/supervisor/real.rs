use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use super::traits::{Fs, WritableFile};

pub(super) struct RealFs;

impl Fs for RealFs {
    type WritableFile = tokio::fs::File;

    async fn create_dir_all(&self, p: &Path) -> io::Result<()> {
        tokio::fs::create_dir_all(p).await
    }

    async fn exists(&self, p: &Path) -> bool {
        tokio::fs::try_exists(p).await.unwrap_or(false)
    }

    async fn remove_file(&self, p: &Path) -> io::Result<()> {
        tokio::fs::remove_file(p).await
    }

    async fn create_new(&self, p: &Path) -> io::Result<tokio::fs::File> {
        tokio::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(p)
            .await
    }

    async fn set_permissions(&self, p: &Path, mode: u32) -> io::Result<()> {
        tokio::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).await
    }

    async fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        tokio::fs::rename(from, to).await
    }
}

impl WritableFile for tokio::fs::File {
    async fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        use tokio::io::AsyncWriteExt;
        AsyncWriteExt::write_all(self, bytes).await
    }

    async fn sync_all(&mut self) -> io::Result<()> {
        tokio::fs::File::sync_all(self).await
    }
}
