use std::io;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileMeta {
    pub size_bytes: u64,
    pub allocated_bytes: u64,
    pub created_unix_secs: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VolumeClaims {
    /// Listed sandboxes, by name — the only claims a user can clear with `lns rm`.
    pub held_by: Vec<String>,
    /// Runs this build will not list but whose record still names this volume; only repairing the run dir clears them.
    pub damaged: Vec<String>,
    /// Runs whose records cannot be read at all; they may claim any volume, so they stand in the way of every one.
    pub unreadable: Vec<String>,
}

/// A stopped sandbox still claims its volume, so this outlives any live run's lease.
pub trait Holders: Send + Sync {
    fn claims_on(&self, name: &str) -> VolumeClaims;
}

pub trait Fs: Send + Sync {
    fn exists(&self, p: &Path) -> impl std::future::Future<Output = bool> + Send;
    fn create_dir_all(&self, p: &Path) -> impl std::future::Future<Output = io::Result<()>> + Send;
    fn create_ext4_image(
        &self,
        p: &Path,
        size_bytes: u64,
    ) -> impl std::future::Future<Output = io::Result<()>> + Send;
    fn read_dir(
        &self,
        dir: &Path,
    ) -> impl std::future::Future<Output = io::Result<Vec<PathBuf>>> + Send;
    fn metadata(&self, p: &Path) -> impl std::future::Future<Output = io::Result<FileMeta>> + Send;
    fn remove_file(&self, p: &Path) -> impl std::future::Future<Output = io::Result<()>> + Send;
    /// Extends an existing image towards `size_bytes`, keeping its contents; a smaller size leaves it alone.
    fn grow_ext4_image(
        &self,
        p: &Path,
        size_bytes: u64,
    ) -> impl std::future::Future<Output = io::Result<()>> + Send;
}
