use crate::artifact::assembly::{self, AssembledWorkload, ResolvedSandbox};
use crate::artifact::fileset::fileset_runtime_specs;
use crate::artifact::{RunPath, dispatch, dispatch_run, resolved_from_sandbox};
use crate::image::{RealRegistry, Registry, registry_auth_for};
use crate::runtime_layer::RuntimeFileSpec;
use anyhow::{Context, Result};
use lns_ipc::{ArtifactInspection, ImageView, SandboxMount, SandboxMountKind};
use oci_client::Reference;

/// A resolved sandbox ready to boot: the assembled workload plus the guest-write specs that materialize its filesets into the microVM.
pub(crate) struct SandboxPlan {
    pub workload: AssembledWorkload,
    pub fileset_specs: Vec<RuntimeFileSpec>,
}

/// Peek a run reference's manifest and, when it is a published sandbox, resolve + assemble it; a plain image returns `None` so the caller runs it directly (a bare `verify_sandbox` reference that resolves to a plain image is refused as "not a sandbox").
pub(crate) async fn peek_and_plan(
    image_ref: &str,
    verify_sandbox: bool,
    run_id: &str,
    microvm: &str,
) -> Result<Option<SandboxPlan>> {
    let reference: Reference = image_ref
        .parse()
        .with_context(|| format!("invalid image reference {image_ref}"))?;
    // Peek through the manifest cache so a digest-pinned reference that was pulled once (e.g. via `lns pull`) resolves without touching the registry again.
    let registry = crate::image::caching_registry_for(image_ref)?;
    let (manifest, digest, config_json) = registry
        .pull_manifest_and_config(&reference)
        .await
        .with_context(|| format!("peeking manifest for {image_ref}"))?;
    match dispatch_run(
        manifest.artifact_type.as_deref(),
        Some(manifest.config.media_type.as_str()),
        image_ref,
        verify_sandbox,
    )? {
        RunPath::SingleImage => Ok(None),
        RunPath::Sandbox => {
            let def = lns_artifact::sandbox::parse(config_json.as_bytes())
                .with_context(|| format!("parsing published sandbox {image_ref}"))?;
            let resolved = resolved_from_sandbox(&def);
            record_sandbox_run(run_id, microvm, image_ref, &digest, &resolved);
            disclose_effective_policy(resolved.policy.as_ref());
            let problems = crate::artifact::published_fileset_problems(&resolved);
            if !problems.is_empty() {
                anyhow::bail!("refusing to run {image_ref}: {}", problems.join("; "));
            }
            let fileset_specs = materialize_filesets(&resolved).await?;
            Ok(Some(SandboxPlan {
                workload: assembly::assemble(&resolved),
                fileset_specs,
            }))
        }
    }
}

/// Plan a local `lns.yaml` definition into a bootable workload, disclosing its shipped policy exactly like a published sandbox run.
pub(crate) async fn plan_local(definition_json: &str) -> Result<SandboxPlan> {
    let resolved = crate::artifact::plan_local_sandbox(definition_json.as_bytes())?;
    disclose_effective_policy(resolved.policy.as_ref());
    let mut fileset_specs = materialize_filesets(&resolved).await?;
    fileset_specs.extend(crate::artifact::fileset::local_fileset_specs(
        &RealSnapshotDir,
        &resolved.local_filesets,
    )?);
    Ok(SandboxPlan {
        workload: assembly::assemble(&resolved),
        fileset_specs,
    })
}

struct RealSnapshotDir;

impl crate::artifact::fileset::SnapshotDir for RealSnapshotDir {
    fn entries(
        &self,
        dir: &std::path::Path,
    ) -> std::io::Result<Vec<crate::artifact::fileset::SnapshotEntry>> {
        use std::os::unix::fs::PermissionsExt;
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let name = entry.file_name().into_string().map_err(|name| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("non-utf8 file name {name:?}"),
                )
            })?;
            if entry.file_type()?.is_symlink() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("symlink {name} — filesets carry only regular files"),
                ));
            }
            let metadata = entry.metadata()?;
            entries.push(crate::artifact::fileset::SnapshotEntry {
                name,
                dir: metadata.is_dir(),
                mode: metadata.permissions().mode() & 0o777,
            });
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }
}

/// Refuse a launch whose definition declares an integration — under `spec.integrations` or as a credential slot — this machine's catalog cannot arm; fail-fast at boot instead of an opaque mid-run credential miss.
pub(crate) fn refuse_unknown_integrations(
    policy: Option<&lns_policy::Policy>,
    credentials: &[lns_artifact::spec::CredentialSlot],
) -> Result<()> {
    let mut declared: Vec<String> = policy.map(|p| p.integrations.clone()).unwrap_or_default();
    declared.extend(credentials.iter().map(|slot| slot.name.clone()));
    if declared.is_empty() {
        return Ok(());
    }
    let catalog = effective_machine_catalog();
    let unknown =
        crate::credential_flow::integrations::unknown_integration_ids(&declared, &catalog);
    if unknown.is_empty() {
        return Ok(());
    }
    anyhow::bail!(crate::credential_flow::integrations::unknown_integrations_refusal(&unknown))
}

