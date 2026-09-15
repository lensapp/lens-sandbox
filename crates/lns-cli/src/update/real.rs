use anyhow::{Context, Result};
use clap::FromArgMatches;

use crate::command::{RunCtx, RunFuture};
use crate::service::real::RealServiceClient;
use crate::update::UpdateArgs;

pub fn run_command<'a>(matches: &'a clap::ArgMatches, _ctx: RunCtx<'a>) -> RunFuture<'a> {
    Box::pin(async move {
        let args = UpdateArgs::from_arg_matches(matches)?;
        run(args).await
    })
}

pub async fn run(args: UpdateArgs) -> Result<i32> {
    if args.dry_run {
        crate::update_check::real::run_dry_run()?;
        return Ok(0);
    }
    let lns_path = std::env::current_exe().context("resolving current `lns` executable path")?;
    let lns_path = tokio::fs::canonicalize(&lns_path)
        .await
        .with_context(|| format!("canonicalizing {}", lns_path.display()))?;
    if cfg!(target_os = "macos") {
        let platform = crate::platform::detect();
        super::platform_key(&platform)?;
        let host = NativeHost {
            client: super::http_client(super::LNS_VERSION, &platform)?,
        };
        return super::native::run_with(&host, args, super::LNS_VERSION, &lns_path).await;
    }
    let service = RealServiceClient::new(
        crate::service::socket_path()?,
        crate::service::find_service_binary(),
    );
    super::run_with(
        args,
        super::LNS_VERSION,
        super::DEFAULT_CDN_BASE,
        &crate::platform::detect(),
        &lns_path,
        &service,
    )
    .await
}

struct NativeHost {
    client: reqwest::Client,
}

impl super::native::Host for NativeHost {
    async fn latest(&self) -> Result<super::ManifestEntry> {
        super::fetch_manifest_entry(&self.client, super::DEFAULT_CDN_BASE, "darwin-aarch64").await
    }

    async fn download(&self, entry: &super::ManifestEntry) -> Result<Vec<u8>> {
        super::fetch_and_verify_tarball(&self.client, entry).await
    }

    async fn install(
        &self,
        entry: &super::ManifestEntry,
        bytes: &[u8],
        executable: &std::path::Path,
    ) -> Result<()> {
        let stage = tempfile::tempdir().context("creating app update staging directory")?;
        let archive = stage.path().join("LNS.zip");
        tokio::fs::write(&archive, bytes).await?;
        let extracted = stage.path().join("extracted");
        checked_command(
            tokio::process::Command::new("/usr/bin/ditto")
                .arg("-x")
                .arg("-k")
                .arg(&archive)
                .arg(&extracted),
        )
        .await?;
        let source = extracted.join("LNS.app");
        let home = dirs::home_dir().context("finding the application directory")?;
        let destination = lns_ipc::desktop_bundle(executable)
            .unwrap_or_else(|| home.join("Applications/LNS.app"));
        let bin = native_cli_directory(&home).await;
        let installer = stage.path().join("install.sh");
        tokio::fs::write(
            stage.path().join("quit.js"),
            include_str!("../../../../clients/macos/scripts/quit.js"),
        )
        .await?;
        tokio::fs::write(
            &installer,
            include_str!("../../../../clients/macos/scripts/install.sh"),
        )
        .await?;
        checked_command(
            tokio::process::Command::new("/bin/sh")
                .arg(&installer)
                .arg(&source)
                .arg(&destination)
                .arg(&bin)
                .arg(&entry.version),
        )
        .await
    }
}

async fn checked_command(command: &mut tokio::process::Command) -> Result<()> {
    let status = command
        .status()
        .await
        .context("running native app installer")?;
    anyhow::ensure!(status.success(), "native app installer failed: {status}");
    Ok(())
}

pub(crate) async fn native_cli_directory(home: &std::path::Path) -> std::path::PathBuf {
    let result = tokio::process::Command::new("/usr/bin/defaults")
        .args(["read", "run.lns.desktop", "CLIInstallDirectory"])
        .output()
        .await;
    if let Ok(output) = result
        && output.status.success()
    {
        let path = std::path::PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
        if path.is_absolute() {
            return path;
        }
    }
    home.join(".local/bin")
}
