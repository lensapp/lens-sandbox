use anyhow::Context;
use tokio::sync::Mutex;

use super::provisioner::real::MiseProvisioner;
use super::{
    EnsuredTools, Libc, ProvisionError, ProvisionTarget, ToolRef, ensure_tools_with_pins, mise,
    prepare_tools,
};
use crate::content_store::ContentStore;

static PROVISION_LOCK: Mutex<()> = Mutex::const_new(());

/// Installing is serialized service-wide, so a second cold run can wait out the first — up to the driver timeout. Say so instead of looking like a hang.
async fn serialized_install() -> tokio::sync::MutexGuard<'static, ()> {
    match PROVISION_LOCK.try_lock() {
        Ok(held) => held,
        Err(_) => {
            crate::log::progress("Waiting", "for another sandbox's tool install", 0, 0);
            PROVISION_LOCK.lock().await
        }
    }
}

async fn cached_or_serialized<T, E, G>(
    cached: impl std::future::Future<Output = Result<Option<T>, E>>,
    serialized: impl std::future::Future<Output = G>,
    install: impl AsyncFnOnce() -> Result<T, E>,
) -> Result<T, E> {
    if let Some(value) = cached.await? {
        return Ok(value);
    }
    let _serialized = serialized.await;
    install().await
}

/// Composition root for a run's declared tools: real record store, real cache over the content store, and the mise provisioner. Installing is serialized across the service's runs so two of them provision a shared tool set once — but a run with nothing to install never joins that queue, or it would wait out an unrelated cold provision.
pub async fn ensure_for_run(
    scratch_id: &str,
    content_store: &ContentStore,
    requests: &[ToolRef],
    target: &ProvisionTarget,
    disclose: &(dyn Fn(&super::ProvisionOutcome) + Send + Sync),
) -> Result<EnsuredTools, ProvisionError> {
    let cache_dir = cache_dir()?;
    let tools_root = cache_dir.join("tools");
    let records = super::record::RealRecordStore::new(&tools_root);
    let cache = super::cache::RealToolCache::new(
        &tools_root,
        content_store.clone(),
        mise::engine_version(),
    );
    let provisioner = MiseProvisioner {
        scratch_id: scratch_id.to_string(),
    };
    let ask = super::EnsureRequest {
        requests,
        target,
        engine_version: mise::engine_version(),
        now_unix_secs: now_unix_secs(),
        disclose,
    };
    let prepared = prepare_tools(&records, &cache, &provisioner, &ask).await?;
    let mut ensured = match prepared.ready {
        Some(ensured) => ensured,
        None => {
            let _serialized = serialized_install().await;
            let scratch = ScratchGuard(cache_dir.join("runs").join(scratch_id));
            let ensured =
                ensure_tools_with_pins(&records, &cache, &provisioner, &ask, &prepared.pins)
                    .await?;
            drop(scratch);
            ensured
        }
    };
    if target.libc == Libc::Musl
        && requests
            .iter()
            .any(|request| super::registry::needs_musl_companions(&request.name, target.libc))
    {
        let companions = cached_or_serialized(
            super::provisioner::real::cached_workload_companion_specs(&cache_dir, target),
            serialized_install(),
            || super::provisioner::real::workload_companion_specs(&cache_dir, target),
        )
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
) -> Result<(), ProvisionError> {
    if artifact.tools.is_empty() {
        return Ok(());
    }
    let requests = lns_artifact::tools::parse_all(&artifact.tools)
        .map_err(|e| ProvisionError::Engine(format!("{e:#}")))?;
    super::registry::refuse_unknown_tools(&requests)?;
    let layers: Vec<&[u8]> = base_image
        .layers
        .iter()
        .map(|layer| &layer.data[..])
        .collect();
    let target = ProvisionTarget {
        arch: super::host_arch(),
        libc: super::libc::detect_libc_off_runtime(&base_image.layer_digests, &layers)
            .map_err(|e| ProvisionError::Engine(format!("reading the base image: {e:#}")))?,
    };
    super::registry::refuse_libc_unsupported(&requests, &target, &artifact.base_image)?;
    let content_store = crate::content_store::ContentStore::new(cache_dir()?.join("content"));
    let scratch_id = format!(
        "pull-{}",
        artifact
            .digest
            .trim_start_matches("sha256:")
            .get(..12)
            .unwrap_or("tools")
    );
    let cx =
        crate::ocsf_audit::OcsfCtx::at_unix(scratch_id.clone(), String::new(), now_unix_secs());
    ensure_for_run(
        &scratch_id,
        &content_store,
        &requests,
        &target,
        &|outcome| {
            crate::log::info!(
                "Provisioned",
                "{} → {} (cached for offline start)",
                outcome.requested,
                outcome.resolved
            );
            // The tool is already cached; a chain that cannot be written is a warning, never a reason to undo an acquisition.
            if let Err(e) = crate::ledger::append_tool_provisioned(&cx, outcome) {
                crate::log::warn!(
                    "could not record the provisioned tool in the machine ledger: {e:#}"
                );
            }
        },
    )
    .await?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cold_tool_installs_remain_serialized() {
        let first = serialized_install().await;
        let (attempted_tx, attempted_rx) = tokio::sync::oneshot::channel();
        let (acquired_tx, mut acquired_rx) = tokio::sync::oneshot::channel();
        let second = tokio::spawn(async move {
            attempted_tx.send(()).unwrap();
            let _second = serialized_install().await;
            acquired_tx.send(()).unwrap();
        });

        attempted_rx.await.unwrap();
        tokio::task::yield_now().await;
        assert!(
            acquired_rx.try_recv().is_err(),
            "a second cold install must wait for the first"
        );

        drop(first);
        acquired_rx.await.unwrap();
        second.await.unwrap();
    }

    #[tokio::test]
    async fn cached_companions_do_not_wait_for_the_install_lock() {
        let cached = async { Ok::<_, &'static str>(Some("cached")) };
        let unavailable_lock = std::future::pending::<()>();
        let result = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            cached_or_serialized(cached, unavailable_lock, || async {
                Err::<&str, _>("the install path must not run")
            }),
        )
        .await
        .expect("a warm run must not wait for the install lock")
        .unwrap();

        assert_eq!(result, "cached");
    }

    #[tokio::test]
    async fn missing_companions_wait_for_the_lock_before_installing() {
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let (installed_tx, mut installed_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            cached_or_serialized(
                async { Ok::<Option<&str>, &'static str>(None) },
                async { release_rx.await.unwrap() },
                || async {
                    installed_tx.send(()).unwrap();
                    Ok("installed")
                },
            )
            .await
        });

        tokio::task::yield_now().await;
        assert!(
            installed_rx.try_recv().is_err(),
            "a cache miss must not install before acquiring the lock"
        );
        release_tx.send(()).unwrap();
        installed_rx.await.unwrap();
        assert_eq!(task.await.unwrap().unwrap(), "installed");
    }
}
