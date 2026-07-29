#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::future::Future;
use std::path::Path;
use std::process::ExitStatus;

use super::vmm_bin::BinaryIdentity;

pub(crate) trait Spawner: Send + Sync {
    type Child: Child;

    fn spawn(&self, program: &Path, args: &[String]) -> std::io::Result<Self::Child>;
}

pub(crate) trait Child: Send {
    fn wait(&mut self) -> impl Future<Output = std::io::Result<ExitStatus>> + Send;
    fn start_kill(&mut self) -> std::io::Result<()>;
}

pub(crate) struct RealSpawner;

pub(crate) struct RealChild(tokio::process::Child);

impl Spawner for RealSpawner {
    type Child = RealChild;

    fn spawn(&self, program: &Path, args: &[String]) -> std::io::Result<RealChild> {
        tokio::process::Command::new(program)
            .args(args)
            .kill_on_drop(true)
            .spawn()
            .map(RealChild)
    }
}

impl Child for RealChild {
    async fn wait(&mut self) -> std::io::Result<ExitStatus> {
        self.0.wait().await
    }

    fn start_kill(&mut self) -> std::io::Result<()> {
        self.0.start_kill()
    }
}

pub(crate) fn virtiofsd_help(program: &Path) -> std::io::Result<Vec<u8>> {
    let output = std::process::Command::new(program).arg("--help").output()?;
    let mut help = output.stdout;
    help.extend_from_slice(&output.stderr);
    Ok(help)
}

pub(crate) fn virtiofsd_identity(program: &Path) -> std::io::Result<BinaryIdentity> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(program).map(|metadata| {
        BinaryIdentity::new(
            metadata.dev(),
            metadata.ino(),
            metadata.len(),
            (metadata.mtime(), metadata.mtime_nsec()),
            (metadata.ctime(), metadata.ctime_nsec()),
        )
    })
}
