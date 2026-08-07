use crate::artifact::assembly::{self, AssembledWorkload, ResolvedSandbox};
use crate::artifact::fileset::{FilesetBudget, fileset_runtime_specs_with_budget};
use crate::artifact::{RunPath, dispatch_run, resolved_from_sandbox};
use crate::image::{RealRegistry, Registry, registry_auth_for};
use crate::runtime_layer::RuntimeFileSpec;
use anyhow::{Context, Result};
use lns_ipc::ArtifactInspection;
use oci_client::Reference;

/// A resolved sandbox ready to boot: the assembled workload plus the guest-write specs that materialize its filesets into the microVM.
pub(crate) struct SandboxPlan {
    pub workload: AssembledWorkload,
    pub fileset_specs: Vec<RuntimeFileSpec>,
    /// The resolved manifest digest of a published sandbox reference, pinning its per-workload grant identity; `None` for a local definition (which keys by directory).
    pub digest: Option<String>,
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
    crate::image::verify_digest_pin(&reference, &digest, image_ref)?;
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
            crate::image_store::record_artifact_run(image_ref, &digest, &resolved.base_image)
                .await
                .with_context(|| format!("recording the sandbox index for {image_ref}"))?;
            disclose_effective_policy(resolved.policy.as_ref());
            let problems = crate::artifact::published_fileset_problems(&resolved);
            if !problems.is_empty() {
                anyhow::bail!("refusing to run {image_ref}: {}", problems.join("; "));
            }
            let fileset_specs = materialize_filesets(&resolved).await?.into_specs();
            Ok(Some(SandboxPlan {
                workload: assembly::assemble(&resolved),
                fileset_specs,
                digest: Some(digest),
            }))
        }
    }
}

