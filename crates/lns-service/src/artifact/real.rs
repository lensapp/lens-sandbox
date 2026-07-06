use crate::artifact::assembly::{self, AssembledWorkload};
use crate::artifact::fetch::fetch_component;
use crate::artifact::resolve::{ComponentFetcher, FetchError, FetchedComponent};
use crate::artifact::{RunPath, dispatch, plan_bundle};
use crate::image::{RealRegistry, Registry, registry_auth_for};
use anyhow::{Context, Result};
use oci_client::Reference;

pub struct RealComponentFetcher;

impl ComponentFetcher for RealComponentFetcher {
    async fn fetch(&self, reference: &str) -> Result<FetchedComponent, FetchError> {
        let registry = RealRegistry::with_auth(registry_auth_for(reference));
        fetch_component(&registry, reference).await
    }
}

/// Peek a run reference's manifest and, when it is an AgentSystem bundle, resolve + assemble it; a plain image returns `None` so the caller keeps its existing single-image path.
pub(crate) async fn peek_and_plan(
    image_ref: &str,
    host_arch: &str,
) -> Result<Option<AssembledWorkload>> {
    let reference: Reference = image_ref
        .parse()
        .with_context(|| format!("invalid image reference {image_ref}"))?;
    let registry = RealRegistry::with_auth(registry_auth_for(image_ref));
    let (manifest, _digest, config_json) = registry
        .pull_manifest_and_config(&reference)
        .await
        .with_context(|| format!("peeking manifest for {image_ref}"))?;
    match dispatch(
        manifest.artifact_type.as_deref(),
        Some(manifest.config.media_type.as_str()),
    )? {
        RunPath::SingleImage => Ok(None),
        RunPath::AssembleBundle => {
            let resolved = plan_bundle(
                config_json.as_bytes(),
                &RealComponentFetcher,
                host_arch,
                &[],
            )
            .await?;
            for fileset in &resolved.filesets {
                crate::log::warn!(
                    "bundle fileset {} is resolved but not yet mounted into the guest",
                    fileset.name
                );
            }
            Ok(Some(assembly::assemble(&resolved)))
        }
    }
}
