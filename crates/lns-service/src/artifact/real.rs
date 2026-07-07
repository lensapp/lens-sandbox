use crate::artifact::assembly::{self, AssembledWorkload, Override, ResolvedBundle};
use crate::artifact::fetch::fetch_component;
use crate::artifact::fileset::fileset_runtime_specs;
use crate::artifact::resolve::{ComponentFetcher, FetchError, FetchedComponent};
use crate::artifact::signature::{self, SignatureStatus, Verdict};
use crate::artifact::{RunPath, dispatch, plan_bundle};
use crate::image::{RealRegistry, Registry, registry_auth_for, want_arch};
use crate::runtime_layer::RuntimeFileSpec;
use anyhow::{Context, Result};
use lns_ipc::{ArtifactInspection, BundleView, FilesetView, ImageView, SignatureView};
use oci_client::Reference;

/// A resolved bundle ready to boot: the assembled workload plus the guest-write specs that materialize its filesets into the microVM.
pub(crate) struct BundlePlan {
    pub workload: AssembledWorkload,
    pub fileset_specs: Vec<RuntimeFileSpec>,
}

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
) -> Result<Option<BundlePlan>> {
    let reference: Reference = image_ref
        .parse()
        .with_context(|| format!("invalid image reference {image_ref}"))?;
    let registry = RealRegistry::for_reference(&reference, registry_auth_for(image_ref));
    let (manifest, digest, config_json) = registry
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
            record_bundle_run(
                run_id, microvm, image_ref, &digest, overrides, &resolved, &verdict,
            );
            disclose_effective_policy(resolved.policy.as_ref());
            let fileset_specs = materialize_filesets(&resolved).await?;
            Ok(Some(BundlePlan {
                workload: assembly::assemble(&resolved),
                fileset_specs,
            }))
        }
    }
}

/// Pull each resolved fileset's content layer and expand it into guest-write specs, so the bundle's filesets land in the microVM at their mount paths.
async fn materialize_filesets(resolved: &ResolvedBundle) -> Result<Vec<RuntimeFileSpec>> {
    let mut specs = Vec::new();
    for fileset in &resolved.filesets {
        let Some(mount) = fileset.paths.first() else {
            continue;
        };
        let layers = pull_fileset_layers(&fileset.reference)
            .await
            .with_context(|| format!("materializing fileset {}", fileset.name))?;
        for layer in &layers {
            specs.extend(fileset_runtime_specs(mount, layer)?);
        }
    }
    Ok(specs)
}

/// Pull every tar content layer of a fileset, in manifest order, so a multi-layer fileset materializes all its files (later layers overlay earlier); the OCI empty/config layer is skipped and a fileset with no content layer is refused.
async fn pull_fileset_layers(reference: &str) -> Result<Vec<Vec<u8>>> {
    let parsed: Reference = reference
        .parse()
        .with_context(|| format!("invalid fileset reference {reference}"))?;
    let registry = RealRegistry::for_reference(&parsed, registry_auth_for(reference));
    let (manifest, _digest, _config) = registry.pull_manifest_and_config(&parsed).await?;
    let content: Vec<_> = manifest
        .layers
        .iter()
        .filter(|layer| layer.media_type.contains("tar"))
        .collect();
    if content.is_empty() {
        anyhow::bail!("fileset {reference} has no content layer");
    }
    let mut blobs = Vec::with_capacity(content.len());
    for layer in content {
        blobs.push(registry.pull_blob(&parsed, layer, &|_| {}).await?);
    }
    Ok(blobs)
}

/// Append a bundle-run event to the audit chain, pinning the resolved bundle digest (not just the mutable tag) plus the effective integrations and shipped-policy hash; a recording failure is logged, never fatal to the launch.
fn record_bundle_run(
    run_id: &str,
    microvm: &str,
    image_ref: &str,
    bundle_digest: &str,
    overrides: &[Override],
    resolved: &ResolvedBundle,
    verdict: &Verdict,
) {
    let override_refs: Vec<String> = overrides.iter().map(|o| o.reference.clone()).collect();
    let integrations = resolved
        .policy
        .as_ref()
        .map(|p| p.integrations.clone())
        .unwrap_or_default();
    let policy_hash = resolved
        .policy
        .as_ref()
        .map(crate::artifact::audit::policy_hash)
        .unwrap_or_else(|| "none".to_string());
    if let Err(e) = crate::audit::record_bundle_run(
        run_id,
        microvm,
        image_ref,
        bundle_digest,
        &override_refs,
        &integrations,
        &policy_hash,
        &crate::artifact::audit::verdict_label(verdict),
        &crate::oauth::RealClock,
    ) {
        crate::log::warn!("failed to record bundle-run audit event: {e:#}");
    }
}

/// Disclose the bundle's shipped network policy at boot: name it as the deny-dominant baseline under the local overlay, and warn prominently if it is over-broad (permissive default / wildcard / broad CIDR).
fn disclose_effective_policy(policy: Option<&lns_policy::Policy>) {
    let Some(policy) = policy else {
        return;
    };
    crate::log::info!(
        "policy",
        "this bundle ships a network policy; it governs the run as a deny-dominant baseline under your local lns-policy.yaml overlay"
    );
    let summary =
        crate::artifact::policy::run_summary(&crate::artifact::policy::guardrail_flags(policy));
    if !summary.is_empty() {
        crate::log::warn!("{summary}");
    }
}

/// Peek a reference's manifest and produce the pre-run inspection: a plain image reports its digest, a bundle reports its base image, filesets, declared integrations, and any over-broad-policy flags; signature trust awaits the verification follow-up.
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
                integrations: resolved
                    .policy
                    .as_ref()
                    .map(|p| p.integrations.clone())
                    .unwrap_or_default(),
                signature: SignatureView::Unsigned,
                policy_flags: resolved
                    .policy
                    .as_ref()
                    .map(|p| {
                        crate::artifact::policy::guardrail_flags(p)
                            .iter()
                            .map(|f| f.message().to_string())
                            .collect()
                    })
                    .unwrap_or_default(),
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
