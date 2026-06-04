use anyhow::{Context, Result};

use super::{Fetcher, GuestTools, PACKAGES, cache_subpath, ensure_with};

pub(super) struct RealFetcher;

impl Fetcher for RealFetcher {
    async fn fetch(&self, url: &str) -> Result<Vec<u8>> {
        let resp = reqwest::get(url)
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
    ensure_with(&RealFetcher, cache, PACKAGES).await
}