/// Plan a local `lns.yaml` definition into a bootable workload, disclosing its shipped policy exactly like a published sandbox run.
pub(crate) async fn plan_local(definition_json: &str) -> Result<SandboxPlan> {
    let resolved = crate::artifact::plan_local_sandbox(definition_json.as_bytes())?;
    disclose_effective_policy(resolved.policy.as_ref());
    let mut materialized = materialize_filesets(&resolved).await?;
    crate::artifact::fileset::local_fileset_specs(
        &RealSnapshotDir,
        &resolved.local_filesets,
        &mut materialized,
    )?;
    Ok(SandboxPlan {
        workload: assembly::assemble(&resolved),
        fileset_specs: materialized.into_specs(),
        digest: None,
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

/// Refuse a launch whose definition declares a connector — under `spec.connectors` or as a credential slot — this machine's catalog cannot arm; fail-fast at boot instead of an opaque mid-run credential miss.
pub(crate) fn refuse_unknown_connectors(
    policy: Option<&lns_policy::Policy>,
    credentials: &[lns_artifact::spec::CredentialSlot],
) -> Result<()> {
    let mut declared: Vec<String> = policy.map(|p| p.connectors.clone()).unwrap_or_default();
    declared.extend(credentials.iter().map(|slot| slot.name.clone()));
    if declared.is_empty() {
        return Ok(());
    }
    let catalog = effective_machine_catalog();
    let unknown = crate::credential_flow::connectors::unknown_connector_ids(&declared, &catalog);
    if unknown.is_empty() {
        return Ok(());
    }
    anyhow::bail!(crate::credential_flow::connectors::unknown_connectors_refusal(&unknown))
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

fn effective_machine_catalog() -> Vec<lns_policy::connectors::Connector> {
    let user = lns_policy::connectors::Catalog::load_or_default(
        &lns_policy::connectors::default_connectors_path(),
    )
    .unwrap_or_else(|e| {
        crate::log::warn!(
            "unreadable user connector catalog ({e}); using the bundled catalog only"
        );
        lns_policy::connectors::Catalog::default()
    });
    lns_policy::connectors::effective_connectors(&user)
}

/// Pull each resolved fileset's content layer and expand it into guest-write specs, so the sandbox's filesets land in the microVM at their mount paths.
async fn materialize_filesets(
    resolved: &ResolvedSandbox,
) -> Result<crate::artifact::fileset::MaterializedFilesets> {
    let content_store =
        crate::content_store::ContentStore::new(crate::cache::root()?.join("content"));
    let mut out = crate::artifact::fileset::MaterializedFilesets::default();
    for fileset in &resolved.filesets {
        let Some(mount) = fileset.paths.first() else {
            continue;
        };
        let layers = pull_fileset_layers(&fileset.reference, &content_store)
            .await
            .with_context(|| format!("materializing fileset {}", fileset.name))?;
        let mut specs = Vec::new();
        let mut budget = FilesetBudget::new();
        for layer in layers {
            let file = std::fs::File::open(&layer)
                .with_context(|| format!("opening fileset layer {}", layer.display()))?;
            specs.extend(fileset_runtime_specs_with_budget(
                mount,
                file,
                &content_store,
                &mut budget,
            )?);
        }
        out.absorb(fileset.owner, mount, specs);
    }
    crate::artifact::fileset::inline_fileset_specs(&resolved.inline_filesets, &mut out);
    Ok(out)
}

async fn pull_fileset_layers(
    reference: &str,
    content_store: &crate::content_store::ContentStore,
) -> Result<Vec<std::path::PathBuf>> {
    let parsed: Reference = reference
        .parse()
        .with_context(|| format!("invalid fileset reference {reference}"))?;
    let registry = RealRegistry::for_reference(&parsed, registry_auth_for(reference));
    pull_fileset_layers_with(&registry, &parsed, content_store).await
}

/// Pull every tar content layer of a fileset, in manifest order, so a multi-layer fileset materializes all its files (later layers overlay earlier); the fileset is refused before any download unless it is a digest-pinned FileSet artifact within the byte ceiling, and a stream failure leaves no partial content staged.
async fn pull_fileset_layers_with<R: Registry>(
    registry: &R,
    parsed: &Reference,
    content_store: &crate::content_store::ContentStore,
) -> Result<Vec<std::path::PathBuf>> {
    let (manifest, digest, config) = registry.pull_manifest_and_config(parsed).await?;
    crate::image::verify_digest_pin(parsed, &digest, &parsed.to_string())?;
    crate::artifact::fileset::validate_fileset_artifact(parsed, &manifest, &config)?;
    crate::artifact::fileset::validate_fileset_layer_sizes(
        &manifest,
        lns_artifact::build::MAX_FILESET_BYTES,
    )?;
    let content: Vec<_> = manifest.layers.iter().collect();
    if content.is_empty() {
        anyhow::bail!("fileset {parsed} has no content layer");
    }
    let mut blobs = Vec::with_capacity(content.len());
    for layer in content {
        let expected_size = u64::try_from(layer.size)
            .with_context(|| format!("fileset layer {} has a negative size", layer.digest))?;
        let staged = content_store.staging_path()?;
        if let Err(error) = registry
            .pull_blob_to_path(parsed, layer, expected_size, &staged, &|_| {})
            .await
        {
            let _ = std::fs::remove_file(&staged);
            return Err(error);
        }
        let installed = content_store
            .commit_verified(staged, &layer.digest, expected_size)
            .with_context(|| format!("verifying fileset layer {}", layer.digest))?;
        blobs.push(installed.path);
    }
    Ok(blobs)
}

/// Append a sandbox-run event to the audit chain, pinning the resolved digest (not just the mutable tag) plus the effective connectors and shipped-policy hash; a recording failure is logged, never fatal to the launch.
fn record_sandbox_run(
    run_id: &str,
    microvm: &str,
    image_ref: &str,
    digest: &str,
    resolved: &ResolvedSandbox,
) {
    let connectors = resolved
        .policy
        .as_ref()
        .map(|p| p.connectors.clone())
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
        &connectors,
        &policy_hash,
        &crate::oauth::RealClock,
    ) {
        crate::log::warn!("failed to record sandbox-run audit event: {e:#}");
    }
}

/// Disclose the sandbox's shipped network policy and declared connectors at boot: name the policy as the deny-dominant baseline under the local overlay (warning if it is over-broad), and disclose that declared connectors seed placeholders but are offered on first use, never armed automatically.
fn disclose_effective_policy(policy: Option<&lns_policy::Policy>) {
    let Some(policy) = policy else {
        return;
    };
    if policy.network != lns_policy::NetworkPolicy::default() {
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
    if !policy.connectors.is_empty() {
        crate::log::info!(
            "policy",
            "this sandbox requests connectors ({}); each seeds a placeholder env var and is offered on first use — accept its connect card to arm it — never armed automatically",
            policy.connectors.join(", ")
        );
    }
}

pub(crate) async fn inspect(image_ref: &str) -> Result<ArtifactInspection> {
    let reference: Reference = image_ref
        .parse()
        .with_context(|| format!("invalid image reference {image_ref}"))?;
    let registry = crate::image::caching_registry_for(image_ref)?;
    let (manifest, digest, config_json) = registry
        .pull_manifest_and_config(&reference)
        .await
        .with_context(|| format!("inspecting {image_ref}"))?;
    crate::image::verify_digest_pin(&reference, &digest, image_ref)?;
    crate::artifact::inspect::project_inspection(
        image_ref,
        digest,
        manifest.artifact_type.as_deref(),
        &manifest.config.media_type,
        &config_json,
        lns_artifact::resources::host::probe(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use oci_client::manifest::{OciDescriptor, OciImageManifest};
    use sha2::{Digest, Sha256};
    use std::sync::atomic::{AtomicBool, Ordering};

    struct StreamingRegistry {
        blob: Vec<u8>,
        fail: bool,
        manifest_digest: Option<String>,
        streamed: AtomicBool,
    }

    impl StreamingRegistry {
        fn fileset(blob: Vec<u8>, fail: bool) -> Self {
            Self {
                blob,
                fail,
                manifest_digest: None,
                streamed: AtomicBool::new(false),
            }
        }
    }

    impl Registry for StreamingRegistry {
        async fn pull_manifest_and_config(
            &self,
            reference: &Reference,
        ) -> Result<(OciImageManifest, String, String)> {
            let digest = format!("sha256:{}", hex::encode(Sha256::digest(&self.blob)));
            Ok((
                OciImageManifest {
                    artifact_type: Some(lns_artifact::spec::Kind::FileSet.artifact_type()),
                    config: OciDescriptor {
                        media_type: lns_artifact::spec::Kind::FileSet.config_media_type(),
                        ..Default::default()
                    },
                    layers: vec![OciDescriptor {
                        media_type: "application/vnd.oci.image.layer.v1.tar".into(),
                        digest,
                        size: self.blob.len() as i64,
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                self.manifest_digest
                    .clone()
                    .or_else(|| reference.digest().map(ToOwned::to_owned))
                    .unwrap_or_else(|| format!("sha256:{}", "b".repeat(64))),
                r#"{"apiVersion":"lens.dev/v1alpha1","kind":"FileSet","metadata":{"name":"files"},"mount":{"path":"/files"},"spec":{}}"#.into(),
            ))
        }

        async fn pull_blob(
            &self,
            _reference: &Reference,
            _descriptor: &OciDescriptor,
            _on_chunk: &(dyn Fn(u64) + Send + Sync),
        ) -> Result<Vec<u8>> {
            anyhow::bail!("fileset pulls must stream, never materialize a blob Vec")
        }

        async fn pull_blob_to_path(
            &self,
            _reference: &Reference,
            _descriptor: &OciDescriptor,
            max_bytes: u64,
            path: &std::path::Path,
            on_chunk: &(dyn Fn(u64) + Send + Sync),
        ) -> Result<()> {
            self.streamed.store(true, Ordering::Relaxed);
            if self.blob.len() as u64 > max_bytes {
                anyhow::bail!("blob exceeds the {max_bytes}-byte limit");
            }
            let split = self.blob.len() / 2;
            tokio::fs::write(path, &self.blob[..split]).await?;
            on_chunk(split as u64);
            if self.fail {
                anyhow::bail!("registry stream failed")
            }
            use tokio::io::AsyncWriteExt;
            let mut file = tokio::fs::OpenOptions::new()
                .append(true)
                .open(path)
                .await?;
            file.write_all(&self.blob[split..]).await?;
            on_chunk((self.blob.len() - split) as u64);
            Ok(())
        }
    }

    fn pinned_reference() -> Reference {
        "registry.example.test/team/files@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            .parse()
            .unwrap()
    }

    #[tokio::test]
    async fn fileset_layers_stream_to_verified_content_without_buffering_a_blob_vec() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::content_store::ContentStore::new(dir.path());
        let registry = StreamingRegistry::fileset(vec![7; 512 * 1024], false);

        let paths = pull_fileset_layers_with(&registry, &pinned_reference(), &store)
            .await
            .unwrap();

        assert!(registry.streamed.load(Ordering::Relaxed));
        assert_eq!(paths.len(), 1);
        assert_eq!(std::fs::read(&paths[0]).unwrap(), registry.blob);
    }

    #[tokio::test]
    async fn a_digest_mismatch_is_refused_before_any_fileset_layer_is_downloaded() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::content_store::ContentStore::new(dir.path());
        let mut registry = StreamingRegistry::fileset(vec![7; 1024], false);
        registry.manifest_digest = Some(format!("sha256:{}", "b".repeat(64)));

        let err = pull_fileset_layers_with(&registry, &pinned_reference(), &store)
            .await
            .unwrap_err();

        assert!(
            format!("{err:#}").contains("manifest digest mismatch"),
            "got: {err:#}"
        );
        assert!(!registry.streamed.load(Ordering::Relaxed));
    }

    #[tokio::test]
    async fn a_failed_fileset_stream_leaves_no_partial_content() {
        let dir = tempfile::tempdir().unwrap();
        let store = crate::content_store::ContentStore::new(dir.path());
        let registry = StreamingRegistry::fileset(vec![7; 128 * 1024], true);

        let err = pull_fileset_layers_with(&registry, &pinned_reference(), &store)
            .await
            .unwrap_err();

        assert!(format!("{err:#}").contains("registry stream failed"));
        let entries = std::fs::read_dir(dir.path().join("sha256"))
            .unwrap()
            .count();
        assert_eq!(entries, 0);
    }
}
