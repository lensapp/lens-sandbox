use crate::artifact::assembly::{self, AssembledWorkload, Override};
use crate::artifact::fetch::fetch_component;
use crate::artifact::resolve::{ComponentFetcher, FetchError, FetchedComponent};
use crate::artifact::signature::{self, SignatureStatus, Verdict};
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

/// Peek a run reference's manifest and, when it is an AgentSystem bundle, verify its signature then resolve + assemble it; a plain image returns `None` so the caller keeps its existing single-image path.
pub(crate) async fn peek_and_plan(
    image_ref: &str,
    host_arch: &str,
    overrides: &[Override],
    insecure: bool,
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
            enforce_signature(image_ref, insecure)?;
            let resolved = plan_bundle(
                config_json.as_bytes(),
                &RealComponentFetcher,
                host_arch,
                overrides,
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

/// Consult the signature-trust gate before running a bundle. Reading the trusted-signer set from `lns config` and verifying the cosign referrer are Layer-1 follow-ups; until they land no key is configured, so an unsigned bundle proceeds with a warning and `--insecure` short-circuits.
fn enforce_signature(image_ref: &str, insecure: bool) -> Result<()> {
    let trusted_keys_configured = false;
    match signature::gate(insecure, trusted_keys_configured, SignatureStatus::Unsigned) {
        Verdict::Verified | Verdict::Skipped => Ok(()),
        Verdict::ProceedUnverified { warning } => {
            crate::log::warn!("{warning}");
            Ok(())
        }
        Verdict::Refused(reason) => {
            anyhow::bail!(
                "refusing to run bundle {image_ref}: {}",
                reason.as_message()
            )
        }
    }
}
