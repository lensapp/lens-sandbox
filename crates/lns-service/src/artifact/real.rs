use crate::artifact::assembly::{self, AssembledWorkload, Override};
use crate::artifact::fetch::fetch_component;
use crate::artifact::resolve::{ComponentFetcher, FetchError, FetchedComponent};
use crate::artifact::signature::{self, SignatureStatus, Verdict};
use crate::artifact::{RunPath, dispatch, plan_bundle};
use crate::image::{RealRegistry, Registry, registry_auth_for, want_arch};
use anyhow::{Context, Result};
use lns_ipc::{ArtifactInspection, BundleView, FilesetView, ImageView, SignatureView};
use oci_client::Reference;

pub struct RealComponentFetcher;

impl ComponentFetcher for RealComponentFetcher {
    async fn fetch(&self, reference: &str) -> Result<FetchedComponent, FetchError> {
        let auth = registry_auth_for(reference);
        let registry = match reference.parse::<Reference>() {
            Ok(parsed) => RealRegistry::for_reference(&parsed, auth),
            Err(_) => RealRegistry::with_auth(auth),
        };
        fetch_component(&registry, reference).await
    }
}

/// Peek a run reference's manifest and, when it is an AgentSystem bundle, verify its signature then resolve + assemble it; a plain image returns `None` so the caller keeps its existing single-image path.
pub(crate) async fn peek_and_plan(
    image_ref: &str,
    host_arch: &str,
    overrides: &[Override],
    insecure: bool,
    run_id: &str,
    microvm: &str,
) -> Result<Option<AssembledWorkload>> {
    let reference: Reference = image_ref
        .parse()
        .with_context(|| format!("invalid image reference {image_ref}"))?;
    let registry = RealRegistry::for_reference(&reference, registry_auth_for(image_ref));
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
            let verdict = enforce_signature(image_ref, insecure)?;
            let resolved = plan_bundle(
                config_json.as_bytes(),
                &RealComponentFetcher,
                host_arch,
                overrides,
            )
            .await?;
            record_bundle_run(run_id, microvm, image_ref, overrides, &verdict);
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

/// Append a bundle-run event to the audit chain; a recording failure is logged, never fatal to the launch.
fn record_bundle_run(
    run_id: &str,
    microvm: &str,
    image_ref: &str,
    overrides: &[Override],
    verdict: &Verdict,
) {
    let override_refs: Vec<String> = overrides.iter().map(|o| o.reference.clone()).collect();
    if let Err(e) = crate::audit::record_bundle_run(
        run_id,
        microvm,
        image_ref,
        &override_refs,
        &crate::artifact::audit::verdict_label(verdict),
        &crate::oauth::RealClock,
    ) {
        crate::log::warn!("failed to record bundle-run audit event: {e:#}");
    }
}

/// Peek a reference's manifest and produce the pre-run inspection: a plain image reports its digest, a bundle reports its base image and filesets; integrations, signature trust, and over-broad-policy flags await the policy-resolution and signature-verification follow-ups.
pub(crate) async fn inspect(image_ref: &str) -> Result<ArtifactInspection> {
    let reference: Reference = image_ref
        .parse()
        .with_context(|| format!("invalid image reference {image_ref}"))?;
    let registry = RealRegistry::for_reference(&reference, registry_auth_for(image_ref));
    let (manifest, digest, config_json) = registry
        .pull_manifest_and_config(&reference)
        .await
        .with_context(|| format!("inspecting {image_ref}"))?;
    match dispatch(
        manifest.artifact_type.as_deref(),
        Some(manifest.config.media_type.as_str()),
    )? {
        RunPath::SingleImage => Ok(ArtifactInspection::Image(ImageView {
            reference: image_ref.to_string(),
            digest,
        })),
        RunPath::AssembleBundle => {
            let resolved = plan_bundle(
                config_json.as_bytes(),
                &RealComponentFetcher,
                &want_arch().to_string(),
                &[],
            )
            .await?;
            Ok(ArtifactInspection::Bundle(BundleView {
                reference: image_ref.to_string(),
                sandbox_base_image: Some(resolved.base_image.clone()),
                filesets: resolved
                    .filesets
                    .iter()
                    .map(|f| FilesetView {
                        name: f.name.clone(),
                        mount_path: f.paths.first().cloned().unwrap_or_default(),
                    })
                    .collect(),
                integrations: Vec::new(),
                signature: SignatureView::Unsigned,
                policy_flags: Vec::new(),
            }))
        }
    }
}

/// Consult the signature-trust gate before running a bundle. Reading the trusted-signer set from `lns config` and verifying the cosign referrer are Layer-1 follow-ups; until they land no key is configured, so an unsigned bundle proceeds with a warning and `--insecure` short-circuits.
fn enforce_signature(image_ref: &str, insecure: bool) -> Result<Verdict> {
    let trusted_keys_configured = false;
    let verdict = signature::gate(insecure, trusted_keys_configured, SignatureStatus::Unsigned);
    match &verdict {
        Verdict::Verified | Verdict::Skipped => {}
        Verdict::ProceedUnverified { warning } => crate::log::warn!("{warning}"),
        Verdict::Refused(reason) => {
            anyhow::bail!(
                "refusing to run bundle {image_ref}: {}",
                reason.as_message()
            )
        }
    }
    Ok(verdict)
}