/// Refuse a launch whose definition requires a credential slot this machine has not bound (or has denied) — before any microVM boots.
pub(crate) fn refuse_unbound_required_credentials(
    credentials: &[lns_artifact::spec::CredentialSlot],
) -> Result<()> {
    if credentials.iter().all(|slot| !slot.required) {
        return Ok(());
    }
    let catalog = effective_machine_catalog();
    let state = {
        use crate::credential_flow::store::{
            CredentialStore, JsonFileCredentialStore, default_credentials_path,
        };
        JsonFileCredentialStore::new(default_credentials_path())
            .load()
            .unwrap_or_default()
    };
    if let Err(failure) =
        crate::artifact::credential_boot::gate_required_slots(credentials, &catalog, &state)
    {
        anyhow::bail!(failure.as_message());
    }
    Ok(())
}

fn effective_machine_catalog() -> Vec<lns_policy::integrations::Integration> {
    let user = lns_policy::integrations::Catalog::load_or_default(
        &lns_policy::integrations::default_integrations_path(),
    )
    .unwrap_or_else(|e| {
        crate::log::warn!(
            "unreadable user integration catalog ({e}); using the bundled catalog only"
        );
        lns_policy::integrations::Catalog::default()
    });
    lns_policy::integrations::effective_integrations(&user)
}

/// Pull each resolved fileset's content layer and expand it into guest-write specs, so the sandbox's filesets land in the microVM at their mount paths.
async fn materialize_filesets(resolved: &ResolvedSandbox) -> Result<Vec<RuntimeFileSpec>> {
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

/// Append a sandbox-run event to the audit chain, pinning the resolved digest (not just the mutable tag) plus the effective integrations and shipped-policy hash; a recording failure is logged, never fatal to the launch.
fn record_sandbox_run(
    run_id: &str,
    microvm: &str,
    image_ref: &str,
    digest: &str,
    resolved: &ResolvedSandbox,
) {
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
    if let Err(e) = crate::audit::record_sandbox_run(
        run_id,
        microvm,
        image_ref,
        digest,
        &integrations,
        &policy_hash,
        &crate::oauth::RealClock,
    ) {
        crate::log::warn!("failed to record sandbox-run audit event: {e:#}");
    }
}

/// Disclose the sandbox's shipped network policy at boot: name it as the deny-dominant baseline under the local overlay, and warn prominently if it is over-broad (permissive default / wildcard / broad CIDR).
fn disclose_effective_policy(policy: Option<&lns_policy::Policy>) {
    let Some(policy) = policy else {
        return;
    };
    crate::log::info!(
        "policy",
        "this sandbox ships a network policy; it governs the run as a deny-dominant baseline under your local lns-policy.yaml overlay"
    );
    let summary =
        crate::artifact::policy::run_summary(&crate::artifact::policy::guardrail_flags(policy));
    if !summary.is_empty() {
        crate::log::warn!("{summary}");
    }
}

fn declared_view_ports(ports: &[lns_artifact::spec::Port]) -> Result<Vec<lns_ipc::SandboxPort>> {
    ports
        .iter()
        .map(|port| {
            Ok(lns_ipc::SandboxPort {
                host: port
                    .host
                    .map(u16::try_from)
                    .transpose()
                    .with_context(|| format!("declared host port {:?} out of range", port.host))?,
                container: u16::try_from(port.container).with_context(|| {
                    format!("declared container port {} out of range", port.container)
                })?,
            })
        })
        .collect()
}

/// Peek a reference's manifest and produce the pre-run inspection: a plain image reports its digest, a published sandbox reports its base image, mounts, filesets, declared integrations, and any over-broad-policy flags.
pub(crate) async fn inspect(image_ref: &str) -> Result<ArtifactInspection> {
    let requested: Reference = image_ref
        .parse()
        .with_context(|| format!("invalid image reference {image_ref}"))?;
    let reference = if requested.digest().is_none() {
        match crate::image_store::cached_digest(image_ref).await? {
            Some(digest) => requested.clone_with_digest(digest),
            None => requested,
        }
    } else {
        requested
    };
    let registry = crate::image::caching_registry_for(&reference.to_string())?;
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
        RunPath::Sandbox => {
            let def = lns_artifact::sandbox::parse(config_json.as_bytes())
                .with_context(|| format!("inspecting sandbox {image_ref}"))?;
            let resolved = resolved_from_sandbox(&def);
            Ok(ArtifactInspection::Sandbox(lns_ipc::SandboxView {
                reference: image_ref.to_string(),
                digest,
                image: resolved.base_image,
                workdir: def.spec.workdir.clone(),
                mounts: def
                    .spec
                    .volumes
                    .iter()
                    .map(|volume| SandboxMount {
                        kind: if volume.is_bind() {
                            SandboxMountKind::Bind
                        } else {
                            SandboxMountKind::Volume
                        },
                        source: volume.source().to_string(),
                        target: volume.target.clone(),
                        read_only: volume.read_only(),
                    })
                    .collect(),
                ports: declared_view_ports(&def.spec.ports)?,
                filesets: def
                    .spec
                    .filesets
                    .iter()
                    .map(|fileset| lns_ipc::SandboxFileset {
                        path: fileset.path.clone(),
                        reference: fileset.reference.clone(),
                        mount_path: fileset.mount_path.clone(),
                    })
                    .collect(),
                integrations: def.spec.integrations,
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
