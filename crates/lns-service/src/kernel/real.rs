use anyhow::{Context, Result};
use std::io;
use std::path::Path;

use futures_util::StreamExt;

use super::{Fetcher, Fs, MAX_KERNEL_BYTES, WritableFile};
use crate::http_cap::CappedBuffer;

pub(super) struct RealFetcher;

impl Fetcher for RealFetcher {
    async fn fetch(&self, url: &str) -> Result<Vec<u8>> {
        let resp = reqwest::get(url)
            .await
            .with_context(|| format!("downloading {url}"))?
            .error_for_status()
            .with_context(|| format!("HTTP error from {url}"))?;
        let mut buf = CappedBuffer::new(resp.content_length(), MAX_KERNEL_BYTES, url)?;
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            buf.push(chunk.map(|b| b.to_vec()), url)?;
        }
        Ok(buf.into_inner())
    }
}

pub(super) struct RealFs;

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
