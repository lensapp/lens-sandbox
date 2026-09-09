use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::image::manifest_cache::ManifestCache;
use crate::log;
use crate::oci_layer_cache::LayerCache;

use super::ext4_upper::Ext4Upper;
use super::image::ParentImage;
use super::import::LocalStore;
use super::{BuiltLayer, upper};

/// The test-only hook that turns a run into slice 1's capture. Slice 3 replaces it with the executor, which decides per instruction instead of per run.
pub const CAPTURE_HOOK_ENV: &str = "LNS_SPIKE_CONTAINERFILE_LAYER";

/// Where the reference of the last built image lands, so the scenario that boots a second guest from it has something to read.
pub const BUILT_REFERENCE_FILE: &str = "built-image";

pub fn capture_hook_enabled() -> bool {
    std::env::var_os(CAPTURE_HOOK_ENV).is_some_and(|value| !value.is_empty())
}

/// Reads what one stopped run wrote, imports it as one layer over the image it booted, and reports
/// the cost from `command_exited`, when the workload's own session ended.
pub async fn capture_after_run(
    run_id: &str,
    parent_reference: &str,
    command: &[String],
    fileset_paths: &[String],
    command_exited: std::time::Instant,
) -> Result<BuiltLayer> {
    let started = std::time::Instant::now();
    let cache_dir = crate::cache::root()?;
    let upper_image = crate::cache::run_dir(&cache_dir, run_id).join("upper.img");
    let manifests = cache_dir.join("manifests");
    let parent = parent_image(&manifests, parent_reference)?;

    let changes = tokio::task::spawn_blocking(move || {
        let tree = Ext4Upper::open_run_upper(&upper_image)?;
        upper::capture(&tree)
    })
    .await
    .context("the upper-volume reader stopped before it finished")??;

    let (changes, dropped) = super::exclude::only_the_workloads_writes(changes, fileset_paths);
    if dropped.total() > 0 {
        log::info!(
            "Excluded",
            "{} entries this boot wrote for the run, {} a fileset seeded",
            dropped.boot,
            dropped.fileset,
        );
    }

    let layers = LayerCache::new(cache_dir.join("layers"));
    let built = super::import_captured(
        &crate::image_store::RealFs,
        &changes,
        &parent,
        &LocalStore {
            layers: &layers,
            manifests,
            images: cache_dir.join("images"),
        },
        &format!("RUN {}", command.join(" ")),
        &crate::time_fmt::rfc3339_now(),
        now_unix_secs(),
    )
    .await?;

    publish(&cache_dir, &built.reference);
    log::info!(
        "Built",
        "{} ({} entries, {} bytes, {:.2?} from exit, {:.2?} to read and import)",
        built.reference,
        built.entries,
        built.layer_bytes,
        command_exited.elapsed(),
        started.elapsed(),
    );
    Ok(built)
}

fn parent_image(manifests: &PathBuf, reference: &str) -> Result<ParentImage> {
    let normalized = crate::image_store::normalize_reference(reference)?;
    let cached = ManifestCache::new(manifests)
        .get(&normalized)
        .with_context(|| {
            format!(
                "the base image {normalized} is not in the local manifest cache; \
             slice 1 builds only on a digest-pinned base a run has already booted"
            )
        })?;
    Ok(ParentImage {
        manifest: cached.manifest,
        config: cached.config,
    })
}

fn publish(cache_dir: &std::path::Path, reference: &str) {
    let path = cache_dir.join(BUILT_REFERENCE_FILE);
    if let Err(e) = std::fs::write(&path, format!("{reference}\n")) {
        log::warn!(
            "the built image reference was not written to {}: {e}",
            path.display()
        );
    }
}

fn now_unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
