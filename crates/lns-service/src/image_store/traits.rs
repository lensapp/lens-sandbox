use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};

pub trait Fs: Send + Sync {
    fn read_dir(
        &self,
        dir: &Path,
    ) -> impl std::future::Future<Output = io::Result<Vec<PathBuf>>> + Send;
    fn read(&self, p: &Path) -> impl std::future::Future<Output = io::Result<Vec<u8>>> + Send;
    fn write(
        &self,
        p: &Path,
        bytes: &[u8],
    ) -> impl std::future::Future<Output = io::Result<()>> + Send;
    fn remove_file(&self, p: &Path) -> impl std::future::Future<Output = io::Result<()>> + Send;
}

pub trait RuntimeCacheFs: Fs {
    fn remove_dir_all(&self, p: &Path) -> impl std::future::Future<Output = io::Result<()>> + Send;
}

pub trait Caches: Send + Sync {
    fn sweep_layers(&self, keep: &HashSet<String>) -> anyhow::Result<u64>;
    fn remove_manifest(&self, reference: &str) -> anyhow::Result<()>;
}
