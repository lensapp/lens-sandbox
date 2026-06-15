use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use lns_ipc::{Method, PlatformInfo};

use crate::command::{CommandSpec, subcommand};
use crate::log;
use crate::service::ServiceClient;

mod real;

pub use real::run;

#[derive(clap::Args)]
pub struct UpdateArgs {
    #[arg(
        long,
        default_value_t = false,
        help = "Re-install even if the running version matches, e.g. when the binary is corrupt or its codesign was invalidated."
    )]
    pub force: bool,

    #[arg(
        long,
        default_value_t = false,
        help = "Print the anonymous update-check payload that would be sent (install ID, version, OS/arch) and exit without contacting the network."
    )]
    pub dry_run: bool,
}

pub fn augment(app: clap::Command) -> clap::Command {
    app.subcommand(
        subcommand::<UpdateArgs>("update")
            .about("Update `lns` and `lns-service` to the latest release."),
    )
}

pub const SPEC: CommandSpec = CommandSpec {
    name: "update",
    augment,
    run: real::run_command,
    announces_update_check: false,
};

pub(in crate::update) const DEFAULT_CDN_BASE: &str = "https://get.lns.run";
const MANIFEST_NAME: &str = "lns-latest.json";
const SERVICE_STOP_TIMEOUT: Duration = Duration::from_secs(10);
const SERVICE_START_TIMEOUT: Duration = Duration::from_secs(10);

pub(in crate::update) const LNS_VERSION: &str = env!("CARGO_PKG_VERSION");

pub(in crate::update) async fn run_with(
    args: UpdateArgs,
    running_version: &str,
    cdn_base: &str,
    platform: &PlatformInfo,
    lns_path: &Path,
    service: &impl ServiceClient,
) -> Result<i32> {
    let plat_key = platform_key(platform)?;
    let client = http_client(running_version, platform)?;
    let entry = fetch_manifest_entry(&client, cdn_base, plat_key).await?;

    if running_version == entry.version && !args.force {
        println!("lns {running_version} is already up to date.");
        return Ok(0);
    }

    log::info!(
        "Updating",
        "lns {running_version} → {target} ({plat_key})",
        target = entry.version
    );

    let tarball = fetch_and_verify_tarball(&client, &entry).await?;
    let extracted =
        extract_release_tarball(&tarball).with_context(|| format!("extracting {}", entry.url))?;

    let lns_path = tokio::fs::canonicalize(lns_path)
        .await
        .with_context(|| format!("canonicalizing {}", lns_path.display()))?;
    let service_path = sibling_service_path(&lns_path)?;

    let staged = stage_release_binaries(&lns_path, &service_path, &extracted).await?;
    let was_running = service.ping().await;
    swap_in_staged(staged, service, was_running).await?;

    println!("Updated to lns {}.", entry.version);
    if let Some(hint) = post_install_hint(was_running) {
        println!("{hint}");
    }
    Ok(0)
}

async fn swap_in_staged(
    staged: StagedInstall,
    service: &impl ServiceClient,
    was_running: bool,
) -> Result<()> {
    if was_running && let Err(e) = stop_running_service(service).await {
        staged.cleanup().await;
        return Err(e);
    }
    if let Err(commit_err) = staged.commit().await {
        staged.cleanup().await;
        if was_running {
            attempt_best_effort_restart(service).await;
        }
        return Err(commit_err);
    }
    if was_running {
        restart_service(service).await?;
    }
    Ok(())
}

fn post_install_hint(service_was_running: bool) -> Option<&'static str> {
    if service_was_running {
        None
    } else {
        Some("Run `lns service start` to launch the new background service.")
    }
}

async fn stage_release_binaries(
    lns_path: &Path,
    service_path: &Path,
    extracted: &ExtractedBinaries,
) -> Result<StagedInstall> {
    let mut staged = StagedInstall::default();
    if let Err(e) = staged.stage(lns_path, &extracted.lns).await {
        staged.cleanup().await;
        return Err(e);
    }
    match extracted.lns_service.as_deref() {
        Some(service_bytes) => {
            if let Err(e) = staged.stage(service_path, service_bytes).await {
                staged.cleanup().await;
                return Err(e);
            }
        }
        None => {
            let service_path_display = service_path.display();
            log::warn!(
                "release tarball did not contain lns-service — leaving the existing {service_path_display} in place"
            );
        }
    }
    Ok(staged)
}

#[derive(Default)]
struct StagedInstall {
    binaries: Vec<StagedBinary>,
}

struct StagedBinary {
    tmp: PathBuf,
    bak: PathBuf,
    dest: PathBuf,
}

impl StagedInstall {
    async fn stage(&mut self, dest: &Path, bytes: &[u8]) -> Result<()> {
        log::info!("Installing", "{}", dest.display());
        let tmp = dest.with_extension("lns-update.tmp");
        write_staged(&tmp, bytes).await.with_context(|| {
            format!(
                "writing {} (insufficient permissions? try re-running with sudo, \
                 or reinstall via the install script)",
                dest.display()
            )
        })?;
        self.binaries.push(StagedBinary {
            tmp,
            bak: dest.with_extension("lns-update.bak"),
            dest: dest.to_path_buf(),
        });
        Ok(())
    }

    async fn commit(&self) -> Result<()> {
        let mut committed: Vec<&StagedBinary> = Vec::new();
        for b in &self.binaries {
            if let Err(e) = b.commit_one().await {
                for done in committed.iter().rev() {
                    done.rollback().await;
                }
                return Err(e);
            }
            committed.push(b);
        }
        for b in &self.binaries {
            b.discard_backup().await;
        }
        Ok(())
    }

    async fn cleanup(&self) {
        for b in &self.binaries {
            let _ = tokio::fs::remove_file(&b.tmp).await;
            let _ = tokio::fs::remove_file(&b.bak).await;
        }
    }
}

impl StagedBinary {
    async fn commit_one(&self) -> Result<()> {
        let ctx = format!("committing {} into place", self.dest.display());
        match tokio::fs::rename(&self.dest, &self.bak).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e).context(ctx),
        }
        tokio::fs::rename(&self.tmp, &self.dest).await.context(ctx)
    }

    async fn rollback(&self) {
        let _ = tokio::fs::rename(&self.dest, &self.tmp).await;
        let _ = tokio::fs::rename(&self.bak, &self.dest).await;
    }

    async fn discard_backup(&self) {
        let _ = tokio::fs::remove_file(&self.bak).await;
    }
}

async fn attempt_best_effort_restart(service: &impl ServiceClient) {
    let timeout_secs = SERVICE_START_TIMEOUT.as_secs();
    match service
        .start_and_wait_for_ready(SERVICE_START_TIMEOUT)
        .await
    {
        Ok(true) => {
            log::warn!("update failed after stopping lns-service — relaunched the previous binary")
        }
        Ok(false) => log::error!(
            "update failed after stopping lns-service AND the best-effort relaunch did not become ready within {timeout_secs}s — run `lns service start` to investigate"
        ),
        Err(e) => log::error!(
            "update failed after stopping lns-service AND the best-effort relaunch errored ({e:#}) — run `lns service start` to investigate"
        ),
    }
}

