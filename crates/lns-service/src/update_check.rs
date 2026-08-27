use anyhow::{Context, Result, bail};
use lns_ipc::UpdateStatus;
use serde::Deserialize;
use std::time::Duration;

mod real;
mod traits;

pub use real::run_periodic;
use traits::{Clock, Fetcher, StateStore};

const CDN_BASE: &str = "https://get.lns.run";
const MANIFEST_NAME: &str = "lns-latest.json";
pub(super) const MAX_MANIFEST_BYTES: u64 = 1024 * 1024;

pub const CHECK_INTERVAL: Duration = Duration::from_secs(60 * 60);
pub const DISABLE_ENV: &str = lns_ipc::NO_UPDATE_CHECK_ENV;

fn generate_install_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn parse_manifest(bytes: &[u8]) -> Result<(String, Option<String>)> {
    #[derive(Deserialize)]
    struct Raw {
        version: String,
        #[serde(default)]
        min_secure_version: Option<String>,
    }
    let raw: Raw = serde_json::from_slice(bytes).context("decoding manifest JSON")?;
    if raw.version.is_empty() {
        bail!("manifest is missing the required `version` field");
    }
    Ok((raw.version, raw.min_secure_version))
}

fn ensure_install_id(store: &impl StateStore) -> Result<String> {
    match store.read_install_id()? {
        Some(id) => Ok(id),
        None => {
            let id = generate_install_id();
            store.write_install_id(&id)?;
            Ok(id)
        }
    }
}

fn establish_install_id(
    env_get: impl Fn(&str) -> Option<String>,
    store: &impl StateStore,
) -> Result<()> {
    if env_get(DISABLE_ENV).is_some() {
        return Ok(());
    }
    ensure_install_id(store).map(|_| ())
}

async fn check_once(
    env_get: impl Fn(&str) -> Option<String>,
    fetcher: &impl Fetcher,
    store: &impl StateStore,
    clock: &impl Clock,
    cdn_base: &str,
    user_agent: &str,
) -> Result<()> {
    if env_get(DISABLE_ENV).is_some() {
        return Ok(());
    }
    let install_id = ensure_install_id(store)?;
    let url = format!("{cdn_base}/{MANIFEST_NAME}");
    let bytes = fetcher.get_manifest(&url, user_agent, &install_id).await?;
    let (latest, min_secure_version) = parse_manifest(&bytes)?;
    store.write_status(&UpdateStatus {
        latest,
        min_secure_version,
        checked_at_unix: clock.now_unix(),
    })
}

#[cfg(test)]
mod tests;
