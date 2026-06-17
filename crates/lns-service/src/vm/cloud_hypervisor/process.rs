#![cfg_attr(not(target_os = "linux"), allow(dead_code))]

use std::future::Future;
use std::path::Path;
use std::process::ExitStatus;

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
