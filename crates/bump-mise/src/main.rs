mod operations;

use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    about = "Bump the pinned mise tool-provisioning engine and regenerate the registry snapshot"
)]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Verify a release against mise's minisign key, rewrite the engine pin, and regenerate the registry snapshot.
    Bump {
        /// The mise release version, without the leading `v` (e.g. 2026.8.0).
        #[arg(long)]
        version: String,
    },
    /// Print the current engine pin.
    Show,
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Cmd::Bump { version } => bump(&version),
        Cmd::Show => show(),
    }
}

fn workspace_paths() -> Result<(PathBuf, PathBuf)> {
    let root = std::env::current_dir().context("resolving the working directory")?;
    let manifest = root.join("crates/lns-service/mise.toml");
    let snapshot = root.join("crates/lns-artifact/src/tools/registry.snapshot");
    if !manifest.is_file() {
        bail!(
            "{} not found — run bump-mise from the workspace root",
            manifest.display()
        );
    }
    Ok((manifest, snapshot))
}

fn show() -> Result<()> {
    let (manifest_path, snapshot_path) = workspace_paths()?;
    let (version, shas) = operations::engine_summary(&std::fs::read_to_string(&manifest_path)?)?;
    println!("engine version: {version}");
    println!("engine sha256: {shas}");
    let entries = std::fs::read_to_string(&snapshot_path)?.lines().count();
    println!("registry snapshot: {entries} entries");
    Ok(())
}

fn bump(version: &str) -> Result<()> {
    let (manifest_path, snapshot_path) = workspace_paths()?;

    println!("==> fetching and verifying SHASUMS256.txt for v{version}");
    let shasums_url = operations::shasums_url(version);
    let shasums = curl_bytes(&shasums_url)?;
    let minisig = curl_bytes(&format!("{shasums_url}.minisig"))?;
    operations::verify_shasums_signature(
        &shasums,
        std::str::from_utf8(&minisig).context("minisig is not UTF-8")?,
    )?;
    let shasums = String::from_utf8(shasums).context("SHASUMS256.txt is not UTF-8")?;
    let shas = operations::musl_binary_shas(&shasums, version)?;

    println!(
        "==> rewriting the engine pin in {}",
        manifest_path.display()
    );
    let manifest = std::fs::read_to_string(&manifest_path)?;
    let bumped = operations::bump_engine_pin(&manifest, version, &shas)?;

    println!("==> regenerating the registry snapshot from the release source");
    let tarball = curl_bytes(&operations::source_tarball_url(version))?;
    let entries = operations::registry_entries_from_tarball(&tarball)?;
    let snapshot = operations::render_registry_snapshot(&entries)?;

    println!("==> refreshing the version-index snapshots the contract tests read");
    let root = std::env::current_dir().context("resolving the working directory")?;
    let mut index_snapshots = Vec::new();
    for tool in operations::INDEX_SNAPSHOT_TOOLS {
        let body = String::from_utf8(curl_bytes(&operations::index_snapshot_url(tool))?)
            .with_context(|| format!("the index body for {tool} is not UTF-8"))?;
        operations::validate_index_snapshot(tool, &body)?;
        index_snapshots.push((operations::index_snapshot_path(&root, tool), body));
    }

    let mut files = vec![
        (manifest_path.as_path(), bumped.into_bytes()),
        (snapshot_path.as_path(), snapshot.into_bytes()),
    ];
    files.extend(
        index_snapshots
            .iter()
            .map(|(path, body)| (path.as_path(), body.clone().into_bytes())),
    );
    operations::replace_all_transactionally(&files)?;

    println!(
        "==> done. Next: re-validate the companion pins in mise.toml if the alpine branch moved,\n\
         \trun `make lint && make coverage`, and let the e2e-microvm tools scenarios gate the PR\n\
         \t(registry semantics can drift between mise versions)."
    );
    Ok(())
}

fn curl_bytes(url: &str) -> Result<Vec<u8>> {
    let output = Command::new("curl")
        .args(["-sfL", url])
        .output()
        .with_context(|| format!("invoking curl for {url}"))?;
    if !output.status.success() {
        bail!(
            "curl {url} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(output.stdout)
}
