use anyhow::Context;
use tokio::sync::Mutex;

use super::provisioner::real::MiseProvisioner;
use super::{EnsuredTools, Libc, ProvisionError, ProvisionTarget, ToolRef, ensure_tools, mise};
use crate::content_store::ContentStore;

static PROVISION_LOCK: Mutex<()> = Mutex::const_new(());

/// Composition root for a run's declared tools: real record store, real cache over the content store, and the mise provisioner, serialized machine-wide so two concurrent runs provision a shared tool set once.
pub async fn ensure_for_run(
    scratch_id: &str,
    content_store: &ContentStore,
    requests: &[ToolRef],
    target: &ProvisionTarget,
) -> Result<EnsuredTools, ProvisionError> {
    let _serialized = PROVISION_LOCK.lock().await;
    let cache_dir = cache_dir()?;
    let tools_root = cache_dir.join("tools");
    let records = super::record::RealRecordStore::new(&tools_root);
    let cache = super::cache::RealToolCache::new(
        &tools_root,
        content_store.clone(),
        mise::engine_version(),
    );
    let scratch = ScratchGuard(cache_dir.join("runs").join(scratch_id));
    let provisioner = MiseProvisioner {
        scratch_id: scratch_id.to_string(),
    };
    let mut ensured = ensure_tools(
        &records,
        &cache,
        &provisioner,
        requests,
        target,
        mise::engine_version(),
        now_unix_secs(),
    )
    .await?;
    drop(scratch);
    if target.libc == Libc::Musl
        && requests
            .iter()
            .any(|request| super::registry::needs_musl_companions(&request.name, target.libc))
    {
        let companions = super::provisioner::real::workload_companion_specs(&cache_dir, target)
            .await
            .map_err(|e| ProvisionError::Engine(format!("staging musl companions: {e:#}")))?;
        ensured.specs.extend(companions);
    }
    Ok(ensured)
}

/// A pulled sandbox keeps the documented offline-start promise only if its (push-pinned) tools are provisioned while we're online; no run exists yet, so like the image fetch itself this is disclosed in the pull output rather than a run chain.
pub async fn pre_provision_for_pull(
    artifact: &crate::image::PulledArtifact,
    base_image: &crate::image::PulledImage,
) -> anyhow::Result<()> {
    if artifact.tools.is_empty() {
        return Ok(());
    }
    let requests = lns_artifact::tools::parse_all(&artifact.tools)?;
    super::registry::refuse_unknown_tools(&requests)?;
    let layers: Vec<Vec<u8>> = base_image
        .layers
        .iter()
        .map(|layer| layer.data.to_vec())
        .collect();
    let target = ProvisionTarget {
        arch: super::host_arch(),
        libc: super::libc::detect_libc(&layers)?,
    };
    super::registry::refuse_libc_unsupported(&requests, &target, &artifact.base_image)?;
    let content_store =
        crate::content_store::ContentStore::new(crate::cache::root()?.join("content"));
    let scratch_id = format!(
        "pull-{}",
        artifact
            .digest
            .trim_start_matches("sha256:")
            .get(..12)
            .unwrap_or("tools")
    );
    let ensured = ensure_for_run(&scratch_id, &content_store, &requests, &target).await?;
    for outcome in &ensured.provisioned {
        crate::log::info!(
            "Provisioned",
            "{} → {} (cached for offline start)",
            outcome.requested,
            outcome.resolved
        );
    }
    Ok(())
}

struct ScratchGuard(std::path::PathBuf);

impl Drop for ScratchGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn cache_dir() -> Result<std::path::PathBuf, ProvisionError> {
    crate::cache::root()
        .context("resolving the cache root")
        .map_err(|e| ProvisionError::Engine(format!("{e:#}")))
}

fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
