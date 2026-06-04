use std::io;
use std::path::Path;

pub(super) trait Fs: Send + Sync {
    type WritableFile: WritableFile + Send;

    fn create_dir_all(&self, p: &Path) -> impl std::future::Future<Output = io::Result<()>> + Send;
    fn exists(&self, p: &Path) -> impl std::future::Future<Output = bool> + Send;
    fn remove_file(&self, p: &Path) -> impl std::future::Future<Output = io::Result<()>> + Send;
    fn create_new(
        &self,
        p: &Path,
    ) -> impl std::future::Future<Output = io::Result<Self::WritableFile>> + Send;
    fn set_permissions(
        &self,
        p: &Path,
        mode: u32,
    ) -> impl std::future::Future<Output = io::Result<()>> + Send;
    fn rename(
        &self,
        from: &Path,
        to: &Path,
    ) -> impl std::future::Future<Output = io::Result<()>> + Send;
}

pub(super) trait WritableFile: Send {
    fn write_all<'a>(
        &'a mut self,
        bytes: &'a [u8],
    ) -> impl std::future::Future<Output = io::Result<()>> + Send + 'a;
    fn sync_all(&mut self) -> impl std::future::Future<Output = io::Result<()>> + Send + '_;
}
