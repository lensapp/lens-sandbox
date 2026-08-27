use anyhow::{Context, Result};
use tokio::sync::OnceCell;

use super::{Fetcher, GuestTools, PACKAGES, cache_subpath, ensure_with, sha256_hex};

static BUILD_ID: OnceCell<String> = OnceCell::const_new();

pub(super) struct RealFetcher;

fn package_client() -> &'static reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(crate::identity::header(lns_ipc::Method::AssetDownload))
            .build()
            // Same failure reqwest::get panics on: the process has no usable TLS backend, so there is nothing to fall back to.
            .expect("a TLS-backed HTTP client")
    })
}

impl Fetcher for RealFetcher {
    async fn fetch(&self, url: &str) -> Result<Vec<u8>> {
        let resp = package_client()
            .get(url)
            .send()
            .await
            .with_context(|| format!("downloading {url}"))?
            .error_for_status()
            .with_context(|| format!("downloading {url}"))?;
        let bytes = resp
            .bytes()
            .await
            .with_context(|| format!("reading body of {url}"))?;
        Ok(bytes.to_vec())
    }
}

pub async fn ensure() -> Result<GuestTools> {
    let cache = cache_subpath(&crate::cache::root()?);
    let build_id = BUILD_ID.get_or_try_init(compute_build_id).await?;
    ensure_with(&RealFetcher, cache, build_id, PACKAGES).await
}

async fn compute_build_id() -> Result<String> {
    let exe = std::env::current_exe()
        .context("resolving current executable to key the guest-tools cache")?;
    tokio::task::spawn_blocking(move || {
        let bytes = std::fs::read(&exe)
            .with_context(|| format!("reading {} to key the guest-tools cache", exe.display()))?;
        anyhow::Ok(sha256_hex(&bytes))
    })
    .await?
}
