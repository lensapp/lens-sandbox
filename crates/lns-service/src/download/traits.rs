use anyhow::Result;
use std::io;
use std::path::Path;

pub(crate) trait Fetcher: Send + Sync {
    fn fetch(&self, url: &str) -> impl std::future::Future<Output = Result<Vec<u8>>> + Send;
}

pub(crate) trait Fs: Send + Sync {
    type WritableFile: WritableFile + Send;

    fn create_dir_all(&self, p: &Path) -> impl std::future::Future<Output = io::Result<()>> + Send;
    fn is_file(&self, p: &Path) -> impl std::future::Future<Output = bool> + Send;
    fn read(&self, p: &Path) -> impl std::future::Future<Output = io::Result<Vec<u8>>> + Send;
    fn remove_file(&self, p: &Path) -> impl std::future::Future<Output = io::Result<()>> + Send;
    fn create_new(
        &self,
        p: &Path,
    ) -> impl std::future::Future<Output = io::Result<Self::WritableFile>> + Send;
    fn set_mode(
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

pub(crate) trait WritableFile: Send {
    fn write_all<'a>(
        &'a mut self,
        bytes: &'a [u8],
    ) -> impl std::future::Future<Output = io::Result<()>> + Send + 'a;
    fn sync_all(&mut self) -> impl std::future::Future<Output = io::Result<()>> + Send + '_;
}