async fn fetch_manifest_entry(
    client: &reqwest::Client,
    cdn_base: &str,
    plat_key: &str,
) -> Result<ManifestEntry> {
    let url = format!("{cdn_base}/{MANIFEST_NAME}");
    log::debug!(url = %url, "fetching update manifest");
    let bytes = http_get_bytes(client, &url)
        .await
        .with_context(|| format!("fetching {url}"))?;
    let body =
        std::str::from_utf8(&bytes).with_context(|| format!("{url} returned non-UTF-8 bytes"))?;
    parse_manifest(body, plat_key).with_context(|| format!("parsing manifest from {url}"))
}

async fn fetch_and_verify_tarball(
    client: &reqwest::Client,
    entry: &ManifestEntry,
) -> Result<Vec<u8>> {
    log::info!("Downloading", "{}", entry.url);
    let bytes = http_get_bytes(client, &entry.url).await?;
    let actual = format!("{:x}", Sha256::digest(&bytes));
    if actual != entry.sha256 {
        bail!(
            "sha256 mismatch from {} — expected {}, got {}. Refusing to install.",
            entry.url,
            entry.sha256,
            actual
        );
    }
    log::debug!(sha256 = %actual, "release tarball sha256 verified");
    Ok(bytes)
}

fn http_client(running_version: &str, platform: &PlatformInfo) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(lns_ipc::user_agent(
            running_version,
            platform,
            Method::CliUpdate,
        ))
        .build()
        .context("building HTTP client for self-update")
}

async fn http_get_bytes(client: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    let resp = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("downloading {url}"))?
        .error_for_status()
        .with_context(|| format!("HTTP error from {url}"))?;
    let bytes = resp
        .bytes()
        .await
        .with_context(|| format!("reading body from {url}"))?;
    Ok(bytes.to_vec())
}

async fn stop_running_service(service: &impl ServiceClient) -> Result<()> {
    log::info!("Stopping", "lns-service");
    if service.shutdown().await.is_none() {
        return Ok(());
    }
    if !service.wait_for_stopped(SERVICE_STOP_TIMEOUT).await {
        bail!(
            "lns-service acknowledged shutdown but did not exit within {}s — refusing \
             to replace its on-disk binary while it is still running",
            SERVICE_STOP_TIMEOUT.as_secs()
        );
    }
    Ok(())
}

async fn restart_service(service: &impl ServiceClient) -> Result<()> {
    log::info!("Starting", "lns-service");
    let ready = service
        .start_and_wait_for_ready(SERVICE_START_TIMEOUT)
        .await
        .context("relaunching lns-service after update")?;
    if !ready {
        bail!(
            "lns-service was relaunched but did not become ready within {}s — \
             run `lns service status` to investigate",
            SERVICE_START_TIMEOUT.as_secs()
        );
    }
    Ok(())
}

async fn write_staged(tmp: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let _ = tokio::fs::remove_file(tmp).await;
    {
        let mut file = tokio::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(tmp)
            .await?;
        use tokio::io::AsyncWriteExt;
        file.write_all(bytes).await?;
        file.sync_all().await?;
    }
    tokio::fs::set_permissions(tmp, std::fs::Permissions::from_mode(0o755)).await?;
    Ok(())
}

fn sibling_service_path(lns_path: &Path) -> Result<PathBuf> {
    let parent = lns_path.parent().ok_or_else(|| {
        anyhow::anyhow!(
            "current `lns` path {} has no parent directory — cannot locate sibling lns-service",
            lns_path.display()
        )
    })?;
    Ok(parent.join("lns-service"))
}

