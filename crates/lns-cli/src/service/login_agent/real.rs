use std::io;
use std::path::Path;

use super::{CommandOutput, CommandRunner, DisableOutcome, EnableOutcome, Fs};

pub(super) struct RealFs;

impl Fs for RealFs {
    async fn write(&self, path: &Path, contents: &str) -> io::Result<()> {
        tokio::fs::write(path, contents).await
    }

    async fn remove_file(&self, path: &Path) -> io::Result<()> {
        tokio::fs::remove_file(path).await
    }

    async fn create_dir_all(&self, path: &Path) -> io::Result<()> {
        tokio::fs::create_dir_all(path).await
    }

    async fn exists(&self, path: &Path) -> bool {
        tokio::fs::try_exists(path).await.unwrap_or(false)
    }
}

pub(super) struct RealCommandRunner;

impl CommandRunner for RealCommandRunner {
    async fn run(&self, program: &str, args: &[String]) -> io::Result<CommandOutput> {
        let output = tokio::process::Command::new(program)
            .args(args)
            .output()
            .await?;
        Ok(CommandOutput {
            status: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

pub(in crate::service) async fn enable() -> EnableOutcome {
    let spec = match super::production_spec() {
        Ok(s) => s,
        Err(reason) => return EnableOutcome::Degraded(reason),
    };
    super::enable_with(&RealFs, &RealCommandRunner, &spec).await
}

pub(in crate::service) async fn disable() -> DisableOutcome {
    let spec = match super::production_spec() {
        Ok(s) => s,
        Err(_) => return DisableOutcome::WasNotRegistered,
    };
    super::disable_with(&RealFs, &RealCommandRunner, &spec).await
}
