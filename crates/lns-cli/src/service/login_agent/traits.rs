use std::io;
use std::path::Path;

pub(super) trait Fs: Send + Sync {
    fn write(
        &self,
        path: &Path,
        contents: &str,
    ) -> impl std::future::Future<Output = io::Result<()>> + Send;
    fn remove_file(&self, path: &Path) -> impl std::future::Future<Output = io::Result<()>> + Send;
    fn create_dir_all(
        &self,
        path: &Path,
    ) -> impl std::future::Future<Output = io::Result<()>> + Send;
    fn exists(&self, path: &Path) -> impl std::future::Future<Output = bool> + Send;
}

pub(super) struct CommandOutput {
    pub status: i32,
    pub stderr: String,
}

pub(super) trait CommandRunner: Send + Sync {
    fn run(
        &self,
        program: &str,
        args: &[String],
    ) -> impl std::future::Future<Output = io::Result<CommandOutput>> + Send;
}
