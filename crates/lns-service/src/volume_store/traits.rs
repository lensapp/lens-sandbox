use std::io;
use std::path::Path;

pub trait Fs: Send + Sync {
    fn exists(&self, p: &Path) -> impl std::future::Future<Output = bool> + Send;
    fn create_dir_all(&self, p: &Path) -> impl std::future::Future<Output = io::Result<()>> + Send;
    fn create_ext4_image(
        &self,
        p: &Path,
        size_bytes: u64,
    ) -> impl std::future::Future<Output = io::Result<()>> + Send;
}