fn platform_key(p: &PlatformInfo) -> Result<&'static str> {
    match (p.os.as_str(), p.arch.as_str()) {
        ("Darwin" | "darwin", "arm64" | "aarch64") => Ok("darwin-aarch64"),
        ("Linux" | "linux", "arm64" | "aarch64") => Ok("linux-aarch64"),
        ("Linux" | "linux", "x86_64" | "amd64") => Ok("linux-x86_64"),
        ("Darwin" | "darwin", "x86_64" | "amd64") => bail!(
            "`lns update` only supports Apple Silicon on macOS; Intel Macs cannot host the guest VM"
        ),
        ("Darwin" | "darwin" | "Linux" | "linux", other_arch) => bail!(
            "unsupported architecture for `lns update`: {other_arch}. \
             Manual install: curl -fsSL https://get.lns.run | bash"
        ),
        (other_os, _) => bail!(
            "unsupported operating system for `lns update`: {other_os}. \
             Manual install: curl -fsSL https://get.lns.run | bash"
        ),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ManifestEntry {
    version: String,
    url: String,
    sha256: String,
}

fn parse_manifest(body: &str, platform_key: &str) -> Result<ManifestEntry> {
    use serde::Deserialize;
    #[derive(Deserialize)]
    struct Raw {
        version: String,
        #[serde(default)]
        platforms: std::collections::BTreeMap<String, RawPlatform>,
    }
    #[derive(Deserialize)]
    struct RawPlatform {
        url: String,
        sha256: String,
    }
    let raw: Raw = serde_json::from_str(body).context("decoding manifest JSON")?;
    if raw.version.is_empty() {
        bail!("manifest is missing the required `version` field");
    }
    let platform = raw.platforms.get(platform_key).ok_or_else(|| {
        anyhow::anyhow!(
            "manifest has no `platforms.{platform_key}` entry — \
             the release may not yet ship for this platform"
        )
    })?;
    if platform.url.is_empty() || platform.sha256.is_empty() {
        bail!("manifest entry for {platform_key} is missing url or sha256");
    }
    Ok(ManifestEntry {
        version: raw.version,
        url: platform.url.clone(),
        sha256: platform.sha256.clone(),
    })
}

#[derive(Debug)]
struct ExtractedBinaries {
    lns: Vec<u8>,
    lns_service: Option<Vec<u8>>,
}

fn extract_release_tarball(bytes: &[u8]) -> Result<ExtractedBinaries> {
    let gz = flate2::read::GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(gz);
    let mut lns: Option<Vec<u8>> = None;
    let mut lns_service: Option<Vec<u8>> = None;
    for entry in archive.entries().context("opening tarball entries")? {
        let mut entry = entry.context("reading tarball entry")?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry.path().context("reading tarball entry path")?;
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(str::to_string);
        let Some(name) = name else { continue };
        match name.as_str() {
            "lns" if lns.is_none() => {
                let mut buf = Vec::new();
                entry
                    .read_to_end(&mut buf)
                    .context("reading `lns` from tarball")?;
                lns = Some(buf);
            }
            "lns-service" if lns_service.is_none() => {
                let mut buf = Vec::new();
                entry
                    .read_to_end(&mut buf)
                    .context("reading `lns-service` from tarball")?;
                lns_service = Some(buf);
            }
            _ => {}
        }
    }
    let lns = lns.ok_or_else(|| {
        anyhow::anyhow!("release tarball did not contain a regular `lns` file at any depth")
    })?;
    Ok(ExtractedBinaries { lns, lns_service })
}

#[cfg(test)]
mod tests {
    use super::*;
    use httpmock::Method::GET;
    use httpmock::MockServer;
    use std::sync::Mutex;

    fn platform(os: &str, arch: &str, kernel_release: &str, shell: &str) -> PlatformInfo {
        PlatformInfo {
            os: os.into(),
            arch: arch.into(),
            kernel_release: kernel_release.into(),
            shell: shell.into(),
        }
    }

    fn darwin_arm() -> PlatformInfo {
        platform("Darwin", "arm64", "24.6.0", "zsh")
    }

    #[test]
    fn env_os_to_uname_sysname_round_trips_through_platform_key() {
        let mapped_mac = platform(
            lns_ipc::env_os_to_uname_sysname("macos").unwrap(),
            "aarch64",
            "24.6.0",
            "zsh",
        );
        assert_eq!(platform_key(&mapped_mac).unwrap(), "darwin-aarch64");
        let mapped_linux = platform(
            lns_ipc::env_os_to_uname_sysname("linux").unwrap(),
            "x86_64",
            "6.6.0",
            "bash",
        );
        assert_eq!(platform_key(&mapped_linux).unwrap(), "linux-x86_64");
    }

    #[test]
    fn platform_key_maps_supported_targets() {
        assert_eq!(platform_key(&darwin_arm()).unwrap(), "darwin-aarch64");
        assert_eq!(
            platform_key(&platform("Linux", "aarch64", "6.6.0", "bash")).unwrap(),
            "linux-aarch64",
        );
        assert_eq!(
            platform_key(&platform("linux", "amd64", "6.6.0", "bash")).unwrap(),
            "linux-x86_64",
        );
        assert_eq!(
            platform_key(&platform("Linux", "x86_64", "6.6.0", "bash")).unwrap(),
            "linux-x86_64",
        );
    }

    #[test]
    fn platform_key_rejects_intel_mac_with_actionable_message() {
        let err = platform_key(&platform("Darwin", "x86_64", "23.6.0", "zsh")).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("Apple Silicon"), "msg: {msg}");
    }

    #[test]
    fn platform_key_rejects_unsupported_os_with_install_script_hint() {
        let err =
            platform_key(&platform("Windows_NT", "x86_64", "10.0", "powershell")).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("unsupported operating system"), "msg: {msg}");
        assert!(msg.contains("curl"), "install-script hint: {msg}");
    }

    #[test]
    fn platform_key_rejects_unsupported_arch_with_install_script_hint() {
        let err = platform_key(&platform("Linux", "riscv64", "6.6.0", "bash")).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("unsupported architecture"), "msg: {msg}");
        assert!(msg.contains("curl"), "install-script hint: {msg}");
    }

    #[test]
    fn parse_manifest_happy_path() {
        let body = sample_manifest("0.4.0");
        let entry = parse_manifest(&body, "darwin-aarch64").unwrap();
        assert_eq!(entry.version, "0.4.0");
        assert_eq!(
            entry.url,
            "https://get.lns.run/lns-0.4.0-darwin-aarch64.tar.gz"
        );
        assert_eq!(entry.sha256.len(), 64);
    }

    #[test]
    fn parse_manifest_bails_when_platform_missing() {
        let body = r#"{"version":"0.4.0","platforms":{"linux-aarch64":{"url":"u","sha256":"s"}}}"#;
        let err = parse_manifest(body, "darwin-aarch64").unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("darwin-aarch64"),
            "platform name in msg: {msg}"
        );
    }

    #[test]
    fn parse_manifest_bails_when_version_missing() {
        let body = r#"{"version":"","platforms":{}}"#;
        let err = parse_manifest(body, "darwin-aarch64").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("version"), "msg: {msg}");
    }

    #[test]
    fn parse_manifest_bails_when_url_blank() {
        let body = r#"{"version":"0.4.0","platforms":{"darwin-aarch64":{"url":"","sha256":"s"}}}"#;
        let err = parse_manifest(body, "darwin-aarch64").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("url or sha256"), "msg: {msg}");
    }

    #[test]
    fn parse_manifest_bails_when_sha_blank() {
        let body = r#"{"version":"0.4.0","platforms":{"darwin-aarch64":{"url":"u","sha256":""}}}"#;
        let err = parse_manifest(body, "darwin-aarch64").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("url or sha256"), "msg: {msg}");
    }

    #[test]
    fn parse_manifest_bails_when_json_garbage() {
        let err = parse_manifest("not json", "darwin-aarch64").unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("decoding manifest"), "msg: {msg}");
    }

    #[test]
    fn sibling_service_path_lives_next_to_lns() {
        let lns_path = Path::new("/opt/homebrew/bin/lns");
        let svc = sibling_service_path(lns_path).unwrap();
        assert_eq!(svc, Path::new("/opt/homebrew/bin/lns-service"));
    }

    #[test]
    fn sibling_service_path_bails_when_lns_has_no_parent() {
        let lns_path = Path::new("/");
        let err = sibling_service_path(lns_path).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("no parent"), "msg: {msg}");
    }

    fn make_tarball(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        {
            let mut builder = tar::Builder::new(&mut gz);
            for (name, bytes) in entries {
                let mut header = tar::Header::new_gnu();
                header.set_path(name).unwrap();
                header.set_size(bytes.len() as u64);
                header.set_mode(0o755);
                header.set_cksum();
                builder.append(&header, *bytes).unwrap();
            }
            builder.finish().unwrap();
        }
        gz.finish().unwrap()
    }

    fn make_tarball_with_symlink(
        symlink_name: &str,
        link_target: &str,
        regular_entries: &[(&str, &[u8])],
    ) -> Vec<u8> {
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        {
            let mut builder = tar::Builder::new(&mut gz);
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_path(symlink_name).unwrap();
            header.set_link_name(link_target).unwrap();
            header.set_size(0);
            header.set_mode(0o777);
            header.set_cksum();
            builder.append(&header, std::io::empty()).unwrap();
            for (name, bytes) in regular_entries {
                let mut header = tar::Header::new_gnu();
                header.set_path(name).unwrap();
                header.set_size(bytes.len() as u64);
                header.set_mode(0o755);
                header.set_cksum();
                builder.append(&header, *bytes).unwrap();
            }
            builder.finish().unwrap();
        }
        gz.finish().unwrap()
    }

    #[test]
    fn extract_release_tarball_finds_both_binaries() {
        let tarball = make_tarball(&[("lns", b"LNS-BYTES"), ("lns-service", b"SVC-BYTES")]);
        let extracted = extract_release_tarball(&tarball).unwrap();
        assert_eq!(extracted.lns, b"LNS-BYTES");
        assert_eq!(extracted.lns_service.as_deref(), Some(&b"SVC-BYTES"[..]));
    }

    #[test]
    fn extract_release_tarball_tolerates_missing_service_binary() {
        let tarball = make_tarball(&[("lns", b"LNS-BYTES")]);
        let extracted = extract_release_tarball(&tarball).unwrap();
        assert_eq!(extracted.lns, b"LNS-BYTES");
        assert!(extracted.lns_service.is_none());
    }

    #[test]
    fn extract_release_tarball_finds_binaries_under_prefix_dir() {
        let tarball = make_tarball(&[
            ("lns-0.4.0-darwin-aarch64/lns", b"LNS-BYTES"),
            ("lns-0.4.0-darwin-aarch64/lns-service", b"SVC-BYTES"),
        ]);
        let extracted = extract_release_tarball(&tarball).unwrap();
        assert_eq!(extracted.lns, b"LNS-BYTES");
        assert_eq!(extracted.lns_service.as_deref(), Some(&b"SVC-BYTES"[..]));
    }

    #[test]
    fn extract_release_tarball_ignores_unrelated_files() {
        let tarball = make_tarball(&[("README", b"hello"), ("lns", b"LNS-BYTES")]);
        let extracted = extract_release_tarball(&tarball).unwrap();
        assert_eq!(extracted.lns, b"LNS-BYTES");
        assert!(extracted.lns_service.is_none());
    }

    #[test]
    fn extract_release_tarball_bails_when_lns_missing() {
        let tarball = make_tarball(&[("lns-service", b"SVC-BYTES")]);
        let err = extract_release_tarball(&tarball).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("did not contain"), "msg: {msg}");
    }

    #[test]
    fn extract_release_tarball_bails_on_non_gzip_bytes() {
        let err = extract_release_tarball(b"plain bytes, not gzip").unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("opening") || msg.contains("reading"),
            "tarball error context surfaces: {msg}"
        );
    }

    #[test]
    fn extract_release_tarball_skips_symlink_named_lns_and_bails_when_no_regular_lns() {
        let tarball = make_tarball_with_symlink("lns", "/etc/passwd", &[]);
        let err = extract_release_tarball(&tarball).unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("did not contain"), "msg: {msg}");
        assert!(
            msg.contains("regular"),
            "msg names the entry-type guard: {msg}"
        );
    }

    #[test]
    fn extract_release_tarball_prefers_regular_lns_over_symlink_imposter() {
        let tarball = make_tarball_with_symlink("lns", "/etc/passwd", &[("lns", b"REAL")]);
        let extracted = extract_release_tarball(&tarball).unwrap();
        assert_eq!(extracted.lns, b"REAL");
    }

    #[test]
    fn extract_release_tarball_skips_symlink_named_lns_service() {
        let tarball =
            make_tarball_with_symlink("lns-service", "/etc/passwd", &[("lns", b"LNS-BYTES")]);
        let extracted = extract_release_tarball(&tarball).unwrap();
        assert_eq!(extracted.lns, b"LNS-BYTES");
        assert!(
            extracted.lns_service.is_none(),
            "symlinked lns-service must not be accepted"
        );
    }

    #[test]
    fn extract_release_tarball_keeps_first_lns_when_duplicated() {
        let tarball = make_tarball(&[("lns", b"FIRST"), ("lns", b"SECOND")]);
        let extracted = extract_release_tarball(&tarball).unwrap();
        assert_eq!(extracted.lns, b"FIRST");
    }

    #[test]
    fn extract_release_tarball_keeps_first_lns_service_when_duplicated() {
        let tarball = make_tarball(&[
            ("lns", b"L"),
            ("lns-service", b"FIRST"),
            ("lns-service", b"SECOND"),
        ]);
        let extracted = extract_release_tarball(&tarball).unwrap();
        assert_eq!(extracted.lns_service.as_deref(), Some(&b"FIRST"[..]));
    }

    #[test]
    fn post_install_hint_only_fires_when_service_was_not_running() {
        assert_eq!(post_install_hint(true), None);
        assert_eq!(
            post_install_hint(false),
            Some("Run `lns service start` to launch the new background service.")
        );
    }

    #[derive(Default)]
    struct FakeServiceCtl {
        ping_responses: Mutex<std::collections::VecDeque<bool>>,
        shutdown_response: Mutex<Option<Option<()>>>,
        wait_for_stopped_response: Mutex<Option<bool>>,
        start_response: Mutex<Option<Result<bool, String>>>,
        calls: Mutex<Vec<String>>,
    }

    impl FakeServiceCtl {
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
        fn record(&self, m: &str) {
            self.calls.lock().unwrap().push(m.to_string());
        }
    }

    impl ServiceClient for FakeServiceCtl {
        fn ping(&self) -> crate::service::client::BoxFuture<'_, bool> {
            self.record("ping");
            let v = self
                .ping_responses
                .lock()
                .unwrap()
                .pop_front()
                .expect("FakeServiceCtl: no ping queued");
            Box::pin(async move { v })
        }
        fn shutdown(&self) -> crate::service::client::BoxFuture<'_, Option<()>> {
            self.record("shutdown");
            let v = self
                .shutdown_response
                .lock()
                .unwrap()
                .take()
                .expect("FakeServiceCtl: no shutdown queued");
            Box::pin(async move { v })
        }
        fn wait_for_stopped(
            &self,
            _total_timeout: Duration,
        ) -> crate::service::client::BoxFuture<'_, bool> {
            self.record("wait_for_stopped");
            let v = self
                .wait_for_stopped_response
                .lock()
                .unwrap()
                .take()
                .expect("FakeServiceCtl: no wait_for_stopped queued");
            Box::pin(async move { v })
        }
        fn start_and_wait_for_ready(
            &self,
            _total_timeout: Duration,
        ) -> crate::service::client::BoxFuture<'_, Result<bool>> {
            self.record("start_and_wait_for_ready");
            let v = self
                .start_response
                .lock()
                .unwrap()
                .take()
                .expect("FakeServiceCtl: no start_and_wait_for_ready queued");
            Box::pin(async move { v.map_err(anyhow::Error::msg) })
        }
        fn wait_for_ready(
            &self,
            _total_timeout: Duration,
        ) -> crate::service::client::BoxFuture<'_, bool> {
            unreachable!("update flow does not call wait_for_ready()")
        }
        fn status(&self) -> crate::service::client::BoxFuture<'_, Option<lns_ipc::StatusInfo>> {
            unreachable!("update flow does not call status()")
        }
        fn cancel_run(&self, _run_id: u32) -> crate::service::client::BoxFuture<'_, ()> {
            unreachable!("update flow does not call cancel_run()")
        }
    }

    #[test]
    #[should_panic(expected = "update flow does not call wait_for_ready()")]
    fn fake_service_ctl_wait_for_ready_is_a_regression_tripwire() {
        drop(FakeServiceCtl::default().wait_for_ready(Duration::from_secs(0)));
    }

    #[test]
    #[should_panic(expected = "update flow does not call status()")]
    fn fake_service_ctl_status_is_a_regression_tripwire() {
        drop(FakeServiceCtl::default().status());
    }

    #[test]
    #[should_panic(expected = "update flow does not call cancel_run()")]
    fn fake_service_ctl_cancel_run_is_a_regression_tripwire() {
        drop(FakeServiceCtl::default().cancel_run(0));
    }

    fn args_default() -> UpdateArgs {
        UpdateArgs {
            force: false,
            dry_run: false,
        }
    }

    fn sha_hex(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    const TARBALL_PATH: &str = "/lns.tar.gz";

    fn sample_manifest(version: &str) -> String {
        format!(
            r#"{{
              "version": "{version}",
              "tag": "lns-v{version}",
              "platforms": {{
                "darwin-aarch64": {{
                  "url": "https://get.lns.run/lns-{version}-darwin-aarch64.tar.gz",
                  "sha256": "{sha}"
                }},
                "linux-aarch64": {{
                  "url": "https://get.lns.run/lns-{version}-linux-aarch64.tar.gz",
                  "sha256": "{sha}"
                }},
                "linux-x86_64": {{
                  "url": "https://get.lns.run/lns-{version}-linux-x86_64.tar.gz",
                  "sha256": "{sha}"
                }}
              }}
            }}"#,
            sha = "0".repeat(64),
        )
    }

    fn manifest_for(version: &str, tarball_url: &str, tarball: &[u8]) -> String {
        format!(
            r#"{{
              "version": "{version}",
              "tag": "lns-v{version}",
              "platforms": {{
                "darwin-aarch64": {{
                  "url": "{tarball_url}",
                  "sha256": "{sha}"
                }}
              }}
            }}"#,
            sha = sha_hex(tarball),
        )
    }

    async fn mount_manifest(server: &MockServer, body: String) {
        server
            .mock_async(|when, then| {
                when.method(GET).path(format!("/{MANIFEST_NAME}"));
                then.status(200).body(body);
            })
            .await;
    }

    async fn mount_tarball(server: &MockServer, bytes: Vec<u8>) -> String {
        server
            .mock_async(|when, then| {
                when.method(GET).path(TARBALL_PATH);
                then.status(200).body(bytes);
            })
            .await;
        format!("{}{TARBALL_PATH}", server.base_url())
    }

    fn install_dir() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let lns = dir.path().join("lns");
        let svc = dir.path().join("lns-service");
        std::fs::write(&lns, b"").unwrap();
        std::fs::write(&svc, b"").unwrap();
        (dir, lns, svc)
    }

    fn block_install_at(target: &Path) {
        let tmp = target.with_extension("lns-update.tmp");
        std::fs::create_dir(&tmp).unwrap();
    }

    fn block_commit_at(dest: &Path) {
        let bak = dest.with_extension("lns-update.bak");
        std::fs::create_dir(&bak).unwrap();
        std::fs::write(bak.join("occupied"), b"x").unwrap();
    }

    async fn invoke_update(
        server: &MockServer,
        running_version: &str,
        lns_path: &Path,
        ctl: &FakeServiceCtl,
    ) -> Result<i32> {
        invoke_update_with(args_default(), server, running_version, lns_path, ctl).await
    }

    async fn invoke_update_with(
        args: UpdateArgs,
        server: &MockServer,
        running_version: &str,
        lns_path: &Path,
        ctl: &FakeServiceCtl,
    ) -> Result<i32> {
        run_with(
            args,
            running_version,
            &server.base_url(),
            &darwin_arm(),
            lns_path,
            ctl,
        )
        .await
    }

    fn init_tracing_capture() {
        use std::sync::Once;
        static ONCE: Once = Once::new();
        ONCE.call_once(|| {
            let subscriber = tracing_subscriber::fmt()
                .with_writer(std::io::sink)
                .with_ansi(false)
                .with_max_level(tracing::Level::TRACE)
                .finish();
            tracing::subscriber::set_global_default(subscriber).ok();
        });
    }

    fn fake_service_already_stopped() -> FakeServiceCtl {
        let ctl = FakeServiceCtl::default();
        ctl.ping_responses.lock().unwrap().push_back(false);
        ctl
    }

    #[tokio::test]
    async fn run_with_already_up_to_date_short_circuits() {
        init_tracing_capture();
        let server = MockServer::start_async().await;
        mount_manifest(&server, sample_manifest("0.4.0")).await;

        let (_dir, lns_bin, svc_bin) = install_dir();
        let ctl = fake_service_already_stopped();
        let code = invoke_update(&server, "0.4.0", &lns_bin, &ctl)
            .await
            .unwrap();
        assert_eq!(code, 0);
        assert!(std::fs::read(&lns_bin).unwrap().is_empty(), "lns untouched");
        assert!(std::fs::read(&svc_bin).unwrap().is_empty(), "svc untouched");
        assert!(ctl.calls().is_empty(), "service ctl untouched");
    }

    #[tokio::test]
    async fn run_with_force_reinstalls_when_already_up_to_date() {
        init_tracing_capture();
        let server = MockServer::start_async().await;
        let tarball_bytes = make_tarball(&[("lns", b"NEW"), ("lns-service", b"NEW-SVC")]);
        let tarball_url = mount_tarball(&server, tarball_bytes.clone()).await;
        mount_manifest(&server, manifest_for("0.4.0", &tarball_url, &tarball_bytes)).await;

        let (_dir, lns_bin, svc_bin) = install_dir();
        let ctl = fake_service_already_stopped();
        let mut args = args_default();
        args.force = true;

        let code = invoke_update_with(args, &server, "0.4.0", &lns_bin, &ctl)
            .await
            .unwrap();
        assert_eq!(code, 0);
        assert_eq!(std::fs::read(&lns_bin).unwrap(), b"NEW");
        assert_eq!(std::fs::read(&svc_bin).unwrap(), b"NEW-SVC");
    }

    #[tokio::test]
    async fn run_with_happy_path_replaces_both_binaries_and_restarts_service() {
        init_tracing_capture();
        let server = MockServer::start_async().await;
        let tarball_bytes = make_tarball(&[("lns", b"NEW-LNS"), ("lns-service", b"NEW-SVC")]);
        let tarball_url = mount_tarball(&server, tarball_bytes.clone()).await;
        mount_manifest(&server, manifest_for("0.4.0", &tarball_url, &tarball_bytes)).await;

        let (_dir, lns_bin, svc_bin) = install_dir();
        let ctl = FakeServiceCtl::default();
        ctl.ping_responses.lock().unwrap().push_back(true);
        *ctl.shutdown_response.lock().unwrap() = Some(Some(()));
        *ctl.wait_for_stopped_response.lock().unwrap() = Some(true);
        *ctl.start_response.lock().unwrap() = Some(Ok(true));

        let code = invoke_update(&server, "0.3.0", &lns_bin, &ctl)
            .await
            .unwrap();
        assert_eq!(code, 0);
        assert_eq!(
            ctl.calls(),
            vec![
                "ping",
                "shutdown",
                "wait_for_stopped",
                "start_and_wait_for_ready"
            ]
        );
        assert_eq!(std::fs::read(&lns_bin).unwrap(), b"NEW-LNS");
        assert_eq!(std::fs::read(&svc_bin).unwrap(), b"NEW-SVC");
        assert!(
            !lns_bin.with_extension("lns-update.bak").exists(),
            "lns backup discarded after a successful commit"
        );
        assert!(
            !svc_bin.with_extension("lns-update.bak").exists(),
            "lns-service backup discarded after a successful commit"
        );
    }

    #[tokio::test]
    async fn run_with_skips_service_lifecycle_when_service_was_not_running() {
        init_tracing_capture();
        let server = MockServer::start_async().await;
        let tarball_bytes = make_tarball(&[("lns", b"NEW"), ("lns-service", b"NEW-SVC")]);
        let tarball_url = mount_tarball(&server, tarball_bytes.clone()).await;
        mount_manifest(&server, manifest_for("0.4.0", &tarball_url, &tarball_bytes)).await;

        let (_dir, lns_bin, _svc_bin) = install_dir();
        let ctl = fake_service_already_stopped();

        let code = invoke_update(&server, "0.3.0", &lns_bin, &ctl)
            .await
            .unwrap();
        assert_eq!(code, 0);
        assert_eq!(ctl.calls(), vec!["ping"]);
    }

    #[tokio::test]
    async fn run_with_tarball_with_only_lns_installs_lns_and_warns_on_missing_service() {
        init_tracing_capture();
        let server = MockServer::start_async().await;
        let tarball_bytes = make_tarball(&[("lns", b"NEW-LNS")]);
        let tarball_url = mount_tarball(&server, tarball_bytes.clone()).await;
        mount_manifest(&server, manifest_for("0.4.0", &tarball_url, &tarball_bytes)).await;

        let (_dir, lns_bin, svc_bin) = install_dir();
        let ctl = fake_service_already_stopped();

        let code = invoke_update(&server, "0.3.0", &lns_bin, &ctl)
            .await
            .unwrap();
        assert_eq!(code, 0);
        assert_eq!(std::fs::read(&lns_bin).unwrap(), b"NEW-LNS");
        assert!(
            std::fs::read(&svc_bin).unwrap().is_empty(),
            "lns-service must not be written when the tarball omits it",
        );
    }

    #[tokio::test]
    async fn run_with_sha_mismatch_bails_before_any_write_or_service_lifecycle() {
        init_tracing_capture();
        let server = MockServer::start_async().await;
        let bad_tarball = make_tarball(&[("lns", b"TAMPERED")]);
        let tarball_url = mount_tarball(&server, bad_tarball).await;
        let body = format!(
            r#"{{"version":"0.4.0","platforms":{{"darwin-aarch64":{{"url":"{tarball_url}","sha256":"{sha}"}}}}}}"#,
            sha = sha_hex(b"different bytes"),
        );
        mount_manifest(&server, body).await;

        let (_dir, lns_bin, svc_bin) = install_dir();
        let ctl = FakeServiceCtl::default();

        let err = invoke_update(&server, "0.3.0", &lns_bin, &ctl)
            .await
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("sha256 mismatch"), "msg: {msg}");
        assert!(msg.contains("Refusing to install"), "msg: {msg}");
        assert!(std::fs::read(&lns_bin).unwrap().is_empty());
        assert!(std::fs::read(&svc_bin).unwrap().is_empty());
        assert!(ctl.calls().is_empty(), "service must not be touched");
    }

    #[tokio::test]
    async fn run_with_manifest_fetch_error_propagates_with_url_context() {
        init_tracing_capture();
        let server = MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.method(GET).path(format!("/{MANIFEST_NAME}"));
                then.status(503);
            })
            .await;

        let (_dir, lns_bin, _svc_bin) = install_dir();
        let ctl = FakeServiceCtl::default();

        let err = invoke_update(&server, "0.3.0", &lns_bin, &ctl)
            .await
            .unwrap_err();
        let msg = format!("{err:#}");
        let manifest_url = format!("{}/{MANIFEST_NAME}", server.base_url());
        assert!(msg.contains("fetching"), "msg: {msg}");
        assert!(msg.contains(&manifest_url), "url in msg: {msg}");
        assert!(msg.contains("HTTP error from"), "msg: {msg}");
    }

    #[tokio::test]
    async fn run_with_non_utf8_manifest_bails() {
        let server = MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.method(GET).path(format!("/{MANIFEST_NAME}"));
                then.status(200).body(vec![0xFFu8, 0xFE, 0xFD]);
            })
            .await;

        let (_dir, lns_bin, _svc_bin) = install_dir();
        let ctl = FakeServiceCtl::default();

        let err = invoke_update(&server, "0.3.0", &lns_bin, &ctl)
            .await
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("non-UTF-8"), "msg: {msg}");
    }

    #[tokio::test]
    async fn run_with_tarball_missing_lns_bails_before_install() {
        init_tracing_capture();
        let server = MockServer::start_async().await;
        let tarball_bytes = make_tarball(&[("lns-service", b"SVC-ONLY")]);
        let tarball_url = mount_tarball(&server, tarball_bytes.clone()).await;
        mount_manifest(&server, manifest_for("0.4.0", &tarball_url, &tarball_bytes)).await;

        let (_dir, lns_bin, svc_bin) = install_dir();
        let ctl = fake_service_already_stopped();

        let err = invoke_update(&server, "0.3.0", &lns_bin, &ctl)
            .await
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("did not contain"), "msg: {msg}");
        assert!(std::fs::read(&lns_bin).unwrap().is_empty());
        assert!(std::fs::read(&svc_bin).unwrap().is_empty());
    }

    #[tokio::test]
    async fn run_with_stage_write_failure_is_actionable_and_never_touches_service() {
        init_tracing_capture();
        let server = MockServer::start_async().await;
        let tarball_bytes = make_tarball(&[("lns", b"NEW")]);
        let tarball_url = mount_tarball(&server, tarball_bytes.clone()).await;
        mount_manifest(&server, manifest_for("0.4.0", &tarball_url, &tarball_bytes)).await;

        let (_dir, lns_bin, svc_bin) = install_dir();
        block_install_at(&lns_bin);
        let ctl = FakeServiceCtl::default();

        let err = invoke_update(&server, "0.3.0", &lns_bin, &ctl)
            .await
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("writing"), "msg: {msg}");
        assert!(msg.contains("sudo"), "actionable hint: {msg}");
        assert!(
            ctl.calls().is_empty(),
            "service must not be contacted on a stage failure"
        );
        assert!(std::fs::read(&lns_bin).unwrap().is_empty());
        assert!(std::fs::read(&svc_bin).unwrap().is_empty());
    }

    #[tokio::test]
    async fn run_with_service_shutdown_timeout_bails_before_replacing_binaries() {
        init_tracing_capture();
        let server = MockServer::start_async().await;
        let tarball_bytes = make_tarball(&[("lns", b"NEW"), ("lns-service", b"NEW-SVC")]);
        let tarball_url = mount_tarball(&server, tarball_bytes.clone()).await;
        mount_manifest(&server, manifest_for("0.4.0", &tarball_url, &tarball_bytes)).await;

        let (_dir, lns_bin, svc_bin) = install_dir();
        let ctl = FakeServiceCtl::default();
        ctl.ping_responses.lock().unwrap().push_back(true);
        *ctl.shutdown_response.lock().unwrap() = Some(Some(()));
        *ctl.wait_for_stopped_response.lock().unwrap() = Some(false);

        let err = invoke_update(&server, "0.3.0", &lns_bin, &ctl)
            .await
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("did not exit"), "msg: {msg}");
        assert_eq!(ctl.calls(), vec!["ping", "shutdown", "wait_for_stopped"]);
        assert!(std::fs::read(&lns_bin).unwrap().is_empty(), "no installs");
        assert!(std::fs::read(&svc_bin).unwrap().is_empty(), "no installs");
        assert!(
            !lns_bin.with_extension("lns-update.tmp").exists(),
            "staged lns tmp cleaned up after stop failure"
        );
        assert!(
            !svc_bin.with_extension("lns-update.tmp").exists(),
            "staged svc tmp cleaned up after stop failure"
        );
    }

    #[tokio::test]
    async fn run_with_service_shutdown_race_treats_unreachable_as_stopped() {
        init_tracing_capture();
        let server = MockServer::start_async().await;
        let tarball_bytes = make_tarball(&[("lns", b"NEW")]);
        let tarball_url = mount_tarball(&server, tarball_bytes.clone()).await;
        mount_manifest(&server, manifest_for("0.4.0", &tarball_url, &tarball_bytes)).await;

        let (_dir, lns_bin, _svc_bin) = install_dir();
        let ctl = FakeServiceCtl::default();
        ctl.ping_responses.lock().unwrap().push_back(true);
        *ctl.shutdown_response.lock().unwrap() = Some(None);
        *ctl.start_response.lock().unwrap() = Some(Ok(true));

        let code = invoke_update(&server, "0.3.0", &lns_bin, &ctl)
            .await
            .unwrap();
        assert_eq!(code, 0);
        assert_eq!(
            ctl.calls(),
            vec!["ping", "shutdown", "start_and_wait_for_ready"]
        );
    }

    #[tokio::test]
    async fn run_with_service_restart_failure_surfaces_actionable_message() {
        init_tracing_capture();
        let server = MockServer::start_async().await;
        let tarball_bytes = make_tarball(&[("lns", b"NEW"), ("lns-service", b"NEW-SVC")]);
        let tarball_url = mount_tarball(&server, tarball_bytes.clone()).await;
        mount_manifest(&server, manifest_for("0.4.0", &tarball_url, &tarball_bytes)).await;

        let (_dir, lns_bin, _svc_bin) = install_dir();
        let ctl = FakeServiceCtl::default();
        ctl.ping_responses.lock().unwrap().push_back(true);
        *ctl.shutdown_response.lock().unwrap() = Some(Some(()));
        *ctl.wait_for_stopped_response.lock().unwrap() = Some(true);
        *ctl.start_response.lock().unwrap() = Some(Ok(false));

        let err = invoke_update(&server, "0.3.0", &lns_bin, &ctl)
            .await
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("did not become ready"), "msg: {msg}");
        assert!(msg.contains("lns service status"), "actionable hint: {msg}");
    }

    #[tokio::test]
    async fn run_with_commit_failure_relaunches_previous_binary() {
        init_tracing_capture();
        let server = MockServer::start_async().await;
        let tarball_bytes = make_tarball(&[("lns", b"NEW")]);
        let tarball_url = mount_tarball(&server, tarball_bytes.clone()).await;
        mount_manifest(&server, manifest_for("0.4.0", &tarball_url, &tarball_bytes)).await;

        let (_dir, lns_bin, _svc_bin) = install_dir();
        block_commit_at(&lns_bin);

        let ctl = FakeServiceCtl::default();
        ctl.ping_responses.lock().unwrap().push_back(true);
        *ctl.shutdown_response.lock().unwrap() = Some(Some(()));
        *ctl.wait_for_stopped_response.lock().unwrap() = Some(true);
        *ctl.start_response.lock().unwrap() = Some(Ok(true));

        let err = invoke_update(&server, "0.3.0", &lns_bin, &ctl)
            .await
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("committing"), "commit error wins: {msg}");
        assert_eq!(
            ctl.calls(),
            vec![
                "ping",
                "shutdown",
                "wait_for_stopped",
                "start_and_wait_for_ready"
            ]
        );
        assert!(
            !lns_bin.with_extension("lns-update.tmp").exists(),
            "staged tmp cleaned up after commit failure"
        );
    }

    #[tokio::test]
    async fn run_with_service_stage_failure_leaves_no_skew_and_never_stops_service() {
        init_tracing_capture();
        let server = MockServer::start_async().await;
        let tarball_bytes = make_tarball(&[("lns", b"NEW"), ("lns-service", b"NEW-SVC")]);
        let tarball_url = mount_tarball(&server, tarball_bytes.clone()).await;
        mount_manifest(&server, manifest_for("0.4.0", &tarball_url, &tarball_bytes)).await;

        let (_dir, lns_bin, svc_bin) = install_dir();
        block_install_at(&svc_bin);

        let ctl = FakeServiceCtl::default();

        let err = invoke_update(&server, "0.3.0", &lns_bin, &ctl)
            .await
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("writing") && msg.contains("lns-service"),
            "service stage error names lns-service: {msg}"
        );
        assert!(ctl.calls().is_empty(), "service must not be stopped");
        assert!(
            std::fs::read(&lns_bin).unwrap().is_empty(),
            "lns must not be committed when the service binary fails to stage"
        );
        assert!(std::fs::read(&svc_bin).unwrap().is_empty());
        assert!(
            !lns_bin.with_extension("lns-update.tmp").exists(),
            "the already-staged lns tmp is cleaned up"
        );
    }

    #[tokio::test]
    async fn run_with_commit_failure_when_best_effort_restart_also_fails_surfaces_commit_error() {
        init_tracing_capture();
        let server = MockServer::start_async().await;
        let tarball_bytes = make_tarball(&[("lns", b"NEW")]);
        let tarball_url = mount_tarball(&server, tarball_bytes.clone()).await;
        mount_manifest(&server, manifest_for("0.4.0", &tarball_url, &tarball_bytes)).await;

        let (_dir, lns_bin, _svc_bin) = install_dir();
        block_commit_at(&lns_bin);

        let ctl = FakeServiceCtl::default();
        ctl.ping_responses.lock().unwrap().push_back(true);
        *ctl.shutdown_response.lock().unwrap() = Some(Some(()));
        *ctl.wait_for_stopped_response.lock().unwrap() = Some(true);
        *ctl.start_response.lock().unwrap() = Some(Err("spawn refused".to_string()));

        let err = invoke_update(&server, "0.3.0", &lns_bin, &ctl)
            .await
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("committing"),
            "commit error wins over restart error: {msg}"
        );
        assert!(
            !msg.contains("spawn refused"),
            "restart failure must not displace the commit error: {msg}"
        );
        let restart_calls = ctl
            .calls()
            .iter()
            .filter(|c| c.as_str() == "start_and_wait_for_ready")
            .count();
        assert_eq!(restart_calls, 1, "exactly one restart attempt");
    }

    #[tokio::test]
    async fn run_with_commit_failure_best_effort_restart_handles_not_ready_arm() {
        init_tracing_capture();
        let server = MockServer::start_async().await;
        let tarball_bytes = make_tarball(&[("lns", b"NEW")]);
        let tarball_url = mount_tarball(&server, tarball_bytes.clone()).await;
        mount_manifest(&server, manifest_for("0.4.0", &tarball_url, &tarball_bytes)).await;

        let (_dir, lns_bin, _svc_bin) = install_dir();
        block_commit_at(&lns_bin);

        let ctl = FakeServiceCtl::default();
        ctl.ping_responses.lock().unwrap().push_back(true);
        *ctl.shutdown_response.lock().unwrap() = Some(Some(()));
        *ctl.wait_for_stopped_response.lock().unwrap() = Some(true);
        *ctl.start_response.lock().unwrap() = Some(Ok(false));

        let err = invoke_update(&server, "0.3.0", &lns_bin, &ctl)
            .await
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("committing"), "commit error wins: {msg}");
        let restart_calls = ctl
            .calls()
            .iter()
            .filter(|c| c.as_str() == "start_and_wait_for_ready")
            .count();
        assert_eq!(restart_calls, 1);
    }

    #[tokio::test]
    async fn run_with_commit_failure_with_service_stopped_skips_relaunch() {
        init_tracing_capture();
        let server = MockServer::start_async().await;
        let tarball_bytes = make_tarball(&[("lns", b"NEW")]);
        let tarball_url = mount_tarball(&server, tarball_bytes.clone()).await;
        mount_manifest(&server, manifest_for("0.4.0", &tarball_url, &tarball_bytes)).await;

        let (_dir, lns_bin, _svc_bin) = install_dir();
        block_commit_at(&lns_bin);

        let ctl = fake_service_already_stopped();

        let err = invoke_update(&server, "0.3.0", &lns_bin, &ctl)
            .await
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("committing"), "commit error: {msg}");
        assert_eq!(
            ctl.calls(),
            vec!["ping"],
            "no relaunch attempted when the service wasn't running"
        );
    }

    #[tokio::test]
    async fn run_with_second_commit_failure_rolls_back_first_binary_no_skew() {
        init_tracing_capture();
        let server = MockServer::start_async().await;
        let tarball_bytes = make_tarball(&[("lns", b"NEW-LNS"), ("lns-service", b"NEW-SVC")]);
        let tarball_url = mount_tarball(&server, tarball_bytes.clone()).await;
        mount_manifest(&server, manifest_for("0.4.0", &tarball_url, &tarball_bytes)).await;

        let (_dir, lns_bin, svc_bin) = install_dir();
        block_commit_at(&svc_bin);

        let ctl = FakeServiceCtl::default();
        ctl.ping_responses.lock().unwrap().push_back(true);
        *ctl.shutdown_response.lock().unwrap() = Some(Some(()));
        *ctl.wait_for_stopped_response.lock().unwrap() = Some(true);
        *ctl.start_response.lock().unwrap() = Some(Ok(true));

        let err = invoke_update(&server, "0.3.0", &lns_bin, &ctl)
            .await
            .unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("committing") && msg.contains("lns-service"),
            "commit error names lns-service: {msg}"
        );
        assert!(
            std::fs::read(&lns_bin).unwrap().is_empty(),
            "lns must be rolled back to its old copy, not left as the new build"
        );
        assert!(std::fs::read(&svc_bin).unwrap().is_empty());
        assert!(
            !lns_bin.with_extension("lns-update.tmp").exists(),
            "staged lns tmp cleaned up after rollback"
        );
        assert!(
            !lns_bin.with_extension("lns-update.bak").exists(),
            "lns backup consumed by the rollback"
        );
        assert_eq!(
            ctl.calls(),
            vec![
                "ping",
                "shutdown",
                "wait_for_stopped",
                "start_and_wait_for_ready"
            ]
        );
    }

    #[tokio::test]
    async fn run_with_commit_installs_when_no_prior_binary_to_back_up() {
        init_tracing_capture();
        let server = MockServer::start_async().await;
        let tarball_bytes = make_tarball(&[("lns", b"NEW-LNS"), ("lns-service", b"NEW-SVC")]);
        let tarball_url = mount_tarball(&server, tarball_bytes.clone()).await;
        mount_manifest(&server, manifest_for("0.4.0", &tarball_url, &tarball_bytes)).await;

        let (_dir, lns_bin, svc_bin) = install_dir();
        std::fs::remove_file(&svc_bin).unwrap();
        let ctl = fake_service_already_stopped();

        let code = invoke_update(&server, "0.3.0", &lns_bin, &ctl)
            .await
            .unwrap();
        assert_eq!(code, 0);
        assert_eq!(std::fs::read(&lns_bin).unwrap(), b"NEW-LNS");
        assert_eq!(std::fs::read(&svc_bin).unwrap(), b"NEW-SVC");
    }

    #[tokio::test]
    async fn http_client_emits_install_script_shaped_user_agent() {
        let server = MockServer::start_async().await;
        let mock = server
            .mock_async(|when, then| {
                when.method(GET).path("/probe").header_matches(
                    "user-agent",
                    r"^lns/[^ ]+ \(os=[^;]+; arch=[^;]+; kernel=[^/]+/[^;]+; shell=[^;]+; method=cli-update\)$",
                );
                then.status(200).body("ok");
            })
            .await;

        let client = http_client("0.3.0", &darwin_arm()).unwrap();
        let bytes = http_get_bytes(&client, &format!("{}/probe", server.base_url()))
            .await
            .unwrap();
        assert_eq!(bytes, b"ok");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn http_get_bytes_wraps_http_error_with_url_context() {
        let server = MockServer::start_async().await;
        server
            .mock_async(|when, then| {
                when.method(GET);
                then.status(503);
            })
            .await;
        let url = format!("{}/manifest", server.base_url());
        let client = http_client("0.3.0", &darwin_arm()).unwrap();
        let err = http_get_bytes(&client, &url).await.unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("HTTP error from"), "msg: {msg}");
        assert!(msg.contains(&url), "msg: {msg}");
    }

    #[tokio::test]
    async fn http_get_bytes_wraps_transport_error_with_url_context() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        let url = format!("http://{addr}/manifest");
        let client = http_client("0.3.0", &darwin_arm()).unwrap();
        let err = http_get_bytes(&client, &url).await.unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("downloading"), "msg: {msg}");
        assert!(msg.contains(&url), "msg: {msg}");
    }
}
