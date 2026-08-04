use anyhow::{Context, Result};
use std::io;
use std::path::Path;

use futures_util::StreamExt;

use super::{Fetcher, Fs, WritableFile};
use crate::http_cap::CappedBuffer;

pub(crate) struct RealFetcher {
    pub max_bytes: u64,
}

/// A connect bound, not a total one: the same client carries the kernel and VMM transfers, where a slow-but-progressing body must not be cut off.
fn shared_client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .build()
            // Same failure reqwest::get panics on: the process has no usable TLS backend, so there is nothing to fall back to.
            .expect("a TLS-backed HTTP client")
    })
}

impl Fetcher for RealFetcher {
    async fn fetch(&self, url: &str) -> Result<Vec<u8>> {
        let resp = shared_client()
            .get(url)
            .send()
            .await
            .with_context(|| format!("downloading {url}"))?
            .error_for_status()
            .with_context(|| format!("HTTP error from {url}"))?;
        let mut buf = CappedBuffer::new(resp.content_length(), self.max_bytes, url)?;
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            buf.push(chunk.map(|b| b.to_vec()), url)?;
        }
        Ok(buf.into_inner())
    }
}

pub(crate) struct RealFs;

impl Fs for RealFs {
    type WritableFile = tokio::fs::File;

    async fn create_dir_all(&self, p: &Path) -> io::Result<()> {
        tokio::fs::create_dir_all(p).await
    }

    async fn is_file(&self, p: &Path) -> bool {
        tokio::fs::metadata(p)
            .await
            .map(|m| m.is_file())
            .unwrap_or(false)
    }

    async fn read(&self, p: &Path) -> io::Result<Vec<u8>> {
        tokio::fs::read(p).await
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

    async fn set_mode(&self, p: &Path, mode: u32) -> io::Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            tokio::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).await
        }
        #[cfg(not(unix))]
        {
            let _ = (p, mode);
            Ok(())
        }
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
